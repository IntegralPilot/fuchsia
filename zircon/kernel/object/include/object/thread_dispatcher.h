// Copyright 2016 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_OBJECT_INCLUDE_OBJECT_THREAD_DISPATCHER_H_
#define ZIRCON_KERNEL_OBJECT_INCLUDE_OBJECT_THREAD_DISPATCHER_H_

#include <arch.h>
#include <platform.h>
#include <sys/types.h>
#include <zircon/compiler.h>
#include <zircon/syscalls/debug.h>
#include <zircon/syscalls/exception.h>
#include <zircon/syscalls/object.h>
#include <zircon/types.h>

#include <arch/exception.h>
#include <fbl/intrusive_double_list.h>
#include <fbl/ref_counted.h>
#include <fbl/ref_ptr.h>
#include <kernel/event.h>
#include <kernel/ffi.h>
#include <kernel/owned_wait_queue.h>
#include <kernel/thread.h>
#include <ktl/atomic.h>
#include <ktl/string_view.h>
#include <object/channel_dispatcher.h>
#include <object/dispatcher.h>
#include <object/exceptionate.h>
#include <object/futex_context.h>
#include <object/handle.h>
#include <object/thread_state.h>
#include <vm/vm_address_region.h>

class ProcessDispatcher;
class ThreadDispatcher;

