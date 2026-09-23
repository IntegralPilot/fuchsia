// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/virtualization/bin/vmm/device/virtio_net/src/cpp/guest_ethernet.h"

#include <lib/async/cpp/task.h>
#include <lib/sync/cpp/completion.h>
#include <lib/syslog/cpp/macros.h>

#include "src/virtualization/bin/vmm/device/virtio_net/src/cpp/guest_ethernet_interface.h"

namespace {
// Maximum Transmission Unit (MTU): the maximum supported size of an incoming/outgoing frame.
constexpr uint32_t kMtu = 1500;

// Ensure the given buffer can be supported by virtio-net.
bool IsTxBufferSupported(const fuchsia_hardware_network_driver::wire::TxBuffer& buffer) {
  // Ensure no padding on the head/tail.
  if (unlikely(buffer.head_length != 0)) {
    FX_LOGS_FIRST_N(WARNING, 10) << "Packet from host contained invalid head length: "
                                 << buffer.head_length;
    return false;
  }
  if (unlikely(buffer.tail_length != 0)) {
    FX_LOGS_FIRST_N(WARNING, 10) << "Packet from host contained invalid tail length: "
                                 << buffer.tail_length;
    return false;
  }

  // Ensure the default port is being used.
  if (unlikely(buffer.meta.port != GuestEthernet::kPortId)) {
    FX_LOGS_FIRST_N(WARNING, 10) << "Packet from host contained invalid device port: "
                                 << buffer.meta.port;
    return false;
  }

  // Ensure the buffer contains a standard ethernet frame.
  if (unlikely(buffer.meta.frame_type != fuchsia_hardware_network::wire::FrameType::kEthernet)) {
    FX_LOGS_FIRST_N(WARNING, 10) << "Packet from host contained unsupported type: "
                                 << static_cast<uint8_t>(buffer.meta.frame_type);
    return false;
  }

  // We currently only support a single data buffer.
  if (unlikely(buffer.data.size() != 1)) {
    FX_LOGS_FIRST_N(WARNING, 10) << "Packet from host contained multiple data buffers";
    return false;
  }

  return true;
}

class GuestEthernetBinder : public network::NetworkDeviceImplBinder {
 public:
  GuestEthernetBinder(GuestEthernet* adapter) : adapter_(adapter) {}
  ~GuestEthernetBinder() override = default;

  zx::result<fdf::ClientEnd<fuchsia_hardware_network_driver::NetworkDeviceImpl>> Bind() override {
    return zx::ok(adapter_->BindDriver());
  }

 private:
  GuestEthernet* adapter_;  // unowned pointer to adapter
};

}  // namespace

GuestEthernet::GuestEthernet(fdf::Dispatcher* sync_dispatcher,
                             const network::DeviceInterfaceDispatchers& netdev_dispatchers)
    : sync_dispatcher_(sync_dispatcher),
      netdev_dispatchers_(netdev_dispatchers),
      trace_provider_(netdev_dispatchers.impl_->async_dispatcher()),
      svc_(sys::ServiceDirectory::CreateFromNamespace()),
      tx_completion_queue_(kPortId, netdev_dispatchers.impl_->async_dispatcher(), &parent_),
      rx_completion_queue_(netdev_dispatchers.impl_->async_dispatcher(), &parent_) {}

GuestEthernet::~GuestEthernet() { Teardown(); }

void GuestEthernet::Teardown() {
  netstack_.Unbind();
  network_.Unbind();
  interface_registration_.Unbind();

  // Tear down the device interface and wait for completion before
  // destroying the completion queues. This drains pending callbacks
  // that capture raw |this| pointers through the completion queues.
  libsync::Completion teardown_complete;
  device_interface_->Teardown(
      [&teardown_complete]() { teardown_complete.Signal(); });
  teardown_complete.Wait();
}

