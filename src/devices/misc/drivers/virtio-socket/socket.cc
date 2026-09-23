// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "socket.h"

#include <assert.h>
#include <fidl/fuchsia.hardware.vsock/cpp/wire.h>
#include <lib/async/cpp/task.h>
#include <lib/fit/defer.h>
#include <lib/stdcompat/functional.h>
#include <limits.h>
#include <stddef.h>
#include <stdint.h>
#include <string.h>
#include <zircon/assert.h>
#include <zircon/status.h>
#include <zircon/types.h>

#include <algorithm>
#include <memory>
#include <mutex>

#include <fbl/algorithm.h>
#include <fbl/array.h>
#include <virtio/virtio.h>

namespace virtio {

static constexpr uint16_t kDataBacklog = 32;

static constexpr uint16_t kEventBacklog = 4;

static constexpr size_t kFrameSize = sizeof(virtio_vsock_hdr_t) + 468;

static constexpr uint16_t kRxId = 0u;
static constexpr uint16_t kTxId = 1u;
static constexpr uint16_t kEventId = 2u;

static virtio_vsock_hdr_t make_hdr(const SocketDevice::ConnectionKey& key, uint16_t op,
                                   uint32_t cid, const SocketDevice::CreditInfo& credit) {
  return virtio_vsock_hdr_t{
      .src_cid = cid,
      .dst_cid = key.addr.remote_cid,
      .src_port = key.addr.local_port,
      .dst_port = key.addr.remote_port,
      .len = 0,
      .type = 1,
      .op = op,
      .flags = op == VIRTIO_VSOCK_OP_SHUTDOWN ? 3u : 0u,
      .buf_alloc = credit.buf_alloc,
      .fwd_cnt = credit.fwd_count,
  };
}

SocketDevice::SocketDevice(zx::bti bti, std::unique_ptr<Backend> backend)
    : virtio::Device(std::move(bti), std::move(backend)),
      rx_(this, kDataBacklog, kFrameSize),
      tx_(this, kDataBacklog, kFrameSize),
      event_(this, kEventBacklog, sizeof(virtio_vsock_event_t)),
      timer_wait_handler_(this),
      callback_closed_handler_(this) {}

SocketDevice::~SocketDevice() {}

void SocketDevice::Start(::fuchsia_hardware_vsock::wire::DeviceStartRequest* request,
                         StartCompleter::Sync& completer) {
  std::lock_guard lock(lock_);
  if (callbacks_.is_valid()) {
    RemoveCallbacksLocked();
  }
  callbacks_.Bind(std::move(request->cb));
  callback_closed_handler_.set_object(callbacks_.client_end().borrow().channel()->get());
  callback_closed_handler_.set_trigger(ZX_SOCKET_PEER_CLOSED);
  callback_closed_handler_.Begin(dispatcher_);

  // Go and process the rings to handle any pending rx descriptors and start
  // queueing new ones.
  UpdateRxRingLocked();

  completer.ReplySuccess();
}

void SocketDevice::SendRst(::fuchsia_hardware_vsock::wire::DeviceSendRstRequest* request,
                           SendRstCompleter::Sync& completer) {
  std::lock_guard lock(lock_);
  CleanupConAndRstLocked(ConnectionKey(request->addr));
  completer.ReplySuccess();
}

void SocketDevice::SendShutdown(::fuchsia_hardware_vsock::wire::DeviceSendShutdownRequest* request,
                                SendShutdownCompleter::Sync& completer) {
  std::lock_guard lock(lock_);
  if (!callbacks_.is_valid()) {
    completer.ReplyError(ZX_ERR_BAD_STATE);
    return;
  }

  auto conn = connections_.find(ConnectionKey(request->addr));
  if (conn == connections_.end() || conn->IsShuttingDown()) {
    completer.ReplyError(ZX_ERR_BAD_STATE);
    return;
  }

  if (conn->BeginShutdown()) {
    SendOpLocked(conn.CopyPointer(), VIRTIO_VSOCK_OP_SHUTDOWN);
  }
  completer.ReplySuccess();
}

void SocketDevice::SendRequest(SendRequestRequestView request,
                               SendRequestCompleter::Sync& completer) {
  std::lock_guard lock(lock_);
  if (!callbacks_.is_valid()) {
    completer.ReplyError(ZX_ERR_BAD_STATE);
    return;
  }

  if (connections_.find(ConnectionKey(request->addr)) != connections_.end()) {
    completer.ReplyError(ZX_ERR_ALREADY_BOUND);
    return;
  }
  auto conn = fbl::MakeRefCounted<Connection>(
      ConnectionKey(request->addr), std::move(request->data),
      cpp20::bind_front(&SocketDevice::ConnectionSocketSignalled, this), cid_, lock_);
  connections_.insert(conn);
  SendOpLocked(conn, VIRTIO_VSOCK_OP_REQUEST);
  completer.ReplySuccess();
}

void SocketDevice::SendResponse(::fuchsia_hardware_vsock::wire::DeviceSendResponseRequest* request,
                                SendResponseCompleter::Sync& completer) {
  std::lock_guard lock(lock_);
  if (!callbacks_.is_valid()) {
    completer.ReplyError(ZX_ERR_BAD_STATE);
    return;
  }

  if (connections_.find(ConnectionKey(request->addr)) != connections_.end()) {
    completer.ReplyError(ZX_ERR_ALREADY_BOUND);
    return;
  }
  auto conn = fbl::MakeRefCounted<Connection>(
      ConnectionKey(request->addr), std::move(request->data),
      cpp20::bind_front(&SocketDevice::ConnectionSocketSignalled, this), cid_, lock_);

  conn->MakeActive(dispatcher_);

  connections_.insert(conn);
  SendOpLocked(conn, VIRTIO_VSOCK_OP_RESPONSE);
  completer.ReplySuccess();
}

void SocketDevice::GetCid(GetCidCompleter::Sync& completer) {
  std::lock_guard lock(lock_);
  completer.Reply(cid_);
}

zx_status_t SocketDevice::Init() {
  std::lock_guard lock(lock_);
  dispatcher_ = fdf::Dispatcher::GetCurrent()->async_dispatcher();
  ZX_DEBUG_ASSERT(dispatcher_);

  // It's a common part for all virtio devices: reset the device, notify
  // about the driver and negotiate supported features.
  DeviceReset();
  DriverStatusAck();
  if (!(DeviceFeaturesSupported() & VIRTIO_F_VERSION_1)) {
    fdf::error("{}: Legacy virtio interface is not supported by this driver", tag());
    return ZX_ERR_NOT_SUPPORTED;
  }
  DriverFeaturesAck(VIRTIO_F_VERSION_1);
  if (zx_status_t status = DeviceStatusFeaturesOk(); status != ZX_OK) {
    fdf::error("Feature negotiation failed: {}", zx_status_get_string(status));
    return status;
  }

  // Plan to clean up unless everything goes right.
  auto cleanup = fit::defer([this]() TA_NO_THREAD_SAFETY_ANALYSIS { ReleaseLocked(); });

  UpdateCidLocked();

  zx_status_t rc;
  rc = event_.Init(kEventId, bti());
  if (rc != ZX_OK) {
    fdf::error("{}: Failed to allocate event ring: {}", tag(), zx_status_get_string(rc));
    return rc;
  }
  rc = rx_.Init(kRxId, bti());
  if (rc != ZX_OK) {
    fdf::error("{}: Failed to allocate rx ring: {}", tag(), zx_status_get_string(rc));
    return rc;
  }
  rc = tx_.Init(kTxId, bti());
  if (rc != ZX_OK) {
    fdf::error("{}: Failed to allocate tx ring: {}", tag(), zx_status_get_string(rc));
    return rc;
  }
  // Determine our bti contiguity.
  zx_info_bti_t bti_info;
  rc = bti_.get_info(ZX_INFO_BTI, &bti_info, sizeof(bti_info), nullptr, nullptr);
  if (rc != ZX_OK) {
    fdf::error("{}: Failed to determine BTI contiguity", tag());
    return rc;
  }
  bti_contiguity_ = bti_info.minimum_contiguity;

  // Start the interrupt thread and set the driver OK status
  StartIrqThread();

  // Setup our timer for retrying TX operations.
  rc = zx::timer::create(ZX_TIMER_SLACK_CENTER, ZX_CLOCK_MONOTONIC, &tx_retry_timer_);
  if (rc != ZX_OK) {
    fdf::error("{}: Failed to create timer: {}", tag(), zx_status_get_string(rc));
    return rc;
  }
  timer_wait_handler_.set_object(tx_retry_timer_.get());
  timer_wait_handler_.set_trigger(ZX_TIMER_SIGNALED);

  event_.RefillRing();

  cleanup.cancel();
  DriverStatusOk();
  return ZX_OK;
}

void SocketDevice::IrqRingUpdate() {
  std::lock_guard lock(lock_);
  tx_.ProcessDescriptors([this](const ConnectionKey& key, uint64_t payload)
                             TA_NO_THREAD_SAFETY_ANALYSIS {
                               auto conn = connections_.find(key);
                               if (conn != connections_.end()) {
                                 conn->NotifyVmoTxComplete(payload);
                               }
                             });
  event_.ProcessDescriptors<virtio_vsock_event_t>(
      [this](virtio_vsock_event_t* event, void* data, uint32_t data_len)
          TA_NO_THREAD_SAFETY_ANALYSIS {
            if (event->id == VIRTIO_VSOCK_EVENT_TRANSPORT_RESET) {
              TransportResetLocked();
            } else {
              fdf::error("{}: Received unknown event: {}", tag(), event->id);
            }
          });

  UpdateRxRingLocked();

  // Send any queued ops in any freed tx descriptors first, in preference to
  // any queued data transfers.
  while (!has_pending_op_.is_empty()) {
    auto conn = has_pending_op_.pop_front();
    uint16_t op = conn->TakePendingOp();
    if (!SendOp_RawLocked(ConnectionKey(conn->GetKey().addr), op, conn->GetCreditInfo())) {
      conn->QueueOp(op);
      has_pending_op_.push_front(conn);
      break;
    }
  }
  RetryTxLocked(false);
}

void SocketDevice::IrqConfigChange() {
  std::lock_guard lock(lock_);
  uint32_t old_cid = cid_;
  UpdateCidLocked();
  if (cid_ != old_cid) {
    TransportResetLocked();
  }
}

void SocketDevice::ProcessRxDescriptor(virtio_vsock_hdr_t* header, void* data, uint32_t data_len) {
  if (header->dst_cid != cid_) {
    fdf::error("{}: Received message for cid {}, but believe our cid is {}", tag(),
               static_cast<uint32_t>(header->dst_cid), cid_);
    return;
  }

  ConnectionKey key = ConnectionKey::FromHdr(header);
  auto conn = connections_.find(key);
  if (conn != connections_.end()) {
    conn->UpdateCredit(header->buf_alloc, header->fwd_cnt);
  }

  if (header->op == VIRTIO_VSOCK_OP_RW) {
    if (conn == connections_.end()) {
      SendRstLocked(key);
    } else {
      if (conn->Rx(data, data_len)) {
        conn->UpdateTxThreshold();
      } else {
        NotifyAndCleanupConLocked(conn.CopyPointer());
      }
    }
  } else {
    RxOpLocked(conn, key, header->op);
  }
}

void SocketDevice::UpdateRxRingLocked() {
  // Refuse to process rx buffers if we don't have callbacks. If the callbacks
  // somehow vanish mid process then that's fine, we'll just dump a lot of
  // requests on the floor, but there's little else we can do.
  if (!callbacks_.is_valid()) {
    return;
  }
  rx_.ProcessDescriptors<virtio_vsock_hdr_t>(
      [this](virtio_vsock_hdr_t* header, void* data, uint32_t data_len)
          TA_NO_THREAD_SAFETY_ANALYSIS { this->ProcessRxDescriptor(header, data, data_len); });
}

void SocketDevice::RxOpLocked(ConnectionIterator conn, const ConnectionKey& key, uint16_t op) {
  switch (op) {
    case VIRTIO_VSOCK_OP_INVALID:
      fdf::error("{}: Received invalid op", tag());
      break;
    case VIRTIO_VSOCK_OP_REQUEST:
      // Don't care if we have a connection or not, just send it to the
      // service.
      if (callbacks_.is_valid()) {
        // TODO(https://fxbug.dev/42180237) Consider handling the error instead of ignoring it.
        [[maybe_unused]] auto _ = callbacks_->Request(key.addr);
      }
      break;
    case VIRTIO_VSOCK_OP_RESPONSE: {
      // Check for existing partial connection.
      if (conn == connections_.end()) {
        fdf::error("{}: Received response for unknown connection", tag());
        // We weren't trying to make a connection, so reject this
        SendRstLocked(key);
        break;
      }
      // Upgrade the channel.
      conn->MakeActive(dispatcher_);
      if (callbacks_.is_valid()) {
        // TODO(https://fxbug.dev/42180237) Consider handling the error instead of ignoring it.
        [[maybe_unused]] auto _ = callbacks_->Response(key.addr);
      }
      break;
    }
    case VIRTIO_VSOCK_OP_RST:
      if (conn != connections_.end()) {
        CleanupConLocked(conn.CopyPointer());
      }
      if (callbacks_.is_valid()) {
        // TODO(https://fxbug.dev/42180237) Consider handling the error instead of ignoring it.
        [[maybe_unused]] auto _ = callbacks_->Rst(key.addr);
      }
      break;
    case VIRTIO_VSOCK_OP_SHUTDOWN:
      if (conn != connections_.end()) {
        // Shutdown and move into the zombie state until the service
        // confirms shutdown by sending the RST
        conn->Close(dispatcher_);
        DequeueTxLocked(conn.CopyPointer());
        DequeueOpLocked(conn.CopyPointer());
      }
      if (callbacks_.is_valid()) {
        // TODO(https://fxbug.dev/42180237) Consider handling the error instead of ignoring it.
        [[maybe_unused]] auto _ = callbacks_->Shutdown(key.addr);
      }
      break;
    case VIRTIO_VSOCK_OP_CREDIT_UPDATE:
      if (conn == connections_.end()) {
        SendRstLocked(key);
      }
      if (QueuedForTxLocked(conn.CopyPointer())) {
        ContinueTxLocked(true, conn.CopyPointer());
      }
      break;
    case VIRTIO_VSOCK_OP_CREDIT_REQUEST:
      if (conn == connections_.end()) {
        SendRstLocked(key);
      } else {
        SendOpLocked(conn.CopyPointer(), VIRTIO_VSOCK_OP_CREDIT_UPDATE);
      }
      break;
    case VIRTIO_VSOCK_OP_RW:
      // This case should've gone to RxOp_Data
      fdf::error("{}: OP_RW not handled here", tag());
      break;
    default:
      fdf::error("{}: Unexpected op {} from host", tag(), op);
      break;
  }
}

bool SocketDevice::SendOp_RawLocked(const ConnectionKey& key, uint16_t op,
                                    const CreditInfo& credit) {
  virtio_vsock_hdr_t hdr = make_hdr(key, op, cid_, credit);
  // Grab a free descriptor
  uint16_t id;
  if (!tx_.AllocInPlace(&id)) {
    return false;
  }
  tx_.SetHeader(id, hdr);
  tx_.SubmitChain(id, 0);
  // Typically we call this in a path with a single TX, so minimal gains from
  // trying to coalesce this
  tx_.Kick();
  return true;
}

void SocketDevice::SendOpLocked(fbl::RefPtr<Connection> conn, uint16_t op,
                                std::optional<CreditInfo> known_credit) {
  // If there's a queue then keep queueing.
  if (!has_pending_op_.is_empty()) {
    // Store the op, updating if already queued.
    conn->QueueOp(op);
    QueueForOpLocked(conn);
    return;
  }
  CreditInfo credit = known_credit.value_or(conn->GetCreditInfo());
  if (!SendOp_RawLocked(conn->GetKey(), op, credit)) {
    conn->QueueOp(op);
    QueueForOpLocked(conn);
  }
}

void SocketDevice::RetryTxLocked(bool force_credit_request) {
  for (auto it = has_pending_tx_.begin(); it != has_pending_tx_.end();) {
    auto prev = it;
    it++;
    ContinueTxLocked(force_credit_request, prev.CopyPointer());
  }
}

void SocketDevice::ContinueTxLocked(bool force_credit_request, fbl::RefPtr<Connection> conn) {
  zx_status_t status = conn->ContinueTx(force_credit_request, tx_, dispatcher_);
  if (status == ZX_OK || status == ZX_ERR_SHOULD_WAIT) {
    if (conn->HasPendingOp() && !QueuedForOpLocked(conn)) {
      SendOpLocked(conn, conn->TakePendingOp());
    }
    if (status == ZX_ERR_SHOULD_WAIT) {
      QueueForTxLocked(conn);
    } else if (status == ZX_OK) {
      DequeueTxLocked(conn);
    }
  } else {
    NotifyAndCleanupConLocked(conn);
  }
}

void SocketDevice::SendRstLocked(const ConnectionKey& key) {
  SendOp_RawLocked(key, VIRTIO_VSOCK_OP_RST, CreditInfo());
}

void SocketDevice::CleanupConLocked(fbl::RefPtr<Connection> conn) {
  conn->Close(dispatcher_);
  DequeueTxLocked(conn);
  DequeueOpLocked(conn);
  connections_.erase(*conn);
}

void SocketDevice::NotifyAndCleanupConLocked(fbl::RefPtr<Connection> conn) {
  if (callbacks_.is_valid()) {
    // TODO(https://fxbug.dev/42180237) Consider handling the error instead of ignoring it.
    [[maybe_unused]] auto _ = callbacks_->Rst(conn->GetKey().addr);
  }
  SendOpLocked(conn, VIRTIO_VSOCK_OP_RST);
  CleanupConLocked(conn);
}

void SocketDevice::CleanupConAndRstLocked(const ConnectionKey& key) {
  auto it = connections_.find(key);
  if (it != connections_.end()) {
    SendOpLocked(it.CopyPointer(), VIRTIO_VSOCK_OP_RST);
    CleanupConLocked(it.CopyPointer());
  } else {
    SendRstLocked(key);
  }
}

void SocketDevice::RemoveCallbacksLocked() {
  for (auto it = connections_.begin(); it != connections_.end(); it++) {
    SendOpLocked(it.CopyPointer(), VIRTIO_VSOCK_OP_RST);
    it->Close(dispatcher_);
  }
  connections_.clear();
  callback_closed_handler_.Cancel();
  callbacks_ = {};
  has_pending_tx_.clear();
  // We don't clear pending ops as we need our RST ops to finish sending.
}

bool SocketDevice::QueuedForTxLocked(fbl::RefPtr<Connection> conn) {
  return conn && fbl::InContainer<PendingTxTag>(*conn);
}

void SocketDevice::QueueForTxLocked(fbl::RefPtr<Connection> conn) {
  if (!QueuedForTxLocked(conn)) {
    has_pending_tx_.push_back(conn);
    EnableTxRetryTimerLocked();
  }
}

void SocketDevice::DequeueTxLocked(fbl::RefPtr<Connection> conn) {
  if (QueuedForTxLocked(conn)) {
    has_pending_tx_.erase(*conn);
  }
}

bool SocketDevice::QueuedForOpLocked(fbl::RefPtr<Connection> conn) {
  return fbl::InContainer<PendingOpTag>(*conn);
}

void SocketDevice::QueueForOpLocked(fbl::RefPtr<Connection> conn) {
  if (!QueuedForOpLocked(conn)) {
    has_pending_op_.push_back(conn);
  }
}

void SocketDevice::DequeueOpLocked(fbl::RefPtr<Connection> conn) {
  if (QueuedForOpLocked(conn)) {
    has_pending_op_.erase(*conn);
  }
}

void SocketDevice::EnableTxRetryTimerLocked() {
  if (!timer_wait_handler_.is_pending()) {
    zx_status_t status = tx_retry_timer_.set(zx::deadline_after(zx::sec(1)), zx::sec(1));
    if (status != ZX_OK) {
      fdf::error("{}: Failed to set timer {}", tag(), zx_status_get_string(status));
      return;
    }
    status = timer_wait_handler_.Begin(dispatcher_);
    if (status != ZX_OK) {
      fdf::error("{}: Failed to wait for timer {}", tag(), zx_status_get_string(status));
      return;
    }
  }
}

void SocketDevice::TimerWaitHandler(async_dispatcher_t* dispatcher, async::WaitBase* wait,
                                    zx_status_t status, const zx_packet_signal_t* signal) {
  if (status != ZX_OK) {
    // Dispatcher shut down.
    return;
  }
  std::lock_guard lock(lock_);
  tx_retry_timer_.cancel();
  RetryTxLocked(true);
  if (!has_pending_tx_.is_empty()) {
    EnableTxRetryTimerLocked();
  }
}

void SocketDevice::CallbacksSignalled(async_dispatcher_t* dispatcher, async::WaitBase* wait,
                                      zx_status_t status, const zx_packet_signal_t* signal) {
  if (status != ZX_OK) {
    // Dispatcher shut down.
    return;
  }
  std::lock_guard lock(lock_);
  RemoveCallbacksLocked();
}
void SocketDevice::ConnectionSocketSignalled(zx_status_t status, const zx_packet_signal_t* signal,
                                             fbl::RefPtr<Connection> conn) {
  if (status != ZX_OK) {
    // Dispatcher shut down
    return;
  }
  std::lock_guard lock(lock_);
  if (conn->IsShuttingDown()) {
    return;
  }
  if (signal->observed & ZX_SOCKET_READABLE) {
    ContinueTxLocked(false, conn);
    return;
  }
  if (signal->observed & ZX_SOCKET_PEER_CLOSED) {
    NotifyAndCleanupConLocked(conn);
    return;
  }
  if (signal->observed & ZX_SOCKET_WRITE_THRESHOLD) {
    CreditInfo credit = conn->ContinueRx(dispatcher_);
    SendOpLocked(conn, VIRTIO_VSOCK_OP_CREDIT_UPDATE, credit);
    return;
  }
}

void SocketDevice::UpdateCidLocked() {
  virtio_vsock_config_t config;
  CopyDeviceConfig(&config, sizeof(config));
  cid_ = static_cast<uint32_t>(config.guest_cid);
}

void SocketDevice::ReleaseLocked() {
  RemoveCallbacksLocked();
  has_pending_op_.clear();

  // The default dispatcher will have already been stopped before DdkRelease
  // is called, so we will have no more wait handlers for things like
  // timer_wait_handler_.
  rx_.FreeBuffers();
  tx_.FreeBuffers();
  event_.FreeBuffers();
  virtio::Device::Release();
}

void SocketDevice::TransportResetLocked() {
  // Reload the CID when receiving a reset.
  fdf::info("{}: Received transport reset!", tag());
  for (auto& it : connections_) {
    it.Close(dispatcher_);
  }
  connections_.clear();
  has_pending_tx_.clear();
  has_pending_op_.clear();
  UpdateCidLocked();
  if (callbacks_.is_valid()) {
    // TODO(https://fxbug.dev/42180237) Consider handling the error instead of ignoring it.
    [[maybe_unused]] auto _ = callbacks_->TransportReset(cid_);
  }
}

SocketDevice::IoBufferRing::IoBufferRing(virtio::Device* device, uint16_t count, uint32_t buf_size,
                                         bool host_write_only)
    : ring_(device), host_write_only_(host_write_only), count_(count), buf_size_(buf_size) {}

SocketDevice::IoBufferRing::~IoBufferRing() { FreeBuffers(); }

zx_status_t SocketDevice::IoBufferRing::Init(uint16_t index, const zx::bti& bti) {
  zx_status_t rc = ring().Init(index, count_);
  if (rc != ZX_OK) {
    return rc;
  }
  auto factory = dma_buffer::CreateBufferFactory();
  size_t size = buf_size() * count_;
  size = fbl::round_up(size, static_cast<size_t>(zx_system_get_page_size()));
  rc = factory->CreateContiguous(bti, size, 0, dma_buffer::CacheOptions::kEnabled, &io_buffer_);
  if (rc != ZX_OK) {
    return rc;
  }
  // Set the flags in all the descriptors if is host_write_only, this means the
  // device (aka host) can write to the buffers, but we as the driver may only
  // read from them.
  if (host_write_only_) {
    for (uint16_t id = 0; id < count_; id++) {
      struct vring_desc* desc = ring().DescFromIndex(id);
      desc->addr = io_buffer_->phys() + id * buf_size_;
      desc->len = buf_size_;
      desc->flags |= VRING_DESC_F_WRITE;
    }
  }
  return ZX_OK;
}

void SocketDevice::IoBufferRing::FreeBuffers() { io_buffer_.reset(); }

SocketDevice::RxIoBufferRing::RxIoBufferRing(virtio::Device* device, uint16_t count,
                                             uint32_t buf_size)
    : IoBufferRing(device, count, buf_size, true) {}

void SocketDevice::RxIoBufferRing::RefillRing() {
  assert(io_buffer());
  bool needs_kick = false;
  uint16_t id;
  struct vring_desc* desc;
  while ((desc = ring().AllocDescChain(1, &id))) {
    desc->len = buf_size();
    ring().SubmitChain(id);
    needs_kick = true;
  }
  if (needs_kick) {
    Kick();
  }
}

template <typename H, typename F>
void SocketDevice::RxIoBufferRing::ProcessDescriptors(F func) {
  ring().IrqRingUpdate([this, &func](vring_used_elem* used_elem) {
    uint16_t last_id = static_cast<uint16_t>(used_elem->id);
    struct vring_desc* desc = ring().DescFromIndex(last_id);
    if (desc->len < sizeof(H)) {
      fdf::error("Descriptor is too short");
    } else if ((desc->flags & VRING_DESC_F_NEXT) != 0) {
      fdf::error("Chained descriptors are not supported");
    } else {
      func(reinterpret_cast<H*>(GetRawDesc(last_id, sizeof(H))), GetRawDesc(last_id, 0, sizeof(H)),
           static_cast<uint32_t>(used_elem->len - sizeof(H)));
    }
    // Handle freeing arbitrarily long descriptor chains
    while ((desc->flags & VRING_DESC_F_NEXT) != 0) {
      uint16_t next_id = desc->next;
      ring().FreeDesc(last_id);
      desc = ring().DescFromIndex(last_id);
      last_id = next_id;
    }
    ring().FreeDesc(last_id);
  });
  RefillRing();
}

SocketDevice::TxIoBufferRing::TxIoBufferRing(virtio::Device* device, uint16_t count,
                                             uint32_t buf_size)
    : IoBufferRing(device, count, buf_size, false) {}

void* SocketDevice::TxIoBufferRing::AllocInPlace(uint16_t* id) {
  struct vring_desc* desc = ring().AllocDescChain(1, id);
  if (desc) {
    desc->addr = io_buffer()->phys() + *id * buf_size();
    return GetRawDesc(*id, 0, sizeof(virtio_vsock_hdr_t));
  }
  return nullptr;
}

bool SocketDevice::TxIoBufferRing::AllocIndirect(const ConnectionKey& key, uint16_t* id) {
  struct vring_desc* desc = ring().AllocDescChain(2, id);
  if (!desc) {
    return false;
  }
  desc->addr = io_buffer()->phys() + *id * buf_size();
  *reinterpret_cast<ConnectionKey*>(
      GetRawDesc(*id, sizeof(ConnectionKey), sizeof(virtio_vsock_hdr_t))) = key;
  return true;
}

void SocketDevice::TxIoBufferRing::SetIndirectPayload(uint16_t id, uintptr_t payload) {
  struct vring_desc* desc = ring().DescFromIndex(ring().DescFromIndex(id)->next);
  desc->addr = payload;
}

void SocketDevice::TxIoBufferRing::SubmitChain(uint16_t id, uint32_t data_len) {
  struct vring_desc* desc = ring().DescFromIndex(id);
  desc->len = sizeof(virtio_vsock_hdr_t);
  if ((desc->flags & VRING_DESC_F_NEXT) == 0) {
    desc->len += data_len;
  } else {
    desc = ring().DescFromIndex(desc->next);
    desc->len = data_len;
  }
  ring().SubmitChain(id);
}

void SocketDevice::TxIoBufferRing::FreeChain(uint16_t id) {
  struct vring_desc* desc = ring().DescFromIndex(id);
  if ((desc->flags & VRING_DESC_F_NEXT) != 0) {
    ring().FreeDesc(desc->next);
  }
  ring().FreeDesc(id);
}

template <typename F>
void SocketDevice::TxIoBufferRing::ProcessDescriptors(F func) {
  ring().IrqRingUpdate([this, &func](vring_used_elem* used_elem) {
    uint16_t id = static_cast<uint16_t>(used_elem->id);
    struct vring_desc* desc = ring().DescFromIndex(id);
    if ((desc->flags & VRING_DESC_F_NEXT) != 0) {
      struct vring_desc* desc2 = ring().DescFromIndex(desc->next);
      ConnectionKey* key = reinterpret_cast<ConnectionKey*>(
          GetRawDesc(id, sizeof(ConnectionKey), sizeof(virtio_vsock_hdr_t)));
      func(*key, desc2->addr);
      ring().FreeDesc(desc->next);
    }
    ring().FreeDesc(id);
  });
}

SocketDevice::Connection::Connection(const ConnectionKey& key, zx::socket data,
                                     SignalHandler wait_handler, uint32_t cid, std::mutex& lock)
    : lock_(lock),
      key_(key),
      state_(Connection::State::WAIT_RESPONSE),
      tx_count_(0),
      rx_count_(0),
      buf_alloc_(0),
      fwd_cnt_(0),
      data_(std::move(data)),
      wait_handler_(data_.get(),
                    ZX_SOCKET_READABLE | ZX_SOCKET_PEER_CLOSED | ZX_SOCKET_WRITE_THRESHOLD, 0,
                    async::Wait::Handler([this, wait_handler = std::move(wait_handler)](
                                             async_dispatcher_t* dispatcher, async::Wait* wait,
                                             zx_status_t status, const zx_packet_signal_t* signal) {
                      fbl::RefPtr<Connection> ref;
                      wait_handler_ref_.swap(ref);
                      wait_handler(status, signal, ref);
                    })),
      pending_vmo_(false),
      has_pending_op_(false),
      cid_(cid) {}

bool SocketDevice::Connection::PendingTx() {
  if (pending_vmo_) {
    return true;
  }
  return SocketTxPending();
}

bool SocketDevice::Connection::IsShuttingDown() {
  return state_ == Connection::State::ZOMBIE || state_ == Connection::State::SHUTTING_DOWN ||
         state_ == Connection::State::WILL_SHUT_DOWN;
}

bool SocketDevice::Connection::BeginShutdown() {
  assert(!IsShuttingDown());
  if (PendingTx()) {
    state_ = Connection::State::WILL_SHUT_DOWN;
    return false;
  }
  state_ = Connection::State::SHUTTING_DOWN;
  return true;
}

bool SocketDevice::Connection::NotifyVmoTxComplete(uintptr_t paddr) {
  if (pending_vmo_ && vmo_.final_paddr_ == paddr) {
    vmo_.Release();
    pending_vmo_ = false;
    return true;
  }
  return false;
}

void SocketDevice::Connection::UpdateCredit(uint32_t buf, uint32_t fwd) {
  buf_alloc_ = buf;
  fwd_cnt_ = fwd;
}

void SocketDevice::Connection::MakeActive(async_dispatcher_t* disp) {
  if (state_ != Connection::State::WAIT_RESPONSE) {
    fdf::error("Received response for already established connection");
    return;
  }
  BeginWait(disp);
  state_ = Connection::State::ACTIVE;
}

bool SocketDevice::Connection::Rx(void* data, size_t len) {
  size_t written = 0;
  zx_status_t status = data_.write(0, data, len, &written);
  rx_count_ += static_cast<uint32_t>(written);
  // The way flow control works in vsock we should never end up in a
  // situation where the socket cannot hold the data. Therefore we consider
  // any failure to be catastrophic and terminate the connection.
  return status == ZX_OK && written == len;
}

void SocketDevice::Connection::UpdateTxThreshold() {
  CreditInfo info = GetCreditInfo();
  if (info.in_socket != 0) {
    SetSocketTxThreshold(info.buf_alloc);
  }
}

SocketDevice::CreditInfo SocketDevice::Connection::ContinueRx(async_dispatcher_t* disp) {
  CreditInfo info = GetCreditInfo();
  if (info.in_socket == 0) {
    SetSocketTxThreshold(0);
  }
  BeginWait(disp);
  return info;
}

void SocketDevice::Connection::SetSocketTxThreshold(uint32_t threshold) {
  // Note: A value of 0 here effectively disables threshold watching.
  size_t s_threshold = static_cast<size_t>(threshold);
  zx_status_t status =
      data_.set_property(ZX_PROP_SOCKET_TX_THRESHOLD, &s_threshold, sizeof(s_threshold));
  switch (status) {
    case ZX_OK:
    // Ignore peer closed, this is handled in the signal waiter.
    case ZX_ERR_PEER_CLOSED:
      break;
    // Any other errors are not really expected to happen here.
    default:
      fdf::error("failed to set socket tx threshold to {}: {}", threshold,
                 zx_status_get_string(status));
      break;
  }
}

SocketDevice::CreditInfo SocketDevice::Connection::GetCreditInfo() {
  zx_info_socket_t info;
  zx_status_t status = data_.get_info(ZX_INFO_SOCKET, &info, sizeof(info), nullptr, nullptr);
  if (status != ZX_OK) {
    return {};
  }
  // We want to find the number of bytes that have been received.
  // We can do this by taking the total number of bytes written to the socket, and subtract
  // the bytes still sitting in the buffer.
  uint32_t in_socket = static_cast<uint32_t>(info.tx_buf_size);
  uint32_t fwd_count = rx_count_ - in_socket;
  return CreditInfo{
      .buf_alloc = static_cast<uint32_t>(info.tx_buf_max),
      .fwd_count = fwd_count,
      .in_socket = in_socket,
  };
}

virtio_vsock_hdr_t SocketDevice::Connection::MakeHdr(uint16_t op) {
  return make_hdr(ConnectionKey(key_.addr), op, cid_, GetCreditInfo());
}

void SocketDevice::Connection::Close(async_dispatcher_t* dispatcher) {
  state_ = Connection::State::ZOMBIE;
  zx_status_t status = async::PostTask(dispatcher, [this] {
    zx_status_t status = wait_handler_.Cancel();
    if (status == ZX_OK) {
      wait_handler_ref_.reset();
    };
  });
  ZX_DEBUG_ASSERT(status == ZX_OK);
}

zx_status_t SocketDevice::Connection::ContinueTx(bool force_credit_request, TxIoBufferRing& tx,
                                                 async_dispatcher_t* dispatcher) {
  if (pending_vmo_) {
    bool more = DoVmoTx(force_credit_request, tx);
    if (more) {
      return ZX_ERR_SHOULD_WAIT;
    }
    // If the vmo has fully transmitted then we are allowed to start transmitting
    // data from the socket again, so we fall through to check the socket.
  }
  if (SocketTxPending()) {
    return DoSocketTx(force_credit_request, tx, dispatcher);
  } else {
    BeginWait(dispatcher);
  }
  return ZX_OK;
}

zx_status_t SocketDevice::Connection::SetVmo(zx::bti& bti, zx::vmo vmo, uint64_t offset,
                                             uint64_t len, uint64_t bti_contiguity) {
  if (pending_vmo_) {
    return ZX_ERR_BAD_STATE;
  }
  zx_status_t result = vmo_.Set(bti, std::move(vmo), offset, len, bti_contiguity);
  if (result != ZX_OK) {
    return result;
  }
  pending_vmo_ = true;
  return result;
}

void SocketDevice::Connection::QueueOp(uint16_t new_op) {
  // RW operations don't get queued here
  assert(new_op != VIRTIO_VSOCK_OP_RW);
  if (!has_pending_op_) {
    pending_op_ = new_op;
    has_pending_op_ = true;
    return;
  }
  // We preference RST, then SHUTDOWN for ops since we never want to
  // overwrite those. Then we preference CREDIT_REQUEST, since if we
  // overwrite a CREDIT_UPDATE this is fine as the REQUEST will contain
  // an update anyway. The only other op we send is REQUEST and RESPONSE
  // and they will never queue over themselves or other ops, except for
  // RST, which have already taken care of.
  if (pending_op_ == VIRTIO_VSOCK_OP_RST || new_op == VIRTIO_VSOCK_OP_RST) {
    pending_op_ = VIRTIO_VSOCK_OP_RST;
  } else if (pending_op_ == VIRTIO_VSOCK_OP_SHUTDOWN || new_op == VIRTIO_VSOCK_OP_SHUTDOWN) {
    pending_op_ = VIRTIO_VSOCK_OP_SHUTDOWN;
  } else if (pending_op_ == VIRTIO_VSOCK_OP_CREDIT_REQUEST ||
             new_op == VIRTIO_VSOCK_OP_CREDIT_REQUEST) {
    pending_op_ = VIRTIO_VSOCK_OP_CREDIT_REQUEST;
  } else {
    pending_op_ = new_op;
  }
}

bool SocketDevice::Connection::HasPendingOp() { return has_pending_op_; }
uint16_t SocketDevice::Connection::TakePendingOp() {
  assert(HasPendingOp());
  has_pending_op_ = false;
  return pending_op_;
}

size_t SocketDevice::Connection::GetHash(const ConnectionKey& key) {
  return key.addr.local_port + key.addr.remote_port + key.addr.remote_cid;
}

const SocketDevice::ConnectionKey& SocketDevice::Connection::GetKey() const { return key_; }

void SocketDevice::Connection::CountTx(uint32_t len) {
  // Previous peer_free amount.
  uint32_t prev_peer_free = buf_alloc_ - (tx_count_ - fwd_cnt_);
  // Determine our projected 'peer_free' amount after this.
  uint32_t next_peer_free = buf_alloc_ - ((tx_count_ + len) - fwd_cnt_);
  // Have we crossed the threshold of 40% or 80% used?
  uint32_t prev_util = 100 - ((prev_peer_free * 100) / buf_alloc_);
  uint32_t next_util = 100 - ((next_peer_free * 100) / buf_alloc_);
  if ((prev_util < 40 && next_util >= 40) || (prev_util < 80 && next_util >= 80)) {
    QueueOp(VIRTIO_VSOCK_OP_CREDIT_REQUEST);
  }
  tx_count_ += len;
}

bool SocketDevice::Connection::SocketTxPending() {
  zx_info_socket_t info;
  zx_status_t status = data_.get_info(ZX_INFO_SOCKET, &info, sizeof(info), nullptr, nullptr);
  if (status != ZX_OK) {
    return false;
  }
  return info.rx_buf_size != 0;
}

bool SocketDevice::Connection::DoVmoTx(bool force_credit_request, TxIoBufferRing& tx) {
  bool needs_kick = false;
  auto auto_kick = fit::defer([&needs_kick, &tx]() {
    if (needs_kick) {
      tx.Kick();
    }
  });
  while (vmo_.transfer_length_ > 0) {
    uint32_t peer_free = GetPeerFree(force_credit_request);
    if (peer_free == 0) {
      return true;
    }

    uint16_t id;
    if (!tx.AllocIndirect(key_, &id)) {
      return true;
    }
    uint32_t len = static_cast<uint32_t>(vmo_.NextChunkLen(peer_free));
    uintptr_t paddr = vmo_.Consume(len);
    tx.SetIndirectPayload(id, paddr);

    virtio_vsock_hdr_t hdr = MakeHdr(VIRTIO_VSOCK_OP_RW);
    hdr.len = len;
    tx.SetHeader(id, hdr);
    tx.SubmitChain(id, len);
    needs_kick = true;
    CountTx(len);
  }
  return false;
}

zx_status_t SocketDevice::Connection::DoSocketTx(bool force_credit_request, TxIoBufferRing& tx,
                                                 async_dispatcher_t* dispatcher) {
  bool needs_kick = false;
  auto auto_kick = fit::defer([&needs_kick, &tx]() {
    if (needs_kick) {
      tx.Kick();
    }
  });
  zx_status_t status;
  do {
    uint32_t peer_free = GetPeerFree(force_credit_request);
    if (peer_free == 0) {
      return ZX_ERR_SHOULD_WAIT;
    }

    uint16_t id;
    void* data = tx.AllocInPlace(&id);
    if (!data) {
      return ZX_ERR_SHOULD_WAIT;
    }

    size_t read_raw;
    status = data_.read(
        0, data,
        std::min(static_cast<uint32_t>(kFrameSize - sizeof(virtio_vsock_hdr_t)), peer_free),
        &read_raw);
    uint32_t read = static_cast<uint32_t>(read_raw);
    if (status == ZX_OK) {
      virtio_vsock_hdr_t hdr = MakeHdr(VIRTIO_VSOCK_OP_RW);
      hdr.len = read;
      tx.SetHeader(id, hdr);
      tx.SubmitChain(id, read);
      needs_kick = true;
      CountTx(read);
    } else {
      tx.FreeChain(id);
    }
  } while (status == ZX_OK);
  if (status == ZX_ERR_SHOULD_WAIT) {
    BeginWait(dispatcher);
    // We have received all the data off the socket, so the correct thing to return
    // to the caller is ZX_OK so that it doesn't think there is still TX pending.
    return ZX_OK;
  }

  return status;
}

void SocketDevice::Connection::BeginWait(async_dispatcher_t* disp) {
  fbl::RefPtr<Connection> wait_ref = fbl::RefPtr(this);
  zx_status_t status = async::PostTask(disp, [wait_ref, disp] {
    std::lock_guard lock(wait_ref->lock_);

    if (!wait_ref->wait_handler_.is_pending()) {
      wait_ref->wait_handler_ref_ = wait_ref;
      zx_status_t status = wait_ref->wait_handler_.Begin(disp);
      if (status != ZX_OK) {
        ZX_DEBUG_ASSERT(status == ZX_ERR_BAD_STATE);
        wait_ref->wait_handler_ref_.reset();
      }
    }
  });
  ZX_DEBUG_ASSERT(status == ZX_OK);
}

uint32_t SocketDevice::Connection::GetPeerFree(bool request_credit) {
  uint32_t peer_free = buf_alloc_ - (tx_count_ - fwd_cnt_);
  if (peer_free == 0 && request_credit) {
    QueueOp(VIRTIO_VSOCK_OP_CREDIT_REQUEST);
  }
  return peer_free;
}

zx_status_t SocketDevice::Connection::VmoWalker::Set(zx::bti& bti, zx::vmo vmo, uint64_t offset,
                                                     uint64_t len, uint64_t bti_contiguity) {
  Release();
  vmo_ = std::move(vmo);
  contiguity_ = bti_contiguity;
  transfer_offset_ = offset;
  transfer_length_ = len;
  // Construct a base pointer that is aligned to the contiguity
  base_addr_ = fbl::round_down(offset, contiguity_);
  // Determine an extended range to take into account the rounding amount
  uint64_t full_range = fbl::round_up((offset - base_addr_) + len, contiguity_);
  size_t num_paddr = full_range / contiguity_;

  paddrs_ = fbl::MakeArray<zx_paddr_t>(num_paddr);

  zx_status_t status = bti.pin(ZX_BTI_PERM_READ | ZX_BTI_COMPRESS, vmo_, base_addr_, full_range,
                               paddrs_.data(), paddrs_.size(), &pinned_pages_);
  if (status != ZX_OK) {
    Release();
  }
  return status;
}

void SocketDevice::Connection::VmoWalker::Release() {
  if (pinned_pages_.is_valid()) {
    pinned_pages_.unpin();
  }
  pinned_pages_.reset();
  vmo_.reset();
  final_paddr_ = 0;
  paddrs_.reset();
}

uint64_t SocketDevice::Connection::VmoWalker::NextChunkLen(uint64_t max) {
  // First constrain max by the remaining transfer
  uint64_t next_len = std::min(max, transfer_length_);
  // Determine the end of the current contiguity region
  uint64_t contiguity_area_end = fbl::round_up(transfer_offset_ + 1, contiguity_);
  uint64_t max_in_contiguity = contiguity_area_end - transfer_offset_;
  // Take the minimum of our transfer and the contiguity
  return std::min(next_len, max_in_contiguity);
}

zx_paddr_t SocketDevice::Connection::VmoWalker::Consume(uint64_t len) {
  assert(NextChunkLen(len) >= len);
  // No need to subtract base_addr off transfer_offset since base_addr is
  // already defined to be aligned to contiguity_ and so is factored out of the
  // mod operation.
  uint64_t contiguity_offset = transfer_offset_ % contiguity_;
  zx_paddr_t ret = paddrs_[(transfer_offset_ - base_addr_) / contiguity_] + contiguity_offset;
  transfer_offset_ += len;
  transfer_length_ -= len;
  if (transfer_length_ == 0) {
    final_paddr_ = ret;
  }
  return ret;
}

}  // namespace virtio