class ThreadDispatcher final : public SoloDispatcher<ThreadDispatcher, ZX_DEFAULT_THREAD_RIGHTS>,
                               public fbl::DoublyLinkedListable<ThreadDispatcher*> {
 public:
  // When in a blocking syscall, or blocked in an exception, the blocking reason.
  // There is one of these for each syscall marked "blocking".
  // See //zircon/vdso.
  enum class Blocked : uint32_t {
    // Not blocked.
    NONE,
    // The thread is blocked in an exception.
    EXCEPTION,
    // The thread is sleeping (zx_nanosleep).
    SLEEPING,
    // zx_futex_wait
    FUTEX,
    // zx_port_wait
    PORT,
    // zx_channel_call
    CHANNEL,
    // zx_object_wait_one
    WAIT_ONE,
    // zx_object_wait_many
    WAIT_MANY,
    // zx_interrupt_wait
    INTERRUPT,
    // pager
    PAGER,
  };

  using EntryState = UserEntryState;

  static zx_status_t Create(fbl::RefPtr<ProcessDispatcher> process, uint32_t flags,
                            ktl::string_view name, KernelHandle<ThreadDispatcher>* out_handle,
                            zx_rights_t* out_rights);
  ~ThreadDispatcher();

  static ThreadDispatcher* GetCurrent() { return Thread::Current::Get()->user_thread(); }

  // Terminates the current thread. Does not return.
  static void ExitCurrent() __NO_RETURN { Thread::Current::Exit(0); }
  // Marks the current thread for termination. The thread will actually termiante when
  // the kernel stack unwinds.
  static void KillCurrent() { Thread::Current::Kill(); }

  // Dispatcher implementation.
  zx_obj_type_t get_type() const final { return ZX_OBJ_TYPE_THREAD; }
  zx_koid_t get_related_koid() const final;

  // Sets whether or not this is the initial thread in its process.
  // Should only be called by ProcessDispatcher upon adding the initialized thread.
  void set_is_initial_thread(bool is_initial_thread) { is_initial_thread_ = is_initial_thread; }

  // Performs initialization on a newly constructed ThreadDispatcher
  // If this fails, then the object is invalid and should be deleted
  zx_status_t Initialize() TA_EXCL(get_lock());
  // Start this thread running inside the parent process with the provided entry state, only
  // valid to be called on a thread in the INITIALIZED state that has not yet been started. If
  // `ensure_initial_thread` is true, the thread will only start if it is the first thread in the
  // process.
  zx_status_t Start(const EntryState& entry, bool ensure_initial_thread);
  // Transitions a thread from the INITIALIZED state to either the RUNNING or SUSPENDED state.
  // Is the caller's responsibility to ensure this thread is registered with the parent process,
  // as such this is only expected to be called from the ProcessDispatcher.
  zx_status_t MakeRunnable(const EntryState& entry, bool suspended);
  void Kill();

  // Suspends the thread.
  // Returns ZX_OK on success, or ZX_ERR_BAD_STATE iff the thread is dying or dead.
  zx_status_t Suspend();
  void Resume();

  // Issues a restricted kick on the thread which will kick the thread out of restricted
  // mode to normal mode if it's currently in restricted mode or remember the kick state for
  // the next attempt to enter restricted state.
  // Returns ZX_OK on success or ZX_ERR_BAD_STATE iff the thread is dying or dead.
  zx_status_t RestrictedKick();

  // accessors
  ProcessDispatcher* process() const { return process_.get(); }

  // Returns true if the thread is dying or dead. Threads never return to a previous state
  // from dying/dead so once this is true it will never flip back to false.
  bool IsDyingOrDead() const TA_EXCL(get_lock());

  // Returns true if the thread was ever started (even if it is dead now).
  // Threads never return to an INITIAL state after starting, so once this is
  // true it will never flip back to false.
  bool HasStarted() const TA_EXCL(get_lock());

  [[nodiscard]] zx_status_t set_name(const char* name, size_t len) final __NONNULL((2))
      TA_EXCL(get_lock());
  [[nodiscard]] zx_status_t get_name(char (&out_name)[ZX_MAX_NAME_LEN]) const final
      TA_EXCL(get_lock());

  // Assuming the thread is stopped waiting for an exception response,
  // fill in |*report| with the exception report.
  // Returns ZX_ERR_BAD_STATE if not in an exception.
  zx_status_t GetExceptionReport(zx_exception_report_t* report);

  Exceptionate* exceptionate();

  // Sends an exception over the exception channel and blocks for a response.
  //
  // |sent| will indicate whether the exception was successfully sent over
  // the given |exceptionate| channel. This can be used in the ZX_ERR_NEXT
  // case to determine whether the exception channel didn't exist or it did
  // exist but the receiver opted not to handle the exception.
  //
  // Returns:
  //   ZX_OK if the exception was processed and the thread should resume.
  //   ZX_ERR_NEXT if there is no channel or the receiver opted to skip.
  //   ZX_ERR_NO_MEMORY on allocation failure.
  //   ZX_ERR_INTERNAL_INTR_KILLED if the thread was killed before
  //       receiving a response.
  zx_status_t HandleException(Exceptionate* exceptionate,
                              fbl::RefPtr<ExceptionDispatcher> exception, bool* sent);

  // Similar to HandleException(), but for single-shot exceptions which are
  // sent to at most one handler, e.g. ZX_EXCP_THREAD_STARTING.
  //
  // The main difference is that this takes |exception_type| and |context|
  // rather than a full exception object, and internally sets up the required
  // state and creates the exception object.
  //
  // Returns true if the exception was sent.
  bool HandleSingleShotException(Exceptionate* exceptionate, zx_excp_type_t exception_type,
                                 const arch_exception_context_t& context) TA_EXCL(get_lock());

  // Fetch the state of the thread for userspace tools.
  zx_info_thread_t GetInfoForUserspace() const;

  // Fetch per thread stats for userspace.
  zx_status_t GetStatsForUserspace(zx_info_thread_stats_t* info) TA_EXCL(get_lock());

  // Fetch a consistent snapshot of the runtime stats, compensated for unaccumulated runtime in the
  // ready or running state.
  TaskRuntimeStats GetCompensatedTaskRuntimeStats() const;
  zx_info_task_runtime_t GetRuntimeStats() const { return GetCompensatedTaskRuntimeStats(); }

  // For debugger usage.
  zx_status_t ReadState(zx_thread_state_topic_t state_kind, user_out_ptr<void> buffer,
                        size_t buffer_size) TA_EXCL(get_lock());
  zx_status_t WriteState(zx_thread_state_topic_t state_kind, user_in_ptr<const void> buffer,
                         size_t buffer_size) TA_EXCL(get_lock());

  // Profile support
  zx_status_t SetBaseProfile(const SchedulerState::BaseProfile& profile) TA_EXCL(get_lock());
  zx_status_t SetSoftAffinity(cpu_mask_t mask) TA_EXCL(get_lock());

  // For ChannelDispatcher use.
  ChannelDispatcher::MessageWaiter* GetMessageWaiter() { return &channel_waiter_; }

  // Blocking syscalls, once they commit to a path that will likely block the
  // thread, use this helper class to properly set/restore |blocked_reason_|.
  class AutoBlocked final {
   public:
    explicit AutoBlocked(Blocked reason)
        : thread_(ThreadDispatcher::GetCurrent()),
          prev_reason(thread_->blocked_reason_.load(ktl::memory_order_acquire)) {
      DEBUG_ASSERT(reason != Blocked::NONE);
      thread_->blocked_reason_.store(reason, ktl::memory_order_release);
    }
    ~AutoBlocked() { thread_->blocked_reason_.store(prev_reason, ktl::memory_order_release); }

   private:
    ThreadDispatcher* const thread_;
    const Blocked prev_reason;
  };

  static Blocked SetBlockedReason(Blocked reason) {
    ThreadDispatcher* thread = ThreadDispatcher::GetCurrent();
    Blocked prev = thread->blocked_reason_.load(ktl::memory_order_acquire);
    thread->blocked_reason_.store(reason, ktl::memory_order_release);
    return prev;
  }

  // This is called from Thread as it is exiting, just before it stops for good.
  // It is an error to call this on anything other than the current thread.
  void ExitingCurrent();

  // callback from kernel when thread is suspending
  void Suspending();
  // callback from kernel when thread is resuming
  void Resuming();

  // Update the runtime stats for this thread/process. This is called by
  // Scheduler to update the runtime stats of the thread as it changes thread
  // state.
  //
  // Must be called with interrupts disabled.
  void UpdateRuntimeStats(thread_state new_state);

  // Update time spent handling page faults. This is called by the VM during page fault handling.
  void AddPageFaultTicks(zx_duration_mono_ticks_t ticks);

  // Update time spent contended on locks. This is called by lock implementations.
  void AddLockContentionTicks(zx_duration_mono_ticks_t ticks);

  class CoreThreadObservation {
   public:
    CoreThreadObservation() = default;
    ~CoreThreadObservation() { Release(); }

    void Release() {
      if (core_thread_ != nullptr) {
        DEBUG_ASSERT(lock_ != nullptr);

        lock_->AssertHeld();
        [this]() TA_NO_THREAD_SAFETY_ANALYSIS { lock_->Release(); }();

        core_thread_ = nullptr;
        lock_ = nullptr;
      } else {
        DEBUG_ASSERT(lock_ == nullptr);
      }
    }

    Thread* core_thread() const { return core_thread_; }

    CoreThreadObservation(const CoreThreadObservation&) = delete;
    CoreThreadObservation& operator=(const CoreThreadObservation&) = delete;

    CoreThreadObservation(CoreThreadObservation&& other) { *this = ktl::move(other); }

    CoreThreadObservation& operator=(CoreThreadObservation&& other) {
      // We should only ever be moving into a CoreThreadObservation instance
      // which has been default constructed (eg, empty)
      DEBUG_ASSERT((core_thread_ == nullptr) && (lock_ == nullptr));

      core_thread_ = other.core_thread_;
      lock_ = other.lock_;
      other.core_thread_ = nullptr;
      other.lock_ = nullptr;

      return *this;
    }

   private:
    friend class ThreadDispatcher;

    CoreThreadObservation(Thread* core_thread, SpinLock* lock)
        : core_thread_(core_thread), lock_(lock) {
      if (lock_ != nullptr) {
        [this]() TA_NO_THREAD_SAFETY_ANALYSIS { lock_->Acquire(); }();
      }
    }

    Thread* core_thread_{nullptr};
    SpinLock* lock_{nullptr};
  };

  CoreThreadObservation ObserveCoreThread() TA_REQ(get_lock()) {
    return CoreThreadObservation{core_thread_, &core_thread_lock_.lock()};
  }

 private:
  ThreadDispatcher(fbl::RefPtr<ProcessDispatcher> process, uint32_t flags);
  ThreadDispatcher(const ThreadDispatcher&) = delete;
  ThreadDispatcher& operator=(const ThreadDispatcher&) = delete;

  // friend FutexContext so that it can manipulate the blocking_futex_id_ member of
  // ThreadDispatcher, and so that it can access the "thread_" member of the class so that
  // wait_queue operations can be performed on ThreadDispatchers
  friend class FutexContext;

  // OwnedWaitQueue is a friend only so that it can access the blocking_futex_id_ member for tracing
  // purposes.
  friend class OwnedWaitQueue;

  // kernel level entry point
  static int StartRoutine(void* arg);

  // Return true if waiting for an exception response.
  bool InExceptionLocked() TA_REQ(get_lock());

  // Returns true if the thread is suspended or processing an exception.
  bool SuspendedOrInExceptionLocked() TA_REQ(get_lock());

  // change states of the object, do what is appropriate for the state transition
  void SetStateLocked(ThreadState::Lifecycle lifecycle) TA_REQ(get_lock());

  bool IsDyingOrDeadLocked() const TA_REQ(get_lock());

  bool HasStartedLocked() const TA_REQ(get_lock());

  template <typename T, typename F>
  zx_status_t ReadStateGeneric(F get_state_func, user_out_ptr<void> buffer, size_t buffer_size)
      TA_EXCL(get_lock());
  template <typename T, typename F>
  zx_status_t WriteStateGeneric(F set_state_func, user_in_ptr<const void> buffer,
                                size_t buffer_size) TA_EXCL(get_lock());

  void ResetCoreThread() TA_REQ(get_lock(), core_thread_lock_) {
    DEBUG_ASSERT(core_thread_ != nullptr);
    core_thread_ = nullptr;
  }

  // a ref pointer back to the parent process.
  const fbl::RefPtr<ProcessDispatcher> process_;

  // The runtime stats for this thread. Placed near the front of ThreadDispatcher due to frequent
  // updates by the scheduler.
  ThreadRuntimeStats runtime_stats_;

  // The thread as understood by the lower kernel. This is set to nullptr when
  // `state_` transitions to DEAD.
  Thread* core_thread_ TA_GUARDED(get_lock()) = nullptr;

  // User thread starting register values.
  EntryState user_entry_;

  ThreadState state_ TA_GUARDED(get_lock());

  // This is only valid while |state_.lifecycle()| is RUNNING.
  //
  // This field is an atomic because it may be accessed concurrently by multiple
  // threads.  It may be read by any thread, but may only be updated by the
  // "this" thread.
  //
  // In general, loads of this field should be performed with acquire semantics
  // and stores with release semantics because this field is used to synchronize
  // threads (think: wait for a thread to become blocked, then inspect some
  // state the thread has written).
  //
  // Because this is simply an atomic, readers must be OK with observing stale
  // values.  That is, by the time a reader can take action on the value, the
  // value may no longer be accurate.
  ktl::atomic<Blocked> blocked_reason_ = Blocked::NONE;

  // Support for sending an exception to an exception handler and then waiting for a response.
  // Exceptionates have internal locking so we don't need to guard it here.
  Exceptionate exceptionate_;

  // Non-null if the thread is currently processing a channel exception.
  fbl::RefPtr<ExceptionDispatcher> exception_ TA_GUARDED(get_lock());

  // Holds the type of the exceptionate currently processing the exception,
  // which may be our |exceptionate_| or one of our parents'.
  uint32_t exceptionate_type_ TA_GUARDED(get_lock()) = ZX_EXCEPTION_CHANNEL_TYPE_NONE;

  // Tracks the number of times Suspend() has been called. Resume() will resume this thread
  // only when this reference count reaches 0.
  int suspend_count_ TA_GUARDED(get_lock()) = 0;

  // Per-thread structure used while waiting in a ChannelDispatcher::Call.
  // Needed to support the requirements of being able to interrupt a Call
  // in order to suspend a thread.
  ChannelDispatcher::MessageWaiter channel_waiter_;

  // If true and ancestor job has a debugger attached, thread will block on
  // start and will send a process start exception.
  bool is_initial_thread_ = false;

  // The ID of the futex we are currently waiting on, or 0 if we are not
  // waiting on any futex at the moment.
  //
  // TODO(johngro): figure out some way to apply clang static thread analysis
  // to this.  Right now, there is no good (cost free) way for the compiler to
  // figure out that this thread belongs to a specific process/futex-context,
  // and therefor the thread's futex-context lock can be used to guard this
  // futex ID.
  FutexId blocking_futex_id_{FutexId::Null()};
  // A lower level lock which is needed to satisfy some odd synchronization
  // requirements when blocking a thread in a futex and declaring a new owner in
  // the process.
  DECLARE_SPINLOCK(ThreadDispatcher) core_thread_lock_;
};

