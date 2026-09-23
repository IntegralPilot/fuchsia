// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "dispatcher.h"

#include <lib/async/receiver.h>
#include <lib/async/sequence_id.h>
#include <lib/fit/defer.h>
#include <lib/trace/event.h>
#include <lib/zx/clock.h>

#include "dispatcher_coordinator.h"
#include "dispatcher_internals.h"
#include "src/devices/lib/log/log.h"
#include "thread_pool.h"

namespace driver_runtime {

namespace {

const async_ops_t g_dispatcher_ops = {
    .version = ASYNC_OPS_V4,
    .reserved = 0,
    .v1 = {
        .now =
            [](async_dispatcher_t* dispatcher) {
              return static_cast<Dispatcher*>(dispatcher)->GetTime();
            },
        .begin_wait =
            [](async_dispatcher_t* dispatcher, async_wait_t* wait) {
              return static_cast<Dispatcher*>(dispatcher)->BeginWait(wait);
            },
        .cancel_wait =
            [](async_dispatcher_t* dispatcher, async_wait_t* wait) {
              return static_cast<Dispatcher*>(dispatcher)->CancelWait(wait);
            },
        .post_task =
            [](async_dispatcher_t* dispatcher, async_task_t* task) {
              return static_cast<Dispatcher*>(dispatcher)->PostTask(task);
            },
        .cancel_task =
            [](async_dispatcher_t* dispatcher, async_task_t* task) {
              return static_cast<Dispatcher*>(dispatcher)->CancelTask(task);
            },
        .queue_packet =
            [](async_dispatcher_t* dispatcher, async_receiver_t* receiver,
               const zx_packet_user_t* data) {
              return static_cast<Dispatcher*>(dispatcher)->QueuePacket(receiver, data);
            },
        .set_guest_bell_trap = [](async_dispatcher_t* dispatcher, async_guest_bell_trap_t* trap,
                                  zx_handle_t guest, zx_vaddr_t addr,
                                  size_t length) { return ZX_ERR_NOT_SUPPORTED; },
    },
    .v2 = {
        .bind_irq =
            [](async_dispatcher_t* dispatcher, async_irq_t* irq) {
              return static_cast<Dispatcher*>(dispatcher)->BindIrq(irq);
            },
        .unbind_irq =
            [](async_dispatcher_t* dispatcher, async_irq_t* irq) {
              return static_cast<Dispatcher*>(dispatcher)->UnbindIrq(irq);
            },
        .create_paged_vmo = [](async_dispatcher_t* dispatcher, async_paged_vmo_t* paged_vmo,
                               uint32_t options, zx_handle_t pager, uint64_t vmo_size,
                               zx_handle_t* vmo_out) { return ZX_ERR_NOT_SUPPORTED; },
        .detach_paged_vmo = [](async_dispatcher_t* dispatcher,
                               async_paged_vmo_t* paged_vmo) { return ZX_ERR_NOT_SUPPORTED; },
    },
    .v3 = {
        .get_sequence_id =
            [](async_dispatcher_t* dispatcher, async_sequence_id_t* out_sequence_id,
               const char** out_error) {
              return static_cast<Dispatcher*>(dispatcher)
                  ->GetSequenceId(out_sequence_id, out_error);
            },
        .check_sequence_id =
            [](async_dispatcher_t* dispatcher, async_sequence_id_t sequence_id,
               const char** out_error) {
              return static_cast<Dispatcher*>(dispatcher)->CheckSequenceId(sequence_id, out_error);
            },
    },
    .v4 = {
        .acquire_shared_ref =
            [](async_dispatcher_t* dispatcher) {
              static_cast<Dispatcher*>(dispatcher)->AddRef();
              return ZX_OK;
            },
        .release_shared_ref =
            [](async_dispatcher_t* dispatcher) {
              // adopt a reference and let its destructor release it.
              fbl::ImportFromRawPtr(static_cast<Dispatcher*>(dispatcher));
              return ZX_OK;
            },
    },
};

}  // namespace

extern const async_ops_t g_veneer_ops = {
    .version = ASYNC_OPS_V4,
    .reserved = 0,
    .v1 = {
        .now =
            [](async_dispatcher_t* dispatcher) {
              auto veneer = reinterpret_cast<Dispatcher::Veneer*>(dispatcher);
              return veneer->dispatcher->GetTime();
            },
        .begin_wait =
            [](async_dispatcher_t* dispatcher, async_wait_t* wait) {
              auto veneer = reinterpret_cast<Dispatcher::Veneer*>(dispatcher);
              return veneer->dispatcher->BeginWait(wait, true /* is_always_on */);
            },
        .cancel_wait =
            [](async_dispatcher_t* dispatcher, async_wait_t* wait) {
              auto veneer = reinterpret_cast<Dispatcher::Veneer*>(dispatcher);
              return veneer->dispatcher->CancelWait(wait);
            },
        .post_task =
            [](async_dispatcher_t* dispatcher, async_task_t* task) {
              auto veneer = reinterpret_cast<Dispatcher::Veneer*>(dispatcher);
              return veneer->dispatcher->PostTask(task, true /* is_always_on */);
            },
        .cancel_task =
            [](async_dispatcher_t* dispatcher, async_task_t* task) {
              auto veneer = reinterpret_cast<Dispatcher::Veneer*>(dispatcher);
              return veneer->dispatcher->CancelTask(task);
            },
        .queue_packet =
            [](async_dispatcher_t* dispatcher, async_receiver_t* receiver,
               const zx_packet_user_t* data) {
              auto veneer = reinterpret_cast<Dispatcher::Veneer*>(dispatcher);
              return veneer->dispatcher->QueuePacket(receiver, data);
            },
        .set_guest_bell_trap = [](async_dispatcher_t* dispatcher, async_guest_bell_trap_t* trap,
                                  zx_handle_t guest, zx_vaddr_t addr,
                                  size_t length) { return ZX_ERR_NOT_SUPPORTED; },
    },
    .v2 = {
        .bind_irq =
            [](async_dispatcher_t* dispatcher, async_irq_t* irq) {
              auto veneer = reinterpret_cast<Dispatcher::Veneer*>(dispatcher);
              return veneer->dispatcher->BindIrq(irq);
            },
        .unbind_irq =
            [](async_dispatcher_t* dispatcher, async_irq_t* irq) {
              auto veneer = reinterpret_cast<Dispatcher::Veneer*>(dispatcher);
              return veneer->dispatcher->UnbindIrq(irq);
            },
        .create_paged_vmo = [](async_dispatcher_t* dispatcher, async_paged_vmo_t* paged_vmo,
                               uint32_t options, zx_handle_t pager, uint64_t vmo_size,
                               zx_handle_t* vmo_out) { return ZX_ERR_NOT_SUPPORTED; },
        .detach_paged_vmo = [](async_dispatcher_t* dispatcher,
                               async_paged_vmo_t* paged_vmo) { return ZX_ERR_NOT_SUPPORTED; },
    },
    .v3 = {
        .get_sequence_id =
            [](async_dispatcher_t* dispatcher, async_sequence_id_t* out_sequence_id,
               const char** out_error) {
              auto veneer = reinterpret_cast<Dispatcher::Veneer*>(dispatcher);
              return veneer->dispatcher->GetSequenceId(out_sequence_id, out_error);
            },
        .check_sequence_id =
            [](async_dispatcher_t* dispatcher, async_sequence_id_t sequence_id,
               const char** out_error) {
              auto veneer = reinterpret_cast<Dispatcher::Veneer*>(dispatcher);
              return veneer->dispatcher->CheckSequenceId(sequence_id, out_error);
            },
    },
    .v4 = {
        .acquire_shared_ref =
            [](async_dispatcher_t* dispatcher) {
              auto veneer = reinterpret_cast<Dispatcher::Veneer*>(dispatcher);
              veneer->dispatcher->AddRef();
              return ZX_OK;
            },
        .release_shared_ref =
            [](async_dispatcher_t* dispatcher) {
              auto veneer = reinterpret_cast<Dispatcher::Veneer*>(dispatcher);
              fbl::ImportFromRawPtr(veneer->dispatcher);
              return ZX_OK;
            },
    },
};

Dispatcher::Dispatcher(uint32_t options, std::string_view name, bool unsynchronized,
                       bool allow_sync_calls, const void* owner,
                       fdf_dispatcher_shutdown_observer_t* observer)
    : DispatcherInterface{&g_dispatcher_ops},
      options_(options),
      unsynchronized_(unsynchronized),
      allow_sync_calls_(allow_sync_calls),
      owner_(owner),
      thread_pool_(nullptr),
      process_shared_dispatcher_(nullptr),
      timer_(this),
      shutdown_observer_(observer),
      veneer_{{&g_veneer_ops}, this} {
  name_.Append(name);
}

// static
zx_status_t Dispatcher::Create(uint32_t options, std::string_view name,
                               std::string_view scheduler_role,
                               fdf_dispatcher_shutdown_observer_t* observer,
                               Dispatcher** out_dispatcher) {
  ZX_DEBUG_ASSERT(out_dispatcher);

  const void* owner = thread_context::GetCurrentDriver();
  if (!owner) {
    return ZX_ERR_INVALID_ARGS;
  }

  bool unsynchronized = options & FDF_DISPATCHER_OPTION_UNSYNCHRONIZED;
  bool allow_sync_calls = options & FDF_DISPATCHER_OPTION_ALLOW_SYNC_CALLS;

  auto dispatcher = fbl::MakeRefCounted<Dispatcher>(options, name, unsynchronized, allow_sync_calls,
                                                    owner, observer);

  zx::event event;
  if (zx_status_t status = zx::event::create(0, &event); status != ZX_OK) {
    return status;
  }

  auto self = dispatcher.get();
  auto event_waiter = std::make_unique<EventWaiter>(
      std::move(event),
      [self](std::unique_ptr<EventWaiter> event_waiter, fbl::RefPtr<Dispatcher> dispatcher_ref) {
        auto ref = dispatcher_ref;
        self->DispatchCallbacks(std::move(event_waiter), std::move(dispatcher_ref));
        ref->thread_pool()->OnThreadWakeup();
      });

  zx_status_t status =
      GetDispatcherCoordinator().AddDispatcher(dispatcher, scheduler_role, std::move(event_waiter));
  if (status != ZX_OK) {
    return status;
  }

  // This reference will be recovered in |Destroy|.
  *out_dispatcher = fbl::ExportToRawPtr(&dispatcher);
  return ZX_OK;
}

Dispatcher::~Dispatcher() {}

zx_status_t Dispatcher::CreateUnmanagedDispatcher(
    uint32_t options, std::string_view name, fdf_dispatcher_shutdown_observer_t* shutdown_observer,
    Dispatcher** out_dispatcher) {
  ZX_DEBUG_ASSERT(out_dispatcher);

  const void* owner = thread_context::GetCurrentDriver();
  if (!owner) {
    return ZX_ERR_INVALID_ARGS;
  }

  bool unsynchronized = options & FDF_DISPATCHER_OPTION_UNSYNCHRONIZED;
  bool allow_sync_calls = options & FDF_DISPATCHER_OPTION_ALLOW_SYNC_CALLS;

  auto dispatcher = fbl::MakeRefCounted<Dispatcher>(options, name, unsynchronized, allow_sync_calls,
                                                    owner, shutdown_observer);

  zx::event event;
  if (zx_status_t status = zx::event::create(0, &event); status != ZX_OK) {
    return status;
  }

  auto self = dispatcher.get();
  auto event_waiter = std::make_unique<EventWaiter>(
      std::move(event),
      [self](std::unique_ptr<EventWaiter> event_waiter, fbl::RefPtr<Dispatcher> dispatcher_ref) {
        auto ref = dispatcher_ref;
        self->DispatchCallbacks(std::move(event_waiter), std::move(dispatcher_ref));
        ref->thread_pool()->OnThreadWakeup();
      });

  zx_status_t status =
      GetDispatcherCoordinator().AddUnmanagedDispatcher(dispatcher, std::move(event_waiter));
  if (status != ZX_OK) {
    return status;
  }

  // This reference will be recovered in |Destroy|.
  *out_dispatcher = fbl::ExportToRawPtr(&dispatcher);
  return ZX_OK;
}

void Dispatcher::ShutdownAsync() {
  {
    fbl::AutoLock lock(&callback_lock_);

    switch (state_) {
      case DispatcherState::kRunning:
        state_ = DispatcherState::kShuttingDown;
        break;
      case DispatcherState::kShuttingDown:
      case DispatcherState::kShutdown:
      case DispatcherState::kDestroyed:
        return;
      default:
        ZX_ASSERT_MSG(false, "Dispatcher::ShutdownAsync got unknown dispatcher state %d",
                      static_cast<int>(state_));
    }

    // Move the requests into a separate queue so we will be able to enter an idle state.
    // This queue will be processed by |CompleteShutdown|.
    shutdown_queue_ = std::move(callback_queue_);
    shutdown_queue_.splice(shutdown_queue_.end(), registered_callbacks_);

    // Try to cancel all outstanding waits. Successfully canceled waits should be have their
    // callbacks triggered.
    auto waits = std::move(waits_);
    for (auto wait = waits.pop_front(); wait; wait = waits.pop_front()) {
      // It's possible that the wait has already been cancelled but not yet pulled
      // from the |waits_| list, in which case the user may have already freed
      // the handle they were waiting on, so we should not try to cancel it again.
      if (!wait->is_pending_cancellation() && wait->Cancel()) {
        // We were successful. Lets queue this up to be processed by |CompleteDestroy|.
        shutdown_queue_.push_back(std::move(wait));
      } else {
        // We weren't successful, |wait| is being run or queued to run and will want to remove this
        // from the |waits_| list.
        waits_.push_back(std::move(wait));
      }
    }

    // It's easier to handle |irqs_| in |CompleteShutdown|, so unbinding will only
    // ever happen on a thread at once. If the irq gets triggered in the meanwhile,
    // |QueueIrq| will return early.

    zx_status_t status = timer_.Cancel();
    // If we could not cancel the timer, it is going to run / is already running in another
    // thread, and we don't want |CompleteShutdown| to run until after that completes.
    if (status != ZX_OK) {
      shutdown_waiting_for_timer_ = true;
    }
    shutdown_queue_.splice(shutdown_queue_.end(), delayed_tasks_);
    shutdown_queue_.splice(shutdown_queue_.end(), sleep_queue_);
    shutdown_queue_.splice(shutdown_queue_.end(), wake_queue_);
    shutdown_queue_.splice(shutdown_queue_.end(), always_on_delayed_tasks_);

    // To avoid race conditions with attempting to cancel a wait that might be scheduled to
    // run, we will cancel the event waiter in the |CompleteShutdown| callback. This is as
    // |async::Wait::Cancel| is not thread safe.
  }

  auto dispatcher_ref = fbl::RefPtr<Dispatcher>(this);

  // The dispatcher shutdown API specifies that on shutdown, tasks and cancellation
  // callbacks should run serialized. Wait for all active threads to
  // complete before calling the cancellation callbacks.
  auto event = RegisterForCompleteShutdownEvent();
  ZX_ASSERT(event.status_value() == ZX_OK);

  // Don't use async::WaitOnce as it sets the handler in a thread unsafe way.
  auto wait = std::make_unique<async::Wait>(
      event->get(), ZX_EVENT_SIGNALED, 0,
      [dispatcher_ref = std::move(dispatcher_ref), event = std::move(*event)](
          async_dispatcher_t* dispatcher, async::Wait* wait, zx_status_t status,
          const zx_packet_signal_t* signal) mutable {
        ZX_ASSERT(status == ZX_OK || status == ZX_ERR_CANCELED);
        dispatcher_ref->CompleteShutdown();
        delete wait;
      });
  ZX_ASSERT(wait->Begin(process_shared_dispatcher_) == ZX_OK);
  wait.release();  // This will be deleted by the wait handler once it is called.
}

void Dispatcher::CompleteShutdown() {
  fbl::DoublyLinkedList<std::unique_ptr<AsyncIrq>> unbound_irqs;
  std::unordered_set<fdf_token_t*> registered_tokens;
  {
    fbl::AutoLock lock(&callback_lock_);

    ZX_ASSERT(state_ == DispatcherState::kShuttingDown);

    ZX_ASSERT_MSG(num_active_threads_ == 0, "CompleteShutdown called but there are active threads");
    ZX_ASSERT_MSG(callback_queue_.is_empty(),
                  "CompleteShutdown called but callback queue has %lu items",
                  callback_queue_.size_slow());
    ZX_ASSERT_MSG(sleep_queue_.is_empty(), "CompleteShutdown called but sleep queue not empty");
    ZX_ASSERT_MSG(wake_queue_.is_empty(), "CompleteShutdown called but wake queue not empty");
    ZX_ASSERT_MSG(always_on_delayed_tasks_.is_empty(),
                  "CompleteShutdown called but always on delayed tasks not empty");
    ZX_ASSERT_MSG((!event_waiter_ || !event_waiter_->signaled()),
                  "CompleteShutdown called but event waiter is still signaled");
    ZX_ASSERT(IsIdleLocked());

    ZX_ASSERT_MSG(!HasFutureOpsScheduledLocked(),
                  "CompleteShutdown called but future ops are scheduled");

    if (event_waiter_) {
      // Since the event waiter holds a reference to the dispatcher,
      // we need to cancel it to reclaim it.
      // This should always succeed, as there should be no other threads processing
      // tasks for this dispatcher, and we should have cleared |event_waiter_| if
      // the AsyncLoopOwned event waiter was dropped.
      ZX_ASSERT(event_waiter_->Cancel() != nullptr);
      event_waiter_ = nullptr;
    }

    unbound_irqs = std::move(irqs_);
    for (auto& irq : unbound_irqs) {
      // It's possible that a callback request may be queued for a triggered irq.
      // We should only queue an additional cancellation callback if one does not
      // already exist.
      auto iter =
          shutdown_queue_.find_if([operation = &irq](const CallbackRequest& callback_request) {
            return callback_request.holds_async_operation(operation);
          });
      if (iter == shutdown_queue_.end()) {
        auto callback_request = irq.CreateCallbackRequest(*this, true /* locked */);
        shutdown_queue_.push_back(std::move(callback_request));
      }
      // If the irq is still in the list, unbinding shouldn't fail.
      // The only case would be if the async loop is also shutting down,
      // but we shouldn't do that before all the driver dispatchers have completed shutdown.
      ZX_ASSERT_MSG(irq.Unbind(), "Dispatcher::ShutdownAsync failed to unbind irq");
    }
    registered_tokens = std::move(registered_tokens_);
  }

  for (auto irq = unbound_irqs.pop_front(); irq; irq = unbound_irqs.pop_front()) {
    // Though the irq has been unbound, it's possible that another |process_shared_dispatcher|
    // thread has already pulled an irq packet from the port and may attempt to call the irq
    // handler. Delay destroying our irq wrapper for a bit in case this race condition happens.
    thread_pool_->CacheUnboundIrq(std::move(irq));
  }

  // We want |fdf_dispatcher_get_current_dispatcher| to work in cancellation and shutdown callbacks.
  thread_context::PushDriver(owner_, this);
  auto pop_driver = fit::defer([]() { thread_context::PopDriver(); });

  // We remove one item at a time from the shutdown queue, in case someone
  // tries to cancel a wait (which has not been canceled yet) from within a
  // canceled callback. We don't use fbl::AutoLock as we want to be able to
  // release and re-acquire the lock in the loop.
  callback_lock_.Acquire();
  while (!shutdown_queue_.is_empty()) {
    auto callback_request = shutdown_queue_.pop_front();
    ZX_ASSERT(callback_request);
    // Call the callbacks outside the lock.
    callback_lock_.Release();
    callback_request->Call(std::move(callback_request), ZX_ERR_CANCELED);
    callback_lock_.Acquire();
  }
  callback_lock_.Release();

  for (auto token : registered_tokens) {
    token->handler(this->to_fdf_dispatcher(), token, ZX_ERR_CANCELED, FDF_HANDLE_INVALID);
  }

  fdf_dispatcher_shutdown_observer_t* shutdown_observer = nullptr;
  {
    fbl::AutoLock lock(&callback_lock_);
    state_ = DispatcherState::kShutdown;
    shutdown_observer = shutdown_observer_;
  }
  GetDispatcherCoordinator().NotifyDispatcherShutdown(*this, std::move(shutdown_observer));
}

void Dispatcher::Destroy(bool user_initiated) {
  {
    fbl::AutoLock lock(&callback_lock_);
    if (state_ != DispatcherState::kShutdown) {
      LOGF(ERROR,
           "Destroying dispatcher which has not completed shutdown, logging dispatcher dump:");
      std::vector<std::string> dump;
      DumpToStringLocked(&dump);
      for (auto& str : dump) {
        LOGF(ERROR, "%s", str.c_str());
      }
    }
    if (state_ == DispatcherState::kDestroyed) {
      return;
    }
    ZX_ASSERT(state_ == DispatcherState::kShutdown);
    state_ = DispatcherState::kDestroyed;

    auto dispatcher_context = thread_context::GetCurrentDispatcher();
    // Construct a new string in case the calling dispatcher is destroyed
    // before we happen to next log the dump state.
    dispatcher_destroy_context_ =
        dispatcher_context ? std::string(dispatcher_context->name_.c_str()) : "unknown";
    dispatcher_destroy_user_initiated_ = user_initiated;
  }
  // Recover the reference created in |CreateWithAdder|.
  auto dispatcher_ref = fbl::ImportFromRawPtr(this);
  GetDispatcherCoordinator().RemoveDispatcher(*this);
}

zx_status_t Dispatcher::Seal(uint32_t option) {
  if (option != FDF_DISPATCHER_OPTION_ALLOW_SYNC_CALLS) {
    return ZX_ERR_INVALID_ARGS;
  }

  {
    fbl::AutoLock lock(&callback_lock_);

    if (thread_context::GetCurrentDispatcher() != this || !IsRunningLocked()) {
      return ZX_ERR_BAD_STATE;
    }

    if (!allow_sync_calls_) {
      return ZX_ERR_ALREADY_EXISTS;
    }

    // Set our field.
    allow_sync_calls_.store(false);
  }

  // Tell our thread pool to remove a thread as we no longer have allow_sync_calls_, which caused
  // an extra thread to be added when this dispatcher was initially created.
  return thread_pool()->OnDispatcherSealed();
}

// async_dispatcher_t implementation

zx_time_t Dispatcher::GetTime() { return zx_clock_get_monotonic(); }

zx_status_t Dispatcher::BeginWait(async_wait_t* wait, bool is_always_on) {
  fbl::AutoLock lock(&callback_lock_);
  if (!IsRunningLocked()) {
    return ZX_ERR_BAD_STATE;
  }
  // TODO(92740): we should do something more efficient rather than creating a new
  // AsyncWait each time.
  auto async_wait = std::make_unique<AsyncWait>(wait, *this);
  async_wait->set_handle(wait->object);
  async_wait->set_signals(wait->trigger);

  if (IsWakeVectorLocked(wait->object, wait->trigger)) {
    async_wait->set_request_type(CallbackRequest::RequestType::kWakeWait);
  } else if (is_always_on) {
    async_wait->set_request_type(CallbackRequest::RequestType::kAlwaysOnWait);
  }

  return AsyncWait::BeginWait(std::move(async_wait), *this);
}

zx_status_t Dispatcher::CancelWait(async_wait_t* wait) {
  // The implementation of this method has to be more complicated than simply
  //
  //   return async_cancel_wait(wait);
  //
  // because the dispatcher wraps the wait's callback with its own custom callback,
  // |OnSignal|, so there is an interval between the wait being pulled off the port and the wait's
  // callback being invoked, during which we need to implement custom logic to cancel the wait.

  // First, try to cancel the async wait from the shared dispatcher.
  auto* async_wait = reinterpret_cast<AsyncWait*>(wait->state.reserved[0]);
  if (async_wait != nullptr) {
    if (async_wait->Cancel()) {
      // We shouldn't have to worry about racing anyone if cancelation was successful.
      ZX_ASSERT(RemoveWait(async_wait) != nullptr);
      return ZX_OK;
    }

    // async_wait->Cancel() will fail in the case that the wait has already been pulled off the
    // port.
  }

  // Second, try to cancel it from the callback queue.
  fbl::AutoLock lock(&callback_lock_);
  auto callback_request = CancelAsyncOperationLocked(wait);
  if (callback_request) {
    return ZX_OK;
  } else if (unsynchronized()) {
    return ZX_ERR_NOT_FOUND;
  } else {
    // The async_wait is set to null right before the callback is invoked, so if it is null it's too
    // late to cancel. If the caller of |CancelWait| is not a dispatcher-managed thread then we
    // can't guarantee the dispatcher isn't currently invoking the callback.
    if (async_wait == nullptr || thread_context::GetCurrentDispatcher() != this) {
      return ZX_ERR_NOT_FOUND;
    }

    // If we failed to cancel it from the callback queue and we are a synchronized dispatcher,
    // then another thread must have pulled the packet from the port and is about to queue the
    // callback (i.e., it is sitting in |OnSignal| right before |QueueWait|). We mark the wait as
    // pending cancellation so that it is cancelled rather than invoked when |QueueWait| is called.
    async_wait->MarkPendingCancellation();
    return ZX_OK;
  }
}

zx::time Dispatcher::GetNextTimeoutLocked() const {
  // Check delayed tasks only when callback_queue_ is empty. We will routinely check if delayed
  // tasks can be moved into the callback queue anyways and reset the timer whenever callback queue
  // is empty.
  if (callback_queue_.is_empty()) {
    zx::time next_regular = zx::time::infinite();
    if (!delayed_tasks_.is_empty()) {
      next_regular = static_cast<const DelayedTask*>(&delayed_tasks_.front())->deadline;
    }

    zx::time next_always_on = zx::time::infinite();
    if (!always_on_delayed_tasks_.is_empty()) {
      next_always_on = static_cast<const DelayedTask*>(&always_on_delayed_tasks_.front())->deadline;
    }

    if (suspend_state_ == SuspendState::kSuspended) {
      return next_always_on;  // Ignore regular tasks when suspended!
    }

    return std::min(next_regular, next_always_on);
  }
  return zx::time::infinite();
}

void Dispatcher::ResetTimerLocked(bool force) {
  zx::time deadline = GetNextTimeoutLocked();
  if (deadline == zx::time::infinite()) {
    // Nothing is left on the queue to fire.
    timer_.Cancel();
    return;
  }

  // The tradeoff of using a task instead of a dedicated timer is that we need to cancel the task
  // every time a task with a shorter deadline comes in. This isn't really too bad, assuming there
  // is at least two delayed tasks scheduled, otherwise the timer will be canceled. If we used a
  // custom implementation for our shared process loop, then we could also have an
  // "UpdateTaskDeadline" method on tasks which would allow us to shift the deadline as necessary,
  // without risking the need to cancel the task.

  if ((force || timer_.current_deadline() > deadline) && timer_.Cancel() == ZX_OK) {
    timer_.BeginWait(deadline);
  }
}

bool Dispatcher::IsWakeVectorLocked(zx_handle_t handle, zx_signals_t signals) const {
  auto iter = wake_vectors_.find(handle);
  if (iter != wake_vectors_.end()) {
    zx_signals_t wv_signals = iter->second;
    if ((wv_signals & signals) || wv_signals == 0 || signals == 0) {
      return true;
    }
  }
  return false;
}

void Dispatcher::InsertDelayedTaskSortedLocked(std::unique_ptr<DelayedTask> task) {
  auto& queue = (task->request_type() == CallbackRequest::RequestType::kAlwaysOnTask)
                    ? always_on_delayed_tasks_
                    : delayed_tasks_;

  // Find the first node that is bigger and insert before it. fbl::DoublyLinkedList handles all of
  // the edge cases for us.
  auto iter = queue.find_if([&](const CallbackRequest& other) {
    return static_cast<const DelayedTask*>(&other)->deadline > task->deadline;
  });
  queue.insert(iter, std::move(task));
}

void Dispatcher::CheckDelayedTasksLocked() {
  if (!IsRunningLocked()) {
    IdleCheckLocked();
    return;
  }
  zx::time now = zx::clock::get_monotonic();
  bool added_tasks = false;

  // Always-on tasks
  auto iter = always_on_delayed_tasks_.find_if([&](const CallbackRequest& task) {
    return static_cast<const DelayedTask*>(&task)->deadline > now;
  });
  if (iter != always_on_delayed_tasks_.begin()) {
    fbl::DoublyLinkedList<std::unique_ptr<CallbackRequest>> done_tasks;
    done_tasks = always_on_delayed_tasks_.split_after(--iter);
    std::swap(always_on_delayed_tasks_, done_tasks);
    callback_queue_.splice(callback_queue_.end(), done_tasks);
    added_tasks = true;
  }

  // Regular tasks
  if (suspend_state_ != SuspendState::kSuspended) {
    iter = delayed_tasks_.find_if([&](const CallbackRequest& task) {
      return static_cast<const DelayedTask*>(&task)->deadline > now;
    });
    if (iter != delayed_tasks_.begin()) {
      fbl::DoublyLinkedList<std::unique_ptr<CallbackRequest>> done_tasks;
      done_tasks = delayed_tasks_.split_after(--iter);
      std::swap(delayed_tasks_, done_tasks);
      callback_queue_.splice(callback_queue_.end(), done_tasks);
      added_tasks = true;
    }
  }

  if (added_tasks) {
    if (event_waiter_ && !event_waiter_->signaled()) {
      event_waiter_->signal();
    }
  } else {
    ResetTimerLocked();
  }
}

void Dispatcher::Timer::Handler() {
  {
    fbl::AutoLock al(&dispatcher_->callback_lock_);
    current_deadline_ = zx::time::infinite();
    dispatcher_->CheckDelayedTasksLocked();
  }
  dispatcher_->thread_pool()->OnThreadWakeup();
  {
    fbl::AutoLock lock(&dispatcher_->callback_lock_);
    // Check if the dispatcher is shutting down and waiting for the handler to complete.
    if (!dispatcher_->IsRunningLocked()) {
      dispatcher_->shutdown_waiting_for_timer_ = false;
      dispatcher_->IdleCheckLocked();
    }
  }
}

zx_status_t Dispatcher::PostTask(async_task_t* task, bool is_always_on) {
  driver_runtime::Callback callback =
      [this, task](std::unique_ptr<driver_runtime::CallbackRequest> callback_request,
                   zx_status_t status) { task->handler(this, task, status); };

  const zx::time now = zx::clock::get_monotonic();
  if (zx::time(task->deadline) <= now) {
    // TODO(92740): we should do something more efficient rather than creating a new
    // callback request each time.
    auto callback_request = std::make_unique<driver_runtime::CallbackRequest>(
        is_always_on ? CallbackRequest::RequestType::kAlwaysOnTask
                     : CallbackRequest::RequestType::kTask);
    callback_request->SetCallback(this, std::move(callback), task);
    CallbackRequest* callback_ptr = callback_request.get();
    callback_request = RegisterCallbackWithoutQueueing(std::move(callback_request));
    // Dispatcher returned callback request as queueing failed.
    if (callback_request) {
      return ZX_ERR_BAD_STATE;
    }
    QueueRegisteredCallback(callback_ptr, ZX_OK);
  } else {
    auto delayed_task = std::make_unique<DelayedTask>(
        zx::time(task->deadline), is_always_on ? CallbackRequest::RequestType::kAlwaysOnTask
                                               : CallbackRequest::RequestType::kTask);
    delayed_task->SetCallback(this, std::move(callback), task);

    fbl::AutoLock al(&callback_lock_);
    if (!IsRunningLocked()) {
      return ZX_ERR_BAD_STATE;
    }
    InsertDelayedTaskSortedLocked(std::move(delayed_task));
    ResetTimerLocked();
  }
  return ZX_OK;
}

zx_status_t Dispatcher::CancelTask(async_task_t* task) {
  fbl::AutoLock lock(&callback_lock_);
  auto callback_request = CancelAsyncOperationLocked(task);
  return callback_request ? ZX_OK : ZX_ERR_NOT_FOUND;
}

zx_status_t Dispatcher::QueuePacket(async_receiver_t* receiver, const zx_packet_user_t* data) {
  fbl::AutoLock lock(&callback_lock_);
  if (!IsRunningLocked()) {
    return ZX_ERR_BAD_STATE;
  }
  return async_queue_packet(process_shared_dispatcher_, receiver, data);
}

zx_status_t Dispatcher::BindIrq(async_irq_t* irq) {
  if (unsynchronized()) {
    // TODO(https://fxbug.dev/42052791): support interrupts on unsynchronized dispatchers.
    return ZX_ERR_NOT_SUPPORTED;
  }

  fbl::AutoLock lock(&callback_lock_);
  if (!IsRunningLocked()) {
    return ZX_ERR_BAD_STATE;
  }
  auto async_irq = std::make_unique<AsyncIrq>(irq, *this);
  return AsyncIrq::Bind(std::move(async_irq), *this);
}

zx_status_t Dispatcher::UnbindIrq(async_irq_t* irq) {
  if (unsynchronized()) {
    // TODO(https://fxbug.dev/42052791): support interrupts on unsynchronized dispatchers.
    return ZX_ERR_NOT_SUPPORTED;
  }

  auto* async_irq = reinterpret_cast<AsyncIrq*>(irq->state.reserved[0]);
  if (!async_irq) {
    return ZX_ERR_NOT_FOUND;
  }
  // Check that the irq is unbound from the same dispatcher it was bound to.
  auto cur_dispatcher = thread_context::GetCurrentDispatcher();
  if (!cur_dispatcher || (cur_dispatcher != async_irq->GetDispatcherRef().get())) {
    return ZX_ERR_BAD_STATE;
  }

  std::unique_ptr<AsyncIrq> unbound_irq;
  {
    // The |callback_lock_| must be held across clearing the |dispatcher_ref_| in
    // the irq, and removing any queued callback request for the irq.
    fbl::AutoLock lock(&callback_lock_);
    if (!async_irq->Unbind()) {
      return ZX_ERR_NOT_FOUND;
    }
    unbound_irq = RemoveIrqLocked(async_irq);
    ZX_ASSERT(unbound_irq != nullptr);
    // If the irq has been triggered, there may be a callback request queued.
    CancelAsyncOperationLocked(async_irq);
  }
  // Though the irq has been unbound, it's possible that another |process_shared_dispatcher|
  // thread has already pulled an irq packet from the port and may attempt to call the irq
  // handler. Delay destroying our irq wrapper for a bit in case this race condition happens.
  thread_pool_->CacheUnboundIrq(std::move(unbound_irq));
  return ZX_OK;
}

namespace {

const char kSequenceIdWrongDispatcherType[] =
    "A synchronized fdf_dispatcher_t is required. "
    "Ensure the fdf_dispatcher_t does not have the |FDF_DISPATCHER_OPTION_UNSYNCHRONIZED| option.";

const char kSequenceIdUnknownThread[] =
    "The current thread is not managed by a driver dispatcher. "
    "Ensure the object is always used from a dispatcher managed thread.";

const char kSequenceIdWrongDispatcherInstance[] =
    "Access from multiple driver dispatchers detected. "
    "This is not allowed. Ensure the object is used from the same |fdf_dispatcher_t|.";

}  // namespace

zx_status_t Dispatcher::GetSequenceId(async_sequence_id_t* out_sequence_id,
                                      const char** out_error) {
  if (unsynchronized()) {
    *out_error = kSequenceIdWrongDispatcherType;
    return ZX_ERR_WRONG_TYPE;
  }
  auto* current_dispatcher = thread_context::GetCurrentDispatcher();
  if (current_dispatcher == nullptr) {
    *out_error = kSequenceIdUnknownThread;
    return ZX_ERR_INVALID_ARGS;
  }
  if (current_dispatcher != this) {
    *out_error = kSequenceIdWrongDispatcherInstance;
    return ZX_ERR_INVALID_ARGS;
  }
  out_sequence_id->value = reinterpret_cast<uint64_t>(this);
  return ZX_OK;
}

zx_status_t Dispatcher::CheckSequenceId(async_sequence_id_t sequence_id, const char** out_error) {
  async_sequence_id_t current_sequence_id;
  zx_status_t status = GetSequenceId(&current_sequence_id, out_error);
  if (status != ZX_OK) {
    return status;
  }
  if (current_sequence_id.value != sequence_id.value) {
    *out_error = kSequenceIdWrongDispatcherInstance;
    return ZX_ERR_OUT_OF_RANGE;
  }
  return ZX_OK;
}

std::unique_ptr<driver_runtime::CallbackRequest> Dispatcher::RegisterCallbackWithoutQueueing(
    std::unique_ptr<driver_runtime::CallbackRequest> callback_request) {
  fbl::AutoLock lock(&callback_lock_);
  if (!IsRunningLocked()) {
    return callback_request;
  }
  registered_callbacks_.push_back(std::move(callback_request));
  return nullptr;
}

fit::result<NonInlinedReason> Dispatcher::ShouldInline(
    std::unique_ptr<CallbackRequest>& callback_request) {
  auto req_type = callback_request->request_type();

  if (thread_pool_->scheduler_role_options() & FDF_SCHEDULER_ROLE_OPTION_NO_SYNC_CALLS) {
    return fit::error(NonInlinedReason::kNoThreadMigration);
  }

  if (!unsynchronized_) {
    // Calling from a non-blocking dispatcher to a blocking dispatcher will lead to
    // the driver runtime queueing the callback onto the async loop.
    if (allow_sync_calls_) {
      auto sender = thread_context::GetCurrentDispatcher();
      bool sender_allows_sync = false;
      // Check if the sender is a blocking or non-blocking dispatcher.
      // We don't have to check further down the call stack, as we would never allow
      // a direct non-blocking to blocking transition.
      if (sender && !sender->unsynchronized()) {
        // Since we are currently running on the sender's dispatcher, the |allow_sync_calls| value
        // should not be able to be modified in the meanwhile, as |Seal| requires to be called while
        // running on the dispatcher.
        sender_allows_sync = sender->allow_sync_calls();
      }
      if (!sender_allows_sync) {
        return fit::error(NonInlinedReason::kAllowSyncCalls);
      }
    }
    // Synchronous dispatchers do not allow parallel callbacks. If we are already
    // dispatching a request on another thread, we will have to queue this request for later.
    if (dispatching_sync_) {
      return fit::error(NonInlinedReason::kDispatchingOnAnotherThread);
    }
    // TODO(https://fxbug.dev/42180471): we should be able to remove the task check once we track
    // drivers through banjo calls, or start each DFv2 driver with a ALLOW_SYNC_CALLS
    // dispatcher.
    if (req_type == CallbackRequest::RequestType::kTask) {
      return fit::error(NonInlinedReason::kTask);
    }
  }
  // Callbacks that are for waits or irqs can skip the reentrancy check.
  // This is as they are always first registered on the global async loop which
  // will initiate the callback when ready, at which point the driver call stack
  // will be empty, but we still want to consider it not reentrant and directly
  // call into the driver.
  bool is_global_loop_callback = (req_type == CallbackRequest::RequestType::kIrq) ||
                                 (req_type == CallbackRequest::RequestType::kWait) ||
                                 (req_type == CallbackRequest::RequestType::kWakeIrq) ||
                                 (req_type == CallbackRequest::RequestType::kWakeWait);
  if (is_global_loop_callback) {
    return fit::ok();
  }
  // Check if the call would be reentrant, in which case we will queue it up to be run
  // later.
  //
  // If it is unknown which driver is calling this function, it is considered
  // to be potentially reentrant.
  //
  // The call stack may be empty if the user writes to a channel, or registers a
  // read callback on a thread not managed by the driver runtime.
  // We use |GetCurrentDriver| rather than |IsCallStackEmpty| as this also
  // handles the case where the testing dispatcher is set as the thread's default dispatcher.
  if (!thread_context::GetCurrentDriver()) {
    return fit::error(NonInlinedReason::kUnknownThread);
  }
  if ((thread_context::GetCurrentDriver() == owner_) ||
      thread_context::IsDriverInCallStack(owner_)) {
    return fit::error(NonInlinedReason::kReentrant);
  }
  return fit::ok();
}

void Dispatcher::QueueRegisteredCallback(driver_runtime::CallbackRequest* request,
                                         zx_status_t callback_reason, bool was_deferred) {
  TRACE_DURATION("driver_runtime", "Dispatcher::QueueRegisteredCallback", "dispatcher_name",
                 name_.c_str(), "callback_reason", zx_status_get_string(callback_reason),
                 "was_deferred", TA_BOOL(was_deferred));

  ZX_ASSERT(request);

  auto decrement_and_idle_check = fit::defer([this]() {
    fbl::AutoLock lock(&callback_lock_);
    ZX_ASSERT(num_active_threads_ > 0);
    num_active_threads_--;
    IdleCheckLocked();
  });

  bool should_trigger_resume = false;
  fdf_env_resume_requester_t* local_resume_requester = nullptr;
  std::unique_ptr<driver_runtime::CallbackRequest> callback_request;
  {
    fbl::AutoLock lock(&callback_lock_);
    // It's possible that we are being called from a |Channel::Write| on the peer of a channel
    // that is registered on this dispatcher. This means that there is no guarantee that the
    // dispatcher will not enter |CompleteShutdown| between when we return from this check
    // and when we decrement |num_active_threads_| in |decrement_and_idle_check|.
    // Instead do not increment |num_active_threads_| until after this check.
    if (!IsRunningLocked()) {
      decrement_and_idle_check.cancel();
      // We still want to do |IdleCheckLocked|, in case this is a completed |Wait| being processed.
      IdleCheckLocked();
      return;
    }
    num_active_threads_++;

    // Finding the callback request may fail if the request was cancelled in the meanwhile.
    // This is possible if the channel was about to queue the registered callback (in response
    // to a channel write or a peer channel closing), but the client cancelled the callback.
    //
    // Calling |request->InContainer| may crash if the callback request was destructed between
    // when we called |RegisterCallbackWithoutQueueing| and now.
    // TODO(https://fxbug.dev/42053744): if we change CallbackRequests to use RefPtrs, we should be
    // able to switch this back to an |InContainer| check.
    callback_request =
        registered_callbacks_.erase_if([request](const CallbackRequest& callback_request) {
          return &callback_request == request;
        });
    if (!callback_request) {
      return;
    }
    callback_request->SetCallbackReason(callback_reason);

    if (suspend_state_ == SuspendState::kSuspended) {
      auto req_type = callback_request->request_type();
      if (req_type == CallbackRequest::RequestType::kWakeIrq ||
          req_type == CallbackRequest::RequestType::kWakeWait) {
        wake_queue_.push_back(std::move(callback_request));
        should_trigger_resume = true;
        local_resume_requester = resume_requester_;
      } else if (!callback_request->IsAlwaysOn()) {
        sleep_queue_.push_back(std::move(callback_request));
        return;  // No signal, just return.
      }
    }

    if (!should_trigger_resume) {
      // Whether we want to call the callback now, or queue it to be run on the async loop.
      fit::result<NonInlinedReason> should_inline = ShouldInline(callback_request);
      debug_stats_.num_total_requests++;
      if (should_inline.is_error()) {
        callback_queue_.push_back(std::move(callback_request));
        if (event_waiter_ && !event_waiter_->signaled()) {
          event_waiter_->signal();
        }

        // If the message was not inlined earlier due to the wait not yet been ready,
        // we should make sure this reason is displayed even if any other of the
        // reasons also apply.
        if (was_deferred) {
          debug_stats_.non_inlined.channel_wait_not_yet_registered++;
        } else {
          switch (should_inline.error_value()) {
            case kAllowSyncCalls:
              debug_stats_.non_inlined.allow_sync_calls++;
              break;
            case kDispatchingOnAnotherThread:
              debug_stats_.non_inlined.parallel_dispatch++;
              break;
            case kTask:
              debug_stats_.non_inlined.task++;
              break;
            case kUnknownThread:
              debug_stats_.non_inlined.unknown_thread++;
              break;
            case kReentrant:
              debug_stats_.non_inlined.reentrant++;
              break;
            case kNoThreadMigration:
              debug_stats_.non_inlined.no_thread_migration++;
              break;
            default:
              LOGF(ERROR, "Unhandled NonInlinedReason");
          };
        }
        return;
      }
      // The request was not queued earlier, so we don't count it as inlined in the stats,
      // even though it is getting inlined in this specific instance.
      if (was_deferred) {
        debug_stats_.non_inlined.channel_wait_not_yet_registered++;
      } else {
        debug_stats_.num_inlined_requests++;
      }

      auto req_type = callback_request->request_type();
      if (!callback_request->IsAlwaysOn()) {
        executing_power_managed_tasks_++;
      }
      if (req_type == CallbackRequest::RequestType::kWakeIrq ||
          req_type == CallbackRequest::RequestType::kWakeWait) {
        executing_wake_vectors_++;
      }
      dispatching_sync_ = true;
    }
  }

  if (should_trigger_resume) {
    if (local_resume_requester && local_resume_requester->handler) {
      local_resume_requester->handler(local_resume_requester);
    }
    return;
  }

  auto req_type = callback_request->request_type();
  DispatchCallback(std::move(callback_request));

  bool should_complete_suspend = false;
  fit::closure completion_callback;
  {
    fbl::AutoLock lock(&callback_lock_);
    dispatching_sync_ = false;

    if (!CallbackRequest::IsAlwaysOn(req_type)) {
      ZX_ASSERT(executing_power_managed_tasks_ > 0);
      executing_power_managed_tasks_--;

      if (req_type == CallbackRequest::RequestType::kWakeIrq ||
          req_type == CallbackRequest::RequestType::kWakeWait) {
        ZX_ASSERT(executing_wake_vectors_ > 0);
        executing_wake_vectors_--;
      }

      if (suspend_state_ == SuspendState::kSuspended && executing_power_managed_tasks_ == 0) {
        if (suspend_completion_callback_) {
          completion_callback = std::move(suspend_completion_callback_);
          should_complete_suspend = true;
        }
      }
    }

    if (!callback_queue_.is_empty() && event_waiter_ && !event_waiter_->signaled() &&
        IsRunningLocked()) {
      event_waiter_->signal();
    }
  }

  if (should_complete_suspend && completion_callback) {
    completion_callback();
  }
}

void Dispatcher::AddWaitLocked(std::unique_ptr<AsyncWait> wait) {
  ZX_DEBUG_ASSERT(!fbl::InContainer<AsyncWaitTag>(*wait));
  waits_.push_back(std::move(wait));
}

std::unique_ptr<AsyncWait> Dispatcher::RemoveWait(AsyncWait* wait) {
  fbl::AutoLock al(&callback_lock_);
  return RemoveWaitLocked(wait);
}

std::unique_ptr<AsyncWait> Dispatcher::RemoveWaitLocked(AsyncWait* wait) {
  ZX_DEBUG_ASSERT(fbl::InContainer<AsyncWaitTag>(*wait));
  auto ret = waits_.erase(*wait);
  IdleCheckLocked();
  return ret;
}

void Dispatcher::QueueWait(AsyncWait* wait, zx_status_t status) {
  fbl::AutoLock al(&callback_lock_);

  ZX_DEBUG_ASSERT(fbl::InContainer<AsyncWaitTag>(*wait));
  if (wait->is_pending_cancellation()) {
    // Wait was cancelled so we return immediately without invoking the callback.
    waits_.erase(*wait);
    // In case this is the last wait that shutdown is waiting on.
    IdleCheckLocked();
    return;
  }

  if (!IsRunningLocked()) {
    // We are waiting for all outstanding waits to be completed. They will be serviced in
    // CompleteDestroy.
    shutdown_queue_.push_back(waits_.erase(*wait));
    IdleCheckLocked();
  } else {
    registered_callbacks_.push_back(waits_.erase(*wait));
    al.release();
    QueueRegisteredCallback(wait, status);
  }
}

void Dispatcher::AddIrqLocked(std::unique_ptr<AsyncIrq> irq) {
  ZX_DEBUG_ASSERT(!irq->InContainer());
  irqs_.push_back(std::move(irq));
}

std::unique_ptr<AsyncIrq> Dispatcher::RemoveIrqLocked(AsyncIrq* irq) {
  ZX_DEBUG_ASSERT(irq->InContainer());
  return irqs_.erase(*irq);
}

void Dispatcher::QueueIrq(AsyncIrq* irq, zx_status_t status) {
  auto callback_request = irq->CreateCallbackRequest(*this, false /* locked */);
  CallbackRequest* callback_ptr = callback_request.get();

  {
    fbl::AutoLock al(&callback_lock_);

    // If the dispatcher is shutting down, we will not deliver any more irqs to the user.
    // |CompleteShutdown| will call the irq handler with |ZX_ERR_CANCELED|.
    if (!IsRunningLocked()) {
      return;
    }
    if (!irq->GetDispatcherRef()) {
      // It's possible that the irq was unbound before we acquired the |callback_lock_|.
      return;
    }
    // Unbinding only happens while the |callback_lock_| is held, so we don't
    // need to hold the irq lock while we register this callback request.
    registered_callbacks_.push_back(std::move(callback_request));
  }
  // If the irq is unbound before calling this, it will remove the callback request from
  // |registered_callbacks_|.
  QueueRegisteredCallback(callback_ptr, status);
}

std::unique_ptr<CallbackRequest> Dispatcher::CancelCallback(CallbackRequest& request_to_cancel) {
  fbl::AutoLock lock(&callback_lock_);

  // The request can be in |registered_callbacks_|, |callback_queue_| or |shutdown_queue_|.
  if (request_to_cancel.InContainer()) {
    return request_to_cancel.RemoveFromContainer();
  }
  return nullptr;
}

bool Dispatcher::SetCallbackReason(CallbackRequest* callback_to_update,
                                   zx_status_t callback_reason) {
  fbl::AutoLock lock(&callback_lock_);
  auto iter = callback_queue_.find_if(
      [callback_to_update](auto& callback) -> bool { return &callback == callback_to_update; });
  if (iter == callback_queue_.end()) {
    iter = sleep_queue_.find_if(
        [callback_to_update](auto& callback) -> bool { return &callback == callback_to_update; });
    if (iter == sleep_queue_.end()) {
      iter = wake_queue_.find_if(
          [callback_to_update](auto& callback) -> bool { return &callback == callback_to_update; });
      if (iter == wake_queue_.end()) {
        return false;
      }
    }
  }
  callback_to_update->SetCallbackReason(callback_reason);
  return true;
}

std::unique_ptr<CallbackRequest> Dispatcher::CancelAsyncOperationLocked(void* operation) {
  auto iter = registered_callbacks_.erase_if([operation](const CallbackRequest& callback_request) {
    return callback_request.holds_async_operation(operation);
  });
  if (iter) {
    return iter;
  }
  iter = callback_queue_.erase_if([operation](const CallbackRequest& callback_request) {
    return callback_request.holds_async_operation(operation);
  });
  if (iter) {
    return iter;
  }
  iter = sleep_queue_.erase_if([operation](const CallbackRequest& callback_request) {
    return callback_request.holds_async_operation(operation);
  });
  if (iter) {
    return iter;
  }
  iter = wake_queue_.erase_if([operation](const CallbackRequest& callback_request) {
    return callback_request.holds_async_operation(operation);
  });
  if (iter) {
    return iter;
  }
  iter = shutdown_queue_.erase_if([operation](const CallbackRequest& callback_request) {
    return callback_request.holds_async_operation(operation);
  });
  if (iter) {
    return iter;
  }
  iter = delayed_tasks_.erase_if([operation](const CallbackRequest& callback_request) {
    return callback_request.holds_async_operation(operation);
  });
  if (iter) {
    ResetTimerLocked();
    return iter;
  }
  iter = always_on_delayed_tasks_.erase_if([operation](const CallbackRequest& callback_request) {
    return callback_request.holds_async_operation(operation);
  });
  if (iter) {
    ResetTimerLocked();
  }
  return iter;
}

void Dispatcher::DispatchCallback(
    std::unique_ptr<driver_runtime::CallbackRequest> callback_request) {
  TRACE_DURATION("driver_runtime", "Dispatcher::DispatchCallback", "dispatcher_name",
                 name_.c_str());

  thread_context::PushDriver(owner_, this);
  auto pop_driver = fit::defer([]() { thread_context::PopDriver(); });

  TRACE_DURATION("driver_dispatcher", name_.c_str());
  callback_request->Call(std::move(callback_request), ZX_OK);
}

void Dispatcher::DispatchCallbacks(std::unique_ptr<EventWaiter> event_waiter,
                                   fbl::RefPtr<Dispatcher> dispatcher_ref) {
  ZX_ASSERT(dispatcher_ref != nullptr);

  auto defer = fit::defer([&]() {
    fbl::AutoLock lock(&callback_lock_);

    if (event_waiter) {
      // We call |BeginWaitWithRef| even when shutting down so that the |event_waiter|
      // stays alive until the dispatcher is destroyed. This allows |IsIdleLocked| to
      // correctly check the state of the event waiter. |CompleteShutdown| will cancel
      // and drop the event waiter.
      zx_status_t status = event_waiter->BeginWaitWithRef(std::move(event_waiter), dispatcher_ref,
                                                          process_shared_dispatcher_);
      if (status == ZX_ERR_BAD_STATE) {
        event_waiter_ = nullptr;
      }
    }
    ZX_ASSERT(num_active_threads_ > 0);
    num_active_threads_--;
    IdleCheckLocked();
  });

  uint32_t num_callbacks_dispatched = 0;
  size_t current_batch_power_managed_count = 0;
  size_t current_batch_wake_vectors_count = 0;

  fbl::DoublyLinkedList<std::unique_ptr<CallbackRequest>> to_call;
  {
    fbl::AutoLock lock(&callback_lock_);
    num_active_threads_++;

    // Parallel callbacks are not allowed in synchronized dispatchers.
    // We should not be scheduled to run on two different dispatcher threads,
    // but it's possible we could still get here if we are currently doing a
    // direct call into the driver. In this case, we should designal the event
    // waiter, and once the direct call completes it will signal it again.
    if ((!unsynchronized_ && dispatching_sync_) || !IsRunningLocked()) {
      event_waiter->designal();
      return;
    }
    dispatching_sync_ = true;

    num_callbacks_dispatched += TakeNextCallbacks(&to_call);

    for (auto& req : to_call) {
      auto type = req.request_type();
      if (type == CallbackRequest::RequestType::kWakeIrq ||
          type == CallbackRequest::RequestType::kWakeWait) {
        current_batch_wake_vectors_count++;
      }
      if (!req.IsAlwaysOn()) {
        current_batch_power_managed_count++;
      }
    }
    executing_power_managed_tasks_ += current_batch_power_managed_count;
    executing_wake_vectors_ += current_batch_wake_vectors_count;

    // Check if there are callbacks left to process and we should wake up an additional
    // thread. For synchronized dispatchers, parallel callbacks are disallowed.
    if (unsynchronized_ && !callback_queue_.is_empty()) {
      zx_status_t status = event_waiter->BeginWaitWithRef(std::move(event_waiter), dispatcher_ref,
                                                          process_shared_dispatcher_);
      if (status == ZX_ERR_BAD_STATE) {
        event_waiter_ = nullptr;
      }
    }
  }

  bool should_complete_suspend = false;
  fit::closure completion_callback;

  while (true) {
    should_complete_suspend = false;
    completion_callback = nullptr;

    // Call the callbacks outside of the lock.
    while (!to_call.is_empty()) {
      auto callback_request = to_call.pop_front();
      ZX_ASSERT(callback_request);
      DispatchCallback(std::move(callback_request));
    }

    {
      fbl::AutoLock lock(&callback_lock_);

      executing_power_managed_tasks_ -= current_batch_power_managed_count;
      executing_wake_vectors_ -= current_batch_wake_vectors_count;
      current_batch_power_managed_count = 0;
      current_batch_wake_vectors_count = 0;

      if (suspend_state_ == SuspendState::kSuspended && executing_power_managed_tasks_ == 0) {
        if (suspend_completion_callback_) {
          completion_callback = std::move(suspend_completion_callback_);
          should_complete_suspend = true;
        }
      }

      // Check if there are any more callbacks to dispatch. This may be the case
      // if we were dispatching an async operation, or if the user queued more
      // operations during the last callback.
      if (!callback_queue_.is_empty() && (num_callbacks_dispatched < kBatchSize)) {
        num_callbacks_dispatched += TakeNextCallbacks(&to_call);

        for (auto& req : to_call) {
          auto type = req.request_type();
          if (type == CallbackRequest::RequestType::kWakeIrq ||
              type == CallbackRequest::RequestType::kWakeWait) {
            current_batch_wake_vectors_count++;
          }
          if (!req.IsAlwaysOn()) {
            current_batch_power_managed_count++;
          }
        }
        executing_power_managed_tasks_ += current_batch_power_managed_count;
        executing_wake_vectors_ += current_batch_wake_vectors_count;
      } else {
        dispatching_sync_ = false;
        if (!callback_queue_.is_empty() && event_waiter_ && !event_waiter_->signaled()) {
          event_waiter_->signal();
        }
      }
    }  // Drop the lock

    // 1. Execute the callback outside the lock BEFORE doing continue/return
    if (should_complete_suspend && completion_callback) {
      completion_callback();
    }

    // 2. Now handle the control flow
    if (!to_call.is_empty()) {
      continue;
    }

    if (!event_waiter) {
      return;
    }

    // 3. Final cleanup and return
    {
      fbl::AutoLock lock(&callback_lock_);
      ResetTimerLocked();
      if (callback_queue_.is_empty() && event_waiter->signaled()) {
        event_waiter->designal();
      }
      return;
    }
  }
}

uint32_t Dispatcher::TakeNextCallbacks(
    fbl::DoublyLinkedList<std::unique_ptr<CallbackRequest>>* out_callbacks) {
  // For synchronized dispatchers, cancellation of ChannelReads are guaranteed to succeed.
  // Since cancellation may be called from the ChannelRead, or from another async operation
  // (like a task), we need to make sure that if we are calling an async operation
  // that is the only callback request pulled from the callback queue.
  // This will guarantee that cancellation will always succeed without having to lock
  // |to_call|.
  bool has_async_op = false;
  uint32_t n = 0;
  while ((n < kBatchSize) && !callback_queue_.is_empty() && !has_async_op) {
    std::unique_ptr<CallbackRequest> callback_request = callback_queue_.pop_front();
    ZX_ASSERT(callback_request);
    has_async_op = !unsynchronized_ && callback_request->has_async_operation();
    // For synchronized dispatchers, an async operation should be the only member in
    // |to_call|.
    if (has_async_op && n > 0) {
      callback_queue_.push_front(std::move(callback_request));
      break;
    }
    out_callbacks->push_back(std::move(callback_request));
    n++;
  }
  return n;
}

zx::result<zx::event> Dispatcher::RegisterForCompleteShutdownEvent() {
  fbl::AutoLock lock_(&callback_lock_);
  auto event = complete_shutdown_event_manager_.GetEvent();
  if (event.is_error()) {
    return event;
  }
  if (IsIdleLocked() && !HasFutureOpsScheduledLocked()) {
    zx_status_t status = complete_shutdown_event_manager_.Signal();
    if (status != ZX_OK) {
      return zx::error(status);
    }
  }
  return event;
}

void Dispatcher::WaitUntilIdle() {
  ZX_ASSERT(!IsRuntimeManagedThread());

  fbl::AutoLock lock_(&callback_lock_);
  if (IsIdleLocked()) {
    return;
  }
  idle_event_.Wait(&callback_lock_);
  return;
}

bool Dispatcher::IsIdleLocked() {
  // If the event waiter was signaled, the thread will be scheduled to run soon.
  return (num_active_threads_ == 0) && callback_queue_.is_empty() &&
         (!event_waiter_ || !event_waiter_->signaled());
}

bool Dispatcher::HasFutureOpsScheduledLocked() {
  return !waits_.is_empty() || timer_.is_armed() || shutdown_waiting_for_timer_;
}

void Dispatcher::IdleCheckLocked() {
  if (IsIdleLocked()) {
    idle_event_.Broadcast();
    if (!HasFutureOpsScheduledLocked()) {
      complete_shutdown_event_manager_.Signal();
    }
  }
}

bool Dispatcher::HasQueuedTasks() {
  fbl::AutoLock lock(&callback_lock_);

  for (auto& callback_request : callback_queue_) {
    if (callback_request.request_type() == CallbackRequest::RequestType::kTask ||
        callback_request.request_type() == CallbackRequest::RequestType::kAlwaysOnTask) {
      return true;
    }
  }
  for (auto& callback_request : sleep_queue_) {
    if (callback_request.request_type() == CallbackRequest::RequestType::kTask) {
      return true;
    }
  }
  for (auto& callback_request : wake_queue_) {
    if (callback_request.request_type() == CallbackRequest::RequestType::kTask) {
      return true;
    }
  }
  for (auto& callback_request : shutdown_queue_) {
    if (callback_request.request_type() == CallbackRequest::RequestType::kTask ||
        callback_request.request_type() == CallbackRequest::RequestType::kAlwaysOnTask) {
      return true;
    }
  }
  return false;
}

zx::result<zx::event> Dispatcher::CompleteShutdownEventManager::GetEvent() {
  if (!event_.is_valid()) {
    // If this is the first waiter to register, we need to create the
    // idle event manager's event.
    zx_status_t status = zx::event::create(0, &event_);
    if (status != ZX_OK) {
      return zx::error(status);
    }
  }
  zx::event dup;
  zx_status_t status = event_.duplicate(ZX_RIGHTS_BASIC, &dup);
  if (status != ZX_OK) {
    return zx::error(status);
  }
  return zx::ok(std::move(dup));
}

zx_status_t Dispatcher::CompleteShutdownEventManager::Signal() {
  if (!event_.is_valid()) {
    return ZX_OK;  // No-one is waiting for idle events.
  }
  zx_status_t status = event_.signal(0u, ZX_EVENT_SIGNALED);
  event_.reset();
  return status;
}

zx_status_t Dispatcher::RegisterPendingToken(fdf_token_t* token) {
  fbl::AutoLock lock(&callback_lock_);
  if (!IsRunningLocked()) {
    return ZX_ERR_BAD_STATE;
  }
  if (registered_tokens_.find(token) != registered_tokens_.end()) {
    return ZX_ERR_BAD_STATE;
  }
  registered_tokens_.insert(token);
  return ZX_OK;
}

void Dispatcher::UnregisterPendingToken(fdf_token_t* token) {
  fbl::AutoLock lock(&callback_lock_);
  registered_tokens_.erase(token);
}

zx_status_t Dispatcher::ScheduleTokenCallback(fdf_token_t* token, zx_status_t status,
                                              fdf::Channel channel) {
  CallbackRequest* callback_request_ptr = nullptr;

  {
    fbl::AutoLock lock(&callback_lock_);
    if (!IsRunningLocked()) {
      return ZX_ERR_BAD_STATE;
    }

    auto callback_request = std::make_unique<CallbackRequest>();
    driver_runtime::Callback callback =
        [dispatcher = this, token, channel = std::move(channel)](
            std::unique_ptr<driver_runtime::CallbackRequest> callback_request,
            zx_status_t status) mutable {
          token->handler(dispatcher->to_fdf_dispatcher(), token, status, channel.release());
        };
    callback_request->SetCallback(this, std::move(callback));

    callback_request_ptr = callback_request.get();

    registered_callbacks_.push_back(std::move(callback_request));
    registered_tokens_.erase(token);
  }

  // If the dispatcher is shutdown in the meanwhile, the callback request will be completed
  // with |ZX_ERR_CANCELED| in |CompleteShutdown|.
  QueueRegisteredCallback(callback_request_ptr, status);

  return ZX_OK;
}

void Dispatcher::SuspendAsync(fit::closure completion_callback) {
  bool should_trigger_resume = false;
  bool should_complete_immediately = false;
  fdf_env_resume_requester_t* local_resume_requester = nullptr;

  {
    fbl::AutoLock lock(&callback_lock_);
    suspend_state_ = SuspendState::kSuspended;

    // Check callback_queue_ for triggered wake vectors.
    fbl::DoublyLinkedList<std::unique_ptr<CallbackRequest>> wake_vectors_to_move;
    auto iter = callback_queue_.begin();
    while (iter != callback_queue_.end()) {
      auto req_type = iter->request_type();
      if (req_type == CallbackRequest::RequestType::kWakeIrq ||
          req_type == CallbackRequest::RequestType::kWakeWait) {
        auto next = iter;
        next++;
        wake_vectors_to_move.push_back(callback_queue_.erase(iter));
        iter = next;
      } else {
        iter++;
      }
    }

    should_trigger_resume = !wake_vectors_to_move.is_empty() || (executing_wake_vectors_ > 0);
    wake_queue_.splice(wake_queue_.end(), wake_vectors_to_move);

    // Move remaining power-managed tasks to sleep queue.
    fbl::DoublyLinkedList<std::unique_ptr<CallbackRequest>> regular_tasks_to_move;
    iter = callback_queue_.begin();
    while (iter != callback_queue_.end()) {
      if (!iter->IsAlwaysOn()) {
        auto next = iter;
        next++;
        regular_tasks_to_move.push_back(callback_queue_.erase(iter));
        iter = next;
      } else {
        iter++;
      }
    }
    sleep_queue_.splice(sleep_queue_.end(), regular_tasks_to_move);

    if (callback_queue_.is_empty() && event_waiter_ && event_waiter_->signaled()) {
      event_waiter_->designal();
    }

    // Clear pending timers to avoid spurious wakeups.
    ResetTimerLocked(true /* force */);

    local_resume_requester = resume_requester_;

    if (executing_power_managed_tasks_ == 0) {
      should_complete_immediately = true;
    } else {
      ZX_ASSERT(!suspend_completion_callback_);
      suspend_completion_callback_ = std::move(completion_callback);
    }
  }  // Lock released here

  if (should_complete_immediately) {
    completion_callback();
  }

  if (should_trigger_resume) {
    if (local_resume_requester && local_resume_requester->handler) {
      local_resume_requester->handler(local_resume_requester);
    }
  }
}

void Dispatcher::Resume() {
  fit::closure completion_callback;
  {
    fbl::AutoLock lock(&callback_lock_);
    if (suspend_state_ == SuspendState::kNone) {
      return;
    }
    suspend_state_ = SuspendState::kNone;

    if (suspend_completion_callback_) {
      completion_callback = std::move(suspend_completion_callback_);
    }

    // Flush tasks from wake_queue_ to the front of callback_queue_ first.
    callback_queue_.splice(callback_queue_.begin(), wake_queue_);

    // Flush tasks from sleep_queue_ back to callback_queue_.
    callback_queue_.splice(callback_queue_.end(), sleep_queue_);

    // Re-arm timer by calling CheckDelayedTasksLocked() and ResetTimerLocked().
    CheckDelayedTasksLocked();
    ResetTimerLocked();

    // Signal worker threads if we have work.
    if (!callback_queue_.is_empty() && event_waiter_ && !event_waiter_->signaled()) {
      event_waiter_->signal();
    }
  }  // Lock released here!

  if (completion_callback) {
    completion_callback();
  }
}

void Dispatcher::SetResumeRequester(fdf_env_resume_requester_t* requester) {
  bool should_trigger_resume = false;
  fdf_env_resume_requester_t* local_resume_requester = nullptr;

  {
    fbl::AutoLock lock(&callback_lock_);
    resume_requester_ = requester;

    if (suspend_state_ == SuspendState::kSuspended &&
        (!wake_queue_.is_empty() || executing_wake_vectors_ > 0)) {
      should_trigger_resume = true;
      local_resume_requester = resume_requester_;
    }
  }

  if (should_trigger_resume && local_resume_requester && local_resume_requester->handler) {
    local_resume_requester->handler(local_resume_requester);
  }
}

// NOTE: Limitations of current Wake Vector implementation:
// 1. fdf_channel wait requests do not currently support wake vectors.
// 2. QueuePacket completely bypasses power management.
// 3. If a wait is posted on the always-on dispatcher but is also a wake vector,
//    it will be treated as a wake vector and delayed during suspend.
zx_status_t Dispatcher::RegisterWakeVector(zx_handle_t handle, zx_signals_t signals) {
  fbl::AutoLock lock(&callback_lock_);
  if (suspend_state_ == SuspendState::kSuspended) {
    return ZX_ERR_BAD_STATE;
  }

  wake_vectors_[handle] |= signals;

  // Search and update RequestType in registered_callbacks_, callback_queue_, and waits_.
  for (auto& req : registered_callbacks_) {
    if (IsWakeVectorLocked(req.handle(), req.signals())) {
      auto type = req.request_type();
      if (type == CallbackRequest::RequestType::kWait) {
        req.set_request_type(CallbackRequest::RequestType::kWakeWait);
      } else if (type == CallbackRequest::RequestType::kIrq) {
        req.set_request_type(CallbackRequest::RequestType::kWakeIrq);
      }
    }
  }
  for (auto& req : callback_queue_) {
    if (IsWakeVectorLocked(req.handle(), req.signals())) {
      auto type = req.request_type();
      if (type == CallbackRequest::RequestType::kWait) {
        req.set_request_type(CallbackRequest::RequestType::kWakeWait);
      } else if (type == CallbackRequest::RequestType::kIrq) {
        req.set_request_type(CallbackRequest::RequestType::kWakeIrq);
      }
    }
  }
  for (auto& wait : waits_) {
    if (IsWakeVectorLocked(wait.handle(), wait.signals())) {
      auto type = wait.request_type();
      if (type == CallbackRequest::RequestType::kWait) {
        wait.set_request_type(CallbackRequest::RequestType::kWakeWait);
      }
    }
  }

  return ZX_OK;
}

zx_status_t Dispatcher::UnregisterWakeVector(zx_handle_t handle, zx_signals_t signals) {
  fbl::AutoLock lock(&callback_lock_);

  auto iter = wake_vectors_.find(handle);
  if (iter == wake_vectors_.end()) {
    return ZX_ERR_NOT_FOUND;
  }

  if (signals == 0) {
    wake_vectors_.erase(iter);
  } else {
    iter->second &= ~signals;
    if (iter->second == 0) {
      wake_vectors_.erase(iter);
    }
  }

  // Revert RequestType back to kWait/kIrq.
  for (auto& req : registered_callbacks_) {
    if (req.handle() == handle && !IsWakeVectorLocked(req.handle(), req.signals())) {
      auto type = req.request_type();
      if (type == CallbackRequest::RequestType::kWakeWait) {
        req.set_request_type(CallbackRequest::RequestType::kWait);
      } else if (type == CallbackRequest::RequestType::kWakeIrq) {
        req.set_request_type(CallbackRequest::RequestType::kIrq);
      }
    }
  }
  for (auto& req : callback_queue_) {
    if (req.handle() == handle && !IsWakeVectorLocked(req.handle(), req.signals())) {
      auto type = req.request_type();
      if (type == CallbackRequest::RequestType::kWakeWait) {
        req.set_request_type(CallbackRequest::RequestType::kWait);
      } else if (type == CallbackRequest::RequestType::kWakeIrq) {
        req.set_request_type(CallbackRequest::RequestType::kIrq);
      }
    }
  }
  for (auto& wait : waits_) {
    if (wait.handle() == handle && !IsWakeVectorLocked(wait.handle(), wait.signals())) {
      auto type = wait.request_type();
      if (type == CallbackRequest::RequestType::kWakeWait) {
        wait.set_request_type(CallbackRequest::RequestType::kWait);
      }
    }
  }
  for (auto& req : sleep_queue_) {
    if (req.handle() == handle && !IsWakeVectorLocked(req.handle(), req.signals())) {
      auto type = req.request_type();
      if (type == CallbackRequest::RequestType::kWakeWait) {
        req.set_request_type(CallbackRequest::RequestType::kWait);
      } else if (type == CallbackRequest::RequestType::kWakeIrq) {
        req.set_request_type(CallbackRequest::RequestType::kIrq);
      }
    }
  }
  for (auto& req : wake_queue_) {
    if (req.handle() == handle && !IsWakeVectorLocked(req.handle(), req.signals())) {
      auto type = req.request_type();
      if (type == CallbackRequest::RequestType::kWakeWait) {
        LOGF(
            ERROR,
            "Actively queued wake item type being changed: kWakeWait to kWait. This item will still "
            "execute with higher priority in the queue.");
        req.set_request_type(CallbackRequest::RequestType::kWait);
      } else if (type == CallbackRequest::RequestType::kWakeIrq) {
        LOGF(ERROR,
             "Actively queued wake item type being changed: kWakeIrq to kIrq. This item will still "
             "execute with higher priority in the queue.");
        req.set_request_type(CallbackRequest::RequestType::kIrq);
      }
    }
  }

  return ZX_OK;
}

}  // namespace driver_runtime