zx_status_t GuestEthernet::Initialize(const void* rust_guest_ethernet, const uint8_t* mac,
                                      size_t mac_len, bool enable_bridge) {
  set_status_ = [rust_guest_ethernet](zx_status_t status) {
    guest_ethernet_set_status(rust_guest_ethernet, status);
  };
  ready_for_guest_tx_ = [rust_guest_ethernet]() {
    guest_ethernet_ready_for_tx(rust_guest_ethernet);
  };
  send_guest_rx_ = [rust_guest_ethernet](const uint8_t* data, size_t len, uint32_t buffer_id) {
    guest_ethernet_receive_rx(rust_guest_ethernet, data, len, buffer_id);
  };

  if (mac_len != VIRTIO_ETH_MAC_SIZE) {
    FX_LOGS(ERROR) << "Virtio-net device received an incorrectly sized MAC address. Expected "
                   << VIRTIO_ETH_MAC_SIZE << ", got " << mac_len << ".";
    return ZX_ERR_INVALID_ARGS;
  }
  std::memcpy(mac_address_, mac, VIRTIO_ETH_MAC_SIZE);

  // Initialize is running on the Rust main thread, but CreateGuestInterface must be run on a driver
  // framework dispatcher. Additionally it must be a synchronous dispatcher that allows synchronous
  // calls. The Rust device will wait for this task to complete.
  async::PostTask(sync_dispatcher_->async_dispatcher(), [this, enable_bridge]() {
    this->set_status_(this->CreateGuestInterface(enable_bridge));
  });

  return ZX_OK;
}

zx_status_t GuestEthernet::CreateGuestInterface(bool enable_bridge) {
  // Connect to netstack.
  netstack_.set_error_handler([this](zx_status_t status) {
    FX_PLOGS(WARNING, status) << "Lost connection to netstack (ControlPtr closed)";
    this->set_status_(status);
  });
  zx_status_t status = svc_->Connect<::fuchsia::net::virtualization::Control>(
      netstack_.NewRequest(netdev_dispatchers_.impl_->async_dispatcher()));
  if (status != ZX_OK) {
    FX_PLOGS(WARNING, status) << "Failed to connect to netstack";
    return status;
  }

  // Set up the GuestEthernet device.
  zx::result<std::unique_ptr<network::NetworkDeviceInterface>> device_interface =
      network::NetworkDeviceInterface::Create(netdev_dispatchers_,
                                              std::make_unique<GuestEthernetBinder>(this));
  if (device_interface.is_error()) {
    FX_PLOGS(WARNING, device_interface.error_value()) << "Failed to create guest interface";
    return device_interface.error_value();
  }
  device_interface_ = std::move(device_interface.value());

  // Create a connection to the device.
  fidl::ClientEnd<::fuchsia_hardware_network::Port> port;
  status = device_interface_->BindPort(kPortId, fidl::ServerEnd<::fuchsia_hardware_network::Port>(
                                                    ::fidl::CreateEndpoints(&port).value()));
  if (status != ZX_OK) {
    FX_PLOGS(ERROR, status) << "Internal error: could not bind to GuestEthernet server";
    return status;
  }

  // Create a new network group.
  fuchsia::net::virtualization::Config config;
  if (enable_bridge) {
    // See b/410037697 for bridging support.
    FX_LOGS(ERROR) << "Bridging is not currently supported";
    return ZX_ERR_NOT_SUPPORTED;
  } else {
    // See https://fxbug.dev/42052026 for NAT support.
    FX_LOGS(ERROR) << "NAT is not currently supported";
    return ZX_ERR_NOT_SUPPORTED;
  }
  netstack_->CreateNetwork(std::move(config),
                           network_.NewRequest(sync_dispatcher_->async_dispatcher()));

  // Add our GuestEthernet device to the network.
  interface_registration_.set_error_handler([this](zx_status_t status) {
    FX_PLOGS(WARNING, status) << "Lost connection to netstack (InterfacePtr closed)";
    this->set_status_(status);
  });
  network_->AddPort(::fidl::InterfaceHandle<fuchsia::hardware::network::Port>(port.TakeChannel()),
                    interface_registration_.NewRequest(sync_dispatcher_->async_dispatcher()));

  return ZX_OK;
}

