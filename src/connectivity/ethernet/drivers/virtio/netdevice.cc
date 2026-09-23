// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "netdevice.h"

#include <fidl/fuchsia.driver.framework/cpp/fidl.h>
#include <fidl/fuchsia.hardware.network/cpp/wire.h>
#include <lib/driver/component/cpp/node_add_args.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/fit/defer.h>
#include <lib/virtio/ring.h>
#include <lib/zircon-internal/align.h>
#include <limits.h>
#include <stddef.h>
#include <stdint.h>
#include <string.h>
#include <unistd.h>
#include <zircon/assert.h>
#include <zircon/status.h>
#include <zircon/types.h>

#include <memory>
#include <utility>

#include <fbl/algorithm.h>
#include <fbl/auto_lock.h>
#include <virtio/net.h>
#include <virtio/virtio.h>

#include "src/connectivity/ethernet/drivers/virtio/virtio_net_driver.h"

// Enables/disables debugging info
#define LOCAL_TRACE 0

namespace virtio {

namespace {

bool IsLinkActive(const virtio_net_config& config, bool is_status_supported) {
  // 5.1.4.2 Driver Requirements: Device configuration layout
  //
  // If the driver does not negotiate the VIRTIO_NET_F_STATUS feature, it SHOULD assume the link
  // is active, otherwise it SHOULD read the link status from the bottom bit of status.
  //
  // https://docs.oasis-open.org/virtio/virtio/v1.1/csprd01/virtio-v1.1-csprd01.html#x1-2000004
  return is_status_supported ? config.status & VIRTIO_NET_S_LINK_UP : true;
}

uint16_t MaxVirtqueuePairs(const virtio_net_config& config, bool is_mq_supported) {
  // 5.1.5 Device Initialization
  //
  // Identify and initialize the receive and transmission virtqueues, up to N of each kind. If
  // VIRTIO_NET_F_MQ feature bit is negotiated, N=max_virtqueue_pairs, otherwise identify N=1.
  //
  // https://docs.oasis-open.org/virtio/virtio/v1.1/csprd01/virtio-v1.1-csprd01.html#x1-2040005
  return is_mq_supported ? config.max_virtqueue_pairs : 1;
}

}  // namespace

NetworkDevice::NetworkDevice(VirtioNetDriver* driver, zx::bti bti_handle,
                             std::unique_ptr<Backend> backend,
                             const std::shared_ptr<fdf::Namespace>& incoming,
                             const std::optional<std::string>& node_name)
    : virtio::Device(std::move(bti_handle), std::move(backend)),
      driver_(driver),
      rx_(this),
      tx_(this),
      incoming_(incoming),
      node_name_(node_name),
      vmo_store_({
          .map =
              vmo_store::MapOptions{
                  .vm_option = ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_REQUIRE_NON_RESIZABLE,
                  .vmar = nullptr,
              },
          .pin =
              vmo_store::PinOptions{
                  .bti = zx::unowned_bti(bti()),
                  .bti_pin_options = ZX_BTI_PERM_READ | ZX_BTI_PERM_WRITE,
                  .index = true,
              },
      }) {}

NetworkDevice::~NetworkDevice() {}

zx_status_t NetworkDevice::Init() {
  zx::result netdev_dispatcher = fdf::UnsynchronizedDispatcher::Create(
      {}, "netdev-dispatcher",
      [this](fdf_dispatcher_t*) { netdevice_dispatcher_shutdown_.Signal(); });
  if (netdev_dispatcher.is_error()) {
    fdf::error("Failed to create netdevice dispatcher: {}", netdev_dispatcher.status_string());
    return netdev_dispatcher.status_value();
  }
  netdevice_dispatcher_ = std::move(netdev_dispatcher.value());

  fbl::AutoLock lock(&state_lock_);

  // Reset the device.
  DeviceReset();

  // Ack and set the driver status bit.
  DriverStatusAck();

  // Ack features. We do DeviceStatusFeaturesOk() when we actually start the network device (in
  // NetworkDeviceImplStart()).
  if (zx_status_t status =
          AckFeatures(&is_status_supported_, &is_multiqueue_supported_, &virtio_hdr_len_);
      status != ZX_OK) {
    fdf::error("failed to ack features: {}", zx_status_get_string(status));
    return status;
  }

  // Read device configuration.
  virtio_net_config_t config;
  CopyDeviceConfig(&config, sizeof(config));

  // We've checked that the config.mac field is valid (VIRTIO_NET_F_MAC) in AckFeatures().
  fdf::debug("mac: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}", config.mac[0], config.mac[1],
             config.mac[2], config.mac[3], config.mac[4], config.mac[5]);
  fdf::debug("link active: {}", IsLinkActive(config, is_status_supported_));
  fdf::debug("max virtqueue pairs: {}", MaxVirtqueuePairs(config, is_multiqueue_supported_));

  static_assert(sizeof(config.mac) == sizeof(mac_.octets));
  std::copy(std::begin(config.mac), std::end(config.mac), mac_.octets.begin());

  if (zx_status_t status = vmo_store_.Reserve(fuchsia_hardware_network::wire::kMaxDataVmos);
      status != ZX_OK) {
    fdf::error("failed to initialize vmo store: {}", zx_status_get_string(status));
    return status;
  }

  // Initialize the zx_device and publish us.
  if (zx_status_t status = AddDevice(); status != ZX_OK) {
    fdf::error("failed to add device: {}", zx_status_get_string(status));
    return status;
  }

  tx_depth_ = std::min(GetRingSize(kTxId), kMaxDepth);
  rx_depth_ = std::min(GetRingSize(kRxId), kMaxDepth);

  // Start the interrupt thread.
  StartIrqThread();
  irq_thread_started_ = true;

  return ZX_OK;
}

zx_status_t NetworkDevice::AddDevice() {
  if (zx::result result =
          compat_server_.Initialize(incoming_, driver_->outgoing(), node_name_, kChildNodeName);
      result.is_error()) {
    fdf::error("Failed to initialize compat server: {}", result.status_string());
    return result.status_value();
  }

  // This callback is invoked when this service is being connected.
  auto protocol = [this](fdf::ServerEnd<netdev::NetworkDeviceImpl> server_end) mutable {
    fdf::BindServer(netdevice_dispatcher_.get(), std::move(server_end), this);
  };

  // Register the callback to handler.
  netdev::Service::InstanceHandler handler({.network_device_impl = std::move(protocol)});

  auto status = driver_->outgoing()->AddService<netdev::Service>(std::move(handler));
  if (status.is_error()) {
    fdf::error("Failed to add service to outgoing directory: {}", status.status_string());
    return status.error_value();
  }

  std::vector offers = compat_server_.CreateOffers2();
  offers.push_back(fdf::MakeOffer2<netdev::Service>());

  auto [controller_client, controller_server] =
      fidl::Endpoints<fuchsia_driver_framework::NodeController>::Create();

  auto args = fuchsia_driver_framework::NodeAddArgs{{
      .name = "virtio-net-netdev",
      .offers2 = std::move(offers),
      .bus_info = fuchsia_driver_framework::BusInfo{{
          .bus = fuchsia_driver_framework::BusType::kVirtio,
      }},
  }};

  auto result = fidl::Call(driver_->node())
                    ->AddChild({{
                        .args = std::move(args),
                        .controller = std::move(controller_server),
                        .node = {},
                    }});
  if (result.is_error()) {
    fdf::error("Failed to add net device child node: {}", result.error_value());
    return result.error_value().is_domain_error() ? ZX_ERR_INTERNAL
                                                  : result.error_value().framework_error().status();
  }
  netdev_child_ = std::move(controller_client);
  return ZX_OK;
}

void NetworkDevice::RemoveDevice() {
  // Remove the driver by destroying the node channel.
  driver_->take_node().reset();
}

zx_status_t NetworkDevice::AckFeatures(bool* is_status_supported, bool* is_multiqueue_supported,
                                       uint16_t* virtio_hdr_len) {
  const uint64_t supported_features = DeviceFeaturesSupported();

  if (!(supported_features & VIRTIO_NET_F_MAC)) {
    fdf::error("device does not have a given MAC address.");
    return ZX_ERR_NOT_SUPPORTED;
  }
  uint64_t enable_features = VIRTIO_NET_F_MAC;

  if (supported_features & VIRTIO_NET_F_STATUS) {
    enable_features |= VIRTIO_NET_F_STATUS;
    *is_status_supported = true;
  } else {
    *is_status_supported = false;
  }

  if (supported_features & VIRTIO_NET_F_MQ) {
    enable_features |= VIRTIO_NET_F_MQ;
    *is_multiqueue_supported = true;
  } else {
    *is_multiqueue_supported = false;
  }

  if (supported_features & VIRTIO_F_VERSION_1) {
    enable_features |= VIRTIO_F_VERSION_1;
    *virtio_hdr_len = sizeof(virtio_net_hdr_t);
  } else {
    // 5.1.6.1 Legacy Interface: Device Operation.
    //
    // The legacy driver only presented num_buffers in the struct
    // virtio_net_hdr when VIRTIO_NET_F_MRG_RXBUF was negotiated; without
    // that feature the structure was 2 bytes shorter.
    //
    // https://docs.oasis-open.org/virtio/virtio/v1.1/csprd01/virtio-v1.1-csprd01.html#x1-2050006
    *virtio_hdr_len = sizeof(virtio_legacy_net_hdr_t);
  }

  DriverFeaturesAck(enable_features);
  return ZX_OK;
}

void NetworkDevice::Shutdown() {
  if (netdevice_dispatcher_.get()) {
    netdevice_dispatcher_.ShutdownAsync();
    netdevice_dispatcher_shutdown_.Wait();
    netdevice_dispatcher_.reset();
  }
  {
    fbl::AutoLock lock(&state_lock_);
    // Destroy the existing client by assigning a default constructed object to it.
    ifc_ = {};
  }
  if (irq_thread_started_) {
    // The Release call assumes that it's safe to join the IRQ thread which is only true if it was
    // created and started.
    virtio::Device::Release();
    irq_thread_started_ = false;
  }
}

void NetworkDevice::IrqRingUpdate() {
  for (;;) {
    bool again = IrqRingUpdateInternal();
    if (!again) {
      break;
    }
  }
}

bool NetworkDevice::IrqRingUpdateInternal() {
  network::SharedAutoLock state_lock(&state_lock_);
  if (!ifc_.is_valid()) {
    return false;
  }

  bool more_work = false;

  fdf::Arena arena(0u);

  std::array<netdev::wire::TxResult, kMaxDepth> tx_results;
  auto tx_it = tx_results.begin();
  {
    std::lock_guard lock(tx_lock_);
    tx_.SetNoInterrupt();
    // Ring::IrqRingUpdate will call this lambda on each tx buffer completed
    // by the underlying device since the last IRQ.
    tx_.IrqRingUpdate([this, &tx_it](vring_used_elem* used_elem) {
      []() __TA_ASSERT(tx_lock_) {}();
      uint16_t id = static_cast<uint16_t>(used_elem->id & 0xffff);
      ZX_ASSERT_MSG(id < kMaxDepth && tx_in_flight_active_[id], "%d is not active", id);
      *tx_it++ = {.id = tx_in_flight_buffer_ids_[id], .status = ZX_OK};
      tx_in_flight_active_[id] = false;
      tx_.FreeDesc(id);
    });
    more_work |= tx_.ClearNoInterruptCheckHasWork();
  }
  if (size_t count = std::distance(tx_results.begin(), tx_it); count != 0) {
    if (fidl::OneWayStatus status = ifc_.buffer(arena)->CompleteTx(
            fidl::VectorView<netdev::wire::TxResult>::FromExternal(tx_results.data(), count));
        !status.ok()) {
      fdf::error("Failed to complete {} TX buffers: {}", count, status.FormatDescription());
      RemoveDevice();
      return false;
    }
  }

  std::array<netdev::wire::RxBuffer, kMaxDepth> rx_buffers;
  std::array<netdev::wire::RxBufferPart, kMaxDepth> rx_buffers_parts;
  auto rx_part_it = rx_buffers_parts.begin();
  auto rx_it = rx_buffers.begin();
  {
    std::lock_guard lock(rx_lock_);
    rx_.SetNoInterrupt();
    // Ring::IrqRingUpdate will call this lambda on each rx buffer filled by
    // the underlying device since the last IRQ.
    rx_.IrqRingUpdate([this, &rx_it, &rx_part_it](vring_used_elem* used_elem) {
      []() __TA_ASSERT(rx_lock_) {}();
      uint16_t id = static_cast<uint16_t>(used_elem->id & 0xffff);
      Descriptor in_flight = rx_in_flight_.Pop();
      ZX_ASSERT_MSG(in_flight.ring_id == id,
                    "rx ring and FIFO id mismatch (%d != %d for buffer %d)", in_flight.ring_id, id,
                    in_flight.buffer_id);
      vring_desc& desc = *rx_.DescFromIndex(id);
      // Driver does not merge rx buffers.
      ZX_ASSERT_MSG((desc.flags & VRING_DESC_F_NEXT) == 0, "descriptor chaining not supported");

      auto parts_list = rx_part_it;
      uint32_t len = used_elem->len - virtio_hdr_len_;
      ZX_ASSERT_MSG(used_elem->len >= virtio_hdr_len_,
                    "got buffer (%u) smaller than virtio header (%u)", used_elem->len,
                    virtio_hdr_len_);
      fdf::trace("Receiving {} bytes (hdrlen = {}):", len, virtio_hdr_len_);
      if (driver_->logger().GetSeverity() <= FUCHSIA_LOG_TRACE) {
        virtio_dump_desc(&desc);
      }
      *rx_part_it++ = {
          .id = in_flight.buffer_id,
          .offset = virtio_hdr_len_,
          .length = len,
      };
      *rx_it++ = netdev::wire::RxBuffer{
          .meta =
              {
                  .port = kPortId,
                  .frame_type = fuchsia_hardware_network::wire::FrameType::kEthernet,
              },
          .data = fidl::VectorView<netdev::wire::RxBufferPart>::FromExternal(&*parts_list, 1),
      };
      rx_.FreeDesc(id);
    });
    more_work |= rx_.ClearNoInterruptCheckHasWork();
    if (rx_in_flight_.Empty() && !rx_space_requested_) {
      rx_space_requested_ = true;
      if (fidl::OneWayStatus status = ifc_.buffer(arena)->RequestRxSpace(rx_depth_); !status.ok()) {
        fdf::error("Failed to request {} RX space: {}", rx_depth_, status.FormatDescription());
        RemoveDevice();
        return false;
      }
    }
  }
  if (size_t count = std::distance(rx_buffers.begin(), rx_it); count != 0) {
    if (fidl::OneWayStatus status = ifc_.buffer(arena)->CompleteRx(
            fidl::VectorView<netdev::wire::RxBuffer>::FromExternal(rx_buffers.data(), count));
        !status.ok()) {
      fdf::error("Failed to complete {} TX buffers: {}", count, status.FormatDescription());
      RemoveDevice();
      return false;
    }
  }

  return more_work;
}

void NetworkDevice::IrqConfigChange() {
  network::SharedAutoLock lock(&state_lock_);
  if (!ifc_.is_valid()) {
    return;
  }

  const fuchsia_hardware_network::PortStatus port_status = ReadStatus();
  fdf::Arena arena(0u);
  if (fidl::OneWayStatus status =
          ifc_.buffer(arena)->PortStatusChanged(kPortId, fidl::ToWire(arena, port_status));
      !status.ok()) {
    fdf::error("Failed to send port status changed: {}", status.FormatDescription());
    RemoveDevice();
  }
}

fuchsia_hardware_network::PortStatus NetworkDevice::ReadStatus() const {
  virtio_net_config config;
  CopyDeviceConfig(&config, sizeof(config));
  return fuchsia_hardware_network::PortStatus{}
      .flags(IsLinkActive(config, is_status_supported_)
                 ? fuchsia_hardware_network::wire::StatusFlags::kOnline
                 : fuchsia_hardware_network::wire::StatusFlags{})
      .mtu(kMtu);
}

void NetworkDevice::Init(
    fuchsia_hardware_network_driver::wire::NetworkDeviceImplInitRequest* request, fdf::Arena& arena,
    InitCompleter::Sync& completer) {
  fbl::AutoLock lock(&state_lock_);
  ifc_.Bind(std::move(request->iface), netdevice_dispatcher_.get());

  auto [client, server] = fdf::Endpoints<fuchsia_hardware_network_driver::NetworkPort>::Create();
  fdf::BindServer(netdevice_dispatcher_.get(), std::move(server), this);

  ifc_.buffer(arena)
      ->AddPort(kPortId, std::move(client))
      .Then([completer = completer.ToAsync()](
                fdf::WireUnownedResult<netdev::NetworkDeviceIfc::AddPort>& result) mutable {
        fdf::Arena arena(0u);
        if (!result.ok()) {
          fdf::error("failed to add port: {}", result.FormatDescription());
          completer.buffer(arena).Reply(result.status());
          return;
        }
        if (result->status != ZX_OK) {
          fdf::error("failed to add port: {}", zx_status_get_string(result->status));
          completer.buffer(arena).Reply(result->status);
          return;
        }
        completer.buffer(arena).Reply(ZX_OK);
      });
}

void NetworkDevice::Start(fdf::Arena& arena, StartCompleter::Sync& completer) {
  zx_status_t status = [&]() {
    // Always reset the device and reconfigure so we know where we are.
    DeviceReset();
    WaitForDeviceReset();
    DriverStatusAck();
    bool is_status_supported, is_multiqueue_supported;
    uint16_t header_length;
    if (zx_status_t status =
            AckFeatures(&is_status_supported, &is_multiqueue_supported, &header_length);
        status != ZX_OK) {
      fdf::error("failed to ack features: {}", zx_status_get_string(status));
      return status;
    }
    ZX_ASSERT_MSG(is_status_supported == is_status_supported_,
                  "status support changed from %u to %u between init and start",
                  is_status_supported_, is_status_supported);
    ZX_ASSERT_MSG(is_multiqueue_supported == is_multiqueue_supported_,
                  "max queue support changed from %u to %u between init and start",
                  is_multiqueue_supported_, is_multiqueue_supported);
    ZX_ASSERT_MSG(header_length == virtio_hdr_len_,
                  "header length changed from %u to %u between init and start", virtio_hdr_len_,
                  header_length);

    if (zx_status_t status = DeviceStatusFeaturesOk(); status != ZX_OK) {
      fdf::error("{}: Feature negotiation failed ({})", tag(), zx_status_get_string(status));
      return status;
    }

    // Allocate virtqueues.
    {
      std::lock_guard rx_lock(rx_lock_);
      std::lock_guard tx_lock(tx_lock_);
      Ring rx_queue(this);
      if (zx_status_t status = rx_queue.Init(kRxId, rx_depth_); status != ZX_OK) {
        fdf::error("failed to allocate rx virtqueue: {}", zx_status_get_string(status));
        return status;
      }
      rx_ = std::move(rx_queue);
      rx_space_requested_ = false;
      Ring tx_queue(this);
      if (zx_status_t status = tx_queue.Init(kTxId, tx_depth_); status != ZX_OK) {
        fdf::error("failed to allocate tx virtqueue: {}", zx_status_get_string(status));
        return status;
      }
      tx_ = std::move(tx_queue);
    }
    DriverStatusOk();

    // Acquire an exclusive state lock to prevent racing with an interrupt, and
    // update our status after bringing the device online.
    {
      fbl::AutoLock lock(&state_lock_);
      if (ifc_.is_valid()) {
        const fuchsia_hardware_network::PortStatus port_status = ReadStatus();
        if (fidl::OneWayStatus status =
                ifc_.buffer(arena)->PortStatusChanged(kPortId, fidl::ToWire(arena, port_status));
            !status.ok()) {
          fdf::error("Failed to send port status changed: {}", status.FormatDescription());
          completer.Close(status.status());
          RemoveDevice();
          return status.status();
        }
      }
    }
    return ZX_OK;
  }();
  completer.buffer(arena).Reply(status);
}

void NetworkDevice::Stop(fdf::Arena& arena, StopCompleter::Sync& completer) {
  DeviceReset();
  WaitForDeviceReset();
  // Once the device is reset, report that the link is offline since we're not
  // going to get config interrupts anymore.
  if (is_status_supported_) {
    fbl::AutoLock lock(&state_lock_);
    if (!ifc_.is_valid()) {
      return;
    }

    fuchsia_hardware_network::PortStatus port_status = ReadStatus();
    port_status.flags().value() &= ~fuchsia_hardware_network::wire::StatusFlags::kOnline;

    if (fidl::OneWayStatus status =
            ifc_.buffer(arena)->PortStatusChanged(kPortId, fidl::ToWire(arena, port_status));
        !status.ok()) {
      fdf::error("Failed to send port status changed: {}", status.FormatDescription());
      completer.Close(status.status());
      RemoveDevice();
      return;
    }
  }

  // Return all pending buffers.
  {
    network::SharedAutoLock state_lock(&state_lock_);
    // Pending tx buffers.
    {
      std::array<netdev::wire::TxResult, kMaxDepth> tx_return;
      auto iter = tx_return.begin();
      {
        std::lock_guard lock(tx_lock_);
        // Free all TX ring entries to prevent the IRQ handler from completing these buffers.
        tx_.IrqRingUpdate([this](vring_used_elem* used_elem) {
          []() __TA_ASSERT(tx_lock_) {}();
          const uint16_t id = static_cast<uint16_t>(used_elem->id & 0xffff);
          tx_.FreeDesc(id);
        });

        for (int i = 0; i < kMaxDepth; ++i) {
          if (tx_in_flight_active_[i]) {
            *iter++ = {
                .id = tx_in_flight_buffer_ids_[i],
                .status = ZX_ERR_BAD_STATE,
            };
            tx_in_flight_active_[i] = false;
          }
        }
      }
      if (iter != tx_return.begin()) {
        const size_t count = std::distance(tx_return.begin(), iter);
        if (fidl::OneWayStatus status = ifc_.buffer(arena)->CompleteTx(
                fidl::VectorView<netdev::wire::TxResult>::FromExternal(tx_return.data(), count));
            !status.ok()) {
          fdf::error("Failed to complete {} TX buffers: {}", count, status.FormatDescription());
          completer.Close(status.status());
          RemoveDevice();
          return;
        }
      }
    }
    // Pending rx buffers.
    {
      std::array<netdev::wire::RxBuffer, kMaxDepth> rx_return;
      std::array<netdev::wire::RxBufferPart, kMaxDepth> rx_return_parts;
      auto iter = rx_return.begin();
      auto parts_iter = rx_return_parts.begin();
      {
        std::lock_guard lock(rx_lock_);
        // Free all RX ring entries to prevent the IRQ handler from completing these buffers.
        rx_.IrqRingUpdate([this](vring_used_elem* used_elem) {
          []() __TA_ASSERT(rx_lock_) {}();
          const uint16_t id = static_cast<uint16_t>(used_elem->id & 0xffff);
          rx_.FreeDesc(id);
        });

        while (!rx_in_flight_.Empty()) {
          Descriptor d = rx_in_flight_.Pop();
          *iter++ = {
              .meta = {.frame_type = fuchsia_hardware_network::FrameType::kEthernet},
              .data = fidl::VectorView<netdev::wire::RxBufferPart>::FromExternal(&*parts_iter, 1),
          };
          *parts_iter++ = {.id = d.buffer_id};
        }
        rx_space_requested_ = false;
      }
      if (iter != rx_return.begin()) {
        const size_t count = std::distance(rx_return.begin(), iter);
        if (fidl::OneWayStatus status = ifc_.buffer(arena)->CompleteRx(
                fidl::VectorView<netdev::wire::RxBuffer>::FromExternal(rx_return.data(), count));
            !status.ok()) {
          fdf::error("Failed to complete {} RX buffers: {}", count, status.FormatDescription());
          completer.Close(status.status());
          RemoveDevice();
          return;
        }
      }
    }
  }

  completer.buffer(arena).Reply();
}

void NetworkDevice::GetInfo(
    fdf::Arena& arena,
    fdf::WireServer<netdev::NetworkDeviceImpl>::GetInfoCompleter::Sync& completer) {
  netdev::wire::DeviceImplInfo info =
      netdev::wire::DeviceImplInfo::Builder(arena)
          .tx_depth(tx_depth_)
          .rx_depth(rx_depth_)
          .rx_threshold(static_cast<uint16_t>(rx_depth_ / 2))
          .max_buffer_parts(1)
          .max_buffer_length(kFrameSize)
          .buffer_alignment(kBufferAlignment)
          .min_rx_buffer_length(kFrameSize)
          .min_rx_buffers(rx_depth_ / 2)
          // Minimum Ethernet frame size on the wire according to
          // IEEE 802.3, minus the frame check sequence.
          .min_tx_buffer_length(60)
          .tx_head_length(virtio_hdr_len_)
          .rx_buffer_management(netdev::wire::RxBufferManagement::WithSimple(
              arena,
              netdev::wire::Simple{
                  // Empirically chosen by using a local emulator.
                  .delay_budget = ZX_USEC(500),
              }))
          .Build();
  completer.buffer(arena).Reply(info);
}

void NetworkDevice::QueueTx(
    fuchsia_hardware_network_driver::wire::NetworkDeviceImplQueueTxRequest* request,
    fdf::Arena& arena, QueueTxCompleter::Sync& completer) {
  network::SharedAutoLock lock(&state_lock_);
  std::lock_guard tx_lock(tx_lock_);
  for (const auto& buffer : request->buffers) {
    ZX_DEBUG_ASSERT_MSG(buffer.data.size() == 1, "received unsupported scatter gather buffer %zu",
                        buffer.data.size());

    const netdev::wire::BufferRegion& data = buffer.data[0];

    // Grab a free descriptor.
    uint16_t id;
    vring_desc* desc = tx_.AllocDescChain(1, &id);
    ZX_ASSERT_MSG(desc != nullptr, "failed to allocate descriptor");

    // Add the data to be sent.
    VmoStore::StoredVmo* stored_vmo = vmo_store_.GetVmo(data.vmo);
    ZX_ASSERT_MSG(stored_vmo != nullptr, "invalid VMO id %d", data.vmo);

    // Get a pointer to the header. Casting it to net header structs is valid
    // because we requested alignment and tx header in
    // NetworkDeviceImpl.GetInfo.
    void* tx_hdr = stored_vmo->data().subspan(data.offset, virtio_hdr_len_).data();

    constexpr virtio_legacy_net_hdr_t kBaseHeader = {
        // If VIRTIO_NET_F_CSUM is not negotiated, the driver MUST set flags to
        // zero and SHOULD supply a fully checksummed packet to the device.
        .flags = 0,
        // If none of the VIRTIO_NET_F_HOST_TSO4, TSO6 or UFO options have been
        // negotiated, the driver MUST set gso_type to VIRTIO_NET_HDR_GSO_NONE.
        .gso_type = VIRTIO_NET_HDR_GSO_NONE,
    };

    switch (virtio_hdr_len_) {
      case sizeof(virtio_net_hdr_t):
        *static_cast<virtio_net_hdr_t*>(tx_hdr) = {
            .base = kBaseHeader,
            // 5.1.6.2.1 Driver Requirements: Packet Transmission
            //
            // The driver MUST set num_buffers to zero.
            //
            // Implementation note: This field doesn't exist if neither
            // |VIRTIO_F_VERSION_1| or |VIRTIO_F_MRG_RXBUF| have been negotiated.
            //
            // https://docs.oasis-open.org/virtio/virtio/v1.1/csprd01/virtio-v1.1-csprd01.html#x1-2050006
            .num_buffers = 0,
        };
        break;
      case sizeof(virtio_legacy_net_hdr_t):
        *static_cast<virtio_legacy_net_hdr_t*>(tx_hdr) = kBaseHeader;
        break;
      default:
        ZX_PANIC("invalid virtio header length %d", virtio_hdr_len_);
    }

    fzl::PinnedVmo::Region region;
    size_t actual_regions = 0;
    zx_status_t status =
        stored_vmo->GetPinnedRegions(data.offset, data.length, &region, 1, &actual_regions);
    ZX_ASSERT_MSG(status == ZX_OK, "failed to retrieve pinned region %s (actual=%zu)",
                  zx_status_get_string(status), actual_regions);

    *desc = {
        .addr = region.phys_addr,
        .len = static_cast<uint32_t>(data.length),
    };
    tx_in_flight_buffer_ids_[id] = buffer.id;
    tx_in_flight_active_[id] = true;
    // Submit the descriptor and notify the back-end.
    if (driver_->logger().GetSeverity() <= FUCHSIA_LOG_TRACE) {
      virtio_dump_desc(desc);
    }
    fdf::trace("Sending {} bytes (hdrlen = {}):", data.length, virtio_hdr_len_);
    tx_.SubmitChain(id);
  }
  if (!tx_.NoNotify()) {
    tx_.Kick();
  }
}

void NetworkDevice::QueueRxSpace(
    fuchsia_hardware_network_driver::wire::NetworkDeviceImplQueueRxSpaceRequest* request,
    fdf::Arena& arena, QueueRxSpaceCompleter::Sync& completer) {
  network::SharedAutoLock lock(&state_lock_);
  std::lock_guard rx_lock(rx_lock_);
  rx_space_requested_ = false;
  for (const auto& buffer : request->buffers) {
    const netdev::wire::BufferRegion& data = buffer.region;

    // Grab a free descriptor.
    uint16_t id;
    vring_desc* desc = rx_.AllocDescChain(1, &id);
    ZX_ASSERT_MSG(desc != nullptr, "failed to allocate descriptor");

    // Add the data to be sent.
    VmoStore::StoredVmo* stored_vmo = vmo_store_.GetVmo(data.vmo);
    ZX_ASSERT_MSG(stored_vmo != nullptr, "invalid VMO id %d", data.vmo);

    fzl::PinnedVmo::Region region;
    size_t actual_regions = 0;
    zx_status_t status =
        stored_vmo->GetPinnedRegions(data.offset, data.length, &region, 1, &actual_regions);
    ZX_ASSERT_MSG(status == ZX_OK, "failed to retrieve pinned region %s (actual=%zu)",
                  zx_status_get_string(status), actual_regions);
    *desc = {
        .addr = region.phys_addr,
        .len = static_cast<uint32_t>(data.length),
        .flags = VRING_DESC_F_WRITE,
    };
    rx_in_flight_.Push({
        .buffer_id = buffer.id,
        .ring_id = id,
    });
    // Submit the descriptor and notify the back-end.
    if (driver_->logger().GetSeverity() <= FUCHSIA_LOG_TRACE) {
      virtio_dump_desc(desc);
    }
    fdf::trace("Queueing rx space with {} bytes:", data.length);
    rx_.SubmitChain(id);
  }
  if (!rx_.NoNotify()) {
    rx_.Kick();
  }
}

void NetworkDevice::PrepareVmo(
    fuchsia_hardware_network_driver::wire::NetworkDeviceImplPrepareVmoRequest* request,
    fdf::Arena& arena, PrepareVmoCompleter::Sync& completer) {
  zx_status_t status = [&]() {
    fbl::AutoLock vmo_lock(&state_lock_);
    return vmo_store_.RegisterWithKey(request->id, std::move(request->vmo));
  }();
  completer.buffer(arena).Reply(status);
}

void NetworkDevice::ReleaseVmo(
    fuchsia_hardware_network_driver::wire::NetworkDeviceImplReleaseVmoRequest* request,
    fdf::Arena& arena, ReleaseVmoCompleter::Sync& completer) {
  fbl::AutoLock vmo_lock(&state_lock_);
  if (zx::result<zx::vmo> status = vmo_store_.Unregister(request->id);
      status.status_value() != ZX_OK) {
    fdf::error("failed to release vmo id = {}: {}", request->id, status);
  }
  completer.buffer(arena).Reply();
}

void NetworkDevice::GetInfo(
    fdf::Arena& arena, fdf::WireServer<netdev::NetworkPort>::GetInfoCompleter::Sync& completer) {
  constexpr fuchsia_hardware_network::wire::FrameType kRxTypesList[] = {
      fuchsia_hardware_network::wire::FrameType::kEthernet};
  constexpr fuchsia_hardware_network::wire::FrameTypeSupport kTxTypesList[] = {{
      .type = fuchsia_hardware_network::wire::FrameType::kEthernet,
      .features = fuchsia_hardware_network::wire::kFrameFeaturesRaw,
  }};

  fuchsia_hardware_network::wire::PortBaseInfo info =
      fuchsia_hardware_network::wire::PortBaseInfo::Builder(arena)
          .port_class(fuchsia_hardware_network::wire::PortClass::kEthernet)
          .rx_types(kRxTypesList)
          .tx_types(kTxTypesList)
          .Build();
  completer.buffer(arena).Reply(info);
}

void NetworkDevice::GetStatus(fdf::Arena& arena, GetStatusCompleter::Sync& completer) {
  completer.buffer(arena).Reply(fidl::ToWire(arena, ReadStatus()));
}

void NetworkDevice::SetActive(
    fuchsia_hardware_network_driver::wire::NetworkPortSetActiveRequest* request, fdf::Arena& arena,
    SetActiveCompleter::Sync& completer) {}

void NetworkDevice::GetMac(fdf::Arena& arena, GetMacCompleter::Sync& completer) {
  auto [client, server] = fdf::Endpoints<fuchsia_hardware_network_driver::MacAddr>::Create();
  fdf::BindServer(netdevice_dispatcher_.get(), std::move(server), this);
  completer.buffer(arena).Reply(std::move(client));
}

void NetworkDevice::Removed(fdf::Arena& arena, RemovedCompleter::Sync& completer) {
  // Do nothing.
}

void NetworkDevice::GetAddress(fdf::Arena& arena, GetAddressCompleter::Sync& completer) {
  fuchsia_net::wire::MacAddress mac;
  std::copy(mac_.octets.begin(), mac_.octets.end(), mac.octets.data());
  completer.buffer(arena).Reply(mac);
}

void NetworkDevice::GetFeatures(fdf::Arena& arena, GetFeaturesCompleter::Sync& completer) {
  netdev::wire::Features features =
      netdev::wire::Features::Builder(arena)
          .multicast_filter_count(0)
          .supported_modes(netdev::wire::SupportedMacFilterMode::kPromiscuous)
          .Build();
  completer.buffer(arena).Reply(features);
}

void NetworkDevice::SetMode(fuchsia_hardware_network_driver::wire::MacAddrSetModeRequest* request,
                            fdf::Arena& arena, SetModeCompleter::Sync& completer) {
  /* We only support promiscuous mode, nothing to do */
  ZX_ASSERT_MSG(request->mode == fuchsia_hardware_network::wire::MacFilterMode::kPromiscuous,
                "unsupported mode %u", static_cast<uint32_t>(request->mode));
  ZX_ASSERT_MSG(request->multicast_macs.size() == 0, "unsupported multicast count %zu",
                request->multicast_macs.size());
  completer.buffer(arena).Reply();
}

}  // namespace virtio
