// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "device_adapter.h"

#include <lib/sync/cpp/completion.h>
#include <lib/syslog/cpp/macros.h>
#include <zircon/status.h>

#include <fbl/auto_lock.h>

namespace network {
namespace tun {

class DeviceAdapterBinder : public network::NetworkDeviceImplBinder {
 public:
  DeviceAdapterBinder(DeviceAdapter* adapter) : adapter_(adapter) {}
  ~DeviceAdapterBinder() override = default;

  zx::result<fdf::ClientEnd<fuchsia_hardware_network_driver::NetworkDeviceImpl>> Bind() override {
    return zx::ok(adapter_->BindDriver());
  }

 private:
  DeviceAdapter* adapter_;  // unowned pointer to adapter
};

zx::result<std::unique_ptr<DeviceAdapter>> DeviceAdapter::Create(
    const DeviceInterfaceDispatchers& dispatchers,
    fdf::UnownedUnsynchronizedDispatcher&& netdev_dispatcher, DeviceAdapterParent* parent) {
  fbl::AllocChecker ac;
  std::unique_ptr<DeviceAdapter> adapter(
      new (&ac) DeviceAdapter(parent, dispatchers, std::move(netdev_dispatcher)));
  if (!ac.check()) {
    return zx::error(ZX_ERR_NO_MEMORY);
  }

  std::unique_ptr<DeviceAdapterBinder> binder(new (&ac) DeviceAdapterBinder(adapter.get()));
  if (!ac.check()) {
    return zx::error(ZX_ERR_NO_MEMORY);
  }

  zx::result device = NetworkDeviceInterface::Create(dispatchers, std::move(binder));
  if (device.is_error()) {
    return device.take_error();
  }
  adapter->device_ = std::move(device.value());

  return zx::ok(std::move(adapter));
}

zx_status_t DeviceAdapter::Bind(fidl::ServerEnd<fuchsia_hardware_network::Device> req) {
  return device_->Bind(std::move(req));
}

zx_status_t DeviceAdapter::BindPort(uint8_t port_id,
                                    fidl::ServerEnd<fuchsia_hardware_network::Port> req) {
  return device_->BindPort(port_id, std::move(req));
}

fdf::ClientEnd<fuchsia_hardware_network_driver::NetworkDeviceImpl> DeviceAdapter::BindDriver() {
  auto [client, server] =
      fdf::Endpoints<fuchsia_hardware_network_driver::NetworkDeviceImpl>::Create();
  fdf::BindServer(netdev_dispatcher_->get(), std::move(server), this);
  return std::move(client);
}

void DeviceAdapter::Init(
    fuchsia_hardware_network_driver::wire::NetworkDeviceImplInitRequest* request, fdf::Arena& arena,
    InitCompleter::Sync& completer) {
  device_iface_.Bind(std::move(request->iface), dispatchers_.port_->get());
  completer.buffer(arena).Reply(ZX_OK);
}

void DeviceAdapter::Start(fdf::Arena& arena, StartCompleter::Sync& completer) {
  {
    fbl::AutoLock lock(&rx_lock_);
    rx_available_ = true;
  }
  {
    fbl::AutoLock lock(&tx_lock_);
    tx_available_ = true;
  }
  completer.buffer(arena).Reply(ZX_OK);
}

void DeviceAdapter::Stop(fdf::Arena& arena, StopCompleter::Sync& completer) {
  {
    // Return all rx buffers.
    fbl::AutoLock lock(&rx_lock_);
    rx_available_ = false;

    std::array<fuchsia_hardware_network_driver::wire::RxBufferPart, kFifoDepth> rx_part;
    std::array<fuchsia_hardware_network_driver::wire::RxBuffer, kFifoDepth> rx_return;
    ZX_ASSERT_MSG(rx_buffers_.size() <= kFifoDepth, "too many pending buffers %zu",
                  rx_buffers_.size());
    auto return_head = rx_return.begin();
    auto part_head = rx_part.begin();
    while (!rx_buffers_.empty()) {
      const auto& buffer = rx_buffers_.front();
      fuchsia_hardware_network_driver::wire::RxBufferPart& part = *part_head++;
      part = {
          .id = buffer.id,
          .length = 0,
      };
      *return_head++ = {
          .meta =
              {
                  .frame_type = fuchsia_hardware_network::FrameType::kEthernet,
              },
          .data =
              fidl::VectorView<fuchsia_hardware_network_driver::wire::RxBufferPart>::FromExternal(
                  &part, 1),
      };
      rx_buffers_.pop();
    }
    if (size_t count = std::distance(rx_return.begin(), return_head); count != 0) {
      fidl::OneWayStatus status = device_iface_.buffer(arena)->CompleteRx(
          fidl::VectorView<fuchsia_hardware_network_driver::wire::RxBuffer>::FromExternal(
              rx_return.data(), count));
      if (!status.ok()) {
        FX_PLOGST(ERROR, "tun", status.status()) << "failed to return rx buffers";
        completer.Close(ZX_ERR_INTERNAL);
        parent_->RequestErrorUnbind();
        return;
      }
    }
  }
  {
    // Return all tx buffers.
    fbl::AutoLock lock(&tx_lock_);
    tx_available_ = false;

    std::array<fuchsia_hardware_network_driver::wire::TxResult, kFifoDepth> tx_return;
    ZX_ASSERT_MSG(tx_buffers_.size() <= kFifoDepth, "too many pending buffers %zu",
                  tx_buffers_.size());
    auto return_head = std::begin(tx_return);
    while (!tx_buffers_.empty()) {
      const TxBuffer& buffer = tx_buffers_.front();
      *return_head++ = {
          .id = buffer.id(),
          .status = ZX_ERR_UNAVAILABLE,
      };
      tx_buffers_.pop();
    }
    if (size_t count = std::distance(tx_return.begin(), return_head); count != 0) {
      fidl::OneWayStatus status = device_iface_.buffer(arena)->CompleteTx(
          fidl::VectorView<fuchsia_hardware_network_driver::wire::TxResult>::FromExternal(
              tx_return.data(), count));
      if (!status.ok()) {
        FX_PLOGST(ERROR, "tun", status.status()) << "failed to return tx buffers";
        completer.Close(ZX_ERR_INTERNAL);
        parent_->RequestErrorUnbind();
        return;
      }
    }
  }

  completer.buffer(arena).Reply();
}

void DeviceAdapter::GetInfo(
    fdf::Arena& arena,
    fdf::WireServer<fuchsia_hardware_network_driver::NetworkDeviceImpl>::GetInfoCompleter::Sync&
        completer) {
  fuchsia_hardware_network_driver::wire::DeviceImplInfo info =
      fuchsia_hardware_network_driver::wire::DeviceImplInfo::Builder(arena)
          .tx_depth(kFifoDepth)
          .rx_depth(kFifoDepth)
          .rx_threshold(kFifoDepth / 2)
          .max_buffer_length(fuchsia_net_tun::wire::kMaxMtu)
          .buffer_alignment(1)
          .min_rx_buffer_length(parent_->config().min_rx_buffer_length)
          .min_tx_buffer_length(parent_->config().min_tx_buffer_length)
          .min_rx_buffers(kFifoDepth)
          // Use default Simple parameters to exercise dynamic Rx buffer management in tun tests.
          .rx_buffer_management(
              fuchsia_hardware_network_driver::wire::RxBufferManagement::WithSimple(
                  arena, fuchsia_hardware_network_driver::wire::Simple{}))
          .Build();
  completer.buffer(arena).Reply(info);
}

void DeviceAdapter::QueueTx(
    fuchsia_hardware_network_driver::wire::NetworkDeviceImplQueueTxRequest* request,
    fdf::Arena& arena, QueueTxCompleter::Sync& _completer) {
  {
    fbl::AutoLock tx_lock(&tx_lock_);
    fidl::VectorView<fuchsia_hardware_network_driver::wire::TxBuffer>& buffers = request->buffers;
    if (!tx_available_) {
      FX_LOGST(DEBUG, "tun") << "Discarding " << tx_available_
                             << " tx buffers, tx queue is invalid";
      for (const auto& b : buffers) {
        EnqueueTx(b.id, ZX_ERR_UNAVAILABLE);
      }
      CommitTx();
      return;
    }
    for (const auto& b : buffers) {
      if (b.meta.port >= port_online_status_.size() || !port_online_status_[b.meta.port]) {
        EnqueueTx(b.id, ZX_ERR_UNAVAILABLE);
        continue;
      }
      tx_buffers_.emplace(vmos_.MakeTxBuffer(b, parent_->config().report_metadata));
    }
    CommitTx();
  }
  parent_->OnTxAvail(this);
}

void DeviceAdapter::QueueRxSpace(
    fuchsia_hardware_network_driver::wire::NetworkDeviceImplQueueRxSpaceRequest* request,
    fdf::Arena& arena, QueueRxSpaceCompleter::Sync& completer) {
  bool has_buffers;
  {
    fbl::AutoLock lock(&rx_lock_);
    rx_space_requested_ = false;
    const fidl::VectorView<fuchsia_hardware_network_driver::wire::RxSpaceBuffer>& buffers =
        request->buffers;
    if (!rx_available_) {
      std::array<fuchsia_hardware_network_driver::wire::RxBufferPart, kFifoDepth> rx_part;
      std::array<fuchsia_hardware_network_driver::wire::RxBuffer, kFifoDepth> rx_return;
      ZX_ASSERT_MSG(buffers.size() <= kFifoDepth, "too many queued buffers %zu", buffers.size());
      auto return_head = rx_return.begin();
      auto part_head = rx_part.begin();
      for (const auto& buffer : buffers) {
        fuchsia_hardware_network_driver::wire::RxBufferPart& part = *part_head++;
        part = {
            .id = buffer.id,
            .length = 0,
        };
        *return_head++ = {
            .meta =
                {
                    .frame_type = fuchsia_hardware_network::FrameType::kEthernet,
                },
            .data =
                fidl::VectorView<fuchsia_hardware_network_driver::wire::RxBufferPart>::FromExternal(
                    &part, 1),
        };
      }
      if (size_t count = std::distance(rx_return.begin(), return_head); count != 0) {
        fidl::OneWayStatus status = device_iface_.buffer(arena)->CompleteRx(
            fidl::VectorView<fuchsia_hardware_network_driver::wire::RxBuffer>::FromExternal(
                rx_return.data(), count));
        if (!status.ok()) {
          FX_PLOGST(ERROR, "tun", status.status()) << "failed to return rx buffers";
          completer.Close(ZX_ERR_INTERNAL);
          parent_->RequestErrorUnbind();
          return;
        }
      }
      return;
    }
    for (const auto& space : buffers) {
      rx_buffers_.push(space);
    }
    has_buffers = !rx_buffers_.empty();
  }
  if (has_buffers) {
    parent_->OnRxAvail(this);
  }
}

void DeviceAdapter::PrepareVmo(
    fuchsia_hardware_network_driver::wire::NetworkDeviceImplPrepareVmoRequest* request,
    fdf::Arena& arena, PrepareVmoCompleter::Sync& completer) {
  zx_status_t status = vmos_.RegisterVmo(request->id, std::move(request->vmo));
  completer.buffer(arena).Reply(status);
}

void DeviceAdapter::ReleaseVmo(
    fuchsia_hardware_network_driver::wire::NetworkDeviceImplReleaseVmoRequest* request,
    fdf::Arena& arena, ReleaseVmoCompleter::Sync& completer) {
  zx_status_t status = vmos_.UnregisterVmo(request->id);
  if (status != ZX_OK) {
    FX_PLOGST(ERROR, "tun", status) << "DeviceAdapter failed to unregister vmo";
  }
  completer.buffer(arena).Reply();
}

bool DeviceAdapter::TryGetTxBuffer(fit::callback<zx_status_t(TxBuffer&, size_t)> callback) {
  uint32_t id;

  fbl::AutoLock lock(&tx_lock_);
  if (tx_buffers_.empty()) {
    return false;
  }
  auto& buff = tx_buffers_.front();
  auto avail = tx_buffers_.size() - 1;
  zx_status_t status = callback(buff, avail);
  id = buff.id();
  tx_buffers_.pop();

  EnqueueTx(id, status);
  CommitTx();
  return true;
}

void DeviceAdapter::RetainTxBuffers(fit::function<zx_status_t(TxBuffer&)> func) {
  fbl::AutoLock lock(&tx_lock_);
  for (size_t size = tx_buffers_.size(); size > 0; size--) {
    TxBuffer& buffer = tx_buffers_.front();
    zx_status_t status = func(buffer);
    if (status == ZX_OK) {
      tx_buffers_.push(std::move(buffer));
    } else {
      EnqueueTx(buffer.id(), status);
    }
    tx_buffers_.pop();
  }
  CommitTx();
}

zx::result<size_t> DeviceAdapter::WriteRxFrame(
    PortAdapter& port, fuchsia_hardware_network::wire::FrameType frame_type, const uint8_t* data,
    size_t count, const std::optional<fuchsia_net_tun::wire::FrameMetadata>& meta) {
  if (!port.online()) {
    return zx::error(ZX_ERR_BAD_STATE);
  }
  if (count > port.mtu()) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  fbl::AutoLock lock(&rx_lock_);
  if (rx_buffers_.empty()) {
    RequestRxSpace();
    return zx::error(ZX_ERR_SHOULD_WAIT);
  }
  zx::result alloc = AllocRxSpace(count);
  if (alloc.is_error()) {
    return alloc.take_error();
  }
  RxBuffer buffer = std::move(alloc.value());
  if (zx_status_t status = buffer.Write(data, count); status != ZX_OK) {
    ReclaimRxSpace(std::move(buffer));
    return zx::error(status);
  }
  EnqueueRx(port.id(), frame_type, std::move(buffer), count, meta);
  CommitRx();

  return zx::ok(rx_buffers_.size());
}

zx::result<size_t> DeviceAdapter::WriteRxFrame(
    PortAdapter& port, fuchsia_hardware_network::wire::FrameType frame_type,
    const fidl::VectorView<uint8_t>& data,
    const std::optional<fuchsia_net_tun::wire::FrameMetadata>& meta) {
  return WriteRxFrame(port, frame_type, data.data(), data.size(), meta);
}

zx::result<size_t> DeviceAdapter::WriteRxFrame(
    PortAdapter& port, fuchsia_hardware_network::wire::FrameType frame_type,
    const std::vector<uint8_t>& data,
    const std::optional<fuchsia_net_tun::wire::FrameMetadata>& meta) {
  return WriteRxFrame(port, frame_type, data.data(), data.size(), meta);
}

void DeviceAdapter::CopyTo(DeviceAdapter* other, bool return_failed_buffers) {
  fbl::AutoLock tx_lock(&tx_lock_);
  fbl::AutoLock rx_lock(&other->rx_lock_);

  while (!tx_buffers_.empty()) {
    TxBuffer& tx_buff = tx_buffers_.front();
    zx::result alloc_rx = other->AllocRxSpace(tx_buff.length());
    if (alloc_rx.is_error()) {
      if (!return_failed_buffers) {
        // stop once we run out of rx buffers to copy to
        FX_LOGST(DEBUG, "tun") << "DeviceAdapter::CopyTo: no more rx buffers";
        break;
      }
      EnqueueTx(tx_buff.id(), ZX_ERR_NO_RESOURCES);
      tx_buffers_.pop();
      continue;
    }
    RxBuffer rx_buff = std::move(alloc_rx.value());
    zx::result status = rx_buff.CopyFrom(tx_buff);
    if (status.is_error()) {
      FX_PLOGST(ERROR, "tun", status.status_value())
          << "DeviceAdapter::CopyTo: Failed to copy buffer";
      EnqueueTx(tx_buff.id(), status.status_value());
      other->ReclaimRxSpace(std::move(rx_buff));
    } else {
      size_t length = status.value();
      // Enqueue the data to be returned in other, and enqueue the complete tx in self.
      std::optional meta = tx_buff.TakeMetadata();
      if (meta.has_value()) {
        meta->flags = 0;
      }
      other->EnqueueRx(tx_buff.port_id(), tx_buff.frame_type(), std::move(rx_buff), length, meta);
      EnqueueTx(tx_buff.id(), ZX_OK);
    }
    tx_buffers_.pop();
  }
  CommitTx();
  other->CommitRx();
}

void DeviceAdapter::Teardown(fit::function<void()> callback) {
  // `device_` may be dropped in DropNetworkDeviceInterfaceForTests().
  if (!device_) {
    callback();
    return;
  }
  device_->Teardown([cb = std::move(callback)]() mutable { cb(); });
}

void DeviceAdapter::TeardownSync() {
  sync_completion_t completion;
  Teardown([&completion]() { sync_completion_signal(&completion); });
  sync_completion_wait_deadline(&completion, ZX_TIME_INFINITE);
}

void DeviceAdapter::EnqueueRx(uint8_t port_id, fuchsia_hardware_network::wire::FrameType frame_type,
                              RxBuffer buffer, size_t length,
                              const std::optional<fuchsia_net_tun::wire::FrameMetadata>& meta)
    __TA_REQUIRES(rx_lock_) {
  // Written length must always fit the buffer.
  ZX_DEBUG_ASSERT(buffer.length() >= length);
  size_t old_rx_parts_count = return_rx_parts_count_;
  buffer.WithReturn(length,
                    [this](const fuchsia_hardware_network_driver::wire::RxBufferPart& part) {
                      // WithReturn is called inline.
                      []() __TA_ASSERT(rx_lock_) {}();

                      // We should not be producing zero-length parts.
                      ZX_DEBUG_ASSERT(part.length != 0);
                      // Can't accumulate more parts than can fit in our array.
                      ZX_ASSERT(return_rx_parts_count_ <= return_rx_parts_.size());
                      return_rx_parts_[return_rx_parts_count_++] = part;
                    });
  auto& ret = return_rx_list_.emplace_back(fuchsia_hardware_network_driver::wire::RxBuffer{
      .meta =
          {
              .port = port_id,
              .frame_type = frame_type,
          },
      .data = fidl::VectorView<fuchsia_hardware_network_driver::wire::RxBufferPart>::FromExternal(
          &return_rx_parts_[old_rx_parts_count], return_rx_parts_count_ - old_rx_parts_count),
  });
  if (meta) {
    ret.meta.flags = meta->flags;
  }
  if (port_rx_checksum_offload_[port_id]) {
    ret.meta.flags |=
        static_cast<uint32_t>(fuchsia_hardware_network::wire::RxFlags::kFullChecksumsVerified);
    ret.full_csums_verified = 0;  // Number of verified checksums minus one.
  }
}

void DeviceAdapter::CommitRx() {
  if (return_rx_list_.empty()) {
    return;
  }

  fdf::Arena arena(0u);
  fidl::OneWayStatus status = device_iface_.buffer(arena)->CompleteRx(
      fidl::VectorView<fuchsia_hardware_network_driver::wire::RxBuffer>::FromExternal(
          return_rx_list_));
  if (!status.ok()) {
    FX_PLOGST(ERROR, "tun", status.status()) << "failed to return rx buffers";
    parent_->RequestErrorUnbind();
    return;
  }
  return_rx_list_.clear();
  return_rx_parts_count_ = 0;
}

void DeviceAdapter::EnqueueTx(uint32_t id, zx_status_t status) {
  auto& tx = return_tx_list_.emplace_back();
  tx.id = id;
  tx.status = status;
}

void DeviceAdapter::CommitTx() {
  if (return_tx_list_.empty()) {
    return;
  }
  fdf::Arena arena(0u);
  fidl::OneWayStatus status = device_iface_.buffer(arena)->CompleteTx(
      fidl::VectorView<fuchsia_hardware_network_driver::wire::TxResult>::FromExternal(
          return_tx_list_));
  if (!status.ok()) {
    FX_PLOGST(ERROR, "tun", status.status()) << "failed to return tx buffers";
    parent_->RequestErrorUnbind();
    return;
  }
  return_tx_list_.clear();
}

DeviceAdapter::DeviceAdapter(DeviceAdapterParent* parent,
                             const DeviceInterfaceDispatchers& dispatchers,
                             fdf::UnownedUnsynchronizedDispatcher&& netdev_dispatcher)
    : parent_(parent), dispatchers_(dispatchers), netdev_dispatcher_(std::move(netdev_dispatcher)) {
  for (std::atomic_bool& p : port_online_status_) {
    p = false;
  }
  for (std::atomic_bool& p : port_rx_checksum_offload_) {
    p = false;
  }
}

zx::result<RxBuffer> DeviceAdapter::AllocRxSpace(size_t length) __TA_REQUIRES(rx_lock_) {
  RxBuffer buffer = vmos_.MakeEmptyRxBuffer();
  size_t parts_added = 0;
  while (!rx_buffers_.empty() &&
         parts_added < fuchsia_hardware_network_driver::wire::kMaxBufferParts) {
    const auto& space = rx_buffers_.front();
    buffer.PushRxSpace(space);
    parts_added++;
    uint64_t space_length = space.region.length;
    rx_buffers_.pop();
    if (space_length >= length) {
      return zx::ok(std::move(buffer));
    }
    length -= space_length;
  }
  // The available rx buffers weren't sufficient to allocate the required space,
  // so we need to reclaim the space from the buffer.
  ReclaimRxSpace(std::move(buffer));
  if (rx_buffers_.empty()) {
    RequestRxSpace();
  }
  return zx::error(ZX_ERR_SHOULD_WAIT);
}

void DeviceAdapter::ReclaimRxSpace(RxBuffer buffer) __TA_REQUIRES(rx_lock_) {
  buffer.WithSpace([this](const fuchsia_hardware_network_driver::wire::RxSpaceBuffer& space) {
    // WithSpace is called inline.
    []() __TA_ASSERT(rx_lock_) {}();

    rx_buffers_.push(space);
  });
}

void DeviceAdapter::OnPortStatusChanged(uint8_t port_id, const PortStatus& new_status) {
  port_online_status_[port_id] = new_status.online;

  fdf::Arena arena(0u);
  auto wire = fuchsia_hardware_network::wire::PortStatus::Builder(arena);
  new_status.AddToBuilder(wire);
  fidl::OneWayStatus status = device_iface_.buffer(arena)->PortStatusChanged(port_id, wire.Build());
  if (!status.ok()) {
    FX_PLOGST(ERROR, "tun", status.status()) << "failed to report online status change";
    parent_->RequestErrorUnbind();
  }
}

zx_status_t DeviceAdapter::AddPort(PortAdapter& port) {
  port_rx_checksum_offload_[port.id()] = port.rx_checksum_offload();
  libsync::Completion completion;
  zx_status_t status;
  fdf::Arena arena(0u);
  device_iface_.buffer(arena)
      ->AddPort(port.id(), port.BindDriver())
      .ThenExactlyOnce(
          [&completion, &status](
              fdf::WireUnownedResult<fuchsia_hardware_network_driver::NetworkDeviceIfc::AddPort>&
                  result) {
            if (!result.ok()) {
              status = result.status();
            } else {
              status = result->status;
            }
            completion.Signal();
          });
  completion.Wait();
  return status;
}

void DeviceAdapter::RemovePort(uint8_t port_id) {
  port_rx_checksum_offload_[port_id] = false;
  fdf::Arena arena(0u);
  fidl::OneWayStatus status = device_iface_.buffer(arena)->RemovePort(port_id);
  if (!status.ok()) {
    FX_PLOGST(ERROR, "tun", status.status()) << "failed to remove port " << port_id;
  }
}

zx_status_t DeviceAdapter::DelegateRxLease(fuchsia_hardware_network::wire::DelegatedRxLease lease) {
  fdf::Arena arena(0u);
  fidl::OneWayStatus status = device_iface_.buffer(arena)->DelegateRxLease(lease);
  return status.status();
}

void DeviceAdapter::RequestRxSpace() {
  if (rx_space_requested_) {
    return;
  }
  rx_space_requested_ = true;

  fdf::Arena arena(0u);
  // We attempt to request the entire Rx FIFO to be filled because we don't
  // want buffer starvation. The memory can be reclaimed back slowly.
  fidl::OneWayStatus status = device_iface_.buffer(arena)->RequestRxSpace(kFifoDepth);
  if (!status.ok()) {
    FX_PLOGST(ERROR, "tun", status.status()) << "failed to request rx space";
  }
}

void DeviceAdapter::DropNetworkDeviceInterfaceForTests() {
  TeardownSync();
  device_.reset();
}

}  // namespace tun
}  // namespace network