zx_status_t GuestEthernet::Send(const void* data, uint16_t length) {
  std::lock_guard guard(mutex_);

  if (!io_vmo_) {
    FX_LOGS(WARNING) << "Send called before IO buffer was set up";
    return ZX_ERR_BAD_STATE;
  }

  if (available_buffers_.empty()) {
    return ZX_ERR_SHOULD_WAIT;
  }

  // Get a buffer.
  AvailableBuffer buffer = available_buffers_.back();
  available_buffers_.pop_back();

  // Ensure the packet will fit in the buffer.
  if (length > buffer.region.size()) {
    FX_LOGS(WARNING) << "Incoming packet of size " << length
                     << " could not be stored in buffer of size " << buffer.region.size();
    // Drop the packet.
    return ZX_ERR_NO_RESOURCES;
  }

  // Copy data from the virtio ring to memory shared with netstack.
  memcpy(buffer.region.data(), data, length);

  // Return the buffer to our parent device.
  TxComplete(buffer.buffer_id, length);

  return ZX_OK;
}

void GuestEthernet::Complete(uint32_t buffer_id, zx_status_t status) {
  std::lock_guard guard(mutex_);
  RxComplete(buffer_id, status);
  FX_DCHECK(in_flight_rx_ > 0);
  in_flight_rx_--;

  // Stop the device if we are shutting down, and no more packets are pending.
  FinishShutdownIfRequired();
}

void GuestEthernet::TxComplete(uint32_t buffer_id, size_t length) {
  FX_DCHECK(length < UINT32_MAX);
  tx_completion_queue_.Complete(buffer_id, static_cast<uint32_t>(length));
}

void GuestEthernet::RxComplete(uint32_t buffer_id, zx_status_t status) {
  rx_completion_queue_.Complete(buffer_id, status);
}

zx::result<cpp20::span<uint8_t>> GuestEthernet::GetIoRegion(uint8_t vmo_id, uint64_t offset,
                                                            uint64_t length) {
  // Ensure the VMO ID matches what we have mapped in.
  if (vmo_id != vmo_id_) {
    return zx::error(ZX_ERR_NOT_FOUND);
  }

  // Ensure the child range does not overflow.
  uint64_t end;  // end of the range, exclusive
  if (add_overflow(offset, length, &end)) {
    return zx::error(ZX_ERR_OUT_OF_RANGE);
  }

  // Ensure the range is within the IO region.
  if (end > io_size_) {
    return zx::error(ZX_ERR_OUT_OF_RANGE);
  }

  return zx::success(cpp20::span<uint8_t>(io_addr_ + offset, length));
}

void GuestEthernet::FinishShutdownIfRequired() {
  if (unlikely(state_ == State::kShuttingDown && in_flight_rx_ == 0)) {
    async::PostTask(netdev_dispatchers_.impl_->async_dispatcher(),
                    std::move(shutdown_complete_callback_));
  }
}

fdf::ClientEnd<fuchsia_hardware_network_driver::NetworkDeviceImpl> GuestEthernet::BindDriver() {
  auto [client, server] =
      fdf::Endpoints<fuchsia_hardware_network_driver::NetworkDeviceImpl>::Create();
  fdf::BindServer(netdev_dispatchers_.impl_->get(), std::move(server), this);
  return std::move(client);
}

