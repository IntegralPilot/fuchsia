// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "tx_queue.h"

#include <lib/async/cpp/task.h>
#include <lib/fdf/cpp/env.h>
#include <zircon/assert.h>
#include <zircon/compiler.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include "device_interface.h"
#include "fbl/auto_lock.h"
#include "lib/fit/defer.h"
#include "log.h"
#include "session.h"
#include "zircon/status.h"

namespace network::internal {

constexpr char kTxSchedulerRole[] = "fuchsia.devices.network.core.tx";

std::atomic<uint32_t> TxQueue::num_instances_ = 0;

TxQueue::~TxQueue() {
  // running_ is tied to the lifetime of the watch thread, it's cleared in`TxQueue::JoinThread`.
  // This assertion protects us from destruction paths where `TxQueue::JoinThread` is not called.
  ZX_ASSERT_MSG(!running_, "TxQueue destroyed without disposing of port and thread first.");
}

void TxQueue::JoinThread() {
  if (!dispatcher_.get()) {
    return;
  }

  if (zx_status_t status = EnqueueUserPacket(kQuitKey); status != ZX_OK) {
    LOGF_ERROR("TxQueue::JoinThread failed to send quit key: %s", zx_status_get_string(status));
  }

  // Mark the queue as not running anymore.
  running_ = false;

  dispatcher_.ShutdownAsync();
  dispatcher_shutdown_.Wait();
  dispatcher_.reset();
  --num_instances_;
}

zx_status_t TxQueue::EnqueueUserPacket(uint64_t key) {
  zx_port_packet_t packet;
  packet.type = ZX_PKT_TYPE_USER;
  packet.key = key;
  packet.status = ZX_OK;
  return port_.queue(&packet);
}

void TxQueue::SessionTransaction::Commit() {
  // when we destroy a session transaction, we commit all the queued buffers to
  // the device.
  if (queued_ != 0) {
    // Send buffers in batches of at most |kMaxTxBuffers| at a time to stay within the FIDL
    // channel maximum.
    netdriver::wire::TxBuffer* buffers = buffers_.data();
    while (queued_ > 0) {
      const uint32_t batch =
          std::min(static_cast<uint32_t>(queued_), netdriver::wire::kMaxTxBuffers);
      queue_->parent_->QueueTx(cpp20::span(buffers, batch));
      buffers += batch;
      queued_ -= batch;
    }
  }
}

fuchsia_hardware_network_driver::wire::TxBuffer* TxQueue::SessionTransaction::GetBuffer() {
  ZX_ASSERT(available_ != 0);
  return &buffers_[queued_];
}

void TxQueue::SessionTransaction::Push(uint16_t descriptor) {
  ZX_ASSERT(available_ != 0);
  queue_->session_->session->TxTaken();
  buffers_[queued_].id = queue_->Enqueue(descriptor);
  available_--;
  queued_++;
}

TxQueue::SessionTransaction::SessionTransaction(
    cpp20::span<fuchsia_hardware_network_driver::wire::TxBuffer> buffers, TxQueue* parent)
    : buffers_(buffers), queue_(parent), queued_(0) {
  // only get available slots after lock is acquired:
  // 0 available slots if parent is not enabled.
  available_ = queue_->parent_->IsDataPlaneOpen() ? queue_->in_flight_->available() : 0;
}

zx::result<std::unique_ptr<TxQueue>> TxQueue::Create(DeviceInterface* parent) {
  // The Tx queue capacity is based on the underlying device's tx queue capacity.
  auto capacity = parent->info().tx_depth().value_or(0);

  fbl::AllocChecker ac;
  std::unique_ptr<TxQueue> queue(new (&ac) TxQueue(parent));
  if (!ac.check()) {
    return zx::error(ZX_ERR_NO_MEMORY);
  }

  fbl::AutoLock lock(&queue->parent_->tx_lock());

  zx::result return_queue = RingQueue<uint32_t>::Create(capacity);
  if (return_queue.is_error()) {
    return return_queue.take_error();
  }
  queue->return_queue_ = std::move(return_queue.value());

  zx::result in_flight = IndexedSlab<InFlightBuffer>::Create(capacity);
  if (in_flight.is_error()) {
    return in_flight.take_error();
  }
  queue->in_flight_ = std::move(in_flight.value());
  std::unique_ptr<fuchsia_hardware_network_driver::wire::TxBuffer[]> tx_buffers(
      new (&ac) fuchsia_hardware_network_driver::wire::TxBuffer[capacity]);
  if (!ac.check()) {
    return zx::error(ZX_ERR_NO_MEMORY);
  }
  // TODO(https://github.com/llvm/llvm-project/issues/54497): remove unnecessary
  // type extraction once clang-format can deal with [] in template parameter.
  using BufferParts_t = BufferParts<fuchsia_hardware_network_driver::wire::BufferRegion>[];
  std::unique_ptr<BufferParts_t> buffer_parts(
      new (&ac) BufferParts<fuchsia_hardware_network_driver::wire::BufferRegion>[capacity]);
  if (!ac.check()) {
    return zx::error(ZX_ERR_NO_MEMORY);
  }
  for (int i = 0; i < capacity; ++i) {
    tx_buffers[i].data =
        fidl::VectorView<fuchsia_hardware_network_driver::wire::BufferRegion>::FromExternal(
            buffer_parts[i].data(), 0);
  }

  if (zx_status_t status = zx::port::create(0, &queue->port_); status != ZX_OK) {
    LOGF_ERROR("failed to create tx queue port");
    return zx::error(status);
  }

  // Make sure the driver framework allows for the creation of the necessary threads. Keep track of
  // the number of TX queue instances globally.
  uint32_t instances = ++num_instances_;
  if (zx_status_t status =
          fdf_env_set_thread_limit(kTxSchedulerRole, strlen(kTxSchedulerRole), instances);
      status != ZX_OK && status != ZX_ERR_OUT_OF_RANGE) {
    // ZX_ERR_OUT_OF_RANGE indicates that the value is less than the current value. This can happen
    // if a number of threads have recently shut down or two TX queues are being created at the same
    // time and the loading of the atomic and setting of the limit are interleaved. It's safe to
    // ignore that in this context, the important part is that there are enough threads.
    LOGF_ERROR("failed to update thread limit: %s", zx_status_get_string(status));
    return zx::error(status);
  }

  // In order to ensure that the async::PostTask below works this has to be a synchronized
  // dispatcher that allows synchronous calls. Any other combination of dispatcher and options could
  // lead to the async::PostTask call being inlined, meaning the task would run on the calling
  // thread, blocking it indefinitely. Use a unique owner to ensure inlining of calls from inside
  // the task. Calls to a dispatcher with the same owner might not be inlined.
  auto dispatcher = fdf_env::DispatcherBuilder::CreateSynchronizedWithOwner(
      queue.get(), fdf::SynchronizedDispatcher::Options::kAllowSyncCalls, "netdevice:tx_watch",
      [queue = queue.get()](fdf_dispatcher_t*) { queue->dispatcher_shutdown_.Signal(); },
      kTxSchedulerRole);
  if (dispatcher.is_error()) {
    LOGF_ERROR("Failed to create tx dispatcher: %s", dispatcher.status_string());
    return dispatcher.take_error();
  }
  queue->dispatcher_ = std::move(dispatcher.value());

  async::PostTask(queue->dispatcher_.async_dispatcher(),
                  [queue = queue.get(), tx_buffers = std::move(tx_buffers),
                   buffer_parts = std::move(buffer_parts),
                   capacity]() { queue->Thread(cpp20::span(&tx_buffers[0], capacity)); });
  queue->running_ = true;

  return zx::ok(std::move(queue));
}

void TxQueue::Thread(cpp20::span<fuchsia_hardware_network_driver::wire::TxBuffer> buffers) {
  for (;;) {
    zx_port_packet_t packet;
    zx_status_t status = port_.wait(zx::time::infinite(), &packet);
    if (status != ZX_OK) {
      LOGF_ERROR("tx thread port wait failed: %s", zx_status_get_string(status));
      return;
    }
    switch (packet.type) {
      case ZX_PKT_TYPE_USER:
        switch (packet.key) {
          case kResumeKey:
            if (zx_status_t status = UpdateFifoWatches(); status != ZX_OK) {
              LOGF_ERROR("failed to install FIFO watches: %s", zx_status_get_string(status));
              return;
            }
            break;
          case kQuitKey:
            return;
          default:
            ZX_PANIC("unexpected user packet key %ld", packet.key);
        }
        break;
      case ZX_PKT_TYPE_SIGNAL_ONE:
        if (zx_status_t status = HandleFifoSignal(buffers, packet.signal.observed);
            status != ZX_OK) {
          LOGF_ERROR("failed to handle FIFO signal: %s", zx_status_get_string(status));
          return;
        }
        break;
      default:
        ZX_PANIC("unexpected packet type %d", packet.type);
    }
  }
}

zx_status_t TxQueue::UpdateFifoWatches() {
  fbl::AutoLock lock(&parent_->tx_lock());
  if (!session_.has_value()) {
    return ZX_OK;
  }

  SessionWaiter& waiter = *session_;
  Session& session = *waiter.session;

  if (session.IsPaused()) {
    if (!waiter.wait_installed) {
      return ZX_OK;
    }
    zx_status_t status = port_.cancel_key(0u, 0u);
    switch (status) {
      case ZX_OK:
        waiter.wait_installed = false;
        return ZX_OK;
      default:
        LOGF_ERROR("failed to cancel FIFO wait for session %s: %s", session.name(),
                   zx_status_get_string(status));
        return status;
    }
  }

  if (waiter.wait_installed) {
    return ZX_OK;
  }

  if (zx_status_t status =
          session.tx_fifo().wait_async(port_, 0u, ZX_FIFO_PEER_CLOSED | ZX_FIFO_READABLE, 0);
      status != ZX_OK) {
    LOGF_ERROR("failed to start FIFO wait for session %s: %s", session.name(),
               zx_status_get_string(status));
    return status;
  }

  waiter.wait_installed = true;
  return ZX_OK;
}

zx_status_t TxQueue::HandleFifoSignal(
    cpp20::span<fuchsia_hardware_network_driver::wire::TxBuffer> buffers, zx_signals_t signals) {
  fbl::AutoLock lock(&parent_->tx_lock());
  // Session already removed from Tx queue, packet was lingering in the port.
  if (!session_.has_value()) {
    return ZX_OK;
  }

  SessionWaiter& waiter = session_.value();
  waiter.wait_installed = false;
  Session& session = *waiter.session;
  const zx::fifo& fifo = session.tx_fifo();
  SessionTransaction transaction(buffers, this);

  // TA really doesn't like defers or its interplay with AutoLock.
  auto defer = fit::defer([&transaction, &lock]() __TA_NO_THREAD_SAFETY_ANALYSIS {
    // Committing the transaction must not be holding any locks, because we call
    // into the device.
    lock.release();
    transaction.Commit();
  });

  if (signals & ZX_FIFO_READABLE) {
    session.AssertParentTxLock(*parent_);
    zx_status_t status = session.FetchTx(transaction);
    switch (status) {
      case ZX_OK:
      case ZX_ERR_SHOULD_WAIT:
        break;
      case ZX_ERR_PEER_CLOSED:
        // FIFO is closed, don't reinstall the wait.
      case ZX_ERR_IO_OVERRUN:
        // We've run out of device buffers, don't reinstall the wait until we're
        // notified of new buffers.
        return ZX_OK;
      default:
        LOGF_ERROR("unexpected error for session %s: %s; killing it", session.name(),
                   zx_status_get_string(status));
        session.Kill();
        return ZX_OK;
    }
  }

  if (signals & ZX_FIFO_PEER_CLOSED) {
    // FIFO is closed, don't reinstall the wait.
    return ZX_OK;
  }

  if (zx_status_t status = fifo.wait_async(port_, 0u, ZX_FIFO_PEER_CLOSED | ZX_FIFO_READABLE, 0);
      status != ZX_OK) {
    LOGF_ERROR("failed to start FIFO wait for session %s: %s", session.name(),
               zx_status_get_string(status));
    return status;
  }

  waiter.wait_installed = true;

  return ZX_OK;
}

void TxQueue::Resume() {
  if (!running_) {
    return;
  }

  if (zx_status_t status = EnqueueUserPacket(kResumeKey); status != ZX_OK) {
    LOGF_ERROR("TxQueue::Resume failed: %s", zx_status_get_string(status));
  }
}

uint32_t TxQueue::Enqueue(uint16_t descriptor) {
  return in_flight_->Push(InFlightBuffer(ZX_OK, descriptor));
}

bool TxQueue::ReturnBuffers() {
  size_t count = 0;

  uint16_t desc_buffer[kMaxFifoDepth];
  ZX_ASSERT(return_queue_->count() <= kMaxFifoDepth);

  uint16_t* descs = desc_buffer;
  // record if the queue was full, meaning that the session may have halted fetching tx frames due
  // to overruns:
  bool was_full = in_flight_->available() == 0;
  while (return_queue_->count()) {
    auto& buff = in_flight_->Get(return_queue_->Peek());
    session_->session->MarkTxReturnResult(buff.descriptor_index, buff.result);
    *descs++ = buff.descriptor_index;
    count++;
    // pop from return queue and free space in in_flight queue:
    in_flight_->Free(return_queue_->Pop());
  }

  if (count != 0) {
    session_->session->ReturnTxDescriptors(desc_buffer, count);
  }

  return was_full;
}

void TxQueue::CompleteTxList(
    const fidl::VectorView<::fuchsia_hardware_network_driver::wire::TxResult>& tx_results) {
  fbl::AutoLock lock(&parent_->tx_lock());
  for (const auto& tx : tx_results.get()) {
    InFlightBuffer& buff = in_flight_->Get(tx.id);
    buff.result = tx.status;
    return_queue_->Push(tx.id);
  }
  bool was_full = ReturnBuffers();
  parent_->NotifyTxReturned(was_full);
  parent_->NotifyTxComplete();
}

void TxQueue::SetSession(Session* session) {
  if (session == nullptr) {
    session_.reset();
  } else {
    ZX_ASSERT_MSG(!session_.has_value(), "Session already installed");
    session_ = SessionWaiter{.session = session, .wait_installed = false};
    if (zx_status_t status = EnqueueUserPacket(kResumeKey); status != ZX_OK) {
      LOGF_ERROR("failed to notify of new session %s: %s", session->name(),
                 zx_status_get_string(status));
    }
  }
}

bool TxQueue::HasSession() { return session_.has_value(); }

}  // namespace network::internal
