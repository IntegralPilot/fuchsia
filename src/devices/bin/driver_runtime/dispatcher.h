// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BIN_DRIVER_RUNTIME_DISPATCHER_H_
#define SRC_DEVICES_BIN_DRIVER_RUNTIME_DISPATCHER_H_

#include <lib/async/dispatcher.h>
#include <lib/fdf/cpp/channel.h>
#include <lib/fdf/env.h>
#include <lib/fdf/token.h>
#include <lib/zx/event.h>

#include <string>
#include <unordered_set>

#include <fbl/auto_lock.h>
#include <fbl/canary.h>
#include <fbl/condition_variable.h>
#include <fbl/intrusive_double_list.h>
#include <fbl/ref_counted.h>
#include <fbl/string_buffer.h>

#include "callback_request.h"
#include "dispatcher_state.h"

namespace driver_runtime {
class Dispatcher;

struct DispatcherInterface : public async_dispatcher_t {
  bool IsVeneer() const;
  Dispatcher* GetDispatcher();
  const Dispatcher* GetDispatcher() const;
  async_dispatcher_t* GetAsyncDispatcher();
  static fdf_dispatcher_t* DowncastAsyncDispatcher(async_dispatcher_t* dispatcher);
};

}  // namespace driver_runtime

struct fdf_dispatcher : public driver_runtime::DispatcherInterface {
  // NOTE: Intentionally empty, do not add to this.
};