void GuestEthernet::Init(
    fuchsia_hardware_network_driver::wire::NetworkDeviceImplInitRequest* request, fdf::Arena& arena,
    InitCompleter::Sync& completer) {
  FX_CHECK(!parent_.is_valid()) << "NetworkDeviceImplInit called multiple times";
  parent_.Bind(std::move(request->iface), netdev_dispatchers_.impl_->get());

  auto [client, server] = fdf::Endpoints<fuchsia_hardware_network_driver::NetworkPort>::Create();
  fdf::BindServer(netdev_dispatchers_.impl_->get(), std::move(server), this);

  // Create port.
  parent_.buffer(arena)
      ->AddPort(kPortId, std::move(client))
      .ThenExactlyOnce(
          [this, completer = completer.ToAsync()](
              fdf::WireUnownedResult<fuchsia_hardware_network_driver::NetworkDeviceIfc::AddPort>&
                  result) mutable {
            fdf::Arena arena(0u);
            if (!result.ok()) {
              FX_LOGS(ERROR) << "Failed to add port: " << result.status_string();
              completer.buffer(arena).Reply(result.status());
              return;
            }

            // Inform our parent that the port is active.
            ZX_ASSERT(parent_.buffer(arena)
                          ->PortStatusChanged(kPortId, fidl::ToWire(arena, GetPortStatus()))
                          .ok());

            completer.buffer(arena).Reply(ZX_OK);
          });
}

void GuestEthernet::Start(fdf::Arena& arena, StartCompleter::Sync& completer) {
  zx_status_t result = ZX_OK;
  {
    std::lock_guard guard(mutex_);
    if (state_ == State::kStopped) {
      state_ = State::kStarted;
    } else {
      result = ZX_ERR_BAD_STATE;
    }
  }
  completer.buffer(arena).Reply(result);
}

void GuestEthernet::Stop(fdf::Arena& arena, StopCompleter::Sync& completer) {
  std::lock_guard guard(mutex_);
  FX_CHECK(state_ == State::kStarted) << "Attempted to stop device when it was in a bad state";

  // Return any available RX buffer.
  while (!available_buffers_.empty()) {
    TxComplete(available_buffers_.back().buffer_id, /*length=*/0);
    available_buffers_.pop_back();
  }

  // Wait for in-flight packets to be completed.
  state_ = State::kShuttingDown;
  shutdown_complete_callback_ = [completer = completer.ToAsync()]() mutable {
    fdf::Arena arena(0u);
    completer.buffer(arena).Reply();
  };

  // If no packets are in-flight, shut down the device.
  FinishShutdownIfRequired();
}

void GuestEthernet::GetInfo(
    fdf::Arena& arena,
    fdf::WireServer<fuchsia_hardware_network_driver::NetworkDeviceImpl>::GetInfoCompleter::Sync&
        completer) {
  auto info = fuchsia_hardware_network_driver::wire::DeviceImplInfo::Builder(arena)
                  // Allow at most kMaxTxDepth/kMaxRxDepth buffers in flight to TX/RX,
                  // respectively.
                  .tx_depth(HostToGuestCompletionQueue::kMaxDepth)
                  .rx_depth(GuestToHostCompletionQueue::kMaxDepth)

                  // Netstack should try to refresh our available RX buffers when they get
                  // to 50% of MaxRxDepth.
                  .rx_threshold(GuestToHostCompletionQueue::kMaxDepth / 2)

                  // We only support buffers with 1 memory region (i.e., no scatter/gather).
                  .max_buffer_parts(1)

                  // Buffers must be aligned to sizeof(uint64_t).
                  .buffer_alignment(sizeof(uint64_t))

                  // Require that all RX buffers are at least the size of our MTU.
                  .min_rx_buffer_length(kMtu)
                  .Build();
  completer.buffer(arena).Reply(info);
}

void GuestEthernet::QueueTx(
    fuchsia_hardware_network_driver::wire::NetworkDeviceImplQueueTxRequest* request,
    fdf::Arena& arena, QueueTxCompleter::Sync& completer) {
  std::lock_guard guard(mutex_);

  for (const auto& buffer : request->buffers) {
    // Reject transactions if we are not running.
    if (state_ != State::kStarted) {
      RxComplete(buffer.id, ZX_ERR_UNAVAILABLE);
      continue;
    }

    // Ignore unsupported buffers.
    if (!IsTxBufferSupported(buffer)) {
      RxComplete(buffer.id, ZX_ERR_NOT_SUPPORTED);
      continue;
    }

    // Get the data payload.
    FX_DCHECK(buffer.data.size() == 1);  // verified in IsTxBufferSupported
    const fuchsia_hardware_network_driver::wire::BufferRegion& region = buffer.data[0];

    // Get the caller-specified region.
    zx::result<cpp20::span<uint8_t>> memory_region =
        GetIoRegion(region.vmo, region.offset, region.length);
    if (memory_region.is_error()) {
      RxComplete(buffer.id, memory_region.error_value());
      continue;
    }

    this->send_guest_rx_(memory_region.value().data(), memory_region.value().size(), buffer.id);
    in_flight_rx_++;
  }
}

