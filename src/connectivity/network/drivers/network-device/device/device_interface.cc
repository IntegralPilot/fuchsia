// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "device_interface.h"

#include <fidl/fuchsia.hardware.network.driver/cpp/fidl.h>
#include <fidl/fuchsia.hardware.network.driver/cpp/wire.h>
#include <lib/async/cpp/task.h>
#include <lib/fdf/cpp/env.h>
#include <lib/fidl/cpp/wire/sync_call.h>
#include <lib/fidl/cpp/wire/traits.h>
#include <lib/fit/defer.h>
#include <lib/trace/event.h>

#include <tuple>

#include <fbl/alloc_checker.h>

#include "definitions.h"
#include "log.h"
#include "rx_queue.h"
#include "session.h"
#include "src/connectivity/lib/network-device/buffer_descriptor/buffer_descriptor.h"
#include "tx_queue.h"

// Static sanity assertions from far-away defined buffer_descriptor_t.
// A buffer descriptor is always described in 64 bit words.
static_assert(sizeof(buffer_descriptor_t) % 8 == 0);
// Verify no unseen padding is being added by the compiler and all padding
// reservation fields are working as expected, check the offset of every 64 bit
// word in the struct.
static_assert(offsetof(buffer_descriptor_t, frame_type) == 0);
static_assert(offsetof(buffer_descriptor_t, port_id) == 8);
static_assert(offsetof(buffer_descriptor_t, offset) == 16);
static_assert(offsetof(buffer_descriptor_t, head_length) == 24);
static_assert(offsetof(buffer_descriptor_t, inbound_flags) == 32);
// Descriptor length is reported as uint8 words in session info, make sure that
// fits.
static_assert(sizeof(buffer_descriptor_t) / sizeof(uint64_t) < std::numeric_limits<uint8_t>::max());

namespace {

namespace fnetwork_driver = fuchsia_hardware_network_driver;

// Assert that the batch sizes dictated by the maximum vector lengths in the
// FIDL library are the largest they can be while remaining within the maximum
// FIDL message size.

constexpr size_t kMaxFidlPayloadSize = ZX_CHANNEL_MAX_MSG_BYTES - sizeof(fidl_message_header_t);

// NetworkDeviceImpl.QueueTx
constexpr size_t kQueueTxSize =
    fidl::MaxSizeInChannel<fnetwork_driver::wire::NetworkDeviceImplQueueTxRequest,
                           fidl::MessageDirection::kSending>();
static_assert(kQueueTxSize <= kMaxFidlPayloadSize);
constexpr size_t kTxBufferSize =
    fidl::MaxSizeInChannel<fnetwork_driver::wire::TxBuffer, fidl::MessageDirection::kSending>();
static_assert(kMaxFidlPayloadSize - kQueueTxSize < kTxBufferSize);

// NetworkDeviceImpl.QueueRxSpace
constexpr size_t kQueueRxSpaceSize =
    fidl::MaxSizeInChannel<fnetwork_driver::wire::NetworkDeviceImplQueueRxSpaceRequest,
                           fidl::MessageDirection::kSending>();
static_assert(kQueueRxSpaceSize <= kMaxFidlPayloadSize);
constexpr size_t kRxSpaceBufferSize = fidl::MaxSizeInChannel<fnetwork_driver::wire::RxSpaceBuffer,
                                                             fidl::MessageDirection::kSending>();
static_assert(kMaxFidlPayloadSize - kQueueRxSpaceSize < kRxSpaceBufferSize);

// NetworkDeviceIfc.CompleteTx
constexpr size_t kCompleteTxSize =
    fidl::MaxSizeInChannel<fnetwork_driver::wire::NetworkDeviceIfcCompleteTxRequest,
                           fidl::MessageDirection::kSending>();
static_assert(kCompleteTxSize <= kMaxFidlPayloadSize);
constexpr size_t kTxResultSize =
    fidl::MaxSizeInChannel<fnetwork_driver::wire::TxResult, fidl::MessageDirection::kSending>();
static_assert(kMaxFidlPayloadSize - kCompleteTxSize < kTxResultSize);

// NetworkDeviceIfc.CompleteRx
constexpr size_t kCompleteRxSize =
    fidl::MaxSizeInChannel<fnetwork_driver::wire::NetworkDeviceIfcCompleteRxRequest,
                           fidl::MessageDirection::kSending>();
static_assert(kCompleteRxSize <= kMaxFidlPayloadSize);
constexpr size_t kRxBufferSize =
    fidl::MaxSizeInChannel<fnetwork_driver::wire::RxBuffer, fidl::MessageDirection::kSending>();
static_assert(kMaxFidlPayloadSize - kCompleteRxSize < kRxBufferSize);

}  // namespace

namespace {
const char* DeviceStatusToString(network::internal::DeviceStatus status) {
  switch (status) {
    case network::internal::DeviceStatus::STARTING:
      return "STARTING";
    case network::internal::DeviceStatus::STARTED:
      return "STARTED";
    case network::internal::DeviceStatus::STOPPING:
      return "STOPPING";
    case network::internal::DeviceStatus::STOPPED:
      return "STOPPED";
  }
}

void TeardownAndFreeBinder(std::unique_ptr<network::NetworkDeviceImplBinder>&& binder) {
  // Keep a raw pointer for calling into since we capture by move in the
  // callback which renders the pointer invalid.
  network::NetworkDeviceImplBinder* binder_ptr = binder.get();

  // It doesn't matter if the teardown is synchronous here. The callback won't
  // be called but since the callback will then be destroyed that means that the
  // unique pointer it captured will also be destroyed, thus achieving the same
  // goal. In fact, the callback doesn't even have to call reset but it's there
  // to explicitly demonstrate the intent of the callback.
  binder_ptr->Teardown([binder = std::move(binder)]() mutable { binder.reset(); });
}

}  // namespace

