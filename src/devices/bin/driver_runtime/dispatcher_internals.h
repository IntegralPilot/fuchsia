// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BIN_DRIVER_RUNTIME_DISPATCHER_INTERNALS_H_
#define SRC_DEVICES_BIN_DRIVER_RUNTIME_DISPATCHER_INTERNALS_H_

#include <lib/async/irq.h>

#include <fbl/intrusive_double_list.h>

#include "async_loop_owned_event_handler.h"
#include "dispatcher.h"

namespace driver_runtime {

// Indirect irq object which is used to ensure irqs are tracked and synchronize irqs on
// SYNCHRONIZED dispatchers.
// Public so it can be referenced by the DispatcherCoordinator.
class AsyncIrq : public async_irq_t, public fbl::DoublyLinkedListable<std::unique_ptr<AsyncIrq>> {
 public:
  AsyncIrq(async_irq_t* original_irq, Dispatcher& dispatcher);
  ~AsyncIrq();

  static zx_status_t Bind(std::unique_ptr<AsyncIrq> irq, Dispatcher& dispatcher)
      __TA_REQUIRES(&dispatcher.callback_lock_);
  bool Unbind();

  static void Handler(async_dispatcher_t* dispatcher, async_irq_t* irq, zx_status_t status,
                      const zx_packet_interrupt_t* packet);
  void OnSignal(async_dispatcher_t* async_dispatcher, zx_status_t status,
                const zx_packet_interrupt_t* packet);

  // Returns a callback request representing the triggered irq.
  std::unique_ptr<driver_runtime::CallbackRequest> CreateCallbackRequest(Dispatcher& dispatcher,
                                                                         bool locked = false);

  fbl::RefPtr<Dispatcher> GetDispatcherRef() {
    fbl::AutoLock lock(&lock_);
    return dispatcher_;
  }

 private:
  void SetDispatcherRef(fbl::RefPtr<Dispatcher> dispatcher) {
    fbl::AutoLock lock(&lock_);
    dispatcher_ = std::move(dispatcher);
  }
  // Unlike async::Wait, we cannot store the dispatcher_ref as a std::atomic<Dispatcher*>.
  //
  // Since the |OnSignal| handler may be called many times, it copies the dispatcher reference,
  // rather than taking ownership of it. While |OnSignal| is accessing |dispatcher_|,
  // another thread could be attempting to unbind the dispatcher, so with an atomic raw pointer,
  // is is possible that the dispatcher has been destructed between when we access |dispatcher_|
  // and when we try to convert it back to a RefPtr.
  //
  // If |lock_| needs to be acquired at the same time as the dispatcher's |callback_lock_|,
  // you must acquire |callback_lock_| first.
  fbl::Mutex lock_;
  fbl::RefPtr<Dispatcher> dispatcher_ __TA_GUARDED(&lock_);

  async_irq_t* original_irq_;

  zx_packet_interrupt_t interrupt_packet_ = {};
};

// Object which waits on an underlying async loop and triggers the dispatcher to
// service its callbacks.
// Public so it can be referenced by the DispatcherCoordinator.
class EventWaiter : public AsyncLoopOwnedEventHandler<EventWaiter> {
  using Callback = fit::inline_function<void(std::unique_ptr<EventWaiter>, fbl::RefPtr<Dispatcher>),
                                        sizeof(Dispatcher*)>;

 public:
  EventWaiter(zx::event event, Callback callback)
      : AsyncLoopOwnedEventHandler<EventWaiter>(std::move(event)), callback_(std::move(callback)) {}

  static void HandleEvent(std::unique_ptr<EventWaiter> event, async_dispatcher_t* dispatcher,
                          async::WaitBase* wait, zx_status_t status,
                          const zx_packet_signal_t* signal);

  // Begins waiting on the provided |async_dispatcher|.
  // This transfers ownership of |event| and the |dispatcher| reference to the async dispatcher.
  // The async dispatcher returns ownership when the handler is invoked.
  static zx_status_t BeginWaitWithRef(std::unique_ptr<EventWaiter> event,
                                      fbl::RefPtr<Dispatcher> dispatcher,
                                      async_dispatcher_t* async_dispatcher);

  bool signaled() const { return signaled_; }

  void signal() {
    ZX_ASSERT(event()->signal(0, ZX_USER_SIGNAL_0) == ZX_OK);
    signaled_ = true;
  }

  void designal() {
    ZX_ASSERT(event()->signal(ZX_USER_SIGNAL_0, 0) == ZX_OK);
    signaled_ = false;
  }