namespace driver_runtime {

// Forward Declarations
class ThreadPool;
class EventWaiter;
class AsyncWait;
class AsyncIrq;
struct DelayedTask;
struct AsyncWaitTag;

// CRITICAL: Dispatcher inherits from DispatcherInterface as its first base class.
// This layout is relied upon by the Veneer implementation to allow safe casting
// between Dispatcher* and Veneer* for specific methods. Do not change the base class order!
class Dispatcher : public DispatcherInterface,
                   public fbl::RefCounted<Dispatcher>,
                   public fbl::DoublyLinkedListable<fbl::RefPtr<Dispatcher>> {
  friend class AsyncIrq;
  friend class AsyncWait;

 public:
  // CRITICAL: The layout of this struct is relied upon by `GetDispatcher`
  // and `GetAsyncDispatcher`. `async_dispatcher` MUST be the first member!
  struct Veneer {
    DispatcherInterface async_dispatcher;
    Dispatcher* dispatcher;
    fbl::Canary<fbl::magic("FDFV")> canary_;
  };

  enum class SuspendState : uint8_t {
    // The dispatcher is active and processing tasks.
    kNone,
    // The dispatcher is suspended and not processing power-managed tasks.
    kSuspended,
  };

  // Public for std::make_unique.
  // Use |Create| instead of calling directly.
  Dispatcher(uint32_t options, std::string_view name, bool unsynchronized, bool allow_sync_calls,
             const void* owner, fdf_dispatcher_shutdown_observer_t* observer);
  ~Dispatcher();

  void SetEventWaiter(EventWaiter* event_waiter) __TA_EXCLUDES(&callback_lock_) {
    fbl::AutoLock lock(&callback_lock_);
    event_waiter_ = event_waiter;
  }

  // This must be called before the dispatcher will actually be running.
  void SetThreadPool(ThreadPool* thread_pool, async_dispatcher_t* process_shared_dispatcher) {
    thread_pool_ = thread_pool;
    process_shared_dispatcher_ = process_shared_dispatcher;
  }

  // fdf_dispatcher_t implementation
  // Returns ownership of the dispatcher in |out_dispatcher|. The caller should call
  // |Destroy| once they are done using the dispatcher. Once |Destroy| is called,
  // the dispatcher will be deleted once all callbacks cancelled or completed by the dispatcher.
  static zx_status_t Create(uint32_t options, std::string_view name,
                            std::string_view scheduler_role, fdf_dispatcher_shutdown_observer_t*,
                            Dispatcher** out_dispatcher);

  // fdf_dispatcher_t implementation
  // Returns ownership of the dispatcher in |out_dispatcher|. The caller should call
  // |Destroy| once they are done using the dispatcher. Once |Destroy| is called,
  // the dispatcher will be deleted once all callbacks cancelled or completed by the dispatcher.
  static zx_status_t CreateUnmanagedDispatcher(
      uint32_t options, std::string_view name,
      fdf_dispatcher_shutdown_observer_t* shutdown_observer, Dispatcher** out_dispatcher);

  void ShutdownAsync();
  // If |user_initiated| is true, |Destroy| was called by the user via |fdf_dispatcher_destroy|
  // otherwise |Destroy| was called by the environment via |fdf_env_destroy_all_dispatchers|.
  void Destroy(bool user_initiated = true);
  zx_status_t Seal(uint32_t option);
  fdf_dispatcher_t* GetAlwaysOnDispatcher() {
    return static_cast<fdf_dispatcher_t*>(&veneer_.async_dispatcher);
  }
  fdf_dispatcher_t* to_fdf_dispatcher() {
    return static_cast<fdf_dispatcher_t*>(static_cast<DispatcherInterface*>(this));
  }
  void SuspendAsync(fit::closure completion_callback);
  void Resume();
  void SetResumeRequester(fdf_env_resume_requester_t* requester);
  zx_status_t RegisterWakeVector(zx_handle_t handle, zx_signals_t signals);
  zx_status_t UnregisterWakeVector(zx_handle_t handle, zx_signals_t signals);

  // async_dispatcher_t implementation
  zx_time_t GetTime();
  zx_status_t BeginWait(async_wait_t* wait, bool is_always_on = false);
  zx_status_t CancelWait(async_wait_t* wait);
  zx_status_t PostTask(async_task_t* task, bool is_always_on = false);
  zx_status_t CancelTask(async_task_t* task);
  zx_status_t QueuePacket(async_receiver_t* receiver, const zx_packet_user_t* data);
  zx_status_t BindIrq(async_irq_t* irq);
  zx_status_t UnbindIrq(async_irq_t* irq);
  zx_status_t GetSequenceId(async_sequence_id_t* out_sequence_id, const char** out_error);
  zx_status_t CheckSequenceId(async_sequence_id_t sequence_id, const char** out_error);

  bool HasQueuedTasks();

  // Registers a callback with a dispatcher that should not yet be run.
  // This should be called by the channel if a client has started waiting with a
  // ChannelRead, but the channel has not yet received a write from its peer.
  //
  // Tracking these requests allows the dispatcher to cancel the callback if the
  // dispatcher is destroyed before any write is received.
  //
  // Takes ownership of |callback_request|. If the dispatcher is already shutting down,
  // ownership of |callback_request| will be returned to the caller.
  std::unique_ptr<driver_runtime::CallbackRequest> RegisterCallbackWithoutQueueing(
      std::unique_ptr<CallbackRequest> callback_request);

  // Returns whether a request should be inlined, or queued for later processing.
  fit::result<NonInlinedReason> ShouldInline(std::unique_ptr<CallbackRequest>& request)
      __TA_REQUIRES(&callback_lock_);

  // Queues a previously registered callback to be invoked by the dispatcher.
  // Asserts if no such callback is found.
  // |unowned_callback_request| is used to locate the callback.
  // |callback_reason| is the status that should be set for the callback.
  // |was_deferred| is true if the request was not queued earlier due to a
  // wait not yet been registered on the corresponding channel.
  // Depending on the dispatcher options set and which driver is calling this,
  // the callback can occur on the current thread or be queued up to run on a dispatcher thread.
  void QueueRegisteredCallback(CallbackRequest* unowned_callback_request,
                               zx_status_t callback_reason, bool was_deferred = false);

  // Adds wait to |waits_|.
  void AddWaitLocked(std::unique_ptr<AsyncWait> wait) __TA_REQUIRES(&callback_lock_);
  // Removes wait from |waits_| and triggers idle check.
  std::unique_ptr<AsyncWait> RemoveWait(AsyncWait* wait) __TA_EXCLUDES(&callback_lock_);
  std::unique_ptr<AsyncWait> RemoveWaitLocked(AsyncWait* wait) __TA_REQUIRES(&callback_lock_);
  // Moves wait from |waits_| queue onto |registered_callbacks_| and signals that it can be called.
  void QueueWait(AsyncWait* wait, zx_status_t status);

  // Adds irq to |irqs_|.
  void AddIrqLocked(std::unique_ptr<AsyncIrq> irq) __TA_REQUIRES(&callback_lock_);
  // Removes irq from |irqs_| and triggers idle check.
  std::unique_ptr<AsyncIrq> RemoveIrqLocked(AsyncIrq* irq) __TA_REQUIRES(&callback_lock_);
  // Creates a new callback request for |irq|, queues it onto |registered_callbacks_| and signals
  // that it can be called.
  void QueueIrq(AsyncIrq* irq, zx_status_t status);

  // Removes the callback matching |callback_request| from the queue and returns it.
  // May return nullptr if no such callback is found.
  std::unique_ptr<CallbackRequest> CancelCallback(CallbackRequest& callback_request);

  // Sets the callback reason for a currently queued callback request.
  // This may fail if the callback is already running or scheduled to run.
  // Returns true if a callback matching |callback_request| was found, false otherwise.
  bool SetCallbackReason(CallbackRequest* callback_request, zx_status_t callback_reason);

  // Removes the callback that manages the async dispatcher |operation| and returns it.
  // May return nullptr if no such callback is found.
  std::unique_ptr<CallbackRequest> CancelAsyncOperationLocked(void* operation)
      __TA_REQUIRES(&callback_lock_);

  // Returns true if the dispatcher has no active threads or queued requests.
  // This does not include unsignaled waits, or tasks which have been scheduled
  // for a future deadline.
  // This unlocked version of |IsIdleLocked| is called by tests.
  bool IsIdle() {
    fbl::AutoLock lock(&callback_lock_);
    return IsIdleLocked();
  }

  // Returns ownership of an event that will be signaled once the dispatcher is ready
  // to complete shutdown.
  zx::result<zx::event> RegisterForCompleteShutdownEvent();

  // Blocks the current thread until the dispatcher is idle.
  void WaitUntilIdle();

  // Registers |token| as waiting for an fdf handle to be transferred. This |token| is already
  // registered with the token manager, but this allows the dispatcher to call the token
  // transfer cancellation callback in the case where the dispatcher shuts down before the
  // transfer is completed. This is as the token manager would not be able to queue a
  // cancellation callback once the dispatcher is in a shutdown state.
  zx_status_t RegisterPendingToken(fdf_token_t* token);
  // Unregisters |token| as waiting for an fdf handle to be transferred.
  void UnregisterPendingToken(fdf_token_t* token);
  // Queues a |CallbackRequest| for the token transfer callback and removes |token|
  // from the pending list. This is called when |fdf_token_register| and |fdf_token_transfer|
  // have been called for the same token.
  // TODO(https://fxbug.dev/42056822): replace fdf::Channel with a generic C++ handle type when
  // available.
  zx_status_t ScheduleTokenCallback(fdf_token_t* token, zx_status_t status, fdf::Channel channel);

  // Dumps the dispatcher state as a vector of formatted strings.
  void DumpToString(std::vector<std::string>* dump_out);
  void DumpToStringLocked(std::vector<std::string>* dump_out) __TA_REQUIRES(&callback_lock_);
  // Dumps the dispatcher state to |out_state|.
  void Dump(DumpState* out_state);
  void DumpLocked(DumpState* out_state) __TA_REQUIRES(&callback_lock_);
  // Converts |dump_state| to a vector of formatted strings.
  // Any existing contents in |dump_out| will be cleared.
  void FormatDump(DumpState* dump_state, std::vector<std::string>* dump_out);

  // Returns the dispatcher options specified by the user.
  uint32_t options() const { return options_; }
  bool unsynchronized() const { return unsynchronized_; }
  bool allow_sync_calls() const { return allow_sync_calls_.load(); }

  // Returns the driver which owns this dispatcher.
  const void* owner() const { return owner_; }

  // Returns the thread pool that backs this dispatcher.
  ThreadPool* thread_pool() { return thread_pool_; }

  const async_dispatcher_t* process_shared_dispatcher() const { return process_shared_dispatcher_; }

  // For use by testing only.
  size_t callback_queue_size_slow() {
    fbl::AutoLock lock(&callback_lock_);
    return callback_queue_.size_slow();
  }

  void AssertCanary() const { canary_.Assert(); }

 private:
  // TODO(https://fxbug.dev/42168999): determine an appropriate size.
  static constexpr uint32_t kBatchSize = 10;

  class CompleteShutdownEventManager {
   public:
    // Returns a duplicate of the event that will be signaled when the dispatcher
    // is ready to complete shutdown.
    zx::result<zx::event> GetEvent();
    // Signal and reset the idle event.
    zx_status_t Signal();

   private:
    zx::event event_;
  };

  // A timer primitive built on top of an async task.
  // We do not use |async::Task|, as |async::Task::Cancel| will assert that cancellation is
  // successful.
  class Timer : public async_task_t {
   public:
    explicit Timer(Dispatcher* dispatcher)
        : async_task_t{{ASYNC_STATE_INIT}, &Timer::Handler, ZX_TIME_INFINITE},
          dispatcher_(dispatcher) {}

    zx_status_t BeginWait(zx::time deadline) {
      ZX_ASSERT(is_armed() == false);
      this->deadline = deadline.get();
      zx_status_t status = async_post_task(dispatcher_->process_shared_dispatcher_, this);
      if (status == ZX_OK) {
        current_deadline_ = deadline;
      }
      return status;
    }

    bool is_armed() const { return current_deadline_ != zx::time::infinite(); }

    zx_status_t Cancel() {
      if (!is_armed()) {
        // Nothing to cancel.
        return ZX_OK;
      }

      zx_status_t status = async_cancel_task(dispatcher_->process_shared_dispatcher_, this);
      // ZX_ERR_NOT_FOUND can happen here when a pending timer fires and
      // the packet is picked up by port_wait in another thread but has
      // not reached dispatch.
      ZX_ASSERT(status == ZX_OK || status == ZX_ERR_NOT_FOUND);
      if (status == ZX_OK) {
        current_deadline_ = zx::time::infinite();
      }
      return status;
    }

    zx::time current_deadline() const { return current_deadline_; }

   private:
    static void Handler(async_dispatcher_t* dispatcher, async_task_t* task, zx_status_t status) {
      auto self = static_cast<Timer*>(task);
      if (status == ZX_OK) {
        self->Handler();
      }
    }

    void Handler();

    // zx::time::infinite() means we are not scheduled.
    zx::time current_deadline_ = zx::time::infinite();
    Dispatcher* dispatcher_;
  };

  zx::time GetNextTimeoutLocked() const __TA_REQUIRES(&callback_lock_);
  void ResetTimerLocked(bool force = false) __TA_REQUIRES(&callback_lock_);
  bool IsWakeVectorLocked(zx_handle_t handle, zx_signals_t signals) const
      __TA_REQUIRES(&callback_lock_);
  void InsertDelayedTaskSortedLocked(std::unique_ptr<DelayedTask> task)
      __TA_REQUIRES(&callback_lock_);
  void CheckDelayedTasksLocked() __TA_REQUIRES(&callback_lock_);

  // Calls |callback_request|.
  void DispatchCallback(std::unique_ptr<driver_runtime::CallbackRequest> callback_request);
  // Calls the callbacks in |callback_queue_|.
  void DispatchCallbacks(std::unique_ptr<EventWaiter> event_waiter,
                         fbl::RefPtr<Dispatcher> dispatcher_ref);
  // Moves the next callbacks to dispatch from |callback_queue_| to |out_callbacks|.
  // Returns the number of callbacks in |out_callbacks|.
  uint32_t TakeNextCallbacks(fbl::DoublyLinkedList<std::unique_ptr<CallbackRequest>>* out_callbacks)
      __TA_REQUIRES(&callback_lock_);

  // Cancels the callbacks in |shutdown_queue_|.
  void CompleteShutdown();

  // Returns true if the dispatcher has no active threads or queued requests.
  // This does not include unsignaled waits.
  bool IsIdleLocked() __TA_REQUIRES(&callback_lock_);

  // Returns true if the dispatcher has waits or tasks scheduled for a future deadline.
  // This includes unsignaled waits and delayed tasks.
  bool HasFutureOpsScheduledLocked() __TA_REQUIRES(&callback_lock_);

  // Checks whether the dispatcher has entered and idle state and if so notifies any registered
  // waiters.
  void IdleCheckLocked() __TA_REQUIRES(&callback_lock_);

  // Returns true if the current thread is managed by the driver runtime.
  bool IsRuntimeManagedThread() { return !thread_context::IsCallStackEmpty(); }

  // Returns whether the dispatcher is in the running state.
  bool IsRunningLocked() __TA_REQUIRES(&callback_lock_) {
    return state_ == DispatcherState::kRunning;
  }

  // User provided name. Useful for debugging purposes.
  fbl::StringBuffer<ZX_MAX_NAME_LEN> name_;

  // Dispatcher options set by the user.
  uint32_t options_;
  bool unsynchronized_;
  std::atomic_bool allow_sync_calls_;

  // The driver which owns this dispatcher. May be nullptr if undeterminable.
  const void* const owner_;

  ThreadPool* thread_pool_;
  // Global dispatcher shared across all dispatchers in a process.
  async_dispatcher_t* process_shared_dispatcher_;
  EventWaiter* event_waiter_ __TA_GUARDED(&callback_lock_);

  fbl::Mutex callback_lock_;
  // Callback requests that have been registered by channels, but not yet queued.
  // This occurs when a client has started waiting on a channel, but the channel
  // has not yet received a write from its peer.
  fbl::DoublyLinkedList<std::unique_ptr<CallbackRequest>> registered_callbacks_
      __TA_GUARDED(&callback_lock_);
  // Queued callback requests from channels. These are requests that should
  // be run on the next available thread.
  fbl::DoublyLinkedList<std::unique_ptr<CallbackRequest>> callback_queue_
      __TA_GUARDED(&callback_lock_);
  // Callback requests that have been removed to be completed by |CompleteShutdown|.
  // These are removed from the active queues to ensure the dispatcher does not
  // attempt to continue processing them.
  fbl::DoublyLinkedList<std::unique_ptr<CallbackRequest>> shutdown_queue_
      __TA_GUARDED(&callback_lock_);

  // Waits which are queued up against |process_shared_dispatcher|. These are moved onto the
  // |registered_callbacks_| queue once completed. They are tracked so that they may be canceled
  // during |Destroy| prior to calling |CompleteDestroy|.
  fbl::TaggedDoublyLinkedList<std::unique_ptr<AsyncWait>, AsyncWaitTag> waits_
      __TA_GUARDED(&callback_lock_);

  // Irqs which are bound to the dispatcher. A new callback request is added to
  // the |registered_callbacks_| queue when an interrupt is triggered.
  // They are tracked so that they may be canceled during |Destroy| prior to calling
  // |CompleteDestroy|.
  fbl::DoublyLinkedList<std::unique_ptr<AsyncIrq>> irqs_ __TA_GUARDED(&callback_lock_);

  Timer timer_ __TA_GUARDED(&callback_lock_);
  // True if the dispatcher has begun shutting down, but is waiting on the timer
  // handler to run and complete in another thread.
  bool shutdown_waiting_for_timer_ __TA_GUARDED(&callback_lock_) = false;

  // Tasks which should move into callback_queue as soon as they are ready.
  // Sorted by earliest deadline first.
  fbl::DoublyLinkedList<std::unique_ptr<CallbackRequest>> delayed_tasks_
      __TA_GUARDED(&callback_lock_);

  // True if currently dispatching a message.
  // This is only relevant in the synchronized mode.
  bool dispatching_sync_ __TA_GUARDED(&callback_lock_) = false;

  // TODO(https://fxbug.dev/42180016): consider using std::atomic.
  DispatcherState state_ __TA_GUARDED(&callback_lock_) = DispatcherState::kRunning;

  SuspendState suspend_state_ __TA_GUARDED(&callback_lock_) = SuspendState::kNone;

  // Queued callback requests that are deferred while the dispatcher is suspended.
  fbl::DoublyLinkedList<std::unique_ptr<CallbackRequest>> sleep_queue_
      __TA_GUARDED(&callback_lock_);

  // Queued callback requests for wake vectors that triggered while suspended.
  fbl::DoublyLinkedList<std::unique_ptr<CallbackRequest>> wake_queue_ __TA_GUARDED(&callback_lock_);

  // Tasks originating from the always-on veneer which should move into callback_queue as soon as
  // they are ready.
  fbl::DoublyLinkedList<std::unique_ptr<CallbackRequest>> always_on_delayed_tasks_
      __TA_GUARDED(&callback_lock_);

  // Number of power-managed tasks currently executing.
  size_t executing_power_managed_tasks_ __TA_GUARDED(&callback_lock_) = 0;
  size_t executing_wake_vectors_ __TA_GUARDED(&callback_lock_) = 0;

  std::unordered_map<zx_handle_t, zx_signals_t> wake_vectors_ __TA_GUARDED(&callback_lock_);

  fdf_env_resume_requester_t* resume_requester_ __TA_GUARDED(&callback_lock_) = nullptr;

  fit::closure suspend_completion_callback_ __TA_GUARDED(&callback_lock_);

  // If a call to |Destroy| has been made, this will store the name of the dispatcher that made the
  // call.
  std::string dispatcher_destroy_context_ __TA_GUARDED(callback_lock_);
  // If true, |Destroy| was called by the user via |fdf_dispatcher_destroy|,
  // otherwise |Destroy| was called by the environment via |fdf_env_destroy_all_dispatchers|.
  std::optional<bool> dispatcher_destroy_user_initiated_ __TA_GUARDED(callback_lock_);

  // Number of threads currently servicing callbacks.
  size_t num_active_threads_ __TA_GUARDED(&callback_lock_) = 0;

  // Stats for debugging a dispatcher.
  DebugStats debug_stats_ __TA_GUARDED(&callback_lock_) = {};

  CompleteShutdownEventManager complete_shutdown_event_manager_ __TA_GUARDED(&callback_lock_);

  // Notified when the dispatcher enters an idle state, not including pending waits or delayed
  // tasks.
  fbl::ConditionVariable idle_event_ __TA_GUARDED(&callback_lock_);

  // The observer that should be called when shutting down the dispatcher completes.
  fdf_dispatcher_shutdown_observer_t* shutdown_observer_ __TA_GUARDED(&callback_lock_) = nullptr;

  // Tokens registered with the token manager, that are waiting for fdf handles to
  // be transferred,
  std::unordered_set<fdf_token_t*> registered_tokens_ __TA_GUARDED(&callback_lock_);

  Veneer veneer_;

  fbl::Canary<fbl::magic("FDFD")> canary_;
};

extern const async_ops_t g_veneer_ops;

inline bool DispatcherInterface::IsVeneer() const { return ops == &g_veneer_ops; }

inline Dispatcher* DispatcherInterface::GetDispatcher() {
  if (IsVeneer()) {
    auto veneer = reinterpret_cast<Dispatcher::Veneer*>(this);
    return veneer->dispatcher;
  }
  return static_cast<Dispatcher*>(this);
}

inline const Dispatcher* DispatcherInterface::GetDispatcher() const {
  if (IsVeneer()) {
    auto veneer = reinterpret_cast<const Dispatcher::Veneer*>(this);
    return veneer->dispatcher;
  }
  return static_cast<const Dispatcher*>(this);
}

inline async_dispatcher_t* DispatcherInterface::GetAsyncDispatcher() {
  if (IsVeneer()) {
    auto veneer = reinterpret_cast<Dispatcher::Veneer*>(this);
    return &veneer->async_dispatcher;
  }
  return static_cast<async_dispatcher_t*>(this);
}

inline fdf_dispatcher_t* DispatcherInterface::DowncastAsyncDispatcher(
    async_dispatcher_t* dispatcher) {
  auto interface = static_cast<DispatcherInterface*>(dispatcher);
  if (interface->IsVeneer()) {
    auto veneer = reinterpret_cast<Dispatcher::Veneer*>(interface);
    veneer->canary_.Assert();
    return static_cast<fdf_dispatcher_t*>(interface);
  }
  auto concrete = static_cast<Dispatcher*>(interface);
  concrete->AssertCanary();
  return static_cast<fdf_dispatcher*>(interface);
}

}  // namespace driver_runtime

#endif  // SRC_DEVICES_BIN_DRIVER_RUNTIME_DISPATCHER_H_
