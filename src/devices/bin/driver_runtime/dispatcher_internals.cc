// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "dispatcher_internals.h"

#include "dispatcher_coordinator.h"
#include "src/devices/lib/log/log.h"
#include "thread_pool.h"

namespace driver_runtime {

AsyncIrq::AsyncIrq(async_irq_t* original_irq, Dispatcher& dispatcher)
    : async_irq_t{{ASYNC_STATE_INIT}, &AsyncIrq::Handler, original_irq->object},
      original_irq_(original_irq) {
  // Store a pointer to our IRQ wrapper, so |UnbindIrq| can back map from the user's IRQ object.
  original_irq_->state.reserved[0] = reinterpret_cast<uintptr_t>(this);
}

AsyncIrq::~AsyncIrq() {
  // This shouldn't destruct until after the irq has been unbound, either by the user or
  // |ShutdownAsync|.
  ZX_ASSERT(dispatcher_ == nullptr);
}

// static
zx_status_t AsyncIrq::Bind(std::unique_ptr<AsyncIrq> irq, Dispatcher& dispatcher) {
  // The AsyncIrq will hold the dispatcher reference until the irq is unbound.
  irq->SetDispatcherRef(fbl::RefPtr(&dispatcher));

  auto* irq_ref = irq.get();
  dispatcher.AddIrqLocked(std::move(irq));

  zx_status_t status = async_bind_irq(dispatcher.process_shared_dispatcher_, irq_ref);
  if (status != ZX_OK) {
    ZX_ASSERT(dispatcher.RemoveIrqLocked(irq_ref) != nullptr);
    irq->SetDispatcherRef(nullptr);
    return status;
  }
  return ZX_OK;
}

bool AsyncIrq::Unbind() {
  auto dispatcher = GetDispatcherRef();
  if (!dispatcher) {
    return false;
  }
  auto status = async_unbind_irq(dispatcher->process_shared_dispatcher_, this);
  if (status != ZX_OK) {
    return false;
  }
  SetDispatcherRef(nullptr);
  original_irq_->state.reserved[0] = 0;
  return true;
}

std::unique_ptr<driver_runtime::CallbackRequest> AsyncIrq::CreateCallbackRequest(
    Dispatcher& dispatcher, bool locked) __TA_NO_THREAD_SAFETY_ANALYSIS {
  auto async_dispatcher = dispatcher.GetAsyncDispatcher();

  // TODO(https://fxbug.dev/42052990): We should consider something more efficient than creating a
  // callback request each time the irq is triggered. This is complex due to an AsyncIrq not having
  // a 1:1 mapping to interrupt callbacks, and we cannot easily return ownership of a
  // |CallbackRequest| after dispatching it. See https://fxbug.dev/42052990 for a longer
  // explanation.
  auto callback_request =
      std::make_unique<driver_runtime::CallbackRequest>(CallbackRequest::RequestType::kIrq);
  callback_request->set_handle(original_irq_->object);

  bool is_wake = false;
  if (locked) {
    is_wake = dispatcher.IsWakeVectorLocked(original_irq_->object, 0);
  } else {
    fbl::AutoLock lock(&dispatcher.callback_lock_);
    is_wake = dispatcher.IsWakeVectorLocked(original_irq_->object, 0);
  }

  if (is_wake) {
    callback_request->set_request_type(CallbackRequest::RequestType::kWakeIrq);
  }

  driver_runtime::Callback callback =
      [this, async_dispatcher](std::unique_ptr<driver_runtime::CallbackRequest> callback_request,
                               zx_status_t status) {
        // We should not clear the reserved state, as this AsyncIrq object is still bound for
        // future interrupts.
        original_irq_->handler(async_dispatcher, original_irq_, status, &interrupt_packet_);
      };
  callback_request->SetCallback(&dispatcher, std::move(callback), this);
  return callback_request;
}

// static
void AsyncIrq::Handler(async_dispatcher_t* dispatcher, async_irq_t* irq, zx_status_t status,
                       const zx_packet_interrupt_t* packet) {
  static_cast<AsyncIrq*>(irq)->OnSignal(dispatcher, status, packet);
}

void AsyncIrq::OnSignal(async_dispatcher_t* global_dispatcher, zx_status_t status,
                        const zx_packet_interrupt_t* packet) {
  fbl::RefPtr<Dispatcher> dispatcher = GetDispatcherRef();
  // This may be cleared if the irq has already been unbound, but this irq packet was already pulled
  // from the port. If so, we should not deliver the irq to the user.
  if (!dispatcher) {
    return;
  }
  interrupt_packet_ = *packet;

  // We do not hold the irq lock before calling |QueueIrq|, as it would cause
  // incorrect lock ordering.
  dispatcher->QueueIrq(this, status);
  dispatcher->thread_pool()->OnThreadWakeup();
}

void EventWaiter::HandleEvent(std::unique_ptr<EventWaiter> event_waiter,
                              async_dispatcher_t* dispatcher, async::WaitBase* wait,
                              zx_status_t status, const zx_packet_signal_t* signal) {
  if (status == ZX_ERR_CANCELED) {
    LOGF(TRACE, "Dispatcher: event waiter shutting down");
    event_waiter->dispatcher_ref_->SetEventWaiter(nullptr);
    event_waiter->dispatcher_ref_ = nullptr;
    return;
  } else if (status != ZX_OK) {
    LOGF(ERROR, "Dispatcher: event waiter error: %d", status);
    event_waiter->dispatcher_ref_->SetEventWaiter(nullptr);
    event_waiter->dispatcher_ref_ = nullptr;
    return;
  }

  if (signal->observed & ZX_USER_SIGNAL_0) {
    // The callback is in charge of calling |BeginWaitWithRef| on the event waiter.
    fbl::RefPtr<Dispatcher> dispatcher_ref = std::move(event_waiter->dispatcher_ref_);
    event_waiter->InvokeCallback(std::move(event_waiter), dispatcher_ref);
  } else {
    LOGF(ERROR, "Dispatcher: event waiter got unexpected signals: %x", signal->observed);
  }
}

// static
zx_status_t EventWaiter::BeginWaitWithRef(std::unique_ptr<EventWaiter> event,
                                          fbl::RefPtr<Dispatcher> dispatcher,
                                          async_dispatcher_t* async_dispatcher) {
  ZX_ASSERT(dispatcher != nullptr);
  event->dispatcher_ref_ = dispatcher;
  return BeginWait(std::move(event), async_dispatcher);
}

AsyncWait::AsyncWait(async_wait_t* original_wait, Dispatcher& dispatcher)
    : CallbackRequest(CallbackRequest::RequestType::kWait),
      async_wait_t{{ASYNC_STATE_INIT},
                   &AsyncWait::Handler,
                   original_wait->object,
                   original_wait->trigger,
                   original_wait->options},
      original_wait_(original_wait) {
  // Use one of the async_wait_t's reserved fields to stash a pointer to the AsyncWait object.
  original_wait_->state.reserved[0] = reinterpret_cast<uintptr_t>(this);

  auto async_dispatcher = dispatcher.GetAsyncDispatcher();
  driver_runtime::Callback callback =
      [this, async_dispatcher](std::unique_ptr<driver_runtime::CallbackRequest> callback_request,
                               zx_status_t status) {
        // Clear the pointer to the AsyncWait object.
        original_wait_->state.reserved[0] = 0;
        zx_packet_signal_t* signal_packet = signal_packet_ ? &signal_packet_.value() : nullptr;
        original_wait_->handler(async_dispatcher, original_wait_, status, signal_packet);
      };
  // Note that this callback is called *after* |OnSignal|, which is the immediate callback that is
  // invoked when the async wait is signaled.
  SetCallback(&dispatcher, std::move(callback), original_wait_);
}

AsyncWait::~AsyncWait() {
  // This shouldn't destruct until the wait was canceled or it has been completed.
  ZX_ASSERT(dispatcher_ref_ == nullptr);
}

// static
zx_status_t AsyncWait::BeginWait(std::unique_ptr<AsyncWait> wait, Dispatcher& dispatcher) {
  // Purposefully create a cycle which is broken in Cancel or OnSignal.
  // This needs to be done ahead of starting the async wait in case another thread on the dispatcher
  // signals the dispatcher.
  auto dispatcher_ref = fbl::RefPtr(&dispatcher);
  wait->dispatcher_ref_ = fbl::ExportToRawPtr(&dispatcher_ref);
  auto* wait_ref = wait.get();
  dispatcher.AddWaitLocked(std::move(wait));

  zx_status_t status = async_begin_wait(
      const_cast<async_dispatcher_t*>(dispatcher.process_shared_dispatcher()), wait_ref);
  if (status != ZX_OK) {
    fbl::ImportFromRawPtr(wait_ref->dispatcher_ref_.exchange(nullptr));
    dispatcher.RemoveWaitLocked(wait_ref);
    return status;
  }
  return ZX_OK;
}

bool AsyncWait::Cancel() {
  // We do a load here rather than an exchange as OnSignal may still be triggered and we need to
  // avoid preventing it from accessing the |dispatcher_ref_|.
  auto* dispatcher_ref = dispatcher_ref_.load();
  if (dispatcher_ref == nullptr) {
    // OnSignal was triggered in another thread.
    return false;
  }
  auto dispatcher = fbl::RefPtr(dispatcher_ref);
  auto status = async_cancel_wait(
      const_cast<async_dispatcher_t*>(dispatcher->process_shared_dispatcher()), this);
  if (status != ZX_OK) {
    // OnSignal was triggered in another thread, or is about to be.
    ZX_DEBUG_ASSERT(status == ZX_ERR_NOT_FOUND);
    return false;
  }
  // It is now safe to recover the dispatcher reference.
  dispatcher_ref = dispatcher_ref_.exchange(nullptr);
  ZX_DEBUG_ASSERT(dispatcher_ref != nullptr);
  fbl::ImportFromRawPtr(dispatcher_ref);

  return true;
}

// static
void AsyncWait::Handler(async_dispatcher_t* dispatcher, async_wait_t* wait, zx_status_t status,
                        const zx_packet_signal_t* signal) {
  static_cast<AsyncWait*>(wait)->OnSignal(dispatcher, status, signal);
}

void AsyncWait::OnSignal(async_dispatcher_t* async_dispatcher, zx_status_t status,
                         const zx_packet_signal_t* signal) {
  auto* dispatcher_ref = dispatcher_ref_.exchange(nullptr);
  ZX_DEBUG_ASSERT(dispatcher_ref != nullptr);
  auto dispatcher = fbl::ImportFromRawPtr(dispatcher_ref);

  if (signal) {
    signal_packet_ = *signal;
  } else {
    signal_packet_ = std::nullopt;
  }

  dispatcher->QueueWait(this, status);
  dispatcher->thread_pool()->OnThreadWakeup();
}

AllowedSchedulerRoles* AllowedSchedulerRoles::Get() {
  static AllowedSchedulerRoles instance;
  return &instance;
}

void AllowedSchedulerRoles::AddForDriver(const void* driver, std::string_view role) {
  fbl::AutoLock al(&lock_);
  allowed_roles_.try_emplace(driver);
  allowed_roles_[driver].emplace(role);
}

bool AllowedSchedulerRoles::IsAllowed(std::string_view role) {
  if (unlikely(!DispatcherCoordinator::enforce_allowed_scheduler_roles())) {
    return true;
  }
  fbl::AutoLock guard(&lock_);
  auto iter = allowed_roles_.find(thread_context::GetCurrentDriver());
  return iter != allowed_roles_.end() && iter->second.contains(std::string(role));
}

}  // namespace driver_runtime