void GuestEthernet::QueueRxSpace(
    fuchsia_hardware_network_driver::wire::NetworkDeviceImplQueueRxSpaceRequest* request,
    fdf::Arena& arena, QueueRxSpaceCompleter::Sync& completer) {
  std::lock_guard guard(mutex_);

  // If we previously ran out of buffers, notify the guest that new ones are available.
  bool need_notify = available_buffers_.empty();

  for (const auto& buffer : request->buffers) {
    // Ensure the specified region is valid.
    zx::result<cpp20::span<uint8_t>> region =
        GetIoRegion(/*vmo_id=*/buffer.region.vmo, /*offset=*/buffer.region.offset,
                    /*length=*/buffer.region.length);
    if (region.is_error()) {
      // Return the buffer unused.
      RxComplete(buffer.id, /*length=*/0);
      continue;
    }

    // Record the buffer.
    available_buffers_.push_back({.buffer_id = buffer.id, .region = *region});
  }

  if (need_notify && !available_buffers_.empty()) {
    this->ready_for_guest_tx_();
  }
}

void GuestEthernet::PrepareVmo(
    fuchsia_hardware_network_driver::wire::NetworkDeviceImplPrepareVmoRequest* request,
    fdf::Arena& arena, PrepareVmoCompleter::Sync& completer) {
  std::lock_guard guard(mutex_);

  // Ensure another VMO hasn't already been mapped.
  if (io_vmo_.is_valid()) {
    FX_LOGS(INFO) << "Attempted to bind multiple VMOs";
    completer.buffer(arena).Reply(ZX_ERR_NO_RESOURCES);
    return;
  }

  // Get the VMO's size.
  uint64_t vmo_size;
  zx_status_t status = request->vmo.get_size(&vmo_size);
  if (status != ZX_OK) {
    FX_PLOGS(INFO, status) << "Failed to get VMO size";
    completer.buffer(arena).Reply(status);
    return;
  }

  // Map in the VMO.
  zx_vaddr_t mapped_address;
  status =
      zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_REQUIRE_NON_RESIZABLE,
                                 0, request->vmo, 0, vmo_size, &mapped_address);
  if (status != ZX_OK) {
    FX_PLOGS(INFO, status) << "Failed to map IO buffer";
    completer.buffer(arena).Reply(status);
    return;
  }

  vmo_id_ = request->id;
  io_addr_ = reinterpret_cast<uint8_t*>(mapped_address);
  io_vmo_ = std::move(request->vmo);
  io_size_ = vmo_size;
  completer.buffer(arena).Reply(ZX_OK);
}

void GuestEthernet::ReleaseVmo(
    fuchsia_hardware_network_driver::wire::NetworkDeviceImplReleaseVmoRequest* request,
    fdf::Arena& arena, ReleaseVmoCompleter::Sync& completer) {
  std::lock_guard guard(mutex_);

  // The NetworkDevice protocol states "`ReleaseVmo` is guaranteed to only be
  // called when the implementation holds no buffers that reference that `id`."
  FX_CHECK(io_vmo_.is_valid() && request->id != io_vmo_);
  FX_CHECK(available_buffers_.empty());

  // Unmap the VMO.
  zx_status_t status =
      zx::vmar::root_self()->unmap(reinterpret_cast<uintptr_t>(io_addr_), io_size_);
  FX_CHECK(status == ZX_OK) << "Could not unmap VMO: " << zx_status_get_string(status);

  // Reset state.
  io_vmo_.reset();
  vmo_id_.reset();
  io_addr_ = nullptr;
  io_size_ = 0;
}

