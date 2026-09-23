// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "test_util.h"

#include <lib/sync/cpp/completion.h>

#include <iostream>

#include <gtest/gtest.h>

#include "src/lib/testing/predicates/status.h"

namespace network::testing {

zx::result<std::vector<uint8_t>> TxBuffer::GetData(const VmoProvider& vmo_provider) {
  if (!vmo_provider) {
    return zx::error(ZX_ERR_INTERNAL);
  }
  // We don't support copying chained buffers.
  if (buffer_.data.size() != 1) {
    return zx::error(ZX_ERR_INTERNAL);
  }
  const fuchsia_hardware_network_driver::wire::BufferRegion& region = buffer_.data.at(0);
  zx::unowned_vmo vmo = vmo_provider(region.vmo);
  if (!vmo->is_valid()) {
    return zx::error(ZX_ERR_INTERNAL);
  }
  std::vector<uint8_t> copy;
  copy.resize(region.length);
  zx_status_t status = vmo->read(copy.data(), region.offset, region.length);
  if (status != ZX_OK) {
    return zx::error(status);
  }
  return zx::ok(std::move(copy));
}

zx_status_t RxBuffer::WriteData(cpp20::span<const uint8_t> data, const VmoProvider& vmo_provider) {
  if (!vmo_provider) {
    return ZX_ERR_INTERNAL;
  }
  if (data.size() > space_.region.length) {
    return ZX_ERR_INVALID_ARGS;
  }
  zx::unowned_vmo vmo = vmo_provider(space_.region.vmo);
  return_part_.length = static_cast<uint32_t>(data.size());
  return vmo->write(data.data(), space_.region.offset, data.size());
}

FakeNetworkPortImpl::FakeNetworkPortImpl()
    : port_info_({
          .port_class = netdev::wire::PortClass::kEthernet,
          .rx_types = {netdev::wire::FrameType::kEthernet},
          .tx_types = {{.type = netdev::wire::FrameType::kEthernet,
                        .features = netdev::wire::kFrameFeaturesRaw,
                        .supported_flags = netdev::wire::TxFlags(0)}},

      }) {
  EXPECT_OK(zx::event::create(0, &event_));
}

FakeNetworkPortImpl::~FakeNetworkPortImpl() {
  if (port_added_) {
    EXPECT_TRUE(port_removed_) << "port was added but remove was not called";
  }
}

void FakeNetworkPortImpl::WaitPortRemoved() {
  if (port_added_) {
    WaitForPortRemoval();
    ASSERT_TRUE(port_removed_) << "port was added but remove was not called";
  }
}

void FakeNetworkPortImpl::GetInfo(fdf::Arena& arena, GetInfoCompleter::Sync& completer) {
  fidl::Arena fidl_arena;
  auto builder = fuchsia_hardware_network::wire::PortBaseInfo::Builder(fidl_arena);
  auto rx_types = fidl::VectorView<netdev::wire::FrameType>::FromExternal(port_info_.rx_types);
  auto tx_types =
      fidl::VectorView<netdev::wire::FrameTypeSupport>::FromExternal(port_info_.tx_types);

  builder.port_class(port_info_.port_class)
      .tx_types(fidl::ObjectView<decltype(tx_types)>::FromExternal(&tx_types))
      .rx_types(fidl::ObjectView<decltype(rx_types)>::FromExternal(&rx_types));

  completer.buffer(arena).Reply(builder.Build());
}

void FakeNetworkPortImpl::GetStatus(fdf::Arena& arena, GetStatusCompleter::Sync& completer) {
  fidl::Arena fidl_arena;
  auto builder = fuchsia_hardware_network::wire::PortStatus::Builder(fidl_arena);
  builder.mtu(status_.mtu).flags(status_.flags);
  completer.buffer(arena).Reply(builder.Build());
}

void FakeNetworkPortImpl::SetActive(
    fuchsia_hardware_network_driver::wire::NetworkPortSetActiveRequest* request, fdf::Arena& arena,
    SetActiveCompleter::Sync& completer) {
  port_active_ = request->active;
  if (on_set_active_) {
    on_set_active_(request->active);
  }
  ASSERT_OK(event_.signal(0, kEventPortActiveChanged));
}

void FakeNetworkPortImpl::Removed(fdf::Arena& arena, RemovedCompleter::Sync& completer) {
  ASSERT_FALSE(port_removed_) << "removed same port twice";
  port_removed_ = true;
  sync_completion_signal(&wait_removed_);
}

void FakeNetworkPortImpl::GetMac(fdf::Arena& arena, GetMacCompleter::Sync& completer) {
  fdf::ClientEnd<fuchsia_hardware_network_driver::MacAddr> client{};
  if (mac_client_end_.has_value()) {
    client = std::move(*mac_client_end_);
    mac_client_end_ = {};
  }
  completer.buffer(arena).Reply(std::move(client));
}

zx_status_t FakeNetworkPortImpl::AddPort(uint8_t port_id, fdf_dispatcher_t* dispatcher,
                                         fidl::WireSyncClient<netdev::Device> device,
                                         NetDevIfcClient& netdev_ifc) {
  if (port_added_) {
    return ZX_ERR_ALREADY_EXISTS;
  }
  id_ = port_id;
  netdev_ifc_ = &netdev_ifc;

  zx::result fidl_endpoints = fidl::CreateEndpoints<fuchsia_hardware_network::PortWatcher>();
  if (fidl_endpoints.is_error()) {
    return fidl_endpoints.status_value();
  }
  auto status = device->GetPortWatcher(std::move(fidl_endpoints->server));
  if (!status.ok()) {
    return status.status();
  }
  fidl::WireSyncClient port_watcher(std::move(fidl_endpoints->client));

  bool found_idle = false;
  while (!found_idle) {
    auto result = port_watcher->Watch();
    if (!result.ok()) {
      return result.status();
    }
    found_idle = result->event.Which() == netdev::wire::DevicePortEvent::Tag::kIdle;
  }

  auto endpoints = fdf::CreateEndpoints<fuchsia_hardware_network_driver::NetworkPort>();
  if (endpoints.is_error()) {
    return endpoints.status_value();
  }

  binding_ = fdf::BindServer(dispatcher, std::move(endpoints->server), this);

  fdf::Arena arena('NETD');
  auto add_port_status =
      netdev_ifc_->sync().buffer(arena)->AddPort(port_id, std::move(endpoints->client));
  if (!add_port_status.ok() || add_port_status->status != ZX_OK) {
    return add_port_status.ok() ? add_port_status->status : add_port_status.status();
  }

  auto result = port_watcher->Watch();
  if (!result.ok()) {
    return result.status();
  }

  if (result->event.Which() != netdev::wire::DevicePortEvent::Tag::kAdded) {
    return ZX_ERR_BAD_STATE;
  }

  port_added_ = true;
  device_ = std::move(device);
  return ZX_OK;
}

zx_status_t FakeNetworkPortImpl::AddPortNoWait(uint8_t port_id, fdf_dispatcher_t* dispatcher,
                                               fidl::WireSyncClient<netdev::Device> device,
                                               NetDevIfcClient& netdev_ifc) {
  if (port_added_) {
    return ZX_ERR_ALREADY_EXISTS;
  }
  id_ = port_id;
  netdev_ifc_ = &netdev_ifc;

  auto endpoints = fdf::CreateEndpoints<fuchsia_hardware_network_driver::NetworkPort>();
  if (endpoints.is_error()) {
    return endpoints.status_value();
  }

  binding_ = fdf::BindServer(
      dispatcher, std::move(endpoints->server), this,
      [](fdf::WireServer<fuchsia_hardware_network_driver::NetworkPort>*, fidl::UnbindInfo foo,
         fdf::ServerEnd<fuchsia_hardware_network_driver::NetworkPort> /*unused*/) {});

  fdf::Arena arena('NETD');
  auto status = netdev_ifc_->sync().buffer(arena)->AddPort(port_id, std::move(endpoints->client));
  if (!status.ok() || status->status != ZX_OK) {
    return status.ok() ? status->status : status.status();
  }

  port_added_ = true;
  device_ = std::move(device);
  return ZX_OK;
}

void FakeNetworkPortImpl::RemoveSync() {
  // Already removed.
  if (!port_added_ || port_removed_) {
    return;
  }
  fdf::Arena arena('NETD');
  EXPECT_TRUE(netdev_ifc_->buffer(arena)->RemovePort(id_).ok());
  WaitForPortRemoval();
}

void FakeNetworkPortImpl::SetOnline(bool online) {
  PortStatus status = status_;
  status.flags = online ? netdev::wire::StatusFlags::kOnline : netdev::wire::StatusFlags();
  SetStatus(status);
}

void FakeNetworkPortImpl::SetStatus(const PortStatus& status) {
  status_ = status;
  if (netdev_ifc_ && netdev_ifc_->is_valid()) {
    fidl::Arena fidl_arena;
    auto builder = fuchsia_hardware_network::wire::PortStatus::Builder(fidl_arena);
    builder.mtu(status_.mtu).flags(status_.flags);
    fdf::Arena arena('NETD');
    EXPECT_TRUE(netdev_ifc_->buffer(arena)->PortStatusChanged(id_, builder.Build()).ok());
  }
}

FakeNetworkDeviceImpl::FakeNetworkDeviceImpl(fdf_dispatcher_t* dispatcher)
    : dispatcher_(dispatcher),
      info_({
          .tx_depth = kDefaultTxDepth,
          .rx_depth = kDefaultRxDepth,
          .rx_threshold = kDefaultRxDepth / 2,
          .max_buffer_length = kDefaultBufferLength,
          .buffer_alignment = kBufferAlignment,
      }) {
  EXPECT_OK(zx::event::create(0, &event_));
}

FakeNetworkDeviceImpl::~FakeNetworkDeviceImpl() {
  // ensure that all VMOs were released
  for (auto& vmo : vmos_) {
    ZX_ASSERT(!vmo.is_valid());
  }
}

void FakeNetworkDeviceImpl::Init(
    fuchsia_hardware_network_driver::wire::NetworkDeviceImplInitRequest* request, fdf::Arena& arena,
    InitCompleter::Sync& completer) {
  device_client_ = fdf::WireSharedClient(std::move(request->iface), dispatcher_);
  completer.buffer(arena).Reply(ZX_OK);
}

void FakeNetworkDeviceImpl::Start(fdf::Arena& arena, StartCompleter::Sync& completer) {
  fbl::AutoLock lock(&lock_);
  EXPECT_FALSE(device_started_) << "called start on already started device";
  if (auto_start_.has_value()) {
    const zx_status_t auto_start = auto_start_.value();
    if (auto_start == ZX_OK) {
      device_started_ = true;
    }
    if (on_start_) {
      on_start_();
    }
    completer.buffer(arena).Reply(auto_start);
  } else {
    ZX_ASSERT(!(pending_start_callback_ || pending_stop_callback_));
    pending_start_callback_ = [completer = completer.ToAsync(), this]() mutable {
      {
        fbl::AutoLock lock(&lock_);
        device_started_ = true;
        if (on_start_) {
          on_start_();
        }
      }
      fdf::Arena arena('NETD');
      completer.buffer(arena).Reply(ZX_OK);
    };
  }
  EXPECT_OK(event_.signal(0, kEventStartInitiated));
}

void FakeNetworkDeviceImpl::Stop(fdf::Arena& arena, StopCompleter::Sync& completer) {
  fbl::AutoLock lock(&lock_);
  EXPECT_TRUE(device_started_) << "called stop on already stopped device";
  device_started_ = false;
  zx_signals_t clear;
  if (auto_stop_) {
    RxFidlReturnTransaction rx_return(this);
    while (!rx_buffers_.is_empty()) {
      std::unique_ptr rx_buffer = rx_buffers_.pop_front();
      // Return unfulfilled buffers with zero length and an invalid port number.
      // Zero length buffers are returned to the pool and the port metadata is ignored.
      rx_buffer->return_part().length = 0;
      rx_return.Enqueue(std::move(rx_buffer), netdev::wire::kMaxPorts);
    }
    rx_return.Commit();

    TxFidlReturnTransaction tx_return(this);
    while (!tx_buffers_.is_empty()) {
      std::unique_ptr tx_buffer = tx_buffers_.pop_front();
      tx_buffer->set_status(ZX_ERR_UNAVAILABLE);
      tx_return.Enqueue(std::move(tx_buffer));
    }
    tx_return.Commit();
    fdf::Arena arena('NETD');
    completer.buffer(arena).Reply();
    //  Must clear the queue signals if we're clearing the queues automatically.
    clear = kEventTx | kEventRxAvailable;
  } else {
    ZX_ASSERT(!(pending_start_callback_ || pending_stop_callback_));
    pending_stop_callback_ = [completer = completer.ToAsync()]() mutable {
      fdf::Arena arena('NETD');
      completer.buffer(arena).Reply();
    };
    clear = 0;
  }
  EXPECT_OK(event_.signal(clear, kEventStop));
}

void FakeNetworkDeviceImpl::GetInfo(fdf::Arena& arena, GetInfoCompleter::Sync& completer) {
  fidl::Arena fidl_arena;
  auto builder = fuchsia_hardware_network_driver::wire::DeviceImplInfo::Builder(fidl_arena);

  builder.device_features(info_.device_features)
      .tx_depth(info_.tx_depth)
      .rx_depth(info_.rx_depth)
      .rx_threshold(info_.rx_threshold)
      .max_buffer_parts(info_.max_buffer_parts)
      .max_buffer_length(info_.max_buffer_length)
      .buffer_alignment(info_.buffer_alignment)
      .buffer_alignment(info_.buffer_alignment)
      .min_rx_buffer_length(info_.min_rx_buffer_length)
      .min_tx_buffer_length(info_.min_tx_buffer_length)
      .tx_head_length(info_.tx_head_length)
      .tx_tail_length(info_.tx_tail_length);
  if (info_.min_rx_buffers != 0) {
    builder.min_rx_buffers(info_.min_rx_buffers);
  }

  completer.buffer(arena).Reply(builder.Build());
}

void FakeNetworkDeviceImpl::QueueTx(
    fuchsia_hardware_network_driver::wire::NetworkDeviceImplQueueTxRequest* request,
    fdf::Arena& arena, QueueTxCompleter::Sync& completer) {
  EXPECT_NE(request->buffers.size(), 0u);
  ASSERT_TRUE(device_client_.is_valid());

  fbl::AutoLock lock(&lock_);
  cpp20::span buffers = request->buffers.get();
  queue_tx_called_.push_back(buffers.size());
  if (immediate_return_tx_ || !device_started_) {
    const zx_status_t return_status = device_started_ ? ZX_OK : ZX_ERR_UNAVAILABLE;
    ASSERT_LE(request->buffers.size(), kDefaultTxDepth);
    std::array<fuchsia_hardware_network_driver::wire::TxResult, kDefaultTxDepth> results;
    auto results_iter = results.begin();
    for (const fuchsia_hardware_network_driver::wire::TxBuffer& buff : buffers) {
      *results_iter++ = {
          .id = buff.id,
          .status = return_status,
      };
    }
    auto output = fidl::VectorView<fuchsia_hardware_network_driver::wire::TxResult>::FromExternal(
        results.data(), request->buffers.size());
    EXPECT_TRUE(device_client_.buffer(arena)->CompleteTx(output).ok());
    return;
  }

  for (const fuchsia_hardware_network_driver::wire::TxBuffer& buff : buffers) {
    auto back = std::make_unique<TxBuffer>(buff);
    tx_buffers_.push_back(std::move(back));
  }
  EXPECT_OK(event_.signal(0, kEventTx));
}

void FakeNetworkDeviceImpl::QueueRxSpace(
    fuchsia_hardware_network_driver::wire::NetworkDeviceImplQueueRxSpaceRequest* request,
    fdf::Arena& arena, QueueRxSpaceCompleter::Sync& completer) {
  ASSERT_TRUE(device_client_.is_valid());
  size_t buf_count = request->buffers.size();

  fbl::AutoLock lock(&lock_);
  queue_rx_space_called_.push_back(buf_count);
  auto buffers = request->buffers.get();
  if (immediate_return_rx_ || !device_started_) {
    const uint32_t length = device_started_ ? kAutoReturnRxLength : 0;
    ASSERT_TRUE(buf_count < kDefaultTxDepth);
    std::array<fuchsia_hardware_network_driver::wire::RxBuffer, kDefaultTxDepth> results;
    std::array<fuchsia_hardware_network_driver::wire::RxBufferPart, kDefaultTxDepth> parts;
    auto results_iter = results.begin();
    auto parts_iter = parts.begin();
    for (const fuchsia_hardware_network_driver::wire::RxSpaceBuffer& space : buffers) {
      fuchsia_hardware_network_driver::wire::RxBufferPart& part = *parts_iter++;
      fuchsia_hardware_network_driver::wire::RxBuffer& rx_buffer = *results_iter++;
      part = {
          .id = space.id,
          .length = length,
      };
      rx_buffer = {
          .meta =
              {
                  .frame_type = fuchsia_hardware_network::wire::FrameType::kEthernet,
              },
          .data =
              fidl::VectorView<fuchsia_hardware_network_driver::wire::RxBufferPart>::FromExternal(
                  &part, 1),
      };
    }
    auto output = fidl::VectorView<fuchsia_hardware_network_driver::wire::RxBuffer>::FromExternal(
        results.data(), buf_count);
    EXPECT_TRUE(device_client_.buffer(arena)->CompleteRx(output).ok());
    return;
  }

  for (const fuchsia_hardware_network_driver::wire::RxSpaceBuffer& buff : buffers) {
    auto back = std::make_unique<RxBuffer>(buff);
    rx_buffers_.push_back(std::move(back));
  }
  EXPECT_OK(event_.signal(0, kEventRxAvailable));
}

void FakeNetworkDeviceImpl::PrepareVmo(
    fuchsia_hardware_network_driver::wire::NetworkDeviceImplPrepareVmoRequest* request,
    fdf::Arena& arena, PrepareVmoCompleter::Sync& completer) {
  zx::vmo& slot = vmos_[request->id];
  EXPECT_FALSE(slot.is_valid()) << "vmo " << static_cast<uint32_t>(request->id)
                                << " already prepared";
  if (prepare_vmo_handler_) {
    if (prepare_vmo_handler_(request->id, slot, completer)) {
      slot = std::move(request->vmo);
    };
  } else {
    slot = std::move(request->vmo);
    completer.buffer(arena).Reply(ZX_OK);
  }
}
void FakeNetworkDeviceImpl::ReleaseVmo(
    fuchsia_hardware_network_driver::wire::NetworkDeviceImplReleaseVmoRequest* request,
    fdf::Arena& arena, ReleaseVmoCompleter::Sync& completer) {
  if (release_vmo_handler_) {
    release_vmo_handler_(request->id);
  }
  zx::vmo& slot = vmos_[request->id];
  EXPECT_TRUE(slot.is_valid()) << "vmo " << static_cast<uint32_t>(request->id)
                               << " already released";
  slot.reset();

  bool all_released = true;
  for (auto& vmo : vmos_) {
    if (vmo.is_valid()) {
      all_released = false;
    }
  }

  if (all_released) {
    sync_completion_signal(&released_completer_);
  }
  completer.buffer(arena).Reply();
}

fit::function<zx::unowned_vmo(uint8_t)> FakeNetworkDeviceImpl::VmoGetter() {
  return [this](uint8_t id) { return zx::unowned_vmo(vmos_[id]); };
}

bool FakeNetworkDeviceImpl::TriggerStart() {
  fbl::AutoLock lock(&lock_);
  auto cb = std::move(pending_start_callback_);
  lock.release();

  if (cb) {
    cb();
    return true;
  }
  return false;
}

bool FakeNetworkDeviceImpl::TriggerStop() {
  fbl::AutoLock lock(&lock_);
  auto cb = std::move(pending_stop_callback_);
  lock.release();

  if (cb) {
    cb();
    return true;
  }
  return false;
}

zx::result<fdf::ClientEnd<fuchsia_hardware_network_driver::NetworkDeviceImpl>>
FakeNetworkDeviceImpl::Binder::Bind() {
  auto endpoints = fdf::CreateEndpoints<
      fuchsia_hardware_network_driver::Service::NetworkDeviceImpl::ProtocolType>();
  if (endpoints.is_error()) {
    return endpoints.take_error();
  }
  binding_ = fdf::BindServer(dispatcher_, std::move(endpoints->server), parent_);
  return zx::ok(std::move(endpoints->client));
}

zx::result<std::unique_ptr<NetworkDeviceInterface>> FakeNetworkDeviceImpl::CreateChild(
    DeviceInterfaceDispatchers dispatchers) {
  dispatcher_ = dispatchers.impl_->get();
  auto binder = std::make_unique<Binder>(this, dispatcher_);

  zx::result device = internal::DeviceInterface::Create(dispatchers, std::move(binder));
  if (device.is_error()) {
    return device.take_error();
  }

  auto& value = device.value();
  value->evt_session_started_.Set(
      [this](const char* session) { event_.signal(0, kEventSessionStarted); });
  value->evt_session_died_.Set(
      [this](const char* session) { event_.signal(0, kEventSessionDied); });
  return zx::ok(std::move(value));
}

const char* LeaseHandleTypeToString(LeaseHandleType type) {
  switch (type) {
    case LeaseHandleType::kChannel:
      return "Channel";
    case LeaseHandleType::kEventpair:
      return "Eventpair";
  }
}

std::tuple<netdev::DelegatedRxLease, zx::handle> CreateDelegatedLease(LeaseHandleType type,
                                                                      uint64_t hold_until_frame) {
  netdev::DelegatedRxLease lease;
  lease.hold_until_frame(hold_until_frame);
  zx::handle handle;
  switch (type) {
    case LeaseHandleType::kChannel: {
      zx::channel a, b;
      EXPECT_OK(zx::channel::create(0, &a, &b));
      lease.handle(netdev::DelegatedRxLeaseHandle::WithChannel(std::move(a)));
      handle = std::move(b);
    } break;
    case LeaseHandleType::kEventpair: {
      zx::eventpair a, b;
      EXPECT_OK(zx::eventpair::create(0, &a, &b));
      lease.handle(netdev::DelegatedRxLeaseHandle::WithEventpair(std::move(a)));
      handle = std::move(b);
    } break;
  }
  return std::make_tuple(std::move(lease), std::move(handle));
}

}  // namespace network::testing
