// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "rx_queue.h"

#include <lib/async/cpp/task.h>
#include <lib/fdf/cpp/env.h>
#include <zircon/assert.h>

#include "log.h"
#include "session.h"

namespace network::internal {

constexpr char kRxSchedulerRole[] = "fuchsia.devices.network.core.rx";

std::atomic<uint32_t> RxQueue::num_instances_ = 0;

RxQueue::~RxQueue() {
  // running_ is tied to the lifetime of the watch thread, it's cleared in`RxQueue::JoinThread`.
  // This assertion protects us from destruction paths where `RxQueue::JoinThread` is not called.
  ZX_ASSERT_MSG(!running_, "RxQueue destroyed without disposing of port and thread first.");
  ZX_ASSERT_MSG(!session_, "RxQueue destroyed without detaching session");
}

zx::result<std::unique_ptr<RxQueue>> RxQueue::Create(DeviceInterface* parent) {
  fbl::AllocChecker ac;
  std::unique_ptr<RxQueue> queue(new (&ac) RxQueue(parent));
  if (!ac.check()) {
    return zx::error(ZX_ERR_NO_MEMORY);
  }

  fbl::AutoLock lock(&queue->parent_->rx_lock());
  // The RxQueue's capacity is the device's FIFO rx depth as opposed to the hardware's queue depth
  // so we can (possibly) reduce the amount of reads on the rx fifo during rx interrupts.
  auto capacity = parent->rx_fifo_depth();

  zx::result available_queue = RingQueue<uint32_t>::Create(capacity);
  if (available_queue.is_error()) {
    return available_queue.take_error();
  }
  queue->available_queue_ = std::move(available_queue.value());

  zx::result in_flight = IndexedSlab<InFlightBuffer>::Create(capacity);
  if (in_flight.is_error()) {
    return in_flight.take_error();
  }
  queue->in_flight_ = std::move(in_flight.value());

  auto device_depth = parent->info().rx_depth().value_or(0);

  std::unique_ptr<fuchsia_hardware_network_driver::wire::RxSpaceBuffer[]> buffers(
      new (&ac) fuchsia_hardware_network_driver::wire::RxSpaceBuffer[device_depth]);
  if (!ac.check()) {
    return zx::error(ZX_ERR_NO_MEMORY);
  }
  if (zx_status_t status = zx::port::create(0, &queue->rx_watch_port_); status != ZX_OK) {
    LOGF_ERROR("failed to create rx watch port: %s", zx_status_get_string(status));
    return zx::error(status);
  }

  // Make sure the driver framework allows for the creation of the necessary threads. Keep track of
  // the number of RX queue instances globally.
  uint32_t instances = ++num_instances_;
  if (zx_status_t status =
          fdf_env_set_thread_limit(kRxSchedulerRole, strlen(kRxSchedulerRole), instances);
      status != ZX_OK && status != ZX_ERR_OUT_OF_RANGE) {
    // ZX_ERR_OUT_OF_RANGE indicates that the value is less than the current value. This can happen
    // if a number of threads have recently shut down or two RX queues are being created at the same
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
      queue.get(), fdf::SynchronizedDispatcher::Options::kAllowSyncCalls, "netdevice:rx_watch",
      [queue = queue.get()](fdf_dispatcher_t*) { queue->dispatcher_shutdown_.Signal(); },
      kRxSchedulerRole);
  if (dispatcher.is_error()) {
    LOGF_ERROR("rx queue failed to create dispatcher: %s", dispatcher.status_string());
    return dispatcher.take_error();
  }
  queue->dispatcher_ = std::move(dispatcher.value());

  async::PostTask(queue->dispatcher_.async_dispatcher(),
                  [queue = queue.get(), rx_buffers = std::move(buffers)]() mutable {
                    queue->WatchThread(std::move(rx_buffers));
                  });

  queue->running_ = true;
  return zx::ok(std::move(queue));
}

void RxQueue::TriggerRxWatch() {
  if (!running_) {
    return;
  }

  zx_port_packet_t packet;
  packet.type = ZX_PKT_TYPE_USER;
  packet.key = kTriggerRxKey;
  packet.status = ZX_OK;
  zx_status_t status = rx_watch_port_.queue(&packet);
  if (status != ZX_OK) {
    LOGF_ERROR("TriggerRxWatch failed: %s", zx_status_get_string(status));
  }
}

void RxQueue::TriggerSessionChanged() {
  if (!running_) {
    return;
  }
  zx_port_packet_t packet;
  packet.type = ZX_PKT_TYPE_USER;
  packet.key = kSessionSwitchKey;
  packet.status = ZX_OK;
  zx_status_t status = rx_watch_port_.queue(&packet);
  if (status != ZX_OK) {
    LOGF_ERROR("TriggerSessionChanged failed: %s", zx_status_get_string(status));
  }
}

void RxQueue::JoinThread() {
  if (dispatcher_.get()) {
    zx_port_packet_t packet;
    packet.type = ZX_PKT_TYPE_USER;
    packet.key = kQuitWatchKey;
    zx_status_t status = rx_watch_port_.queue(&packet);
    if (status != ZX_OK) {
      LOGF_ERROR("RxQueue::JoinThread failed to send quit key: %s", zx_status_get_string(status));
    }
    // Mark the queue as not running anymore.
    running_ = false;
    dispatcher_.ShutdownAsync();
    dispatcher_shutdown_.Wait();
    dispatcher_.reset();
    --num_instances_;
  }
}

void RxQueue::PurgeSession() {
  fbl::AutoLock lock(&parent_->rx_lock());
  // Get rid of all available buffers that belong to the session and stop its rx path.
  session_->AssertParentRxLock(*parent_);
  if (rx_vmo_state_.estimator) {
    rx_vmo_state_.estimator.reset();
  }
  session_->StopRx();
  for (auto it = rx_vmo_state_.vmos.begin(); it != rx_vmo_state_.vmos.end(); ++it) {
    ReleaseWithheldBuffers(it);
  }
  for (auto nu = available_queue_->count(); nu > 0; nu--) {
    auto b = available_queue_->Pop();
    in_flight_->Free(b);
  }
}

std::tuple<RxQueue::InFlightBuffer*, uint32_t> RxQueue::GetBuffer() {
  if (available_queue_->count() != 0) {
    auto idx = available_queue_->Pop();
    return std::make_tuple(&in_flight_->Get(idx), idx);
  }
  // Need to fetch more from the session.
  if (in_flight_->available() == 0) {
    // No more space to keep in flight buffers.
    // Not an error because buffers can be held for VMOs not prepared.
    return std::make_tuple(nullptr, 0);
  }

  RxSessionTransaction transaction(this);
  switch (zx_status_t status = parent_->LoadRxDescriptors(transaction); status) {
    case ZX_OK:
      break;
    default:
      LOGF_ERROR("failed to load rx buffer descriptors: %s", zx_status_get_string(status));
      __FALLTHROUGH;
    case ZX_ERR_PEER_CLOSED:  // FIFO closed.
    case ZX_ERR_SHOULD_WAIT:  // No Rx buffers available in FIFO.
    case ZX_ERR_BAD_STATE:    // Session stopped or paused.
      return std::make_tuple(nullptr, 0);
  }
  // LoadRxDescriptors can't return OK if it couldn't load any descriptors.
  auto idx = available_queue_->Pop();
  return std::make_tuple(&in_flight_->Get(idx), idx);
}

zx_status_t RxQueue::PrepareBuff(fuchsia_hardware_network_driver::wire::RxSpaceBuffer* buff) {
  auto [session_buffer, index] = GetBuffer();
  if (session_buffer == nullptr) {
    return ZX_ERR_NO_RESOURCES;
  }

  buff->id = index;
  session_->AssertParentControlLockShared(*parent_);
  if (zx_status_t status = session_->FillRxSpace(session_buffer->descriptor_index, buff);
      status != ZX_OK) {
    // If the session can't fill Rx for any reason, kill it.
    session_->Kill();
    // Put the index back at the end of the available queue.
    available_queue_->Push(index);
    return status;
  }

  session_->RxTaken();
  device_buffer_count_++;
  return ZX_OK;
}

void RxQueue::CompleteRxList(
    const fidl::VectorView<::fuchsia_hardware_network_driver::wire::RxBuffer>& rx_buffer_list) {
  fbl::AutoLock lock(&parent_->rx_lock());
  if (rx_vmo_state_.estimator) {
    zx::time now = zx::clock::get_monotonic();
    if (rx_vmo_state_.last_complete_time != zx::time::infinite_past()) {
      zx::duration elapsed = now - rx_vmo_state_.last_complete_time;
      if (elapsed.get()) {
        uint64_t sample_rate = std::max(rx_buffer_list.size() * ZX_SEC(1) / elapsed.get(), 1UL);
        UpdatePeakRateInSamplePeriod(sample_rate);
      }
    }
    rx_vmo_state_.last_complete_time = now;
  }
  ZX_ASSERT_MSG(session_ != nullptr,
                "Session should not be null while we still have inflight buffers");
  SharedAutoLock control_lock(&parent_->control_lock());
  for (const auto& rx_buffer : rx_buffer_list.get()) {
    // Always increment frame index for anything the device sends us. The session
    // gets its local index for frames that make their way through.
    rx_completed_frame_index_++;
    ZX_ASSERT_MSG(rx_buffer.data.size() <= netdriver::wire::kMaxBufferParts,
                  "too many buffer parts in rx buffer: %ld", rx_buffer.data.size());
    std::array<SessionRxBuffer, netdriver::wire::kMaxBufferParts> session_parts;
    auto session_parts_iter = session_parts.begin();
    uint32_t total_length = 0;

    cpp20::span rx_parts = rx_buffer.data.get();
    if (rx_parts.empty()) {
      // Buffer contained no parts.
      LOG_WARN("attempted to return an rx buffer with no parts");
      continue;
    }

    if (device_buffer_count_ >= rx_parts.size()) {
      device_buffer_count_ -= rx_parts.size();
    } else {
      LOGF_ERROR("device returned more rx parts (%ld) than device_buffer_count_ (%hu)",
                 rx_parts.size(), device_buffer_count_);
      device_buffer_count_ = 0;
    }

    for (const fuchsia_hardware_network_driver::wire::RxBufferPart& rx_part : rx_parts) {
      const InFlightBuffer& in_flight_buffer = in_flight_->Get(rx_part.id);

      total_length += rx_part.length;
      *session_parts_iter++ = SessionRxBuffer{
          .descriptor = in_flight_buffer.descriptor_index,
          .offset = rx_part.offset,
          .length = rx_part.length,
      };
    }

    // Drop any frames containing no data.
    if (total_length == 0) {
      for (const fuchsia_hardware_network_driver::wire::RxBufferPart& rx_part : rx_parts) {
        session_->AssertParentRxLock(*parent_);
        if (session_->CompleteUnfulfilledRx()) {
          // Make buffer available again for reuse if session is still valid.
          available_queue_->Push(rx_part.id);
        } else {
          // Free it otherwise.
          in_flight_->Free(rx_part.id);
        }
      }
      continue;
    }

    session_->AssertParentControlLockShared(*parent_);
    parent_->NotifyPortRxFrame(rx_buffer.meta.port, total_length);
    const RxFrameInfo frame_info = {
        .meta = rx_buffer.meta,
        .port_id_salt = parent_->GetPortSalt(rx_buffer.meta.port),
        .buffers = cpp20::span(session_parts.begin(), session_parts_iter),
        .total_length = total_length,
        .full_csums_verified = rx_buffer.full_csums_verified,
    };
    session_->AssertParentRxLock(*parent_);
    if (session_->CompleteRx(frame_info)) {
      std::for_each(rx_parts.begin(), rx_parts.end(),
                    [this](const fuchsia_hardware_network_driver::wire::RxBufferPart& rx)
                        __TA_REQUIRES(parent_->rx_lock()) { available_queue_->Push(rx.id); });
    } else {
      std::for_each(rx_parts.begin(), rx_parts.end(),
                    [this](const fuchsia_hardware_network_driver::wire::RxBufferPart& rx)
                        __TA_REQUIRES(parent_->rx_lock()) { in_flight_->Free(rx.id); });
    }
  }
  parent_->CommitSession();
  if (device_buffer_count_ <=
      std::min(parent_->rx_notify_threshold(), rx_vmo_state_.target_available_buffers)) {
    TriggerRxWatch();
  }
  parent_->TryDelegateRxLease(rx_completed_frame_index_);
}

int RxQueue::WatchThread(
    std::unique_ptr<fuchsia_hardware_network_driver::wire::RxSpaceBuffer[]> space_buffers) {
  auto loop = [this, space_buffers = std::move(space_buffers)]() -> zx_status_t {
    fbl::RefPtr<RefCountedFifo> observed_fifo(nullptr);
    bool waiting_on_fifo = false;
    for (;;) {
      zx_port_packet_t packet;
      bool fifo_readable = false;
      if (zx_status_t status = rx_watch_port_.wait(zx::time::infinite(), &packet);
          status != ZX_OK) {
        LOGF_ERROR("RxQueue::WatchThread port wait failed %s", zx_status_get_string(status));
        return status;
      }
      parent_->NotifyRxQueuePacket(packet.key);
      switch (packet.key) {
        case kTimerWatchKey: {
          {
            fbl::AutoLock lock(&parent_->rx_lock());
            TimerTick();
          }
          break;
        }
        case kQuitWatchKey:
          LOG_TRACE("RxQueue::WatchThread got quit key");
          return ZX_OK;
        case kSessionSwitchKey: {
          if (observed_fifo && waiting_on_fifo) {
            if (zx_status_t status = rx_watch_port_.cancel_key(0u, kFifoWatchKey);
                status != ZX_OK) {
              LOGF_ERROR("RxQueue::WatchThread port cancel failed %s",
                         zx_status_get_string(status));
              return status;
            }
            waiting_on_fifo = false;
          }
          observed_fifo = parent_->rx_fifo();
          LOGF_TRACE("RxQueue FIFO changed, valid=%d", static_cast<bool>(observed_fifo));
        } break;
        case kFifoWatchKey:
          if ((packet.signal.observed & ZX_FIFO_PEER_CLOSED) || packet.status != ZX_OK) {
            // If observing the FIFO fails, we're assuming that the session is being closed. We're
            // just going to dispose of our reference to the observed FIFO and wait for
            // `DeviceInterface` to signal us that a new session is available when that
            // happens.
            observed_fifo.reset();
            LOGF_TRACE("RxQueue fifo closed or bad status %s", zx_status_get_string(packet.status));
          } else {
            fifo_readable = true;
          }
          waiting_on_fifo = false;
          break;

        default:
          ZX_ASSERT_MSG(packet.key == kTriggerRxKey, "Unrecognized packet in rx queue");
          break;
      }

      uint16_t pushed = 0;
      bool should_wait_on_fifo;

      fbl::AutoLock rx_lock(&parent_->rx_lock());
      SharedAutoLock control_lock(&parent_->control_lock());
      const uint16_t rx_depth = parent_->info().rx_depth().value_or(0);
      const uint16_t target = std::min(rx_vmo_state_.current_available_buffers, rx_depth);
      uint16_t push_count = target - device_buffer_count_;
      if (parent_->IsDataPlaneOpen()) {
        for (; pushed < push_count; pushed++) {
          if (zx_status_t status = PrepareBuff(&space_buffers[pushed]); status != ZX_OK) {
            break;
          }
        }
      }

      if (fifo_readable && in_flight_->available()) {
        RxSessionTransaction transaction(this);
        parent_->LoadRxDescriptors(transaction);
      }
      // We only need to wait on the FIFO if we didn't get enough buffers.
      // Otherwise, we'll trigger the loop again once the device calls CompleteRx.
      //
      // Similarly, we should not wait on the FIFO if the device has not started yet.
      should_wait_on_fifo = device_buffer_count_ < target && parent_->IsDataPlaneOpen();

      // We release the main rx queue and control locks before calling into the parent device so we
      // don't cause a re-entrant deadlock.
      control_lock.release();
      rx_lock.release();

      if (pushed != 0) {
        // Send buffers in batches of at most |kMaxRxSpaceBuffer| at a time to stay within the
        // FIDL channel maximum.
        netdriver::wire::RxSpaceBuffer* buffers = space_buffers.get();
        while (pushed > 0) {
          const uint32_t batch =
              std::min(static_cast<uint32_t>(pushed), netdriver::wire::kMaxRxSpaceBuffers);
          parent_->QueueRxSpace(cpp20::span(buffers, static_cast<uint32_t>(batch)));
          buffers += batch;
          pushed -= batch;
        }
      }

      // No point waiting in RX fifo if we filled the device buffers, we'll get a signal to wait
      // on the fifo later.
      if (should_wait_on_fifo) {
        if (!observed_fifo) {
          // This can happen if we get triggered to fetch more buffers, but the session is
          // already tearing down, it's fine to just proceed.
          LOG_TRACE("RxQueue::WatchThread Should wait but no FIFO is here");
        } else if (!waiting_on_fifo) {
          zx_status_t status = observed_fifo->fifo.wait_async(
              rx_watch_port_, kFifoWatchKey, ZX_FIFO_READABLE | ZX_FIFO_PEER_CLOSED, 0);
          if (status == ZX_OK) {
            waiting_on_fifo = true;
          } else {
            LOGF_ERROR("RxQueue::WatchThread wait_async failed: %s", zx_status_get_string(status));
            return status;
          }
        }
      }
    }
  };
  zx_status_t status = loop();
  if (status != ZX_OK) {
    LOGF_ERROR("RxQueue::WatchThread finished loop with error: %s", zx_status_get_string(status));
  }
  LOG_TRACE("watch thread done");
  return 0;
}

void RxQueue::SetRxVmos(RxVmoList rx_vmos) {
  if (rx_vmos.is_empty()) {
    rx_vmo_state_.vmos.clear();
  } else {
    rx_vmo_state_.vmos = std::move(rx_vmos);
  }
  rx_vmo_state_.total_buffers = 0;
  rx_vmo_state_.current_available_buffers = 0;
  rx_vmo_state_.target_available_buffers = 0;
  rx_vmo_state_.pending_op = VmoOperation{};
  rx_vmo_state_.last_in_use = rx_vmo_state_.vmos.end();
  for (auto it = rx_vmo_state_.vmos.begin(); it != rx_vmo_state_.vmos.end(); ++it) {
    it->withheld_rx_buffers = 0;
    it->head_withheld_rx_buffers = kInvalidIdx;
    ZX_ASSERT(rx_vmo_state_.total_buffers <=
              std::numeric_limits<uint16_t>::max() - it->num_rx_buffers);
    rx_vmo_state_.total_buffers += it->num_rx_buffers;
    if (it->state == VmoState::kPrepared) {
      rx_vmo_state_.current_available_buffers += it->num_rx_buffers;
      rx_vmo_state_.target_available_buffers += it->num_rx_buffers;
      rx_vmo_state_.last_in_use = it;
    }
  }
}

void RxQueue::ReleaseWithheldBuffers(RxVmoList::iterator target) {
  while (target->head_withheld_rx_buffers != kInvalidIdx) {
    InFlightBuffer& buffer = in_flight_->Get(target->head_withheld_rx_buffers);
    uint32_t next = buffer.next_withheld_inflight_buffer;
    buffer.next_withheld_inflight_buffer = kInvalidIdx;
    available_queue_->Push(target->head_withheld_rx_buffers);
    target->head_withheld_rx_buffers = next;
    target->withheld_rx_buffers--;
  }
  ZX_ASSERT(target->withheld_rx_buffers == 0);
}

void RxQueue::SetRxBufferManagement(const netdriver::RxBufferManagement& management)
    __TA_REQUIRES(parent_->rx_lock()) {
  auto new_estimator = RxBufferEstimatorFromFidl(management);

  bool request_rx_space = false;
  if (new_estimator) {
    uint16_t default_min =
        rx_vmo_state_.vmos.is_empty() ? 0 : rx_vmo_state_.vmos.begin()->num_rx_buffers;
    rx_vmo_state_.min_buffers = parent_->info().min_rx_buffers().value_or(
        parent_->info().rx_threshold().value_or(default_min));
    request_rx_space = SetTargetRxBuffersIfMore(rx_vmo_state_.min_buffers);
  } else {
    rx_vmo_state_.min_buffers = rx_vmo_state_.target_available_buffers =
        rx_vmo_state_.total_buffers;
  }

  if (rx_vmo_state_.estimator) {
    rx_vmo_state_.estimator.reset();
  }

  rx_vmo_state_.estimator = std::move(new_estimator);

  if (rx_vmo_state_.estimator) {
    if (zx_status_t status = rx_vmo_state_.estimator->RearmTimer(rx_watch_port_, kTimerWatchKey);
        status != ZX_OK) {
      rx_vmo_state_.estimator.reset();
      rx_vmo_state_.min_buffers = rx_vmo_state_.target_available_buffers =
          rx_vmo_state_.total_buffers;
      LOGF_ERROR("failed to set the timer, reverting back to static management: %s",
                 zx_status_get_string(status));
      return;
    }
  }
}

void RxQueue::UpdatePeakRateInSamplePeriod(uint64_t sample_rate) __TA_REQUIRES(parent_->rx_lock()) {
  ZX_ASSERT(rx_vmo_state_.estimator);
  if (sample_rate > rx_vmo_state_.current_max_packet_rate) {
    rx_vmo_state_.current_max_packet_rate = sample_rate;
    uint16_t need_immediate_buffers = rx_vmo_state_.estimator->NeedImmediateBuffers(sample_rate);
    if (SetTargetRxBuffersIfMore(need_immediate_buffers)) {
      RequestRxSpace();
    }
  }
}

void RxQueue::MaybeReleaseCandidateVmo() {
  // Attempt to release the candidate VMO (`last_in_use`) if:
  // 1. The candidate VMO is in use (`IsValid`).
  // 2. All of its Rx buffers are sent by client but not in available queue.
  // 3. No other VMO operation is currently in progress (`pending_op.type == kNone`).
  // 4. Removing its buffers still leaves enough available buffers to meet the target.
  if (rx_vmo_state_.last_in_use.IsValid() &&
      rx_vmo_state_.last_in_use->withheld_rx_buffers == rx_vmo_state_.last_in_use->num_rx_buffers &&
      rx_vmo_state_.pending_op.type == VmoOperation::Type::kNone &&
      rx_vmo_state_.target_available_buffers <=
          rx_vmo_state_.current_available_buffers - rx_vmo_state_.last_in_use->num_rx_buffers) {
    rx_vmo_state_.pending_op.type = VmoOperation::Type::kReleaseSingle;
    if (parent_->ReleaseRxVmo(*rx_vmo_state_.last_in_use)) {
      LOGF_TRACE("RxQueue: Releasing Rx VMO %u", rx_vmo_state_.last_in_use->id);
    } else {
      rx_vmo_state_.pending_op = VmoOperation{};
      // We fail to release this VMO because it is registered for Tx, release all held
      // buffers because we won't be able to release it anyway.
      ReleaseWithheldBuffers(rx_vmo_state_.last_in_use);
    }
  }
}

void RxQueue::TimerTick() {
  if (!rx_vmo_state_.estimator) {
    rx_vmo_state_.current_max_packet_rate = 0;
    return;
  }
  rx_vmo_state_.estimator->Update(rx_vmo_state_.current_max_packet_rate);
  if (SetTargetRxBuffers(rx_vmo_state_.estimator->CalculateTargetBuffers())) {
    RequestRxSpace();
  }
  MaybeReleaseCandidateVmo();
  rx_vmo_state_.current_max_packet_rate = 0;
  if (zx_status_t status = rx_vmo_state_.estimator->RearmTimer(rx_watch_port_, kTimerWatchKey);
      status != ZX_OK) {
    rx_vmo_state_.estimator.reset();
    rx_vmo_state_.target_available_buffers = rx_vmo_state_.min_buffers =
        rx_vmo_state_.total_buffers;
    RequestRxSpace();
  }
}

bool RxQueue::SetTargetRxBuffers(uint16_t target) __TA_REQUIRES(parent_->rx_lock()) {
  uint16_t old_target = rx_vmo_state_.target_available_buffers;
  rx_vmo_state_.target_available_buffers =
      std::min(std::max(rx_vmo_state_.min_buffers, target), rx_vmo_state_.total_buffers);

  return rx_vmo_state_.target_available_buffers > old_target;
}

bool RxQueue::SetTargetRxBuffersIfMore(uint16_t target) __TA_REQUIRES(parent_->rx_lock()) {
  if (target > rx_vmo_state_.target_available_buffers) {
    return SetTargetRxBuffers(target);
  }
  return false;
}

void RxQueue::RequestRxSpace(
    fit::callback<void(fit::result<std::tuple<zx_status_t, DataVmoList>>)> cb) {
  if (rx_vmo_state_.vmos.is_empty() || !session_) {
    return;
  }

  if (rx_vmo_state_.pending_op.type != VmoOperation::Type::kNone) {
    return;
  }

  if (rx_vmo_state_.last_in_use.IsValid() &&
      rx_vmo_state_.target_available_buffers >
          rx_vmo_state_.current_available_buffers - rx_vmo_state_.last_in_use->num_rx_buffers &&
      rx_vmo_state_.last_in_use->withheld_rx_buffers) {
    LOGF_TRACE("RxQueue: there are %d inflight buffers being withheld, making them available",
               rx_vmo_state_.last_in_use->withheld_rx_buffers);
    ReleaseWithheldBuffers(rx_vmo_state_.last_in_use);
  }

  uint16_t total_available_buffers = rx_vmo_state_.current_available_buffers;
  auto start_it = rx_vmo_state_.last_in_use.IsValid() ? std::next(rx_vmo_state_.last_in_use)
                                                      : rx_vmo_state_.vmos.begin();
  auto end_it = start_it;
  while (total_available_buffers < rx_vmo_state_.target_available_buffers &&
         end_it != rx_vmo_state_.vmos.end()) {
    total_available_buffers = total_available_buffers + end_it->num_rx_buffers;
    ++end_it;
  }
  if (end_it != start_it) {
    rx_vmo_state_.pending_op.type = VmoOperation::Type::kPrepareBatch;
    rx_vmo_state_.pending_op.first_vmo_to_not_prepare = end_it;
    if (parent_->PrepareRxVmos(start_it, &rx_vmo_state_.pending_op.first_vmo_to_not_prepare,
                               std::move(cb))) {
      VmoOpFinished();
    } else if (rx_vmo_state_.pending_op.first_vmo_to_not_prepare == start_it) {
      rx_vmo_state_.pending_op = VmoOperation{};
    }
    return;
  }
}

void RxQueue::VmoOpFinished(zx_status_t status) {
  if (status != ZX_OK) {
    LOGF_WARN("RxQueue: VmoOpFinished failed with status %s", zx_status_get_string(status));
    rx_vmo_state_.pending_op = VmoOperation{};
    session_->Kill();
    return;
  }
  if (!session_) {
    rx_vmo_state_.pending_op = VmoOperation{};
    return;
  }
  session_->AssertParentRxLock(*parent_);
  if (!session_->IsRxValid()) {
    rx_vmo_state_.pending_op = VmoOperation{};
    return;
  }
  switch (rx_vmo_state_.pending_op.type) {
    case VmoOperation::Type::kNone:
      break;
    case VmoOperation::Type::kPrepareBatch: {
      auto start_it = rx_vmo_state_.last_in_use.IsValid() ? std::next(rx_vmo_state_.last_in_use)
                                                          : rx_vmo_state_.vmos.begin();
      for (auto it = start_it; it != rx_vmo_state_.pending_op.first_vmo_to_not_prepare; it++) {
        LOGF_TRACE("RxQueue: Rx VMO %u prepared successfully", it->id);
        rx_vmo_state_.current_available_buffers += it->num_rx_buffers;
        ReleaseWithheldBuffers(it);
      }
      rx_vmo_state_.last_in_use = std::prev(rx_vmo_state_.pending_op.first_vmo_to_not_prepare);
      break;
    }
    case VmoOperation::Type::kReleaseSingle: {
      ZX_ASSERT(rx_vmo_state_.last_in_use->withheld_rx_buffers ==
                rx_vmo_state_.last_in_use->num_rx_buffers);
      LOGF_TRACE("RxQueue: Rx VMO %u released successfully", rx_vmo_state_.last_in_use->id);
      parent_->DecommitVmo(rx_vmo_state_.last_in_use->id);
      ZX_ASSERT(rx_vmo_state_.current_available_buffers >=
                rx_vmo_state_.last_in_use->num_rx_buffers);
      rx_vmo_state_.current_available_buffers -= rx_vmo_state_.last_in_use->num_rx_buffers;
      --rx_vmo_state_.last_in_use;
      break;
    }
  }
  rx_vmo_state_.pending_op = VmoOperation{};
  // Target may have changed during the pending operation.
  RequestRxSpace();
  TriggerRxWatch();
}

uint32_t RxQueue::SessionTransaction::remaining() __TA_REQUIRES(queue_->parent_->rx_lock()) {
  // NB: __TA_REQUIRES here is just encoding that a SessionTransaction always holds a lock for
  // its parent queue, the protection from misuse comes from the annotations on
  // `SessionTransaction`'s constructor and destructor.
  return queue_->in_flight_->available();
}

bool RxQueue::SessionTransaction::Push(uint16_t descriptor)
    __TA_REQUIRES(queue_->parent_->rx_lock()) {
  const buffer_descriptor_t* desc = queue_->session_->checked_descriptor(descriptor);
  if (!desc) {
    LOGF_ERROR("unknown descriptor from session: %d", descriptor);
    queue_->session_->Kill();
    return false;
  }
  // NB: __TA_REQUIRES here is just encoding that a SessionTransaction always holds a lock for
  // its parent queue, the protection from misuse comes from the annotations on
  // `SessionTransaction`'s constructor and destructor.
  uint32_t idx = queue_->in_flight_->Push(InFlightBuffer(descriptor));

  netdev::VmoId vmo_id = desc->vmo_id;
  // If the VMO is before the last Rx VMO in use, then it must not be in the process of
  // release, make it available.
  if (queue_->rx_vmo_state_.last_in_use.IsValid() &&
      vmo_id < queue_->rx_vmo_state_.last_in_use->id) {
    queue_->available_queue_->Push(idx);
    return true;
  }

  // If the VMO is the last Rx VMO in use:
  if (queue_->rx_vmo_state_.last_in_use.IsValid() &&
      vmo_id == queue_->rx_vmo_state_.last_in_use->id) {
    // If this VMO is registered for tx, we cannot release this VMO, so might as well
    // just use the full VMO.
    if (queue_->rx_vmo_state_.last_in_use->tx_registered) {
      queue_->ReleaseWithheldBuffers(queue_->rx_vmo_state_.last_in_use);
      queue_->available_queue_->Push(idx);
      return true;
    }
    // Otherwise, we check if the target buffer count can be achieved without this VMO,
    // if possible, we have to make this buffer available immediately.
    if (queue_->rx_vmo_state_.target_available_buffers >
        queue_->rx_vmo_state_.current_available_buffers -
            queue_->rx_vmo_state_.last_in_use->num_rx_buffers) {
      queue_->ReleaseWithheldBuffers(queue_->rx_vmo_state_.last_in_use);
      queue_->available_queue_->Push(idx);
      return true;
    }
  }

  // Finally, we can't make the buffer available because of either of the reasons:
  // 1) This VMO is not prepared, so we need to withhold it until the VMO is prepared.
  // 2) This VMO is the last in use and eligible for release. We need to withhold these
  //    buffers so that the release operation can make progress.
  auto begin = queue_->rx_vmo_state_.last_in_use.IsValid() ? queue_->rx_vmo_state_.last_in_use
                                                           : queue_->rx_vmo_state_.vmos.begin();
  auto it = std::find_if(begin, queue_->rx_vmo_state_.vmos.end(),
                         [vmo_id](DataVmoMeta& meta) { return meta.id == vmo_id; });
  if (it == queue_->rx_vmo_state_.vmos.end()) {
    LOGF_ERROR("unknown vmo id from session: %d", vmo_id);
    queue_->session_->Kill();
    return false;
  }
  it->withheld_rx_buffers++;
  queue_->in_flight_->Get(idx).next_withheld_inflight_buffer = it->head_withheld_rx_buffers;
  it->head_withheld_rx_buffers = idx;
  return false;
}

}  // namespace network::internal