extern "C" {
bool cpp_thread_dispatcher_is_current(const ThreadDispatcher* thread);
zx_status_t cpp_thread_dispatcher_create(
    ProcessDispatcher* process, uint32_t flags, const char* name_ptr, size_t name_len,
    ffi::Uninitialized<KernelHandle<ThreadDispatcher>>* out_handle,
    ffi::Uninitialized<zx_rights_t>* out_rights);
zx_status_t cpp_thread_dispatcher_initialize(ThreadDispatcher* thread);
zx_status_t cpp_thread_dispatcher_start(ThreadDispatcher* thread, zx_vaddr_t entry,
                                        zx_vaddr_t stack, uint64_t arg1, uint64_t arg2, uint64_t tp,
                                        uint64_t abi_reg, bool ensure_initial_thread);
void cpp_thread_dispatcher_exit_current();
void cpp_thread_dispatcher_kill_current();
void cpp_thread_dispatcher_kill(ThreadDispatcher* thread);
zx_status_t cpp_thread_dispatcher_suspend(ThreadDispatcher* thread);
void cpp_thread_dispatcher_resume(ThreadDispatcher* thread);
zx_status_t cpp_thread_dispatcher_restricted_kick(ThreadDispatcher* thread);
zx_status_t cpp_thread_dispatcher_read_state(ThreadDispatcher* thread, uint32_t state_kind,
                                             void* buffer, size_t buffer_size);
zx_status_t cpp_thread_dispatcher_write_state(ThreadDispatcher* thread, uint32_t state_kind,
                                              const void* buffer, size_t buffer_size);
zx_status_t cpp_thread_dispatcher_set_base_profile(ThreadDispatcher* thread,
                                                   const SchedulerState::BaseProfile* profile);
zx_status_t cpp_thread_dispatcher_set_soft_affinity(ThreadDispatcher* thread, cpu_mask_t mask);
void cpp_thread_dispatcher_get_info_for_userspace(const ThreadDispatcher* thread,
                                                  ffi::Uninitialized<zx_info_thread_t>* out_info);
zx_status_t cpp_thread_dispatcher_get_stats_for_userspace(
    ThreadDispatcher* thread, ffi::Uninitialized<zx_info_thread_stats_t>* out_info);
void cpp_thread_dispatcher_get_runtime_stats(const ThreadDispatcher* thread,
                                             ffi::Uninitialized<zx_info_task_runtime_t>* out_info);
zx_status_t cpp_sys_thread_raise_exception(uint32_t options, zx_excp_type_t type,
                                           const zx_exception_context_t* user_context);
zx_status_t cpp_sys_thread_legacy_yield(uint32_t options);
ThreadDispatcher::Blocked cpp_thread_dispatcher_set_blocked_reason(
    ThreadDispatcher::Blocked reason);
zx_status_t cpp_thread_dispatcher_get_exception_report(
    const ThreadDispatcher* thread, ffi::Uninitialized<zx_exception_report_t>* out_report);
}

#endif  // ZIRCON_KERNEL_OBJECT_INCLUDE_OBJECT_THREAD_DISPATCHER_H_