void GuestEthernet::GetAddress(fdf::Arena& arena, GetAddressCompleter::Sync& completer) {
  fuchsia_net::wire::MacAddress mac;
  std::memcpy(mac.octets.data(), mac_address_, VIRTIO_ETH_MAC_SIZE);
  completer.buffer(arena).Reply(mac);
}

void GuestEthernet::GetFeatures(fdf::Arena& arena, GetFeaturesCompleter::Sync& completer) {
  auto features =
      fuchsia_hardware_network_driver::wire::Features::Builder(arena)
          // We don't support multicast filtering.
          .multicast_filter_count(0)

          // We don't perform any filtering.
          .supported_modes(
              fuchsia_hardware_network_driver::wire::SupportedMacFilterMode::kPromiscuous)
          .Build();

  completer.buffer(arena).Reply(features);
}

void GuestEthernet::SetMode(fuchsia_hardware_network_driver::wire::MacAddrSetModeRequest* request,
                            fdf::Arena& arena, SetModeCompleter::Sync& completer) {
  FX_LOGS(WARNING) << "MacAddrSetMode not implemented";
  completer.buffer(arena).Reply();
}

void GuestEthernet::GetInfo(
    fdf::Arena& arena,
    fdf::WireServer<fuchsia_hardware_network_driver::NetworkPort>::GetInfoCompleter::Sync&
        completer) {
  static constexpr std::array<fuchsia_hardware_network::wire::FrameType, 1> kRxTypes = {
      fuchsia_hardware_network::wire::FrameType::kEthernet,
  };
  static const std::array<fuchsia_hardware_network::wire::FrameTypeSupport, 1> kTxTypes = {
      fuchsia_hardware_network::wire::FrameTypeSupport{
          .type = fuchsia_hardware_network::wire::FrameType::kEthernet,
          .features = static_cast<uint32_t>(fuchsia_hardware_network::wire::EthernetFeatures::kRaw),
      },
  };

  // Advertise we are a virtual port implementing support for TX/RX of raw
  // ethernet frames.
  auto info = fuchsia_hardware_network::wire::PortBaseInfo::Builder(arena)
                  .port_class(fuchsia_hardware_network::wire::PortClass::kVirtual)
                  .rx_types(kRxTypes)
                  .tx_types(kTxTypes)
                  .Build();
  completer.buffer(arena).Reply(info);
}

void GuestEthernet::GetStatus(fdf::Arena& arena, GetStatusCompleter::Sync& completer) {
  completer.buffer(arena).Reply(fidl::ToWire(arena, GetPortStatus()));
}

fuchsia_hardware_network::PortStatus GuestEthernet::GetPortStatus() {
  // Port status flags.
  fuchsia_hardware_network::PortStatus status;
  // Status flags, as defined in [`fuchsia.hardware.network/Status`].
  status.flags(fuchsia_hardware_network::wire::StatusFlags::kOnline);
  // Port's maximum transmission unit, in bytes.
  status.mtu(kMtu);
  return status;
}

void GuestEthernet::SetActive(
    fuchsia_hardware_network_driver::wire::NetworkPortSetActiveRequest* request, fdf::Arena& arena,
    SetActiveCompleter::Sync& completer) {}

void GuestEthernet::GetMac(fdf::Arena& arena, GetMacCompleter::Sync& completer) {
  auto [client, server] = fdf::Endpoints<fuchsia_hardware_network_driver::MacAddr>::Create();
  fdf::BindServer(netdev_dispatchers_.ifc_->get(), std::move(server), this);
  completer.buffer(arena).Reply(std::move(client));
}

void GuestEthernet::Removed(fdf::Arena& arena, RemovedCompleter::Sync& completer) {}