  void InvokeCallback(std::unique_ptr<EventWaiter> event_waiter,
                      fbl::RefPtr<Dispatcher> dispatcher_ref) {
    callback_(std::move(event_waiter), std::move(dispatcher_ref));
  }

  std::unique_ptr<EventWaiter> Cancel() {
    // Cancelling may fail if the callback is happening right now, in which
    // case the callback will take ownership of the dispatcher reference.
    auto event = AsyncLoopOwnedEventHandler<EventWaiter>::Cancel();
    if (event) {
      event->dispatcher_ref_ = nullptr;
    }
    return event;
  }

 private:
  bool signaled_ = false;
  Callback callback_;

  // The EventWaiter is provided ownership of a dispatcher reference when
  // |BeginWaitWithRef| is called, and returns the reference with the callback.
  fbl::RefPtr<Dispatcher> dispatcher_ref_;
};

struct AsyncWaitTag {};

// Indirect wait object which is used to ensure waits are tracked and synchronize waits on
// SYNCHRONIZED dispatchers.
class AsyncWait
    : public CallbackRequest,
      public async_wait_t,
      // This is owned by a Dispatcher, but in two different lists, however only one at a time. We
      // could avoid this by storing |waits_| as a CallbackRequest, however that would require
      // additional casts and pointer math when erasing the wait from the list.
      public fbl::ContainableBaseClasses<fbl::TaggedDoublyLinkedListable<
          std::unique_ptr<AsyncWait>, AsyncWaitTag, fbl::NodeOptions::AllowMultiContainerUptr>> {
 public:
  AsyncWait(async_wait_t* original_wait, Dispatcher& dispatcher);
  ~AsyncWait();

  static zx_status_t BeginWait(std::unique_ptr<AsyncWait> wait, Dispatcher& dispatcher)
      __TA_REQUIRES(&dispatcher.callback_lock_);

  bool Cancel();

  static void Handler(async_dispatcher_t* dispatcher, async_wait_t* wait, zx_status_t status,
                      const zx_packet_signal_t* signal);

  void OnSignal(async_dispatcher_t* async_dispatcher, zx_status_t status,
                const zx_packet_signal_t* signal);

  // Sets the pending_cancellation_ flag to true. See that field's comment for details.
  void MarkPendingCancellation() { pending_cancellation_ = true; }
  bool is_pending_cancellation() const { return pending_cancellation_; }

 private:
  // Implementing a specialization of std::atomic<fbl::RefPtr<T>> is more challenging than just
  // manipulating it as a raw pointer. It must be stored as an atomic because it is mutated from
  // multiple threads after AsyncWait is constructed, and we wish to avoid a lock.
  std::atomic<Dispatcher*> dispatcher_ref_;
  async_wait_t* original_wait_;

  // If true, CancelWait() has been called on another thread and we should cancel the wait rather
  // than invoking the callback.
  //
  // This condition occurs when a wait has been pulled off the dispatcher's port but the callback
  // has not yet been invoked. AsyncWait wraps the underlying async_wait_t callback in its own
  // custom callback (OnSignal), so there is an interval between when OnSignal is invoked and the
  // underlying callback is invoked during which a race with Dispatcher::CancelWait() can occur.
  // See https://fxbug.dev/42061372 for details.
  bool pending_cancellation_ = false;

  // driver_runtime::Callback can store only 2 pointers, so we store other state in the async
  // wait.
  std::optional<zx_packet_signal_t> signal_packet_;
};

// A task which will be triggered at some point in the future.
struct DelayedTask : public CallbackRequest {
  explicit DelayedTask(zx::time deadline, RequestType request_type = RequestType::kTask)
      : CallbackRequest(request_type), deadline(deadline) {}
  zx::time deadline;
};

// Singleton to keep track of allowed scheduler roles.
class AllowedSchedulerRoles {
 public:
  static AllowedSchedulerRoles* Get();

  AllowedSchedulerRoles(const AllowedSchedulerRoles&) = delete;
  AllowedSchedulerRoles& operator=(const AllowedSchedulerRoles&) = delete;

  void AddForDriver(const void* driver, std::string_view role);
  bool IsAllowed(std::string_view role);

 private:
  AllowedSchedulerRoles() = default;

  fbl::Mutex lock_;
  std::unordered_map<const void*, std::unordered_set<std::string>> allowed_roles_
      __TA_GUARDED(&lock_);
};

}  // namespace driver_runtime

#endif  // SRC_DEVICES_BIN_DRIVER_RUNTIME_DISPATCHER_INTERNALS_H_