namespace network {

zx::result<std::unique_ptr<OwnedDeviceInterfaceDispatchers>>
OwnedDeviceInterfaceDispatchers::Create() {
  fbl::AllocChecker ac;

  std::unique_ptr<OwnedDeviceInterfaceDispatchers> dispatchers(new (&ac)
                                                                   OwnedDeviceInterfaceDispatchers);
  if (!ac.check()) {
    LOGF_ERROR("Failed to allocate OwnedDeviceInterfaceDispatchers");
    return zx::error(ZX_ERR_NO_MEMORY);
  }

  zx::result impl_dispatcher = fdf::UnsynchronizedDispatcher::Create(
      {}, "netdev-impl", [dispatchers = dispatchers.get()](fdf_dispatcher_t*) {
        dispatchers->impl_shutdown_.Signal();
      });
  if (impl_dispatcher.is_error()) {
    LOGF_ERROR("failed to create impl dispatcher: %s", impl_dispatcher.status_string());
    return impl_dispatcher.take_error();
  }
  dispatchers->impl_ = std::move(impl_dispatcher.value());

  zx::result ifc_dispatcher = fdf::UnsynchronizedDispatcher::Create(
      {}, "netdev-ifc", [dispatchers = dispatchers.get()](fdf_dispatcher_t*) {
        dispatchers->ifc_shutdown_.Signal();
      });
  if (ifc_dispatcher.is_error()) {
    LOGF_ERROR("failed to create ifc dispatcher: %s", ifc_dispatcher.status_string());
    return ifc_dispatcher.take_error();
  }
  dispatchers->ifc_ = std::move(ifc_dispatcher.value());

  zx::result port_dispatcher = fdf::UnsynchronizedDispatcher::Create(
      {}, "netdev-port", [dispatchers = dispatchers.get()](fdf_dispatcher_t*) {
        dispatchers->port_shutdown_.Signal();
      });
  if (port_dispatcher.is_error()) {
    LOGF_ERROR("failed to create port dispatcher: %s", port_dispatcher.status_string());
    return port_dispatcher.take_error();
  }
  dispatchers->port_ = std::move(port_dispatcher.value());

  return zx::ok(std::move(dispatchers));
}

DeviceInterfaceDispatchers OwnedDeviceInterfaceDispatchers::Unowned() {
  return DeviceInterfaceDispatchers(impl_, ifc_, port_);
}

void OwnedDeviceInterfaceDispatchers::ShutdownSync() {
  if (impl_.get()) {
    impl_.ShutdownAsync();
    impl_shutdown_.Wait();
  }
  if (ifc_.get()) {
    ifc_.ShutdownAsync();
    ifc_shutdown_.Wait();
  }
  if (port_.get()) {
    port_.ShutdownAsync();
    port_shutdown_.Wait();
  }
}

OwnedDeviceInterfaceDispatchers::OwnedDeviceInterfaceDispatchers() = default;

zx::result<std::unique_ptr<NetworkDeviceInterface>> NetworkDeviceInterface::Create(
    const DeviceInterfaceDispatchers& dispatchers,
    std::unique_ptr<NetworkDeviceImplBinder>&& binder) {
  return internal::DeviceInterface::Create(dispatchers, std::move(binder));
}

namespace internal {

uint16_t TransformFifoDepth(uint16_t device_depth) {
  // We're going to say the depth is twice the depth of the device to account
  // for in-flight buffers, as long as it doesn't go over the maximum fifo
  // depth.

  // Check for overflow.
  if (device_depth > (std::numeric_limits<uint16_t>::max() >> 1)) {
    return kMaxFifoDepth;
  }

  return std::min(kMaxFifoDepth, static_cast<uint16_t>(device_depth << 1));
}

zx::result<std::unique_ptr<DeviceInterface>> DeviceInterface::Create(
    const DeviceInterfaceDispatchers& dispatchers,
    std::unique_ptr<NetworkDeviceImplBinder>&& binder) {
  fbl::AllocChecker ac;
  std::unique_ptr<DeviceInterface> device(new (&ac) DeviceInterface(dispatchers));
  if (!ac.check()) {
    TeardownAndFreeBinder(std::move(binder));
    return zx::error(ZX_ERR_NO_MEMORY);
  }

  zx_status_t status = device->Init(std::move(binder));
  if (status != ZX_OK) {
    return zx::error(status);
  }

  return zx::ok(std::move(device));
}

DeviceInterface::~DeviceInterface() {
  ZX_ASSERT_MSG(session_ == nullptr, "can't destroy DeviceInterface with active session. (%s)",
                session_->name());
  ZX_ASSERT_MSG(bindings_.is_empty(), "can't destroy device interface with %ld attached bindings.",
                bindings_.size());
  size_t active_ports = std::count_if(ports_.begin(), ports_.end(),
                                      [](const PortSlot& port) { return port.port != nullptr; });
  ZX_ASSERT_MSG(!active_ports, "can't destroy device interface with %ld ports", active_ports);
}

zx_status_t DeviceInterface::Init(std::unique_ptr<NetworkDeviceImplBinder>&& binder) {
  LOGF_TRACE("%s", __FUNCTION__);

  if (binder_) {
    LOGF_ERROR("init: already initialized");
    TeardownAndFreeBinder(std::move(binder));
    return ZX_ERR_BAD_STATE;
  }
  binder_ = std::move(binder);

  // If init fails the binder has to be torn down. The DeviceInterface::Teardown
  // method is not going to be called at that point but the binder might have
  // state that needs to be torn down in an orderly fashion.
  auto teardown_and_free_binder = fit::defer([this] { TeardownAndFreeBinder(std::move(binder_)); });

  zx::result<fdf::ClientEnd<netdriver::NetworkDeviceImpl>> device = binder_->Bind();
  if (device.is_error()) {
    LOGF_ERROR("init: failed to bind NetworkDeviceImpl: %s", device.status_string());
    return device.status_value();
  }

  // Initialization is synchronous.
  fdf::WireSyncClient sync_client(std::move(device.value()));
  fdf::Arena arena('NETD');

  fdf::WireUnownedResult info_result = sync_client.buffer(arena)->GetInfo();
  if (!info_result.ok()) {
    LOGF_ERROR("init: GetInfo() failed: %s", info_result.FormatDescription().c_str());
    return info_result.status();
  }

  device_info_ = fidl::ToNatural(info_result.value().info);

  if (device_info_.buffer_alignment().value_or(0) == 0) {
    LOGF_ERROR("init: device reports invalid zero buffer alignment");
    return ZX_ERR_NOT_SUPPORTED;
  }
  const uint16_t rx_depth = device_info_.rx_depth().value_or(0);
  const uint16_t tx_depth = device_info_.tx_depth().value_or(0);
  const uint16_t rx_threshold = device_info_.rx_threshold().value_or(0);
  if (rx_threshold > rx_depth) {
    LOGF_ERROR("init: device reports rx_threshold = %u larger than rx_depth %u", rx_threshold,
               rx_depth);
    return ZX_ERR_NOT_SUPPORTED;
  }
  if (rx_depth > kMaxFifoDepth || tx_depth > kMaxFifoDepth) {
    LOGF_ERROR("init: device reports too large FIFO depths: %u/%u (max=%u)", rx_depth, tx_depth,
               kMaxFifoDepth);
    return ZX_ERR_NOT_SUPPORTED;
  }

  zx::result tx_queue = TxQueue::Create(this);
  if (tx_queue.is_error()) {
    LOGF_ERROR("init: device failed to start Tx Queue: %s", tx_queue.status_string());
    return tx_queue.status_value();
  }
  tx_queue_ = std::move(tx_queue.value());

  zx::result rx_queue = RxQueue::Create(this);
  if (rx_queue.is_error()) {
    LOGF_ERROR("init: device failed to start Rx Queue: %s", rx_queue.status_string());
    return rx_queue.status_value();
  }
  rx_queue_ = std::move(rx_queue.value());

  {
    fbl::AutoLock lock(&control_lock_);
    if (zx_status_t status = vmo_store_.Reserve(netdev::wire::kMaxDataVmos); status != ZX_OK) {
      LOGF_ERROR("init: failed to init session identifiers %s", zx_status_get_string(status));
      return status;
    }
  }

  zx::result endpoints = fdf::CreateEndpoints<netdriver::NetworkDeviceIfc>();
  if (endpoints.is_error()) {
    LOGF_ERROR("init: CreateEndpoints failed: %s", endpoints.status_string());
    return endpoints.status_value();
  }
  ifc_binding_ = fdf::BindServer(
      dispatchers_.ifc_->get(), std::move(endpoints->server), this,
      [this](DeviceInterface*, fidl::UnbindInfo, fdf::ServerEnd<netdriver::NetworkDeviceIfc>) {
        control_lock_.Acquire();
        ifc_binding_.reset();
        ContinueTeardown(TeardownState::IFC_BINDING);
      });

  // A call to `NetworkDeviceImpl.Init` could theoretically call back into this
  // type. As a result, the client is converted into the asynchronous version
  // prior to the call.
  device_impl_ = fdf::WireSharedClient<fuchsia_hardware_network_driver::NetworkDeviceImpl>(
      sync_client.TakeClientEnd(), dispatchers_.impl_->get(),
      fidl::AnyTeardownObserver::ByCallback([this]() mutable {
        control_lock_.Acquire();
        // Reset the client to ensure that the teardown process doesn't
        // attempt to tear it down if the channel is already closed.
        device_impl_ = fdf::WireSharedClient<fuchsia_hardware_network_driver::NetworkDeviceImpl>();
        this->ContinueTeardown(TeardownState::DEVICE_IMPL);
      }));

  // Making this a synchronous call simplifies the creation process of
  // DeviceInterface at the expense of blocking the calling thread until Init is
  // complete. This requires that netdevice allows some re-entrant calls as many
  // drivers will call AddPort during initialization. Vendor drivers need to be
  // cautious with locks to ensure that further re-entrant calls from AddPort
  // will not cause a deadlock.
  fdf::WireUnownedResult init_status =
      device_impl_.sync().buffer(arena)->Init(std::move(endpoints->client));
  if (!init_status.ok()) {
    LOGF_ERROR("init: Init() failed: %s", init_status.FormatDescription().c_str());
    return init_status.status();
  }
  if (init_status.value().s != ZX_OK) {
    LOGF_ERROR("init: Init() failed: %s", zx_status_get_string(init_status.value().s));
    return init_status.value().s;
  }
  // Now that everything succeeded do NOT tear down the factory.
  teardown_and_free_binder.cancel();

  return ZX_OK;
}

void DeviceInterface::Teardown(fit::callback<void()> teardown_callback) {
  // stop all rx queue operation immediately.
  rx_queue_->JoinThread();
  tx_queue_->JoinThread();
  LOGF_TRACE("%s", __FUNCTION__);

  control_lock_.Acquire();
  // Can't call teardown again until the teardown process has ended.
  ZX_ASSERT(teardown_callback_ == nullptr);
  teardown_callback_ = std::move(teardown_callback);

  ContinueTeardown(TeardownState::RUNNING);
}

zx_status_t DeviceInterface::Bind(fidl::ServerEnd<netdev::Device> req) {
  fbl::AutoLock lock(&control_lock_);
  // Don't attach new bindings if we're tearing down.
  if (teardown_state_ != TeardownState::RUNNING) {
    return ZX_ERR_BAD_STATE;
  }
  return Binding::Bind(this, std::move(req));
}

zx_status_t DeviceInterface::BindPort(uint8_t port_id, fidl::ServerEnd<netdev::Port> req) {
  fbl::AutoLock lock(&control_lock_);
  if (teardown_state_ != TeardownState::RUNNING) {
    return ZX_ERR_BAD_STATE;
  }
  if (port_id >= netdev::wire::kMaxPorts) {
    LOGF_WARN("Port id %u exceeds max port id %u", port_id, netdev::wire::kMaxPorts);
    return ZX_ERR_NOT_FOUND;
  }
  PortSlot& slot = ports_[port_id];
  if (slot.port == nullptr) {
    LOGF_WARN("No port slot available for port %u", port_id);
    return ZX_ERR_NOT_FOUND;
  }
  slot.port->Bind(std::move(req));
  return ZX_OK;
}

void DeviceInterface::PortStatusChanged(
    netdriver::wire::NetworkDeviceIfcPortStatusChangedRequest* request, fdf::Arena& arena,
    PortStatusChangedCompleter::Sync& completer) {
  SharedAutoLock lock(&control_lock_);
  // Skip port status changes if tearing down. During teardown ports may
  // disappear and device implementation may not be aware of it yet.
  if (teardown_state_ != TeardownState::RUNNING) {
    return;
  }
  const uint8_t port_id = request->id;
  const netdev::wire::PortStatus& new_status = request->new_status;
  WithPort(port_id, [&new_status, port_id](const std::unique_ptr<DevicePort>& port) {
    uint32_t flags(new_status.flags());
    if (!port) {
      LOGF_ERROR("StatusChanged on unknown port=%u flags=%u mtu=%u", port_id, flags,
                 new_status.mtu());
      return;
    }

    LOGF_TRACE("StatusChanged(port=%u) flags=%u mtu=%u", port_id, flags, new_status.mtu());
    port->StatusChanged(new_status);
  });
}

void DeviceInterface::AddPort(netdriver::wire::NetworkDeviceIfcAddPortRequest* request,
                              fdf::Arena& p_arena, AddPortCompleter::Sync& completer) {
  const uint8_t port_id = request->id;
  LOGF_TRACE("%s(%d)", __FUNCTION__, port_id);

  fdf::Arena arena('NETD');
  fbl::AutoLock lock(&control_lock_);
  if (zx_status_t status = CanCreatePortWithId(port_id); status != ZX_OK) {
    completer.buffer(arena).Reply(status);
    return;
  }

  // Pre-generate a salted port ID, if another AddPort call comes in while this
  // one is in progress they will both be allowed to proceed but only one can
  // complete the port construction. The behavior isn't necessarily fair; it
  // doesn't guarantee that the first caller wins but this should be infrequent
  // enough to not matter. This behavior allows the DevicePort object to
  // maintain a const port id.
  PortSlot& port_slot = ports_[port_id];
  const netdev::wire::PortId salted_id = {
      .base = port_id,
      // NB: This relies on wrapping overflow.
      .salt = static_cast<uint8_t>(port_slot.salt + 1),
  };

  fdf::WireSharedClient<netdriver::NetworkPort> port_client(std::move(request->port),
                                                            dispatchers_.port_->get());

  DevicePort::Create(
      this, dispatchers_.port_->async_dispatcher(), salted_id, std::move(port_client),
      dispatchers_.impl_->get(), [this](DevicePort& port) { OnPortTeardownComplete(port); },
      [this, port_id, salted_id,
       completer = completer.ToAsync()](zx::result<std::unique_ptr<DevicePort>> result) mutable {
        fdf::Arena arena('NETD');
        if (result.is_error()) {
          LOGF_ERROR("Failed to create port: %s", result.status_string());
          completer.buffer(arena).Reply(result.status_value());
          return;
        }

        fbl::AutoLock lock(&control_lock_);
        // Check again, another AddPort with the same port ID could potentially
        // have completed while in the asynchronous creation flow.
        if (zx_status_t status = CanCreatePortWithId(port_id); status != ZX_OK) {
          completer.buffer(arena).Reply(status);
          return;
        }

        PortSlot& port_slot = ports_[port_id];
        // Update slot with newly created port and its salt.
        port_slot.salt = salted_id.salt;
        port_slot.port = std::move(result.value());

        for (auto& watcher : port_watchers_) {
          watcher.PortAdded(salted_id);
        }
        completer.buffer(arena).Reply(ZX_OK);
      });
}

void DeviceInterface::RemovePort(
    fuchsia_hardware_network_driver::wire::NetworkDeviceIfcRemovePortRequest* request, fdf::Arena&,
    RemovePortCompleter::Sync&) {
  LOGF_TRACE("%s(%d)", __FUNCTION__, request->id);
  SharedAutoLock lock(&control_lock_);
  // Ignore if we're tearing down, all ports will be removed as part of
  // teardown.
  if (teardown_state_ != TeardownState::RUNNING) {
    return;
  }
  WithPort(request->id,
           [this](const std::unique_ptr<DevicePort>& port) __TA_REQUIRES_SHARED(control_lock_) {
             if (port) {
               for (auto& watcher : port_watchers_) {
                 watcher.PortRemoved(port->id());
               }
               port->Teardown();
             }
           });
}

void DeviceInterface::CompleteRx(
    fuchsia_hardware_network_driver::wire::NetworkDeviceIfcCompleteRxRequest* request, fdf::Arena&,
    CompleteRxCompleter::Sync&) {
  LOGF_TRACE("%s(_, %lu)", __FUNCTION__, request->rx.size());
  TRACE_DURATION("network-device", "CompleteRx", "count", request->rx.size());
  rx_queue_->CompleteRxList(request->rx);
}

void DeviceInterface::CompleteTx(
    fuchsia_hardware_network_driver::wire::NetworkDeviceIfcCompleteTxRequest* request, fdf::Arena&,
    CompleteTxCompleter::Sync&) {
  LOGF_TRACE("%s(_, %lu)", __FUNCTION__, request->tx.size());
  TRACE_DURATION("network-device", "CompleteTx", "count", request->tx.size());
  tx_queue_->CompleteTxList(request->tx);
}

void DeviceInterface::DelegateRxLease(
    netdriver::wire::NetworkDeviceIfcDelegateRxLeaseRequest* request, fdf::Arena& arena,
    DelegateRxLeaseCompleter::Sync&) {
  netdev::wire::DelegatedRxLease& lease = request->delegated;
  // Ensure all required fields are set.
  ZX_ASSERT_MSG(lease.has_handle() && lease.has_hold_until_frame(),
                "missing required fields in DelegatedRxLease");

  fbl::AutoLock lock(&rx_lock_);
  if (rx_lease_pending_.has_value()) {
    netdev::DelegatedRxLease& pending = rx_lease_pending_.value();
    // Only keep one of the pending leases. Drop the old one if the new one has
    // a later hold_until frame value.
    if (pending.hold_until_frame().value() > lease.hold_until_frame()) {
      return;
    }
    DropDelegatedRxLease(std::move(pending));
  }
  rx_lease_pending_.emplace(fidl::ToNatural(lease));

  SharedAutoLock control_lock(&control_lock_);
  rx_queue_->AssertParentRxLocked(*this);
  TryDelegateRxLease(rx_queue_->rx_completed_frame_index());
}

void DeviceInterface::UpdateRxBufferParams(
    netdriver::wire::NetworkDeviceIfcUpdateRxBufferParamsRequest* request, fdf::Arena& arena,
    UpdateRxBufferParamsCompleter::Sync& completer) {
  {
    fbl::AutoLock lock(&control_lock_);
    device_info_.rx_buffer_management(fidl::ToNatural(request->param));
  }
  {
    fbl::AutoLock rx_lock(&rx_lock_);
    rx_queue_->AssertParentRxLocked(*this);
    rx_queue_->SetRxBufferManagement(fidl::ToNatural(request->param));
    rx_queue_->RequestRxSpace();
  }
  completer.buffer(arena).Reply(fit::ok());
}

void DeviceInterface::RequestRxSpace(
    netdriver::wire::NetworkDeviceIfcRequestRxSpaceRequest* request, fdf::Arena& arena,
    RequestRxSpaceCompleter::Sync&) {
  fbl::AutoLock rx_lock(&rx_lock_);
  rx_queue_->AssertParentRxLocked(*this);
  uint16_t target = static_cast<uint16_t>(
      std::min(request->requested, static_cast<uint64_t>(std::numeric_limits<uint16_t>::max())));

  if (rx_queue_->SetTargetRxBuffersIfMore(target)) {
    rx_queue_->RequestRxSpace();
  };
}

void DeviceInterface::GetInfo(GetInfoCompleter::Sync& completer) {
  LOGF_TRACE("%s", __FUNCTION__);

  constexpr uint32_t kDefaultBufferAlignment = 0;
  constexpr uint8_t kDefaultMaxBufferParts = 0;
  constexpr uint32_t kDefaultMinRxBufLen = 0;
  constexpr uint32_t kDefaultMinTxBufLen = 0;
  constexpr uint16_t kDefaultTxHeadLength = 0;
  constexpr uint16_t kDefaultTxTailLength = 0;

  const uint8_t min_descriptor_length = sizeof(buffer_descriptor_t) / sizeof(uint64_t);
  const uint8_t descriptor_version = NETWORK_DEVICE_DESCRIPTOR_VERSION;
  const uint16_t rx_depth = rx_fifo_depth();
  const uint16_t tx_depth = tx_fifo_depth();
  const uint32_t buffer_alignment =
      device_info_.buffer_alignment().value_or(kDefaultBufferAlignment);
  const uint8_t max_buffer_parts = device_info_.max_buffer_parts().value_or(kDefaultMaxBufferParts);
  const uint32_t min_rx_buffer_length =
      device_info_.min_rx_buffer_length().value_or(kDefaultMinRxBufLen);
  const uint32_t min_tx_buffer_length =
      device_info_.min_tx_buffer_length().value_or(kDefaultMinTxBufLen);
  const uint16_t min_tx_buffer_head = device_info_.tx_head_length().value_or(kDefaultTxHeadLength);
  const uint16_t min_tx_buffer_tail = device_info_.tx_tail_length().value_or(kDefaultTxTailLength);

  fidl::Arena arena;

  auto device_base_info_builder = netdev::wire::DeviceBaseInfo::Builder(arena)
                                      .rx_depth(rx_depth)
                                      .tx_depth(tx_depth)
                                      .buffer_alignment(buffer_alignment)
                                      .max_buffer_parts(max_buffer_parts)
                                      .min_rx_buffer_length(min_rx_buffer_length)
                                      .min_tx_buffer_length(min_tx_buffer_length)
                                      .min_tx_buffer_head(min_tx_buffer_head)
                                      .min_tx_buffer_tail(min_tx_buffer_tail);

  const std::optional<uint32_t>& max_buffer_length = device_info_.max_buffer_length();
  if (max_buffer_length.has_value() && max_buffer_length.value() != 0) {
    device_base_info_builder.max_buffer_length(max_buffer_length.value());
  }
  if (device_info_.min_rx_buffers().has_value()) {
    device_base_info_builder.min_rx_buffers(device_info_.min_rx_buffers().value());
  }

  netdev::wire::DeviceBaseInfo device_base_info = device_base_info_builder.Build();

  netdev::wire::DeviceInfo device_info = netdev::wire::DeviceInfo::Builder(arena)
                                             .min_descriptor_length(min_descriptor_length)
                                             .descriptor_version(descriptor_version)
                                             .base_info(device_base_info)
                                             .Build();

  completer.Reply(device_info);
}

namespace {

// VMOs are sorted by ID to ensure a deterministic order in which RxQueue prepares and releases
// VMOs. This allows RxQueue to determine if a buffer's backing VMO is prepared with device impl
// efficiently. If the VMO ID is smaller than the last VMO ID in use, it must be prepared and ready
// to be queued to the device impl. Because the VMOs are created by the client, there is no
// guarantee that they are sorted by ID. For VMOs that are already sorted, this method always pushes
// to the end of the list.
void InsertRxVmoSorted(RxVmoList& list, DataVmoMeta* meta) {
  if (meta->num_rx_buffers == 0) {
    return;
  }
  if (list.is_empty() || list.back().id <= meta->id) {
    list.push_back(meta);
    return;
  }
  auto it = list.end();
  do {
    --it;
    if (it->id <= meta->id) {
      list.insert(++it, meta);
      return;
    }
  } while (it != list.begin());
  list.push_front(meta);
}

}  // namespace

void DeviceInterface::OpenSession(OpenSessionRequestView request,
                                  OpenSessionCompleter::Sync& completer) {
  bool has_rx_vmo = false;
  zx::result sync_result = [this, &request,
                            &has_rx_vmo]() -> zx::result<netdev::wire::DeviceOpenSessionResponse> {
    fbl::AutoLock rx_lock(&rx_lock_);
    fbl::AutoLock tx_lock(&tx_lock_);
    fbl::AutoLock lock(&control_lock_);
    // We're currently tearing down and can't open any new sessions.
    if (teardown_state_ != TeardownState::RUNNING) {
      return zx::error(ZX_ERR_UNAVAILABLE);
    }

    if (session_) {
      return zx::error(ZX_ERR_ALREADY_EXISTS);
    }

    zx::result endpoints = fidl::CreateEndpoints<netdev::Session>();
    if (endpoints.is_error()) {
      return endpoints.take_error();
    }

    fidl::StringView& name = request->session_name;
    netdev::wire::SessionInfo& session_info = request->session_info;
    zx::result session_creation =
        Session::Create(dispatchers_.impl_->async_dispatcher(), session_info, name, this);

    if (session_creation.is_error()) {
      return session_creation.take_error();
    }
    auto& [session, fifos] = session_creation.value();

    if (!session_info.has_data()) {
      return zx::error(ZX_ERR_INVALID_ARGS);
    }

    for (netdev::wire::DataVmo& data_vmo : session_info.data()) {
      if (!data_vmo.has_id()) {
        return zx::error(ZX_ERR_INVALID_ARGS);
      }
      if (!data_vmo.has_vmo()) {
        return zx::error(ZX_ERR_INVALID_ARGS);
      }
      if (!data_vmo.has_num_rx_buffers()) {
        return zx::error(ZX_ERR_INVALID_ARGS);
      }
      if (data_vmo.id() >= fuchsia_hardware_network::wire::kMaxDataVmos) {
        return zx::error(ZX_ERR_INVALID_ARGS);
      }
    }

    // This is safe because we can only get here if session_ is nullptr,
    // and we always remove all VMOs before setting session_ to nullptr.
    ZX_ASSERT_MSG(vmo_store_.count() == 0, "Must have no sessions");
    RxVmoList sorted_rx_vmos;

    for (netdev::wire::DataVmo& data_vmo : session_info.data()) {
      VmoId vmo_id = data_vmo.id();
      const uint16_t num_rx_buffers = data_vmo.num_rx_buffers();
      if (zx_status_t status = vmo_store_.RegisterWithKey(
              vmo_id, std::move(data_vmo.vmo()),
              DataVmoMeta{.id = vmo_id, .num_rx_buffers = num_rx_buffers});
          status != ZX_OK) {
        return zx::error(status);
      }
      DataVmoMeta* meta = &vmo_store_.GetVmo(vmo_id)->meta();
      unprepared_vmos_.push_back(meta);
      InsertRxVmoSorted(sorted_rx_vmos, meta);
    }

    has_rx_vmo = !sorted_rx_vmos.is_empty();

    session->AssertParentTxLock(*this);
    session->InstallTx();
    session->Bind(std::move(endpoints->server));

    session_ = std::move(session);
    rx_queue_->AssertParentRxLocked(*this);
    rx_queue_->SetRxVmos(std::move(sorted_rx_vmos));
    rx_queue_->SetSession(session_.get());
    rx_queue_->SetRxBufferManagement(device_info_.rx_buffer_management().value_or(
        fuchsia_hardware_network_driver::RxBufferManagement::WithStatic_({})));
    rx_queue_->TriggerSessionChanged();

    return zx::ok(netdev::wire::DeviceOpenSessionResponse{
        .session = std::move(endpoints->client),
        .fifos = std::move(fifos),
    });
  }();

  if (sync_result.is_error()) {
    completer.ReplyError(sync_result.error_value());
    return;
  }
  auto response = std::move(sync_result.value());
  if (!has_rx_vmo) {
    completer.ReplySuccess(std::move(response.session), std::move(response.fifos));
    return;
  }

  fbl::AutoLock rx_lock(&rx_lock_);
  rx_queue_->AssertParentRxLocked(*this);
  rx_queue_->RequestRxSpace([this, response = std::move(response), completer = completer.ToAsync()](
                                fit::result<std::tuple<zx_status_t, DataVmoList>> result) mutable {
    if (result.is_error()) {
      auto& [status, rest_vmos] = result.error_value();
      LOGF_WARN("cannot prepare VMOs for new session: %s", zx_status_get_string(status));
      fbl::AutoLock lock(&control_lock_);
      unprepared_vmos_.splice(unprepared_vmos_.end(), rest_vmos);
      completer.ReplyError(ZX_ERR_INTERNAL);
      // Dropping of the session channel will cause the session to
      // be dead and hence undo all the prepared VMOs.
      return;
    }
    completer.ReplySuccess(std::move(response.session), std::move(response.fifos));
  });
}

bool DeviceInterface::PrepareVmosLocked(
    DataVmoList vmos, fit::callback<void(fit::result<std::tuple<zx_status_t, DataVmoList>>)> cb) {
  ZX_ASSERT(!vmos.is_empty());
  for (auto& vmo : vmos) {
    vmo.state = VmoState::kPreparing;
  }
  bool should_start = pending_prepare_vmos_.is_empty();
  vmos.back().batch_completion = std::move(cb);
  pending_prepare_vmos_.splice(pending_prepare_vmos_.end(), vmos);
  return should_start;
}

bool DeviceInterface::ReleaseVmosLocked(
    DataVmoList vmos, fit::callback<void(fit::result<std::tuple<zx_status_t, DataVmoList>>)> cb) {
  ZX_ASSERT(!vmos.is_empty());
  for (auto& vmo : vmos) {
    vmo.state = VmoState::kReleasing;
  }
  bool should_start = pending_release_vmos_.is_empty();
  vmos.back().batch_completion = std::move(cb);
  pending_release_vmos_.splice(pending_release_vmos_.end(), vmos);
  return should_start;
}

void DeviceInterface::PrepareNextVmo(fdf::Arena arena) {
  DataVmoMeta* cur;
  {
    fbl::AutoLock lock(&control_lock_);
    if (pending_prepare_vmos_.is_empty()) {
      PruneDeadSession();
      return;
    }
    cur = &pending_prepare_vmos_.front();
  }
  ZX_ASSERT_MSG(cur->state == VmoState::kPreparing, "vmo %d is not in preparing state", cur->id);

  zx::vmo duplicated_vmo;
  zx_status_t status;
  {
    SharedAutoLock lock(&control_lock_);
    DataVmoStore::StoredVmo* stored_vmo = vmo_store_.GetVmo(cur->id);
    status = stored_vmo->vmo()->duplicate(ZX_RIGHT_SAME_RIGHTS, &duplicated_vmo);
  }
  fit::callback<void(fdf::Arena, zx_status_t)> fail = [this, cur](fdf::Arena arena,
                                                                  zx_status_t status) {
    DataVmoList failed_batch;
    fit::callback<void(fit::result<std::tuple<zx_status_t, DataVmoList>>)> batch_cb;
    bool last_vmo = false;
    {
      fbl::AutoLock lock(&control_lock_);
      ZX_ASSERT(!pending_prepare_vmos_.is_empty() && &pending_prepare_vmos_.front() == cur);
      while (!pending_prepare_vmos_.is_empty()) {
        DataVmoMeta* node = pending_prepare_vmos_.pop_front();
        node->state = VmoState::kUnprepared;
        failed_batch.push_back(node);
        if (node->batch_completion) {
          batch_cb = std::move(node->batch_completion);
          break;
        }
      }
      last_vmo = pending_prepare_vmos_.is_empty();
    }
    if (batch_cb) {
      batch_cb(zx::error(std::make_tuple(status, std::move(failed_batch))));
    }
    if (last_vmo) {
      SharedAutoLock lock(&control_lock_);
      PruneDeadSession();
      return;
    }
    PrepareNextVmo(std::move(arena));
  };
  if (status != ZX_OK) {
    LOGF_ERROR("Failed to duplicate VMO %d: %s", cur->id, zx_status_get_string(status));
    fail(std::move(arena), status);
    return;
  }
  device_impl_.buffer(arena)
      ->PrepareVmo(cur->id, std::move(duplicated_vmo))
      .Then([this, cur, fail = std::move(fail), arena = std::move(arena)](
                fdf::WireUnownedResult<netdriver::NetworkDeviceImpl::PrepareVmo>& result) mutable {
        LOGF_TRACE("%s: driver PrepareVmo for VMO %d completed, ok=%d", __FUNCTION__, cur->id,
                   result.ok());
        if (!result.ok() || result.value().s != ZX_OK) {
          LOGF_ERROR("PrepareVmo failed: %s", result.ok() ? zx_status_get_string(result.value().s)
                                                          : result.FormatDescription().c_str());
          zx_status_t err = result.ok() ? result.value().s : result.error().status();
          fail(std::move(arena), err);
          return;
        }
        fit::callback<void(fit::result<std::tuple<zx_status_t, DataVmoList>>)> batch_cb;
        bool last_vmo = false;
        {
          fbl::AutoLock lock(&control_lock_);
          DataVmoMeta* popped = pending_prepare_vmos_.pop_front();
          ZX_ASSERT(popped == cur);
          cur->state = VmoState::kPrepared;
          prepared_vmos_.push_back(cur);
          if (cur->batch_completion) {
            batch_cb = std::move(cur->batch_completion);
          }
          last_vmo = pending_prepare_vmos_.is_empty();
        }
        if (batch_cb) {
          batch_cb(fit::ok());
        }
        if (last_vmo) {
          SharedAutoLock lock(&control_lock_);
          PruneDeadSession();
          return;
        }
        PrepareNextVmo(std::move(arena));
      });
}

void DeviceInterface::ReleaseNextVmo(fdf::Arena arena) {
  DataVmoMeta* cur;
  {
    fbl::AutoLock lock(&control_lock_);
    if (pending_release_vmos_.is_empty()) {
      PruneDeadSession();
      return;
    }
    cur = &pending_release_vmos_.front();
  }
  ZX_ASSERT_MSG(cur->state == VmoState::kReleasing, "vmo %d is not in releasing state", cur->id);

  device_impl_.buffer(arena)->ReleaseVmo(cur->id).Then(
      [this, cur, arena = std::move(arena)](
          fdf::WireUnownedResult<netdriver::NetworkDeviceImpl::ReleaseVmo>& result) mutable {
        LOGF_TRACE("%s: driver ReleaseVmo for VMO %d completed, ok=%d", __FUNCTION__, cur->id,
                   result.ok());
        bool last_vmo = false;
        if (!result.ok()) {
          LOGF_ERROR("ReleaseVmo failed: %s", result.FormatDescription().c_str());
          DataVmoList failed_batch;
          fit::callback<void(fit::result<std::tuple<zx_status_t, DataVmoList>>)> batch_cb;
          {
            fbl::AutoLock lock(&control_lock_);
            ZX_ASSERT(!pending_release_vmos_.is_empty() && &pending_release_vmos_.front() == cur);
            while (!pending_release_vmos_.is_empty()) {
              DataVmoMeta* node = pending_release_vmos_.pop_front();
              node->state = VmoState::kPrepared;
              failed_batch.push_back(node);
              if (node->batch_completion) {
                batch_cb = std::move(node->batch_completion);
                break;
              }
            }
            last_vmo = pending_release_vmos_.is_empty();
          }
          if (batch_cb) {
            batch_cb(fit::error(std::make_tuple(result.status(), std::move(failed_batch))));
          }
          if (last_vmo) {
            SharedAutoLock lock(&control_lock_);
            PruneDeadSession();
            return;
          }
          ReleaseNextVmo(std::move(arena));
          return;
        }
        fit::callback<void(fit::result<std::tuple<zx_status_t, DataVmoList>>)> batch_cb;
        {
          fbl::AutoLock lock(&control_lock_);
          DataVmoMeta* popped = pending_release_vmos_.pop_front();
          ZX_ASSERT(popped == cur);
          cur->state = VmoState::kUnprepared;
          unprepared_vmos_.push_back(cur);
          if (cur->batch_completion) {
            batch_cb = std::move(cur->batch_completion);
          }
          last_vmo = pending_release_vmos_.is_empty();
        }
        if (batch_cb) {
          batch_cb(fit::ok());
        }
        if (last_vmo) {
          SharedAutoLock lock(&control_lock_);
          PruneDeadSession();
          return;
        }
        ReleaseNextVmo(std::move(arena));
      });
}

void DeviceInterface::RegisterForTx(
    cpp20::span<const VmoId> vmos,
    fidl::WireServer<netdev::Session>::RegisterForTxCompleter::Async completer) {
  LOGF_TRACE("%s: %zu VMOs", __FUNCTION__, vmos.size());
  {
    fbl::AutoLock lock(&control_lock_);
    if (teardown_state_ != TeardownState::RUNNING) {
      completer.Reply(0, ZX_ERR_BAD_STATE);
      return;
    }
    if (session_ == nullptr) {
      completer.Reply(0, ZX_ERR_BAD_STATE);
      return;
    }
    session_->AssertParentControlLock(*this);
    if (session_->IsDying()) {
      completer.Reply(0, ZX_ERR_BAD_STATE);
      return;
    }

    // We use `seen` as a bitmask to detect duplicate VMOs in the request.
    // This static assert ensures that `seen` has enough bits to represent
    // all possible VMO IDs up to `netdev::kMaxDataVmos`.
    uint32_t seen = 0;
    static_assert(sizeof(seen) * 8 == netdev::kMaxDataVmos);
    for (VmoId id : vmos) {
      if (id >= netdev::kMaxDataVmos) {
        completer.Reply(0, ZX_ERR_NOT_FOUND);
        return;
      }
      uint32_t mask = 1U << id;
      if (seen & mask) {
        completer.Reply(0, ZX_ERR_INVALID_ARGS);
        return;
      }
      seen |= mask;

      auto* stored_vmo = vmo_store_.GetVmo(id);
      if (!stored_vmo) {
        LOGF_WARN("RegisterForTx: VMO %d not found, replying NOT_FOUND", id);
        completer.Reply(0, ZX_ERR_NOT_FOUND);
        return;
      }
      if (stored_vmo->meta().tx_registered) {
        LOGF_WARN("RegisterForTx: VMO %d already registered for TX, replying ALREADY_EXISTS", id);
        completer.Reply(0, ZX_ERR_ALREADY_EXISTS);
        return;
      }
      if (stored_vmo->meta().state == VmoState::kReleasing ||
          stored_vmo->meta().state == VmoState::kPreparing) {
        LOGF_WARN("%s: VMO %hu has pending operation, cannot register for Tx", __FUNCTION__, id);
        completer.Reply(0, ZX_ERR_SHOULD_WAIT);
        return;
      }
    }

    DataVmoList to_prepare;
    bool has_skipped = false;
    for (auto it = vmos.begin(); it != vmos.end(); ++it) {
      auto* stored_vmo = vmo_store_.GetVmo(*it);
      stored_vmo->meta().tx_registered = true;
      switch (stored_vmo->meta().state) {
        case VmoState::kPreparing:
        case VmoState::kReleasing:
          __UNREACHABLE;
        case VmoState::kPrepared:
          has_skipped = true;
          continue;
        case VmoState::kUnprepared: {
          unprepared_vmos_.erase(stored_vmo->meta());
          to_prepare.push_back(&stored_vmo->meta());
        }
      }
    }
    uint8_t total = static_cast<uint8_t>(vmos.size());
    if (to_prepare.is_empty()) {
      lock.release();
      completer.Reply(total, ZX_OK);
      return;
    }

    // If we have skipped a VMO, we need to keep the original list of VMO IDs to
    // find out how many VMOs succeeded. Otherwise, the entirety is in the
    // to_prepare list and we don't need any allocation.
    std::vector<VmoId> vmo_ids;
    if (has_skipped) {
      vmo_ids = std::vector<VmoId>(vmos.begin(), vmos.end());
    }
    bool should_start = PrepareVmosLocked(
        std::move(to_prepare),
        [this, completer = std::move(completer), vmo_ids = std::move(vmo_ids),
         total](fit::result<std::tuple<zx_status_t, DataVmoList>> result) mutable {
          if (result.is_error()) {
            auto& [status, rest_vmos] = result.error_value();
            {
              fbl::AutoLock lock(&control_lock_);
              if (!vmo_ids.empty() && !rest_vmos.is_empty()) {
                auto it = std::ranges::find(vmo_ids, rest_vmos.front().id);
                for (; it != vmo_ids.end(); ++it) {
                  auto* stored_vmo = vmo_store_.GetVmo(*it);
                  stored_vmo->meta().tx_registered = false;
                  total--;
                }
              } else {
                for (auto& vmo : rest_vmos) {
                  vmo.tx_registered = false;
                  total--;
                }
              }
              for (auto& vmo : rest_vmos) {
                vmo.state = VmoState::kUnprepared;
              }
              unprepared_vmos_.splice(unprepared_vmos_.end(), rest_vmos);
            }
            completer.Reply(total, status);
            return;
          }
          completer.Reply(total, ZX_OK);
        });
    if (!should_start) {
      return;
    }
  }
  fdf::Arena arena('NETD');
  PrepareNextVmo(std::move(arena));
}

void DeviceInterface::UnregisterForTx(
    cpp20::span<const VmoId> vmos,
    fidl::WireServer<netdev::Session>::UnregisterForTxCompleter::Async completer) {
  LOGF_TRACE("%s: %zu VMOs", __FUNCTION__, vmos.size());
  {
    fbl::AutoLock lock(&control_lock_);
    if (teardown_state_ != TeardownState::RUNNING) {
      completer.Reply(0, ZX_ERR_BAD_STATE);
      return;
    }
    if (session_ == nullptr) {
      completer.Reply(0, ZX_ERR_BAD_STATE);
      return;
    }
    session_->AssertParentControlLock(*this);
    if (session_->IsDying()) {
      completer.Reply(0, ZX_ERR_BAD_STATE);
      return;
    }

    // We use `seen` as a bitmask to detect duplicate VMOs in the request.
    // This static assert ensures that `seen` has enough bits to represent
    // all possible VMO IDs up to `netdev::kMaxDataVmos`.
    uint32_t seen = 0;
    static_assert(sizeof(seen) * 8 == netdev::kMaxDataVmos);
    for (VmoId id : vmos) {
      if (id >= netdev::kMaxDataVmos) {
        completer.Reply(0, ZX_ERR_INVALID_ARGS);
        return;
      }
      uint32_t mask = 1U << id;
      if (seen & mask) {
        completer.Reply(0, ZX_ERR_INVALID_ARGS);
        return;
      }
      seen |= mask;

      auto* stored_vmo = vmo_store_.GetVmo(id);
      if (!stored_vmo) {
        LOGF_WARN("UnregisterForTx: VMO %d not found, replying NOT_FOUND", id);
        completer.Reply(0, ZX_ERR_NOT_FOUND);
        return;
      }
      if (!stored_vmo->meta().tx_registered) {
        LOGF_WARN("UnregisterForTx: VMO %d not registered for TX, replying INVALID_ARGS", id);
        completer.Reply(0, ZX_ERR_INVALID_ARGS);
        return;
      }
      if (stored_vmo->meta().state == VmoState::kReleasing ||
          stored_vmo->meta().state == VmoState::kPreparing) {
        LOGF_WARN("%s: VMO %hu has pending operation, cannot unregister for Tx", __FUNCTION__, id);
        completer.Reply(0, ZX_ERR_SHOULD_WAIT);
        return;
      }
    }

    DataVmoList to_release;
    bool has_skipped = false;
    for (VmoId id : vmos) {
      auto* stored_vmo = vmo_store_.GetVmo(id);
      stored_vmo->meta().tx_registered = false;
      switch (stored_vmo->meta().state) {
        case VmoState::kPreparing:
        case VmoState::kReleasing:
          __UNREACHABLE;
        case VmoState::kUnprepared:
          has_skipped = true;
          continue;
        case VmoState::kPrepared: {
          if (stored_vmo->meta().num_rx_buffers != 0) {
            has_skipped = true;
            continue;
          }
          prepared_vmos_.erase(stored_vmo->meta());
          to_release.push_back(&stored_vmo->meta());
        }
      }
    }
    uint8_t total_release = static_cast<uint8_t>(vmos.size());
    if (to_release.is_empty()) {
      lock.release();
      completer.Reply(total_release, ZX_OK);
      return;
    }

    // If we have skipped a VMO, we need to keep the original list of VMO IDs to
    // find out how many VMOs succeeded. Otherwise, the entirety is in the
    // to_release list and we don't need any allocation.
    std::vector<VmoId> vmo_ids;
    if (has_skipped) {
      vmo_ids = std::vector<VmoId>(vmos.begin(), vmos.end());
    }
    bool should_start = ReleaseVmosLocked(
        std::move(to_release),
        [this, completer = std::move(completer), vmo_ids = std::move(vmo_ids),
         total = total_release](fit::result<std::tuple<zx_status_t, DataVmoList>> result) mutable {
          if (result.is_error()) {
            auto& [status, rest_vmos] = result.error_value();
            {
              fbl::AutoLock lock(&control_lock_);
              // Undo the tx_registered so that the operation can be tried again.
              if (!vmo_ids.empty() && !rest_vmos.is_empty()) {
                auto it = std::ranges::find(vmo_ids, rest_vmos.front().id);
                for (; it != vmo_ids.end(); ++it) {
                  auto* stored_vmo = vmo_store_.GetVmo(*it);
                  stored_vmo->meta().tx_registered = true;
                  total--;
                }
              } else {
                for (auto& vmo : rest_vmos) {
                  vmo.tx_registered = true;
                  total--;
                }
              }
              for (auto& vmo : rest_vmos) {
                vmo.state = VmoState::kPrepared;
              }
              prepared_vmos_.splice(prepared_vmos_.end(), rest_vmos);
            }
            completer.Reply(total, status);
            return;
          }
          completer.Reply(total, ZX_OK);
        });
    if (!should_start) {
      return;
    }
  }
  fdf::Arena arena('NETD');
  ReleaseNextVmo(std::move(arena));
}

void DeviceInterface::GetPort(GetPortRequestView request, GetPortCompleter::Sync& _completer) {
  SharedAutoLock lock(&control_lock_);
  WithPort(request->id.base, [req = std::move(request->port), salt = request->id.salt](
                                 const std::unique_ptr<DevicePort>& port) mutable {
    if (port && port->id().salt == salt) {
      port->Bind(std::move(req));
    } else {
      req.Close(ZX_ERR_NOT_FOUND);
    }
  });
}

void DeviceInterface::GetPortWatcher(GetPortWatcherRequestView request,
                                     GetPortWatcherCompleter::Sync& _completer) {
  fbl::AutoLock lock(&control_lock_);
  if (teardown_state_ != TeardownState::RUNNING) {
    // Don't install new watchers after teardown has started.
    return;
  }

  fbl::AllocChecker ac;
  auto watcher = fbl::make_unique_checked<PortWatcher>(&ac);
  if (!ac.check()) {
    request->watcher.Close(ZX_ERR_NO_MEMORY);
    return;
  }

  std::array<netdev::wire::PortId, netdev::wire::kMaxPorts> port_ids;
  size_t port_id_count = 0;

  for (const PortSlot& port : ports_) {
    if (port.port) {
      port_ids[port_id_count++] = port.port->id();
    }
  }

  zx_status_t status = watcher->Bind(dispatchers_.impl_->async_dispatcher(),
                                     cpp20::span(port_ids.begin(), port_id_count),
                                     std::move(request->watcher), [this](PortWatcher& watcher) {
                                       control_lock_.Acquire();
                                       port_watchers_.erase(watcher);
                                       ContinueTeardown(TeardownState::PORT_WATCHERS);
                                     });

  if (status != ZX_OK) {
    LOGF_ERROR("failed to bind port watcher: %s", zx_status_get_string(status));
    return;
  }
  port_watchers_.push_back(std::move(watcher));
}

void DeviceInterface::Clone(CloneRequestView request, CloneCompleter::Sync& _completer) {
  if (zx_status_t status = Bind(std::move(request->device)); status != ZX_OK) {
    LOGF_ERROR("bind failed %s", zx_status_get_string(status));
  }
}

void DeviceInterface::DecommitVmo(netdev::VmoId vmo_id) {
  DataVmoStore::StoredVmo* stored_vmo;
  {
    fbl::AutoLock lock(&control_lock_);
    stored_vmo = vmo_store_.GetVmo(vmo_id);
    ZX_ASSERT(stored_vmo != nullptr);
    ZX_ASSERT(stored_vmo->meta().state == VmoState::kUnprepared);
    ZX_ASSERT(!stored_vmo->meta().tx_registered);
    ZX_ASSERT(stored_vmo->meta().withheld_rx_buffers == stored_vmo->meta().num_rx_buffers);
  }
  uint64_t size;
  ZX_ASSERT(stored_vmo->vmo()->get_size(&size) == ZX_OK);
  // TODO(https://fxbug.dev/542730293): The client should inform us of the range of the VMO that is
  // valid for RX buffers so we only decommit that range.
  zx_status_t status = stored_vmo->vmo()->op_range(ZX_VMO_OP_DECOMMIT, 0, size, nullptr, 0);
  // Not a fatal error if we failed to decommit the pages.
  if (status != ZX_OK) {
    LOGF_ERROR("cannot decommit VMO %d: %s", vmo_id, zx_status_get_string(status));
  }
}

bool DeviceInterface::PrepareRxVmos(
    RxVmoList::iterator start, RxVmoList::iterator* in_out_end,
    fit::callback<void(fit::result<std::tuple<zx_status_t, DataVmoList>>)> cb) {
  DataVmoList to_prepare;
  RxVmoList::iterator end = *in_out_end;
  RxVmoList::iterator last_releasing = end;
  bool should_start_prepare = false;
  {
    fbl::AutoLock lock(&control_lock_);
    if (session_ == nullptr) {
      *in_out_end = start;
      return false;
    }
    session_->AssertParentRxLock(*this);
    if (!session_->IsRxValid()) {
      *in_out_end = start;
      return false;
    }
    bool has_preparing = false;
    for (auto it = start; it != end; ++it) {
      switch (it->state) {
        case VmoState::kReleasing:
          last_releasing = it;
          break;
        case VmoState::kPreparing:
          has_preparing = true;
          __FALLTHROUGH;
        case VmoState::kPrepared:
          continue;
        case VmoState::kUnprepared:
          unprepared_vmos_.erase(*it);
          to_prepare.push_back(&*it);
      }
    }
    if (to_prepare.is_empty()) {
      // No preparing or releasing VMO, everything is prepared.
      if (!has_preparing && last_releasing == end) {
        lock.release();
        *in_out_end = end;
        if (cb) {
          cb(fit::ok());
        }
        return true;
      }
      *in_out_end = start;
      return false;
    }
    should_start_prepare =
        PrepareVmosLocked(std::move(to_prepare),
                          [this, cb = std::move(cb)](
                              fit::result<std::tuple<zx_status_t, DataVmoList>> result) mutable {
                            zx_status_t status = ZX_OK;
                            if (result.is_error()) {
                              auto& [err_status, rest_vmos] = result.error_value();
                              status = err_status;
                              fbl::AutoLock lock(&control_lock_);
                              for (auto& vmo : rest_vmos) {
                                vmo.state = VmoState::kUnprepared;
                              }
                              unprepared_vmos_.splice(unprepared_vmos_.end(), rest_vmos);
                            }
                            {
                              fbl::AutoLock lock(&rx_lock_);
                              if (rx_queue_) {
                                rx_queue_->AssertParentRxLocked(*this);
                                rx_queue_->VmoOpFinished(status);
                              }
                            }
                            NotifyRxVmoOpFinished(status);
                            if (cb) {
                              cb(std::move(result));
                            }
                          });
  }
  *in_out_end = last_releasing;
  if (should_start_prepare) {
    async::PostTask(dispatchers_.impl_->async_dispatcher(), [this]() {
      fdf::Arena arena('NETD');
      PrepareNextVmo(std::move(arena));
    });
  }
  return false;
}

bool DeviceInterface::ReleaseRxVmo(DataVmoMeta& vmo) {
  DataVmoList to_release;
  bool should_start_release = false;
  {
    fbl::AutoLock lock(&control_lock_);
    if (vmo.tx_registered || vmo.state != VmoState::kPrepared) {
      return false;
    }
    if (session_ == nullptr) {
      return false;
    }
    session_->AssertParentRxLock(*this);
    if (!session_->IsRxValid()) {
      return false;
    }
    prepared_vmos_.erase(vmo);
    to_release.push_back(&vmo);
    should_start_release = ReleaseVmosLocked(
        std::move(to_release), [this](fit::result<std::tuple<zx_status_t, DataVmoList>> result) {
          zx_status_t status = ZX_OK;
          if (result.is_error()) {
            auto& [status, rest_vmos] = result.error_value();
            {
              fbl::AutoLock lock(&control_lock_);
              for (auto& v : rest_vmos) {
                v.state = VmoState::kPrepared;
              }
              prepared_vmos_.splice(prepared_vmos_.end(), rest_vmos);
            }
          }
          {
            fbl::AutoLock lock(&rx_lock_);
            if (rx_queue_) {
              rx_queue_->AssertParentRxLocked(*this);
              rx_queue_->VmoOpFinished(status);
            }
          }
          NotifyRxVmoOpFinished(status);
        });
  }
  if (should_start_release) {
    async::PostTask(dispatchers_.impl_->async_dispatcher(), [this]() {
      fdf::Arena arena('NETD');
      ReleaseNextVmo(std::move(arena));
    });
  }
  return true;
}

uint16_t DeviceInterface::rx_fifo_depth() const {
  return TransformFifoDepth(device_info_.rx_depth().value_or(0));
}

uint16_t DeviceInterface::tx_fifo_depth() const {
  return TransformFifoDepth(device_info_.tx_depth().value_or(0));
}

void DeviceInterface::SessionStarted() {
  const char* session_name = session_->name();
  StartDeviceLocked();
  evt_session_started_.Trigger(session_name);
}

void DeviceInterface::SessionStopped() { StopDevice(); }

void DeviceInterface::StartDevice() {
  LOGF_TRACE("%s", __FUNCTION__);
  control_lock_.Acquire();
  StartDeviceLocked();
}

void DeviceInterface::StartDeviceLocked() {
  LOGF_TRACE("%s", __FUNCTION__);

  bool start = false;
  // Start the device if we haven't done so already.
  switch (device_status_) {
    case DeviceStatus::STARTED:
    case DeviceStatus::STARTING:
      // Remove any pending operations we may have.
      pending_device_op_ = PendingDeviceOperation::NONE;
      break;
    case DeviceStatus::STOPPING:
      // Device is currently stopping, let's record that we want to start it.
      pending_device_op_ = PendingDeviceOperation::START;
      break;
    case DeviceStatus::STOPPED:
      // Device is in STOPPED state, start it.
      device_status_ = DeviceStatus::STARTING;
      start = true;
      break;
  }

  control_lock_.Release();
  if (start) {
    StartDeviceInner();
  }
}

void DeviceInterface::StartDeviceInner() {
  LOGF_TRACE("%s", __FUNCTION__);

  fdf::Arena arena('NETD');
  device_impl_.buffer(arena)->Start().Then([this](auto& result) {
    this->control_lock_.Acquire();
    ZX_ASSERT_MSG(this->device_status_ == DeviceStatus::STARTING,
                  "device not in starting status: %s", DeviceStatusToString(this->device_status_));
    if (result.ok() && result.value().s == ZX_OK) {
      this->DeviceStarted();
      return;
    }

    LOGF_ERROR("failed to start implementation: %s", result.ok()
                                                         ? zx_status_get_string(result.value().s)
                                                         : result.FormatDescription().c_str());
    switch (this->SetDeviceStatus(DeviceStatus::STOPPED)) {
      case PendingDeviceOperation::STOP:
      case PendingDeviceOperation::NONE:
        break;
      case PendingDeviceOperation::START:
        ZX_PANIC("unexpected start pending while starting already");
        break;
    }
    if (this->session_) {
      LOGF_ERROR("killing session '%s' because device failed to start", this->session_->name());
      this->session_->Kill();
    }
    // We have effectively shut down the device, so finish tearing it down.
    this->ContinueTeardown(TeardownState::SESSION);
  });
}

void DeviceInterface::StopDevice(std::optional<TeardownState> continue_teardown) {
  LOGF_TRACE("%s", __FUNCTION__);
  bool stop = false;
  switch (device_status_) {
    case DeviceStatus::STOPPED:
    case DeviceStatus::STOPPING:
      // Remove any pending operations we may have.
      pending_device_op_ = PendingDeviceOperation::NONE;
      break;
    case DeviceStatus::STARTING:
      // Device is currently starting, let's record that we want to stop it.
      pending_device_op_ = PendingDeviceOperation::STOP;
      break;
    case DeviceStatus::STARTED:
      // Device is in STARTED state, stop it.
      device_status_ = DeviceStatus::STOPPING;
      stop = true;
  }
  if (continue_teardown.has_value()) {
    bool did_teardown = ContinueTeardown(continue_teardown.value());
    stop = stop && !did_teardown;
  } else {
    control_lock_.Release();
  }
  if (stop) {
    StopDeviceInner();
  }
}

void DeviceInterface::StopDeviceInner() {
  LOGF_TRACE("%s", __FUNCTION__);
  fdf::Arena arena('NETD');
  device_impl_.buffer(arena)->Stop().Then(
      [this](fdf::WireUnownedResult<netdriver::NetworkDeviceImpl::Stop>& result) {
        this->DeviceStopped();
      });
}

PendingDeviceOperation DeviceInterface::SetDeviceStatus(DeviceStatus status) {
  PendingDeviceOperation pending_op = pending_device_op_;
  device_status_ = status;
  pending_device_op_ = PendingDeviceOperation::NONE;
  return pending_op;
}

void DeviceInterface::DeviceStarted() {
  LOGF_TRACE("%s", __FUNCTION__);
  switch (SetDeviceStatus(DeviceStatus::STARTED)) {
    case PendingDeviceOperation::STOP:
      StopDevice();
      return;
    case PendingDeviceOperation::NONE:
    case PendingDeviceOperation::START:
      break;
  }
  NotifyTxQueueAvailable();
  control_lock_.Release();
  // Notify Rx queue that the device has started.
  rx_queue_->TriggerRxWatch();
}

void DeviceInterface::DeviceStopped() {
  LOGF_TRACE("%s", __FUNCTION__);
  control_lock_.Acquire();

  PendingDeviceOperation pending_op = SetDeviceStatus(DeviceStatus::STOPPED);
  if (ContinueTeardown(TeardownState::SESSION)) {
    return;
  }
  switch (pending_op) {
    case PendingDeviceOperation::START:
      StartDevice();
      return;
    case PendingDeviceOperation::NONE:
    case PendingDeviceOperation::STOP:
      break;
  }
}

bool DeviceInterface::ContinueTeardown(network::internal::DeviceInterface::TeardownState state) {
  // The teardown process goes through different phases, encoded by the
  // TeardownState enumeration.
  // - RUNNING: no teardown is in process. We move out of the RUNNING state by
  // calling Unbind on all the DeviceInterface's bindings.
  // - BINDINGS: Waiting for all bindings to close. Only moves to next state
  // once all bindings are closed, then calls unbind on all watchers and moves
  // to the WATCHERS state.
  // - PORTS: Waiting for all ports to teardown. Only moves to the next state
  // once all ports are destroyed, then proceeds to stop and destroy all
  // sessions.
  // - SESSIONS: Waiting for all sessions to be closed and destroyed (dead or
  // alive). Once all the sessions are properly destroyed proceed to tear down
  // the device implementation.
  // - DEVICE_IMPL: Waiting for the device impl wire client to complete
  // teardown. Only moves to the next state once the wire client has completed
  // teardown and moves to the IFC_DISPATCHER state.
  // - FACTORY: Waiting for the network device factory to complete shutdown if
  // an asynchronous shutdown was indicated.
  // - IFC_DISPATCHER: Waiting for the the network device interface dispatcher
  // to complete shutdown. Only moves to the next state once the dispatcher is
  // completely shut down.
  // - PORT_DISPATCHER: Waiting for the port dispatcher to complete shutdown.
  // This is the final stage, once the wire client is torn down the
  // teardown_callback_ will be triggered, marking the end of the teardown
  // process. To protect the linearity of the teardown process, once it has
  // started (the state is no longer RUNNING) no more bindings, watchers, or
  // sessions can be created.

  fit::callback<void()> teardown_callback =
      [this, state]() __TA_REQUIRES(control_lock_) -> fit::callback<void()> {
    if (state != teardown_state_) {
      return nullptr;
    }
    switch (teardown_state_) {
      case TeardownState::RUNNING: {
        teardown_state_ = TeardownState::BINDINGS;
        LOGF_TRACE("teardown state is BINDINGS (%ld bindings to destroy)", bindings_.size());
        if (!bindings_.is_empty()) {
          for (auto& b : bindings_) {
            b.Unbind();
          }
        }
        __FALLTHROUGH;
      }
      case TeardownState::BINDINGS: {
        // Pre-condition to enter port watchers state: bindings must be empty.
        if (!bindings_.is_empty()) {
          return nullptr;
        }
        teardown_state_ = TeardownState::PORT_WATCHERS;
        LOGF_TRACE("teardown state is PORT_WATCHERS (%ld watchers to destroy)",
                   port_watchers_.size());
        if (!port_watchers_.is_empty()) {
          for (auto& w : port_watchers_) {
            w.Unbind();
          }
        }
        __FALLTHROUGH;
      }
      case TeardownState::PORT_WATCHERS: {
        // Pre-condition to enter ports state: port watchers must be empty.
        if (!port_watchers_.is_empty()) {
          return nullptr;
        }
        teardown_state_ = TeardownState::PORTS;
        size_t port_count = 0;
        for (auto& p : ports_) {
          if (p.port) {
            p.port->Teardown();
            port_count++;
          }
        }
        LOGF_TRACE("teardown state is PORTS (%ld ports to destroy)", port_count);
        __FALLTHROUGH;
      }
      case TeardownState::PORTS: {
        // Pre-condition to enter sessions state: ports must all be destroyed.
        if (std::any_of(ports_.begin(), ports_.end(),
                        [](const PortSlot& port) { return static_cast<bool>(port.port); })) {
          return nullptr;
        }
        teardown_state_ = TeardownState::SESSION;
        LOGF_TRACE("teardown state is SESSION");
        if (session_) {
          // If we have any sessions, signal all of them to stop their threads
          // callback. Each session that finishes operating will go through the
          // `NotifyDeadSession` machinery. The teardown is only complete when all
          // sessions are destroyed.
          LOG_TRACE("teardown: sessions are running, scheduling teardown");
          if (session_->AssertParentControlLock(*this), !session_->IsDying()) {
            session_->Kill();
          }
          return nullptr;
        }
        // We can teardown immediately, let it fall through
        __FALLTHROUGH;
      }
      case TeardownState::SESSION: {
        // Condition to finish teardown: no more sessions exists (dead or alive)
        // and the device state is STOPPED.
        if (!session_ && device_status_ == DeviceStatus::STOPPED) {
          teardown_state_ = TeardownState::DEVICE_IMPL;
          LOG_TRACE("teardown: async teardown of device");
          if (device_impl_.is_valid()) {
            device_impl_.AsyncTeardown();
            return nullptr;
          }
        } else {
          LOG_TRACE("teardown: Still pending sessions teardown");
          return nullptr;
        }
        // The device impl is already torn down, continue on to the next state.
        __FALLTHROUGH;
      }
      case TeardownState::DEVICE_IMPL:
        LOG_TRACE("teardown state is DEVICE_IMPL");
        teardown_state_ = TeardownState::IFC_BINDING;
        if (ifc_binding_.has_value()) {
          ifc_binding_->Unbind();
          return nullptr;
        }
        // No IFC binding, proceed to next step.
        __FALLTHROUGH;
      case TeardownState::IFC_BINDING:
        LOG_TRACE("teardown state is IFC_BINDING");
        teardown_state_ = TeardownState::BINDER;
        if (binder_) {
          NetworkDeviceImplBinder::Synchronicity synchronicity = binder_->Teardown([this] {
            control_lock_.Acquire();
            ContinueTeardown(TeardownState::BINDER);
          });
          if (synchronicity == NetworkDeviceImplBinder::Synchronicity::Async) {
            // The teardown of the binder will complete asynchronously, the
            // callback will trigger the transition to the next state.
            LOG_TRACE("teardown: async teardown of binder");
            return nullptr;
          }
          // The teardown complete synchronously, continue on to the next state.
        }
        // There was no binder or teardown is already complete, move immediately
        // to the next step.
        __FALLTHROUGH;
      case TeardownState::BINDER:
        LOG_TRACE("teardown state is BINDER");
        teardown_state_ = TeardownState::FINISHED;
        return std::move(teardown_callback_);
      case TeardownState::FINISHED:
        ZX_PANIC("nothing to do if the teardown state is finished.");
    }
  }();
  control_lock_.Release();
  if (teardown_callback) {
    teardown_callback();
    return true;
  }
  return false;
}

void DeviceInterface::NotifyPortRxFrame(uint8_t base_id, uint64_t frame_length) {
  WithPort(base_id, [&frame_length](const std::unique_ptr<DevicePort>& port) {
    if (port) {
      DevicePort::Counters& counters = port->counters();
      counters.rx_frames.fetch_add(1);
      counters.rx_bytes.fetch_add(frame_length);
    }
  });
}

zx::result<AttachedPort> DeviceInterface::AcquirePort(
    netdev::wire::PortId port_id, cpp20::span<const netdev::wire::FrameType> rx_frame_types) {
  return WithPort(port_id.base,
                  [this, &rx_frame_types, salt = port_id.salt](
                      const std::unique_ptr<DevicePort>& port) -> zx::result<AttachedPort> {
                    if (port == nullptr || port->id().salt != salt) {
                      return zx::error(ZX_ERR_NOT_FOUND);
                    }
                    if (std::any_of(rx_frame_types.begin(), rx_frame_types.end(),
                                    [&port](netdev::wire::FrameType frame_type) {
                                      return !port->IsValidRxFrameType(frame_type);
                                    })) {
                      return zx::error(ZX_ERR_INVALID_ARGS);
                    }
                    return zx::ok(AttachedPort(this, port.get(), rx_frame_types));
                  });
}

void DeviceInterface::OnPortTeardownComplete(DevicePort& port) {
  LOGF_TRACE("%s(%d)", __FUNCTION__, port.id().base);

  control_lock_.Acquire();
  bool stop_device = false;
  if (session_) {
    session_->AssertParentControlLock(*this);
    if (session_->OnPortDestroyed(port.id().base)) {
      stop_device = true;
    }
  }
  ports_[port.id().base].port = nullptr;
  if (stop_device) {
    StopDevice(TeardownState::PORTS);
  } else {
    ContinueTeardown(TeardownState::PORTS);
  }
}

fbl::RefPtr<RefCountedFifo> DeviceInterface::rx_fifo() {
  SharedAutoLock lock(&control_lock_);
  if (!session_) {
    return nullptr;
  }
  session_->AssertParentControlLockShared(*this);
  if (!session_->IsDying()) {
    return session_->rx_fifo();
  }
  return nullptr;
}

void DeviceInterface::UpdatePacketArrivalRate(uint64_t sample_rate) {
  fbl::AutoLock lock(&rx_lock_);
  ZX_ASSERT(rx_queue_);
  rx_queue_->AssertParentRxLocked(*this);
  rx_queue_->UpdatePeakRateInSamplePeriod(sample_rate);
}

void DeviceInterface::TriggerTimerTick() {
  fbl::AutoLock lock(&rx_lock_);
  ZX_ASSERT(rx_queue_);
  rx_queue_->AssertParentRxLocked(*this);
  rx_queue_->TimerTick();
}

void DeviceInterface::NotifyTxQueueAvailable() { tx_queue_->Resume(); }

void DeviceInterface::NotifyTxReturned(bool was_full) {
  SharedAutoLock lock(&control_lock_);
  if (was_full) {
    NotifyTxQueueAvailable();
  }
  PruneDeadSession();
}

void DeviceInterface::QueueRxSpace(cpp20::span<netdriver::wire::RxSpaceBuffer> rx) {
  LOGF_TRACE("%s(_, %lu)", __FUNCTION__, rx.size());
  TRACE_DURATION("network-device", "QueueRxSpace", "count", rx.size());
  fdf::Arena arena('NETD');
  fidl::VectorView data =
      fidl::VectorView<netdriver::wire::RxSpaceBuffer>::FromExternal(rx.data(), rx.size());
  fidl::OneWayStatus status = device_impl_.buffer(arena)->QueueRxSpace(data);
  if (!status.ok()) {
    LOGF_ERROR("failed to queue %zu rx space: %s", rx.size(), status.FormatDescription().c_str());
  }
}

void DeviceInterface::QueueTx(cpp20::span<netdriver::wire::TxBuffer> tx) {
  LOGF_TRACE("%s(_, %lu)", __FUNCTION__, tx.size());
  TRACE_DURATION("network-device", "QueueTx", "count", tx.size());
  fdf::Arena arena('NETD');
  fidl::VectorView data =
      fidl::VectorView<netdriver::wire::TxBuffer>::FromExternal(tx.data(), tx.size());
  fidl::OneWayStatus status = device_impl_.buffer(arena)->QueueTx(data);
  if (!status.ok()) {
    LOGF_ERROR("failed to queue %zu tx buffers: %s", tx.size(), status.FormatDescription().c_str());
  }
}

void DeviceInterface::NotifyDeadSession() {
  control_lock_.Acquire();
  LOGF_TRACE("%s('%s')", __FUNCTION__, session_->name());
  bool paused = session_->IsPaused();
  const char* session_name = session_->name();
  if (!paused) {
    // Stop the session.
    // NB: SessionStopped releases the control lock.
    SessionStopped();
  } else {
    control_lock_.Release();
  }

  // Tell rx queue this session can't be used anymore.
  rx_queue_->PurgeSession();

  rx_queue_->TriggerSessionChanged();

  LOGF_TRACE("%s('%s') session is dead, waiting for buffers to be reclaimed", __FUNCTION__,
             session_name);
  // The session may also be eligible for immediate destruction if all buffers
  // are already returned. Let PruneDeadSession do the checking and cleanup
  // work.
  SharedAutoLock lock(&control_lock_);
  PruneDeadSession();
}

void DeviceInterface::PruneDeadSession() __TA_REQUIRES_SHARED(control_lock_) {
  if (!session_) {
    return;
  }
  session_->AssertParentControlLockShared(*this);
  if (!session_->IsDying()) {
    return;
  }
  if (!pending_prepare_vmos_.is_empty() || !pending_release_vmos_.is_empty()) {
    LOGF_TRACE("%s: VMO operations still pending", __FUNCTION__);
    return;
  }
  if (!session_->ShouldDestroy()) {
    LOGF_TRACE("%s: %s still pending", __FUNCTION__, session_->name());
    return;
  }

  // Schedule for destruction.
  //
  // Destruction must happen later because we currently hold shared access to
  // the control lock and we need an exclusive lock to release the VMO.
  //
  // ShouldDestroy should only return true once in the lifetime of a session,
  // which guarantees that postponing the destruction on the dispatcher is
  // always safe.
  async::PostTask(dispatchers_.impl_->async_dispatcher(), [this]() {
    DataVmoList vmos;
    bool should_start_release = false;
    bool is_empty = false;
    {
      fbl::AutoLock lock(&control_lock_);
      LOGF_TRACE("destroying %s", session_->name());
      vmos = std::move(prepared_vmos_);
      is_empty = vmos.is_empty();
      if (!is_empty) {
        should_start_release = ReleaseVmosLocked(
            std::move(vmos),
            [this](fit::result<std::tuple<zx_status_t, DataVmoList>> result) mutable {
              if (result.is_error()) {
                fbl::AutoLock lock(&control_lock_);
                auto&& [status, rest] = result.error_value();
                // We failed to release it from the vendor driver, we should still
                // unregister it locally.
                DataVmoMeta* failed = rest.pop_front();
                failed->state = VmoState::kUnprepared;
                if (zx::result<zx::vmo> result = vmo_store_.Unregister(failed->id);
                    !result.is_ok()) {
                  LOGF_ERROR("%s: Failed to unregister fail-to-release VMO %u: %s",
                             session_->name(), failed->id, result.status_string());
                }
                // Retry to release the rest of the VMOs.
                for (auto& v : rest) {
                  v.state = VmoState::kPrepared;
                }
                prepared_vmos_ = std::move(rest);
                PruneDeadSession();
                return;
              }
              RemoveDeadSession();
            });
      }
    }
    if (is_empty) {
      RemoveDeadSession();
      return;
    }
    if (!should_start_release) {
      return;
    }
    fdf::Arena arena('NETD');
    ReleaseNextVmo(std::move(arena));
  });
}

void DeviceInterface::RemoveDeadSession() __TA_EXCLUDES(rx_lock_) __TA_EXCLUDES(control_lock_) {
  {
    fbl::AutoLock rx_lock(&rx_lock_);
    rx_queue_->AssertParentRxLocked(*this);
    rx_queue_->SetSession(nullptr);
    rx_queue_->SetRxVmos({});
  }
  control_lock_.Acquire();
  std::string session_name = session_->name();
  ZX_ASSERT_MSG(prepared_vmos_.is_empty(), "should not have leftover prepared vmos");
  ZX_ASSERT_MSG(pending_prepare_vmos_.is_empty(), "should not have leftover pending prepare vmos");
  ZX_ASSERT_MSG(pending_release_vmos_.is_empty(), "should not have leftover pending release vmos");
  while (!unprepared_vmos_.is_empty()) {
    DataVmoMeta* cur = unprepared_vmos_.pop_front();
    ZX_ASSERT_MSG(cur->state == VmoState::kUnprepared,
                  "VMO %d in unprepared_vmos_ is not unprepared", cur->id);
    if (zx::result<zx::vmo> result = vmo_store_.Unregister(cur->id); !result.is_ok()) {
      LOGF_ERROR("%s: Failed to unregister unprepared VMO %u: %s", session_name.c_str(), cur->id,
                 result.status_string());
    }
  }
  session_ = nullptr;
  evt_session_died_.Trigger(session_name.c_str());
  ContinueTeardown(TeardownState::SESSION);
}

void DeviceInterface::CommitSession() {
  if (session_) {
    session_->AssertParentRxLock(*this);
    session_->CommitRx();
  }
  PruneDeadSession();
}

zx_status_t DeviceInterface::LoadRxDescriptors(RxSessionTransaction& transact) {
  if (!session_) {
    return ZX_ERR_BAD_STATE;
  }
  return session_->LoadRxDescriptors(transact);
}

bool DeviceInterface::IsDataPlaneOpen() { return device_status_ == DeviceStatus::STARTED; }

zx_status_t DeviceInterface::CanCreatePortWithId(uint8_t port_id) {
  // Don't allow new ports if tearing down.
  if (teardown_state_ != TeardownState::RUNNING) {
    LOGF_ERROR("port %u not added, teardown in progress", port_id);
    return ZX_ERR_BAD_STATE;
  }
  if (port_id >= ports_.size()) {
    LOGF_ERROR("port id %u out of allowed range: [0, %lu)", port_id, ports_.size());
    return ZX_ERR_INVALID_ARGS;
  }
  if (ports_[port_id].port != nullptr) {
    LOGF_ERROR("port %u already exists", port_id);
    return ZX_ERR_ALREADY_EXISTS;
  }
  return ZX_OK;
}

void DeviceInterface::NotifyRxQueuePacket(uint64_t key) { evt_rx_queue_packet_.Trigger(key); }

void DeviceInterface::NotifyRxVmoOpFinished(zx_status_t status) {
  evt_rx_vmo_op_finished_.Trigger(status);
}

void DeviceInterface::NotifyTxComplete() { evt_tx_complete_.Trigger(); }

void DeviceInterface::DropDelegatedRxLease(netdev::DelegatedRxLease lease) {
  // Expand all variants in case the representation of a lease changes such that
  // simply destroying the natural type is not enough to drop the lease.
  switch (lease.handle()->Which()) {
    case netdev::DelegatedRxLeaseHandle::Tag::kChannel:
    case netdev::DelegatedRxLeaseHandle::Tag::kEventpair:
    case netdev::DelegatedRxLeaseHandle::Tag::_do_not_handle_this__write_a_default_case_instead:
      break;
  }
}

void DeviceInterface::TryDelegateRxLease(uint64_t completed_frame_index) {
  if (!rx_lease_pending_.has_value()) {
    return;
  }
  netdev::DelegatedRxLease& pending = rx_lease_pending_.value();
  if (completed_frame_index < pending.hold_until_frame().value()) {
    return;
  }

  if (session_ && session_->AllowRxLeaseDelegation()) {
    session_->AssertParentRxLock(*this);
    session_->DelegateRxLease(std::move(pending));
  } else {
    DropDelegatedRxLease(std::move(pending));
  }
  rx_lease_pending_.reset();
}

DeviceInterface::DeviceInterface(const DeviceInterfaceDispatchers& dispatchers)
    : dispatchers_(dispatchers),
      vmo_store_(vmo_store::Options{
          vmo_store::MapOptions{ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_REQUIRE_NON_RESIZABLE,
                                nullptr},
          std::nullopt,
      }) {
  // Seed the port salts to some non-random but unpredictable value.
  union {
    uint8_t b[sizeof(uintptr_t)];
    uintptr_t ptr;
  } seed = {.ptr = reinterpret_cast<uintptr_t>(this)};
  for (size_t i = 0; i < ports_.size(); i++) {
    ports_[i].salt = static_cast<uint8_t>(i) ^ seed.b[i % sizeof(seed.b)];
  }
}

zx_status_t DeviceInterface::Binding::Bind(DeviceInterface* interface,
                                           fidl::ServerEnd<netdev::Device> channel) {
  fbl::AllocChecker ac;
  std::unique_ptr<Binding> binding(new (&ac) Binding);
  if (!ac.check()) {
    return ZX_ERR_NO_MEMORY;
  }
  auto* binding_ptr = binding.get();
  binding->binding_ = fidl::BindServer(
      interface->dispatchers_.impl_->async_dispatcher(), std::move(channel), interface,
      [binding_ptr](DeviceInterface* interface, fidl::UnbindInfo /*unused*/,
                    fidl::ServerEnd<fuchsia_hardware_network::Device> /*unused*/) {
        bool bindings_empty;
        interface->control_lock_.Acquire();
        interface->bindings_.erase(*binding_ptr);
        bindings_empty = interface->bindings_.is_empty();
        if (bindings_empty) {
          interface->ContinueTeardown(TeardownState::BINDINGS);
        } else {
          interface->control_lock_.Release();
        }
      });
  interface->bindings_.push_front(std::move(binding));
  return ZX_OK;
}

void DeviceInterface::Binding::Unbind() {
  auto binding = std::move(binding_);
  if (binding.has_value()) {
    binding->Unbind();
  }
}

}  // namespace internal
}  // namespace network
