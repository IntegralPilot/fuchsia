// Copyright 2016 The Fuchsia Authors
// Copyright (c) 2008-2015 Travis Geiselbrecht
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_INCLUDE_KERNEL_THREAD_H_
#define ZIRCON_KERNEL_INCLUDE_KERNEL_THREAD_H_

#include <arch.h>
#include <debug.h>
#include <lib/backtrace.h>
#include <lib/concurrent/capability_token.h>
#include <lib/fit/function.h>
#include <lib/flow_id.h>
#include <lib/fxt/thread_ref.h>
#include <lib/kconcurrent/chainlock.h>
#include <lib/page-map/accessor.h>
#include <lib/relaxed_atomic.h>
#include <lib/zircon-internal/thread_annotations.h>
#include <lib/zx/result.h>
#include <platform.h>
#include <sys/types.h>
#include <zircon/compiler.h>
#include <zircon/syscalls/object.h>
#include <zircon/syscalls/rseq.h>
#include <zircon/syscalls/scheduler.h>
#include <zircon/types.h>

#include <arch/arch_thread.h>
#include <arch/exception.h>
#include <arch/ops.h>
#include <fbl/canary.h>
#include <fbl/intrusive_double_list.h>
#include <fbl/intrusive_single_list.h>
#include <fbl/macros.h>
#include <fbl/null_lock.h>
#include <fbl/wavl_tree_best_node_observer.h>
#include <kernel/cpu.h>
#include <kernel/deadline.h>
#include <kernel/koid.h>
#include <kernel/preempt_disabled_token.h>
#include <kernel/restricted_state.h>
#include <kernel/scheduler_state.h>
#include <kernel/spinlock.h>
#include <kernel/task_runtime_stats.h>
#include <kernel/timer.h>
#include <kernel/wait.h>
#include <ktl/array.h>
#include <ktl/atomic.h>
#include <ktl/optional.h>
#include <ktl/string_view.h>
#include <ktl/unique_ptr.h>
#include <lockdep/thread_lock_state.h>
#include <vm/kstack.h>

class Dpc;
struct BrwLockOps;
class OwnedWaitQueue;
class ThreadDispatcher;
class VmAspace;
struct WaitQueueLockOps;
struct Thread;
class ThreadDumper;  // A class which is a friend of the various internal thread
                     // state classes and structures, allowing it to access
                     // private variables for debugging purposes during debug
                     // dumps of thread state.

// These forward declarations are needed so that Thread can friend
// them before they are defined.
static inline Thread* arch_get_current_thread();
static inline void arch_set_current_thread(Thread*);

// The types of affinity the scheduler understands.
enum class AffinityType : bool { Hard, Soft };

static inline fbl::NullLock is_current_thread_token;

// Returns a string constant for the given thread state.
const char* ToString(enum thread_state state);

// TODO(https://fxbug.dev/528405969): Remove cfi_unchecked_callee once cross-language
// CFI between Clang C++ and rustc is supported in the kernel.
typedef int (*thread_start_routine)(void* arg) [[clang::cfi_unchecked_callee]];

// Thread trampolines are always called with:
//
// 1) The thread's lock held
// 2) interrupts disabled.
//
// Implementers of trampolines must always remember to unconditionally release
// the current thread's lock, and *then* enable interrupts, before jumping to
// the thread's entry point.
//
// Sadly, there does not seem to be a good way to statically annotate this.
typedef void (*thread_trampoline_routine)() __NO_RETURN;

// clang-format off
#define THREAD_FLAG_DETACHED                 (1 << 0)
#define THREAD_FLAG_FREE_STRUCT              (1 << 1)
#define THREAD_FLAG_IDLE                     (1 << 2)
#define THREAD_FLAG_VCPU                     (1 << 3)
#define THREAD_FLAG_RESTRICTED_KICK_PENDING  (1 << 4)

#define THREAD_SIGNAL_KILL                   (1 << 0)
#define THREAD_SIGNAL_SUSPEND                (1 << 1)
#define THREAD_SIGNAL_POLICY_EXCEPTION       (1 << 2)
#define THREAD_SIGNAL_RESTRICTED_KICK        (1 << 3)
#define THREAD_SIGNAL_SAMPLE_STACK           (1 << 4)
#define THREAD_SIGNAL_CHECK_RSEQ             (1 << 5)
#define THREAD_SIGNAL_POLICY_KILL            (1 << 6)
// clang-format on

// thread priority
#define NUM_PRIORITIES (32)
#define LOWEST_PRIORITY (0)
#define HIGHEST_PRIORITY (NUM_PRIORITIES - 1)
#define DPC_PRIORITY (NUM_PRIORITIES - 2)
#define IDLE_PRIORITY LOWEST_PRIORITY
#define LOW_PRIORITY (NUM_PRIORITIES / 4)
#define DEFAULT_PRIORITY (NUM_PRIORITIES / 2)
#define HIGH_PRIORITY ((NUM_PRIORITIES / 4) * 3)

class PreemptionState {
 public:
  // Counters contained in state_ are limited to 15 bits.
  static constexpr uint32_t kMaxCountValue = 0x7fff;
  // The preempt disable count is in the lowest 15 bits.
  static constexpr uint32_t kPreemptDisableMask = kMaxCountValue;
  // The eager resched disable count is in the next highest 15 bits.
  static constexpr uint32_t kEagerReschedDisableShift = 15;
  static constexpr uint32_t kEagerReschedDisableMask = kMaxCountValue << kEagerReschedDisableShift;
  // Finally, the timeslice extension flags are the highest 2 bits.
  static constexpr uint32_t kTimesliceExtensionFlagsShift = 30;
  static constexpr uint32_t kTimesliceExtensionFlagsMask =
      ~(kPreemptDisableMask | kEagerReschedDisableMask);
  enum TimesliceExtensionFlags {
    // Thread has a timeslice extension that may or may not be Active.
    Present = 0b01 << kTimesliceExtensionFlagsShift,
    // Thread has an Active (in use) timeslice extension.
    Active = 0b10 << kTimesliceExtensionFlagsShift,
  };

  cpu_mask_t preempts_pending() const { return preempts_pending_.load(); }
  void preempts_pending_clear() { preempts_pending_.store(0); }
  void preempts_pending_add(cpu_mask_t mask) { preempts_pending_.fetch_or(mask); }

  bool PreemptIsEnabled() const {
    // Preemption is enabled iff both counts are zero and there's no runtime
    // extension.
    return state_.load() == 0;
  }

  uint32_t PreemptDisableCount() const { return PreemptDisableCount(state_.load()); }
  uint32_t EagerReschedDisableCount() const { return EagerReschedDisableCount(state_.load()); }

  // PreemptDisable() increments the preempt disable counter for the current
  // thread. While preempt disable is non-zero, preemption of the thread is
  // disabled, including preemption from interrupt handlers. During this time,
  // any call to Reschedule() will only record that a reschedule is pending, and
  // won't do a context switch.
  //
  // Note that this does not disallow blocking operations (e.g.
  // mutex.Acquire()). Disabling preemption does not prevent switching away from
  // the current thread if it blocks.
  //
  // A call to PreemptDisable() must be matched by a later call to
  // PreemptReenable() to decrement the preempt disable counter.
  void PreemptDisable() {
    const uint32_t old_state = state_.fetch_add(1);
    ASSERT(PreemptDisableCount(old_state) < kMaxCountValue);
  }

  // PreemptReenable() decrements the preempt disable counter and flushes any
  // pending local preemption operation.  Callers must ensure that they are
  // calling from a context where blocking is allowed, as the call may result in
  // the immediate preemption of the calling thread.
  void PreemptReenable() {
    const uint32_t old_state = state_.fetch_sub(1);
    ASSERT_MSG(PreemptDisableCount(old_state) > 0, "unmasked preemption state: %" PRIu32,
               old_state);

    // First, check for the expected situation of dropping the preempt count to zero
    // with a zero eager resched disable count and no timeslice extension.
    if (old_state == 1) {
      FlushPending(Flush::FlushLocal);
      return;
    }

    // Things must be more complicated.  Check for the various situations in
    // decreasing order of likeliness.

    // Are either of the counters non-zero?
    if (EagerReschedDisableCount(old_state) > 0 || PreemptDisableCount(old_state) > 1) {
      // We've got a non-zero count in one of the counters.
      return;
    }

    // The counters are both zero.  At this point, we must have a timeslice
    // extension installed.  This extension may be inactive, active and
    // not-yet-expired, or active and expired.

    // Is there an active extension?
    if (HasActiveTimesliceExtension(old_state)) {
      // Has it expired?
      if (ClearActiveTimesliceExtensionIfExpired()) {
        // It has.  We can flush.
        DEBUG_ASSERT(PreemptIsEnabled());
        FlushPending(Flush::FlushLocal);
        return;
      }
    }

    // We have an extension that's either inactive or active+unexpired.
  }

  void PreemptDisableAnnotated() TA_ACQ(preempt_disabled_token) {
    preempt_disabled_token.Acquire();
    PreemptDisable();
  }

  void PreemptReenableAnnotated() TA_REL(preempt_disabled_token) {
    preempt_disabled_token.Release();
    PreemptReenable();
  }

  static void AssertPreemptionDisabled() TA_ASSERT(preempt_disabled_token) {
    preempt_disabled_token.AssertHeld();
  }

  // PreemptReenableDelayFlush() decrements the preempt disable counter, but
  // deliberately does _not_ flush any pending local preemption operation.
  // Instead, if local preemption has become enabled again after the count
  // drops, and the local pending bit is set, the method will clear the bit and
  // return true.  Otherwise, it will return false.
  //
  // This method may only be called when interrupts are disabled and blocking is
  // not allowed.
  //
  // Callers of this method are "taking" ownership of the responsibility to
  // ensure that preemption on the local CPU takes place in the near future
  // after the call if the method returns true.
  //
  // Use of this method is strongly discouraged outside of top-level interrupt
  // glue and early threading setup.
  //
  // TODO(johngro): Consider replacing the bool return type with a move-only
  // RAII type which wraps the bool, and ensures that preemption event _must_
  // happen, either by having the user call a method on the object to manually
  // force the preemption event, or when the object destructs.
  [[nodiscard]] bool PreemptReenableDelayFlush() {
    DEBUG_ASSERT(arch_ints_disabled());
    DEBUG_ASSERT(arch_blocking_disallowed());

    const uint32_t old_state = state_.fetch_sub(1);
    ASSERT(PreemptDisableCount(old_state) > 0);

    // First, check for the expected situation of dropping the preempt count to zero
    // with a zero eager resched disable count and no timeslice extension.
    if (old_state == 1) {
      const cpu_mask_t local_mask = cpu_num_to_mask(arch_curr_cpu_num());
      const cpu_mask_t prev_mask = preempts_pending_.fetch_and(~local_mask);
      return (local_mask & prev_mask) != 0;
    }

    if (EagerReschedDisableCount(old_state) > 0 || PreemptDisableCount(old_state) > 1) {
      // We've got a non-zero count in one of the counters.
      return false;
    }

    // The counters are both zero.  At this point, we must have a timeslice
    // extension installed.  This extension may be inactive, active and
    // not-yet-expired, or active and expired.

    // Is there an active extension?
    if (HasActiveTimesliceExtension(old_state)) {
      // Has it expired?
      if (ClearActiveTimesliceExtensionIfExpired()) {
        // It has.
        DEBUG_ASSERT(PreemptIsEnabled());
        const cpu_mask_t local_mask = cpu_num_to_mask(arch_curr_cpu_num());
        const cpu_mask_t prev_mask = preempts_pending_.fetch_and(~local_mask);
        return (local_mask & prev_mask) != 0;
      }
    }

    // We have an extension that's either inactive or active+unexpired.
    return false;
  }

  // EagerReschedDisable() increments the eager resched disable counter for the
  // current thread. When early resched disable is non-zero, issuing local and
  // remote preemptions is disabled, including from interrupt handlers. During
  // this time, any call to Reschedule() or other scheduler entry points that
  // imply a reschedule will only record the pending reschedule for the affected
  // CPU, but will not perform reschedule IPIs or a local context switch.
  //
  // As with PreemptDisable, blocking operations are still allowed while
  // eager resched disable is non-zero.
  //
  // A call to EagerReschedDisable() must be matched by a later call to
  // EagerReschedReenable() to decrement the eager resched disable counter.
  void EagerReschedDisable() {
    const uint32_t old_state = state_.fetch_add(1 << kEagerReschedDisableShift);
    ASSERT_MSG(EagerReschedDisableCount(old_state) < kMaxCountValue, "old_state: %#" PRIx32,
               old_state);
  }

  // EagerReschedReenable() decrements the eager resched disable counter and
  // flushes pending local and/or remote preemptions if enabled, respectively.
  void EagerReschedReenable() {
    const uint32_t old_state = state_.fetch_sub(1 << kEagerReschedDisableShift);
    ASSERT_MSG(EagerReschedDisableCount(old_state) > 0, "old_state: %#" PRIx32, old_state);

    // First check the expected case.
    if (old_state == 1 << kEagerReschedDisableShift) {
      // Counts are both zero and there's no timeslice extension.
      //
      // Flushing all might reschedule this CPU, make sure it's OK to block.
      FlushPending(Flush::FlushAll);
      return;
    }

    if (EagerReschedDisableCount(old_state) > 1) {
      // Nothing to do since eager resched disable implies preempt disable.
      return;
    }

    // We know we can at least flush remote.  Can we also flush local?
    if (PreemptDisableCount(old_state) > 0) {
      // Nope, we've got a non-zero preempt disable count.
      FlushPending(Flush::FlushRemote);
      return;
    }

    // Is there an active extension?
    if (HasActiveTimesliceExtension(old_state)) {
      // Has it expired?
      if (ClearActiveTimesliceExtensionIfExpired()) {
        // Yes, preempt disable count is zero and the active extension has
        // expired.  We can flush all.
        DEBUG_ASSERT(PreemptIsEnabled());
        FlushPending(Flush::FlushAll);
        return;
      }
      // Extension is active, can't flush local.
    }

    // We have an inactive extension or an unexpired active extension.  Either
    // way, we can flush remote, but not local.
    FlushPending(Flush::FlushRemote);
  }

  void EagerReschedDisableAnnotated() TA_ACQ(preempt_disabled_token) {
    preempt_disabled_token.Acquire();
    EagerReschedDisable();
  }

  void EagerReschedReenableAnnotated() TA_REL(preempt_disabled_token) {
    preempt_disabled_token.Release();
    EagerReschedReenable();
  }

  // Sets a timeslice extension if one is not already set.
  //
  // This method should only be called in normal thread context.
  //
  // Returns false if a timeslice extension was already present or if the
  // supplied duration is <= 0.
  //
  // Note: It OK to call this from a context where preemption is (hard)
  // disabled.  If preemption is requested while the preempt disable count is
  // non-zero and a timeslice extension is in place, the extension will be
  // activated, but preemption will not occur until the count has dropped to
  // zero and the extension has expired or has been clear.
  bool SetTimesliceExtension(zx_duration_mono_t extension_duration) {
    if (extension_duration <= 0) {
      return false;
    }

    uint32_t state = state_.load();
    if (HasTimesliceExtension(state)) {
      return false;
    }
    timeslice_extension_.store(extension_duration);
    // Make sure that the timeslice extension value becomes visible to an
    // interrupt handler in this thread prior to the state_ flag becoming
    // visible.  See comment at |timeslice_extension_|.
    ktl::atomic_signal_fence(ktl::memory_order_release);
    state_.fetch_or(TimesliceExtensionFlags::Present);
    return true;
  }

  // Unconditionally clears any timeslice extension.
  //
  // This method must be called in normal thread context because it may trigger
  // local preemption.
  void ClearTimesliceExtension() {
    // Clear any present timeslice extension.
    const uint32_t old_state = state_.fetch_and(~kTimesliceExtensionFlagsMask);
    // Are the counters both zero?
    if ((old_state & ~kTimesliceExtensionFlagsMask) == 0) {
      FlushPending(Flush::FlushLocal);
    }
  }

  // PreemptSetPending() marks a pending preemption for the given CPUs.
  //
  // This is similar to Reschedule(), except that it may only be used inside an
  // interrupt handler while interrupts and preemption are disabled, between
  // PreemptDisable() and PreemptReenable(). It is similar to Reschedule(),
  // except that it does not need to be called with thread's lock held.
  void PreemptSetPending(cpu_mask_t reschedule_mask = cpu_num_to_mask(arch_curr_cpu_num())) {
    DEBUG_ASSERT(arch_ints_disabled());
    DEBUG_ASSERT(arch_blocking_disallowed());
    DEBUG_ASSERT(!PreemptIsEnabled());

    preempts_pending_.fetch_or(reschedule_mask);

    // Are we pending for the local CPU?
    const cpu_mask_t local_cpu_mask = cpu_num_to_mask(arch_curr_cpu_num());
    const bool local_cpu_in_resched_mask = (reschedule_mask & local_cpu_mask) != 0;
    if (!local_cpu_in_resched_mask) {
      // Nope.
      return;
    }

    EvaluateTimesliceExtension();
  }

  // Evaluate the thread's timeslice extension (if present), activating or
  // expiring it as necessary.
  //
  // Returns whether preemption is enabled.
  bool EvaluateTimesliceExtension() {
    const uint32_t old_state = state_.load();
    if (old_state == 0) {
      // No counts, no extension.  The common case.
      return true;
    }

    if (!HasTimesliceExtension(old_state)) {
      // No extension, but we have a non-zero count.
      return false;
    }

    if (HasActiveTimesliceExtension(old_state)) {
      if (!ClearActiveTimesliceExtensionIfExpired()) {
        return false;
      }
      // The active extension has expired.  If the counts are both zero, then
      // we're ready for preemption.
      return (old_state & ~kTimesliceExtensionFlagsMask) == 0;
    }

    // We have a not-yet-active extension.  Time to activate it.
    //
    // See comment at |timeslice_extension_| for why the signal fence is needed.
    ktl::atomic_signal_fence(ktl::memory_order_acquire);
    const zx_duration_mono_t extension_duration = timeslice_extension_.load();
    if (extension_duration <= 0) {
      // Already expired.
      state_.fetch_and(~kTimesliceExtensionFlagsMask);
      return (old_state & ~kTimesliceExtensionFlagsMask) == 0;
    }
    const zx_instant_mono_t deadline =
        zx_time_add_duration(current_mono_time(), extension_duration);
    timeslice_extension_deadline_.store(deadline);
    // See comment at |timeslice_extension_deadline_| for why the signal fence
    // is needed.
    ktl::atomic_signal_fence(ktl::memory_order_release);
    state_.fetch_or(TimesliceExtensionFlags::Active);
    SetPreemptionTimerForExtension(deadline);
    return false;
  }

  // Resets the preemption state. This should only be called when reviving an
  // idle/power thread that halted while taking a CPU offline.
  void Reset() {
    state_ = 0;
    preempts_pending_ = 0;
    timeslice_extension_ = 0;
    timeslice_extension_deadline_ = 0;
  }

 private:
  friend class PreemptDisableTestAccess;
  friend concurrent::CapabilityToken<PreemptionState>;

  // Implements the runtime assertion for concurrent::CapabilityToken<PreemptionState>::AssertHeld.
  static void AssertCapabilityHeld(concurrent::CapabilityToken<PreemptionState>::TagType);

  static inline uint32_t EagerReschedDisableCount(uint32_t state) {
    return (state & kEagerReschedDisableMask) >> kEagerReschedDisableShift;
  }

  static inline uint32_t PreemptDisableCount(uint32_t state) { return state & kPreemptDisableMask; }

  static inline bool HasTimesliceExtension(uint32_t state) {
    return (state & TimesliceExtensionFlags::Present) != 0;
  }

  static inline bool HasActiveTimesliceExtension(uint32_t state) {
    return (state & TimesliceExtensionFlags::Active) != 0;
  }

  // A non-inlined helper method to set the preemption timer when a timeslice
  // has been extended.  This must be non-inline to avoid an #include cycle with
  // percpu.h and thread.h.
  static void SetPreemptionTimerForExtension(zx_instant_mono_t deadline);

  // Checks whether the active timeslice extension has expired and if so, clears
  // it and returns true.
  //
  // Should only be called when there is an active timeslice extension.
  bool ClearActiveTimesliceExtensionIfExpired() {
    // Has the extension expired?
    //
    // See comment at |timeslice_extension_deadline_| for why the signal fence is needed.
    ktl::atomic_signal_fence(ktl::memory_order_acquire);
    if (current_mono_time() >= timeslice_extension_deadline_.load()) {
      state_.fetch_and(~kTimesliceExtensionFlagsMask);
      return true;
    }
    return false;
  }

  enum Flush { FlushLocal = 0x1, FlushRemote = 0x2, FlushAll = FlushLocal | FlushRemote };

  // Flushes local, remote, or all pending preemptions.
  //
  // This method is split in two so that the early out case of no pending
  // preemptions may be inlined without creating a header include cycle.
  void FlushPending(Flush flush) {
    // Early out to avoid unnecessarily taking the thread lock. This check races
    // any potential flush due to context switch, however, the context switch can
    // only clear bits that would have been flushed below, no new pending
    // preemptions are possible in the mask bits indicated by |flush|.
    if (likely(preempts_pending_.load() == 0)) {
      return;
    }
    FlushPendingContinued(flush);
  }
  void FlushPendingContinued(Flush flush);

  // state_ contains three fields:
  //
  //  * a 15-bit preempt disable counter (bits 0-14)
  //  * a 15-bit eager resched disable counter (bits 15-29)
  //  * a 2-bit for TimesliceExtensionFlags (bits 30-31)
  //
  // This is a single field so that both counters and the flags can be compared
  // against zero with a single memory access and comparison.
  //
  // state_'s counts are modified by interrupt handlers, but the counts are
  // always restored to their original value before the interrupt handler
  // returns, so modifications are not visible to the interrupted thread.
  RelaxedAtomic<uint32_t> state_{};

  // preempts_pending_ tracks pending reschedules to both local and remote CPUs
  // due to activity in the context of the current thread.
  //
  // This value can be changed asynchronously by an interrupt handler.
  //
  // preempts_pending_ should only be non-zero:
  //  * if PreemptDisableCount() or EagerReschedDisable() are non-zero, or
  //  * after PreemptDisableCount() or EagerReschedDisable() have been
  //    decremented, while preempts_pending_ is being checked.
  RelaxedAtomic<cpu_mask_t> preempts_pending_{};

  // The maximum duration of the thread's timeslice extension.
  //
  // This field is only valid when |state_|'s
  // |kTimeSliceExtensionFlags::Present| flag it set.
  //
  // This field may only be accessed by its owning thread or in an interrupt
  // context of the owning thread.  When reading this field, be sure to issue an
  // atomic_signal_fence (compiler barrier) with acquire semantics after
  // observing the Present flag.  Likewise, when writing this field, use an
  // atomic_signal_fence with release semantics prior to setting the Present
  // flag.  By using these fences, we ensure the flag and field value remain in
  // sync.
  RelaxedAtomic<zx_duration_mono_t> timeslice_extension_{};

  // The deadline at which the thread timeslice extension expires.
  //
  // This field is only valid when |kTimeSliceExtensionFlags::Active| flag it
  // set.
  //
  // This field may only be accessed by its owning thread or in an interrupt
  // context of the owning thread.  When reading this field, be sure to issue an
  // atomic_signal_fence (compiler barrier) with acquire semantics after
  // observing the Active flag.  Likewise, when writing this field, use an
  // atomic_signal_fence with release semantics prior to setting the Active
  // flag.  By using these fences, we ensure the flag and field value remain in
  // sync.
  RelaxedAtomic<zx_instant_mono_t> timeslice_extension_deadline_{};
};

// TaskState is responsible for running the task defined by
// |entry(arg)|, and reporting its value to any joining threads.
//
// TODO: the detached state in Thread::flags_ probably belongs here.
class TaskState {
 public:
  TaskState() = default;

  void Init(thread_start_routine entry, void* arg);

  zx_status_t Join(Thread* current_thread, zx_instant_mono_t deadline) TA_REL(get_lock())
      TA_REQ(chainlock_transaction_token, ChainLockable::GetLock(*current_thread));

  // Attempt to wake all of our joiners.  This operation might fail because of a
  // conflict while attempting to obtain the required locks, in which case the
  // caller needs to back off (drop all currently held chain locks) and try
  // again..
  bool TryWakeJoiners(zx_status_t status)
      TA_REQ(chainlock_transaction_token, preempt_disabled_token);

  thread_start_routine entry() const { return entry_; }
  void* arg() const { return arg_; }

  int retcode() const { return retcode_; }
  void set_retcode(int retcode) { retcode_ = retcode; }

  ChainLock& get_lock() TA_RET_CAP(retcode_wait_queue_.get_lock()) {
    return retcode_wait_queue_.get_lock();
  }

 private:
  // Dumping routines are allowed to see inside us.
  friend class ThreadDumper;

  // The Thread's entry point, and its argument.
  thread_start_routine entry_ = nullptr;
  void* arg_ = nullptr;

  // Storage for the return code.
  //
  // TODO(johngro): This is logically protected by our thread's lock.  What is a
  // good way to represent this with a static annotation?
  int retcode_ = 0;

  // Other threads waiting to join this Thread.
  WaitQueue retcode_wait_queue_;
};

// Keeps track of whether a thread is allowed to allocate memory.
//
// A thread's |MemoryAllocationState| should only be accessed by that thread itself or interrupt
// handlers running in the thread's context.
class MemoryAllocationState {
 public:
  void Disable() {
    ktl::atomic_signal_fence(ktl::memory_order_seq_cst);
    disable_count_ = disable_count_ + 1;
    ktl::atomic_signal_fence(ktl::memory_order_seq_cst);
  }

  void Enable() {
    ktl::atomic_signal_fence(ktl::memory_order_seq_cst);
    DEBUG_ASSERT(disable_count_ > 0);
    disable_count_ = disable_count_ - 1;
    ktl::atomic_signal_fence(ktl::memory_order_seq_cst);
  }

  // Returns true if memory allocation is allowed.
  bool IsEnabled() {
    ktl::atomic_signal_fence(ktl::memory_order_seq_cst);
    return disable_count_ == 0;
  }

 private:
  // Notice that we aren't using atomic operations to access the field.  We don't need atomic
  // operations here as long as...
  //
  // 1. We use atomic_signal_fence to prevent compiler reordering.
  //
  // 2. We use volatile to ensure the compiler actually generates loads and stores for the value (so
  // the interrupt handler can see what the thread see, and vice versa).
  //
  // 3. Upon completion, an interrupt handler that modified the field restores it to the value it
  // held at the start of the interrupt.
  volatile uint32_t disable_count_ = 0;
};

// Stall accounting state of a thread.
//
// Kernel-only threads are never considered for the purpose of memory stall
// accounting (`IgnoredKernelOnly`).
//
// If a user thread is not signalling a stall, it will contribute to the current
// CPU's memory stall accumulator as `Progressing` while running and do not
// contribute (`Inactive`) in any other state.
//
// If a user thread signals that a stall is ongoing, it will contribute to the
// current CPU's accumulator as `Stalling`. If preempted, the contribution will
// persist on the last CPU's accumulator. Normal operation is resumed when the
// thread signals that the stall is over.
enum class ThreadStallState {
  IgnoredKernelOnly,
  Inactive,
  Progressing,
  Stalling,
};

struct Thread : public ChainLockable {
  // TODO(kulakowski) Are these needed?
  // Default constructor/destructor declared to be not-inline in order to
  // avoid circular include dependencies involving Thread, WaitQueue, and
  // OwnedWaitQueue.
  Thread();
  ~Thread();

  // The default constructor leaves the object in a state equivalent to before
  // construction, and is only used for special cases that get re-constructed
  // via placement new using this signature.
  explicit Thread(ktl::string_view name);

  static Thread* CreateIdleThread(cpu_num_t cpu_num);

  // Revive the idle/power thread for the given CPU so that it starts fresh after
  // halting while taking the CPU offline. Most state is preserved, including
  // stack memory locations (useful for debugging) and accumulated runtime stats.
  static void ReviveIdlePowerThread(cpu_num_t cpu_num) TA_EXCL(chainlock_transaction_token);

  // Creates a thread with |name| that will execute |entry| at |priority|. |arg|
  // will be passed to |entry| when executed, the return value of |entry| will be
  // passed to Exit().
  // This call allocates a thread and places it in the global thread list. This
  // memory will be freed by either Join() or Detach(), one of these
  // MUST be called.
  // The thread will not be scheduled until Resume() is called.
  static Thread* Create(ktl::string_view name, thread_start_routine entry, void* arg, int priority);
  static Thread* Create(ktl::string_view name, thread_start_routine entry, void* arg,
                        const SchedulerState::BaseProfile& profile);
  static Thread* CreateEtc(Thread* t, ktl::string_view name, thread_start_routine entry, void* arg,
                           const SchedulerState::BaseProfile& profile,
                           thread_trampoline_routine alt_trampoline);

  // Public routines used by debugging code to dump thread state.

  // Dump the information of a single thread. If |full| is true then a multi-line verbose dump of
  // much information is performed, otherwise a single short summary line is dumped.
  void Dump(bool full) const TA_EXCL(get_lock());
  // Finds all threads whose |OwnerName| is the same as |match_owner|, or just all thread if
  // |match_owner| is nullptr, and call |Dump| on them.
  static void DumpAll(const char* match_owner, bool full) TA_EXCL(list_lock_);
  // Finds a thread whose |tid()| matches the provided |tid| and calls |Dump| on it.
  static void DumpTid(zx_koid_t tid, bool full) TA_EXCL(list_lock_);

  // Same stuff, but skip the locks when we are panic'ing
  inline void DumpDuringPanic(bool full) const TA_NO_THREAD_SAFETY_ANALYSIS;
  static inline void DumpAllDuringPanic(const char* match_owner,
                                        bool full) TA_NO_THREAD_SAFETY_ANALYSIS;
  static inline void DumpTidDuringPanic(zx_koid_t tid, bool full) TA_NO_THREAD_SAFETY_ANALYSIS;

  // Internal initialization routines. Eventually, these should be private.
  void SecondaryCpuInitEarly();

  // Associate this Thread to the given ThreadDispatcher.
  void SetUsermodeThread(fbl::RefPtr<ThreadDispatcher> user_thread) TA_EXCL(get_lock());

  void AssertIsCurrentThread() const TA_ASSERT(is_current_thread_token) {
    DEBUG_ASSERT(this == Thread::Current::Get());
  }

  // Returns the lock that protects the thread's internal state, particularly with respect to
  // scheduling.
  fbl::NullLock& get_scheduler_variable_lock() const TA_RET_CAP(scheduler_variable_lock_) {
    return scheduler_variable_lock_;
  }
  static auto& get_list_lock() TA_RET_CAP(list_lock_) { return list_lock_; }

  // Get the associated ThreadDispatcher.
  ThreadDispatcher* user_thread() { return user_thread_.get(); }
  const ThreadDispatcher* user_thread() const { return user_thread_.get(); }

  // Returns the koid of the associated ProcessDispatcher for user threads or
  // ZX_KOID_INVLID for kernel threads.
  zx_koid_t pid() const { return pid_; }

  // Returns the koid of the associated ThreadDispatcher for user threads or an
  // independent koid for kernel threads.
  zx_koid_t tid() const { return tid_; }

  // Return the pid/tid of the thread as a tracing thread reference.
  fxt::ThreadRef<fxt::RefType::kInline> fxt_ref() const { return {pid(), tid()}; }

  // Implicit conversion to a tracing thread reference.
  operator fxt::ThreadRef<fxt::RefType::kInline>() const { return fxt_ref(); }

  // Called to mark a thread as schedulable.
  void Resume() TA_EXCL(chainlock_transaction_token);
  zx_status_t Suspend() { return SuspendOrKillInternal(SuspendOrKillOp::Suspend); }
  void Forget();
  zx_status_t RestrictedKick();
  // Marks a thread as detached, in this state its memory will be released once
  // execution is done.
  zx_status_t Detach();
  zx_status_t DetachAndResume();
  // Waits |deadline| time for a thread to complete execution then releases its memory.
  zx_status_t Join(int* retcode, zx_instant_mono_t deadline);
  // Deliver a kill signal to a thread.
  void Kill() { SuspendOrKillInternal(SuspendOrKillOp::Kill); }

  // Checks whether the kill or suspend signal has been raised. If kill has been
  // raised, then `ZX_ERR_INTERNAL_INTR_KILLED` will be returned. If suspend has
  // been raised, then `ZX_ERR_INTERNAL_INTR_RETRY` will be returned. Otherwise,
  // `ZX_OK` will be returned.
  zx_status_t CheckKillOrSuspendSignal() const;

  // Erase this thread from all global lists, where applicable.
  void EraseFromListsLocked() TA_REQ(list_lock_);

  /**
   * @brief Set the base profile for this thread.
   *
   * The chosen scheduling discipline and its associate parameters are defined by the BaseProfile
   * object.
   *
   * @param params The weight to apply to the thread.
   */
  void SetBaseProfile(const SchedulerState::BaseProfile& profile) TA_EXCL(get_lock());

  void* recursive_object_deletion_list() { return recursive_object_deletion_list_; }
  void set_recursive_object_deletion_list(void* ptr) { recursive_object_deletion_list_ = ptr; }

  // Get/set the mask of valid CPUs that thread may run on. If a new mask
  // is set, the thread will be migrated to satisfy the new constraint.
  //
  // Affinity comes in two flavours:
  //
  //   * "hard affinity", which will always be respected by the scheduler.
  //     The scheduler will panic if it can't satisfy this affinity.
  //
  //   * "soft affinity" indicating where the thread should ideally be scheduled.
  //     The scheduler will respect the mask unless there are no other
  //     options (e.g., the soft affinity and hard affinity don't contain
  //     any common CPUs).
  //
  // If the two masks conflict, the hard affinity wins.
  //
  // Setting the affinity mask returns the previous value to make pinning and
  // restoring operations simpler and more efficient.
  //
  // See scheduler.h for the declaration of the affinity setters.

  cpu_mask_t SetCpuAffinity(cpu_mask_t affinity) TA_EXCL(get_lock());
  cpu_mask_t GetCpuAffinity() const TA_EXCL(get_lock());
  cpu_mask_t SetSoftCpuAffinity(cpu_mask_t affinity) TA_EXCL(get_lock());
  cpu_mask_t GetSoftCpuAffinity() const TA_EXCL(get_lock());

  enum class MigrateStage {
    // The stage before the thread has migrated. Called from the old CPU to save state.
    Save,
    // The stage after the thread has migrated. Called from the new CPU to restore state.
    Restore,
    // The Thread is exiting. Can be called from any CPU.
    Exiting,
  };
  // The migrate function will be invoked twice when a thread is migrate between
  // CPUs. Firstly when the thread is removed from the old CPUs scheduler,
  // secondly when the thread is rescheduled on the new CPU. When the migrate
  // function is called, |thread_lock| is held.
  //
  // TODO(johngro): what to do about this?  It is all fine and good to say that
  // a MigrateFn needs to have the thread's lock held when it is called, but
  // there does not seem to be any good way to annotate this as a property of
  // the fit::inline function's callable type. Perhaps the best thing to do is
  // annotate the migrate function member itself with being guarded by the
  // thread's lock, so that invocation (at least) would require holding the
  // lock?
  //
  // Implementers of migrate functions should be able to annotate their
  // callbacks as requiring the thread's lock in order to be invoked, but
  // unfortunately this does not work along side of the fit::inline_function's
  // type.  If a user attempts to annotate their lambda as such, the
  // fit::inline_function fails expansion when its `operator()` attempt to call
  // the lambda which requires that the lock be held.  If there was some way
  // using the static lock annotation system to have the operator() inherit
  // these annotations from its templated target type, we could avoid the issue,
  // but that does not seem to be an option.

  using MigrateFn = fit::inline_function<void(Thread* thread, MigrateStage stage), sizeof(void*)>;
  void SetMigrateFn(MigrateFn migrate_fn) TA_EXCL(list_lock_, get_lock());
  void SetMigrateFnLocked(MigrateFn migrate_fn) TA_REQ(list_lock_, get_lock());
  void CallMigrateFnLocked(MigrateStage stage) TA_REQ(get_lock());

  // Call |migrate_fn| for each thread that was last run on the current CPU.
  // Note: no locks should be held during this operation, and interrupts need to
  // be disabled.
  static void CallMigrateFnForCpu(cpu_num_t cpu);

  // The context switch function will be invoked when a thread is context
  // switched to or away from. This will be called when a thread is about to be
  // run on a CPU, after it's stopped from running on a CPU, or about to exit.
  using ContextSwitchFn = fit::inline_function<void(), sizeof(void*)>;
  void SetContextSwitchFn(ContextSwitchFn context_switch_fn) TA_EXCL(get_lock());
  void SetContextSwitchFnLocked(ContextSwitchFn context_switch_fn) TA_REQ(get_lock());
  // Call |context_switch_fn| for this thread.
  void CallContextSwitchFnLocked() TA_REQ(get_lock()) {
    if (unlikely(context_switch_fn_)) {
      context_switch_fn_();
    }
  }

  void SignalCheckRestartableSequenceIfNeeded() {
    if (likely(!rseq_accessor_.IsValid())) {
      return;
    }
    set_signals(THREAD_SIGNAL_CHECK_RSEQ);
  }

  void set_rseq_accessor(page_map::Accessor<zx_rseq_t> accessor) {
    rseq_accessor_ = ktl::move(accessor);
  }

  void CheckRestartableSequence(GeneralRegsSource source, void* gregs);

  void OwnerName(char (&out_name)[ZX_MAX_NAME_LEN]) const;
  // Return the number of nanoseconds a thread has been running for.
  zx_duration_mono_t Runtime() const TA_EXCL(get_lock());

  // Last cpu this thread was running on, or INVALID_CPU if it has never run.
  cpu_num_t LastCpu() const TA_EXCL(get_lock());
  cpu_num_t LastCpuLocked() const TA_REQ_SHARED(get_lock());

  // Return true if thread has been signaled.
  bool IsSignaled() { return signals() != 0; }
  bool IsIdle() const { return !!(flags_ & THREAD_FLAG_IDLE); }

  // Returns true if this Thread's user state has been saved.
  //
  // Caller must hold the thread's lock.
  bool IsUserStateSavedLocked() const TA_REQ(get_lock()) { return user_state_saved_; }

  // Callback for the Timer used for SleepEtc.
  static void SleepHandler(Timer* timer, zx_instant_mono_t now, void* arg);
  void HandleSleep(Timer* timer, zx_instant_mono_t now);

  // Request a thread to check if it should sample its backtrace. When the thread returns to
  // usermode, it will take a sample of its userstack if sampling is enabled.
  static void SignalSampleStack(Timer* t, zx_instant_mono_t, void* session_id);

  // All of these operations implicitly operate on the current thread.
  struct Current {
    // This is defined below, just after the Thread declaration.
    static inline Thread* Get();

    // Scheduler routines to be used by regular kernel code.
    static void Yield();
    static void Preempt();
    static void Reschedule() TA_EXCL(chainlock_transaction_token);
    static void Exit(int retcode) __NO_RETURN;
    static void Kill();
    static void BecomeIdle() __NO_RETURN;

    // Wait until the deadline has occurred.
    //
    // If interruptible, may return early with ZX_ERR_INTERNAL_INTR_KILLED if
    // thread is signaled for kill.
    static zx_status_t SleepEtc(const Deadline& deadline, Interruptible interruptible,
                                zx_instant_mono_t now) TA_EXCL(chainlock_transaction_token);
    // Non-interruptible version of SleepEtc.
    static zx_status_t Sleep(zx_instant_mono_t deadline);
    // Non-interruptible relative delay version of Sleep.
    static zx_status_t SleepRelative(zx_duration_mono_t delay);
    // Interruptible version of Sleep.
    static zx_status_t SleepInterruptible(zx_instant_mono_t deadline);

    // Transition the current thread to the THREAD_SUSPENDED state.
    static void DoSuspend();

    // Write the current thread's stack to the assigned sampler buffers
    static void DoSampleStack(GeneralRegsSource source, void* gregs);

    // |policy_exception_code| should be a ZX_EXCP_POLICY_CODE_* value.
    static void SignalPolicyException(uint32_t policy_exception_code,
                                      uint32_t policy_exception_data);

    // Raise a thread signal that when processed, will kill the calling thread.
    static void SignalPolicyKill();

    // Process any pending thread signals.
    //
    // This method may never return if the thread has a pending kill signal.
    //
    // Interrupt state - This method modifies interrupt state.  It is critical that this method be
    // called with interrupts disabled to eliminate a "lost wakeup" race condition.  While
    // interrupts must be disabled prior to call this method, the method may re-enable them during
    // the processing of certain signals.  This method guarantees that if it does return, it will do
    // so with interrupts disabled.
    static void ProcessPendingSignals(GeneralRegsSource source, void* gregs);

    // Migrates the current thread to the CPU identified by target_cpu.
    static void MigrateToCpu(cpu_num_t target_cpuid);

    static void SetName(const char* name);

    static PreemptionState& preemption_state() {
      return Thread::Current::Get()->preemption_state();
    }

    static MemoryAllocationState& memory_allocation_state() {
      return Thread::Current::Get()->memory_allocation_state_;
    }

    static cpu_mask_t SetCpuAffinity(cpu_mask_t affinity) {
      return Thread::Current::Get()->SetCpuAffinity(affinity);
    }

    // If a restricted kick is pending on this thread, clear it and return true.
    // Otherwise return false.
    // Must be called with interrupts disabled.
    [[nodiscard]] static bool CheckForRestrictedKick();

    static RestrictedState* restricted_state() {
      return Thread::Current::Get()->restricted_state();
    }

    static VmAspace* active_aspace() { return Thread::Current::Get()->aspace_; }

    static VmAspace* switch_aspace(VmAspace* aspace) TA_NO_THREAD_SAFETY_ANALYSIS {
      return Thread::Current::Get()->switch_aspace(aspace);
    }

    // These three functions handle faults on the address space containing va. If there is no
    // aspace that contains va, or we don't have access to it, a ZX_ERR_NOT_FOUND is returned.
    //
    // Calling any of these methods on a pure kernel thread (i.e. one without an associated
    // `ThreadDispatcher`) is a programming error.
    // May block on page requests and must be called without locks held.
    static zx_status_t PageFault(vaddr_t va, uint flags);
    static zx_status_t SoftFault(vaddr_t va, uint flags);
    static zx_status_t SoftFaultInRange(vaddr_t va, uint flags, size_t len);
    static zx_status_t AccessedFault(vaddr_t va);

    // Generate a backtrace for the calling thread.
    //
    // |out_bt| will be reset() prior to be filled in and if a backtrace cannot
    // be obtained, it will be left empty.
    static void GetBacktrace(Backtrace& out_bt);

    // Generate a backtrace for the calling thread starting at frame pointer |fp|.
    //
    // |out_bt| will be reset() prior to be filled in and if a backtrace cannot
    // be obtained, it will be left empty.
    static void GetBacktrace(vaddr_t fp, Backtrace& out_bt);

    static void Dump(bool full) { Thread::Current::Get()->Dump(full); }
    static void DumpDuringPanic(bool full) TA_NO_THREAD_SAFETY_ANALYSIS {
      Thread::Current::Get()->DumpDuringPanic(full);
    }

   private:
    // Handles a virtual memory fault in the address space that contains va. If there is no aspace
    // that contains va, or we don't have access to it, a ZX_ERR_NOT_FOUND is returned.
    template <typename F>
    static zx_status_t Fault(vaddr_t va, F resolve_fault);
  };  // struct Current;

  // Trait for the global Thread list.
  struct ThreadListTrait {
    static fbl::DoublyLinkedListNodeState<Thread*>& node_state(Thread& thread) {
      return thread.thread_list_node_;
    }
  };
  using List = fbl::DoublyLinkedListCustomTraits<Thread*, ThreadListTrait>;

  // Traits for the temporary unblock list, used to batch-unblock threads.
  struct UnblockListTrait {
    static fbl::DoublyLinkedListNodeState<Thread*>& node_state(Thread& thread) {
      return thread.unblock_list_node_;
    }
  };
  using UnblockList = fbl::DoublyLinkedListCustomTraits<Thread*, UnblockListTrait>;

  // Traits for the save state list, used to run the Save stage of migration functions.
  struct SaveStateListTrait {
    static fbl::SinglyLinkedListNodeState<Thread*>& node_state(Thread& thread) {
      return thread.save_state_list_node_;
    }
  };
  using SaveStateList = fbl::SinglyLinkedListCustomTraits<Thread*, SaveStateListTrait>;

  struct Linebuffer {
    size_t pos = 0;
    ktl::array<char, 128> buffer{};
  };

  // Called by the scheduler to update runtime stats.
  void UpdateRuntimeStats(thread_state new_state);

  // Accessors into Thread state. When the conversion to all-private
  // members is complete (bug 54383), we can revisit the overall
  // Thread API.

  thread_state state() const TA_REQ_SHARED(get_lock()) { return scheduler_state_.state(); }
  SchedulerQueueState::Disposition disposition() const TA_REQ(get_scheduler_variable_lock()) {
    return scheduler_queue_state().disposition();
  }

  // The scheduler can set threads to be running, or to be ready to run.
  void set_running() TA_REQ(get_lock()) { scheduler_state_.set_state(THREAD_RUNNING); }
  void set_ready() TA_REQ(get_lock()) { scheduler_state_.set_state(THREAD_READY); }
  // While wait queues can set threads to be blocked.
  void set_blocked() TA_REQ(get_lock()) { scheduler_state_.set_state(THREAD_BLOCKED); }
  void set_blocked_read_lock() TA_REQ(get_lock()) {
    scheduler_state_.set_state(THREAD_BLOCKED_READ_LOCK);
  }
  // The thread can set itself to be sleeping.
  void set_sleeping() TA_REQ(get_lock()) { scheduler_state_.set_state(THREAD_SLEEPING); }
  void set_death() TA_REQ(get_lock()) { scheduler_state_.set_state(THREAD_DEATH); }
  void set_suspended() TA_REQ(get_lock()) { scheduler_state_.set_state(THREAD_SUSPENDED); }

  // Accessors for specific flags_ bits.
  bool detatched() const { return (flags_ & THREAD_FLAG_DETACHED) != 0; }
  void set_detached(bool value) {
    if (value) {
      flags_ |= THREAD_FLAG_DETACHED;
    } else {
      flags_ &= ~THREAD_FLAG_DETACHED;
    }
  }
  bool free_struct() const { return (flags_ & THREAD_FLAG_FREE_STRUCT) != 0; }
  void set_free_struct(bool value) {
    if (value) {
      flags_ |= THREAD_FLAG_FREE_STRUCT;
    } else {
      flags_ &= ~THREAD_FLAG_FREE_STRUCT;
    }
  }
  bool idle() const { return (flags_ & THREAD_FLAG_IDLE) != 0; }
  void set_idle(bool value) {
    if (value) {
      flags_ |= THREAD_FLAG_IDLE;
    } else {
      flags_ &= ~THREAD_FLAG_IDLE;
    }
  }
  bool vcpu() const { return (flags_ & THREAD_FLAG_VCPU) != 0; }
  void set_vcpu(bool value) {
    if (value) {
      flags_ |= THREAD_FLAG_VCPU;
    } else {
      flags_ &= ~THREAD_FLAG_VCPU;
    }
  }

  // Access to the entire flags_ value, for diagnostics.
  unsigned int flags() const { return flags_; }

  // Only these three methods directly access the signals_ member.
  // TODO(https://fxbug.dev/42077109): Write down memory order requirements for accessing signals_.
  uint32_t signals() const { return signals_.load(ktl::memory_order_relaxed); }
  void set_signals(uint32_t bits) { signals_.fetch_or(bits, ktl::memory_order_relaxed); }
  void clear_signals(uint32_t bits) { signals_.fetch_and(~bits, ktl::memory_order_relaxed); }

  bool has_migrate_fn() const TA_REQ_SHARED(get_lock()) { return migrate_fn_ != nullptr; }
  bool migrate_pending() const TA_REQ_SHARED(get_lock()) { return migrate_pending_; }

  TaskState& task_state() { return task_state_; }
  const TaskState& task_state() const { return task_state_; }

  PreemptionState& preemption_state() { return preemption_state_; }
  const PreemptionState& preemption_state() const { return preemption_state_; }

  SchedulerState& scheduler_state() TA_REQ(get_lock()) { return scheduler_state_; }
  const SchedulerState& scheduler_state() const TA_REQ_SHARED(get_lock()) {
    return scheduler_state_;
  }

  SchedulerQueueState& scheduler_queue_state() TA_REQ(scheduler_variable_lock_) {
    return scheduler_queue_state_;
  }
  const SchedulerQueueState& scheduler_queue_state() const TA_REQ(scheduler_variable_lock_) {
    return scheduler_queue_state_;
  }

  WaitQueueCollection::ThreadState& wait_queue_state() TA_REQ(get_lock()) {
    return wait_queue_state_;
  }
  const WaitQueueCollection::ThreadState& wait_queue_state() const TA_REQ_SHARED(get_lock()) {
    return wait_queue_state_;
  }

#if WITH_LOCK_DEP
  lockdep::ThreadLockState& lock_state() { return lock_state_; }
  const lockdep::ThreadLockState& lock_state() const { return lock_state_; }
#endif

  RestrictedState* restricted_state() { return restricted_state_.get(); }
  const RestrictedState* restricted_state() const { return restricted_state_.get(); }
  void set_restricted_state(RestrictedState* raw_rs) { restricted_state_.reset(raw_rs); }
  bool in_restricted() const {
    return restricted_state_ ? rust_restricted_state_in_restricted(restricted_state_.get()) : false;
  }

  arch_thread& arch() { return arch_; }
  const arch_thread& arch() const { return arch_; }

  KernelStack& stack() { return stack_; }
  const KernelStack& stack() const { return stack_; }

  // Rules for the aspace_ member.
  //
  // 1) Raw VmAspace pointers held by Thread object are guaranteed to always be
  //    "alive" from a C++ perspective (the object has not been destructed, and
  //    the memory is still alive).  The actual VmAspace object is a ref-counted
  //    object which is guaranteed to be alive because:
  // 1a) It is a member of a thread's process (the main aspace, as well as the
  //     restricted mode aspace), and a thread's process object cannot be
  //     destroyed while the thread is still alive.  Or --
  // 1b) It is a special statically allocated aspace object, such as the EFI
  //     address space, or the kernel address space.
  // 2) Only the current thread is permitted to mutate its own aspace_ member.
  //    The only exception to this is during initialization, before the thread
  //    is running, by VmAspace::AttachToThread.
  // 3) The currently running thread is allowed to access its own currently
  //    assigned aspace without needing any locks.
  // 4) Any other thread who wishes to access a thread's aspace must do so via
  //    either GetAspaceRef or GetAspaceRefLocked, which will return a RefPtr to
  //    the thread's current aspace as long as the thread has not entered the
  //    THREAD_DEATH state.
  // 5) Other threads who access a thread's aspace via GetAspaceRef cannot
  //    assume that either the thread or the aspace is still alive after the
  //    thread's lock is dropped.  At best, they know that the RefPtr is keeping
  //    the VmAspace object alive from a C++ perspective, nothing more.
  // 6) The scheduler and low level context switch routines are allowed to
  //    directly access the aspace_ of both the old and new threads involved in the
  //    context switch.  Neither one is currently really running (the aspace_
  //    members cannot change), and both threads have to be alive enough that
  //    their process is still alive, and therefore so is their aspace_ (see
  //    #1).  Note; it is possible that the old thread involved in a context
  //    switch is in the process of dying, but we know that its process cannot
  //    exit (destroying its aspace in the process) until after it has context
  //    switched for the last time.
  fbl::RefPtr<VmAspace> GetAspaceRef() const TA_EXCL(get_lock());
  fbl::RefPtr<VmAspace> GetAspaceRefLocked() const TA_REQ_SHARED(get_lock());
  VmAspace* aspace() TA_REQ(is_current_thread_token) { return aspace_; }
  const VmAspace* aspace() const TA_REQ(is_current_thread_token) { return aspace_; }
  VmAspace* switch_aspace(VmAspace* aspace) TA_REQ(is_current_thread_token) {
    VmAspace* old_aspace = aspace_;
    aspace_ = aspace;
    return old_aspace;
  }

  // Returns the currently active address space, which is the address space currently hosting
  // page tables for the thread.
  //
  // The active address space should be used only when context switching. It should not be used for
  // resolving faults, as it may be a unified aspace that does not keep track of its own mappings.
  //
  // Kernel-only thread -- This will return nullptr, unless a caller has explicitly set aspace_
  // using `switch_aspace`, which is done by a few kernel unittests.
  //
  // User thread -- If the thread is in Restricted Mode, this will return the restricted aspace.
  // Otherwise, it will return the process's normal aspace.
  //
  // Note, the normal aspace is, by definition, the aspace that's active when a thread is in Normal
  // Mode.  All threads not in Restricted Mode are said to be in Normal Mode.  See
  // `ProcessDispatcher::normal_aspace()` for more information.
  VmAspace* active_aspace() { return aspace_; }
  const VmAspace* active_aspace() const { return aspace_; }

  const char* name() const { return name_; }
  // This may truncate |name|, so that it (including a trailing NUL
  // byte) fit in ZX_MAX_NAME_LEN bytes.
  void set_name(ktl::string_view name);

  Linebuffer& linebuffer() { return linebuffer_; }

  using Canary = fbl::Canary<fbl::magic("thrd")>;
  const Canary& canary() const { return canary_; }

  // Generate a backtrace for |this| thread.
  //
  // |this| must be blocked, sleeping or suspended (i.e. not running).
  //
  // |out_bt| will be reset() prior to be filled in and if a backtrace cannot be
  // obtained, it will be left empty.
  void GetBacktrace(Backtrace& out_bt) TA_EXCL(get_lock());

  // Returns the last flow id allocated by TakeNextLockFlowId() for this thread.
  uint64_t lock_flow_id() const { return lock_flow_id_; }

  // Returns a unique flow id for lock contention tracing. The same value is
  // returned by lock_flow_id() until another id is allocated for this thread
  // by calling this method again.
  uint64_t TakeNextLockFlowId() { return lock_flow_id_ = flow_id_generate(); }

  void RecomputeEffectiveProfile() TA_REQ(get_lock()) {
    scheduler_state_.RecomputeEffectiveProfile();
  }

  SchedulerState::EffectiveProfile SnapshotEffectiveProfileLocked() const
      TA_REQ_SHARED(get_lock()) {
    return scheduler_state_.effective_profile_;
  }

  SchedulerState::EffectiveProfile SnapshotEffectiveProfile() const
      TA_EXCL(chainlock_transaction_token, get_lock());

  SchedulerState::BaseProfile SnapshotBaseProfileLocked() const TA_REQ_SHARED(get_lock()) {
    return scheduler_state_.base_profile_;
  }

  SchedulerState::BaseProfile SnapshotBaseProfile() const
      TA_EXCL(chainlock_transaction_token, get_lock());

  ThreadStallState memory_stall_state() const TA_REQ(preempt_disabled_token) {
    return memory_stall_state_;
  }
  void set_memory_stall_state(ThreadStallState value) TA_REQ(preempt_disabled_token) {
    memory_stall_state_ = value;
  }

 private:
  // The architecture-specific methods for getting and setting the
  // current thread may need to see Thread's arch_ member via offsetof.
  friend inline Thread* arch_get_current_thread();
  friend inline void arch_set_current_thread(Thread*);

  // OwnedWaitQueues manipulate wait queue state.
  friend class OwnedWaitQueue;

  // ScopedThreadExceptionContext is the only public way to call
  // SaveUserStateLocked and RestoreUserStateLocked.
  friend class ScopedThreadExceptionContext;

  // Dumping routines are allowed to see inside us.
  friend class ThreadDumper;

  // Type used to select which operation (suspend or kill) we want during a call
  // to SuspendOrKillInternal.
  enum class SuspendOrKillOp { Suspend, Kill };

  // The default trampoline used when running the Thread. This can be
  // replaced by the |alt_trampoline| parameter to CreateEtc().
  static void Trampoline() __NO_RETURN;

  // Dpc callback used for cleaning up a detached Thread's resources.
  static void FreeDpc(Dpc* dpc);

  // Save the arch-specific user state.
  //
  // Returns true when the user state will later need to be restored.
  [[nodiscard]] bool SaveUserStateLocked() TA_REQ(get_lock());

  // Restore the arch-specific user state.
  void RestoreUserStateLocked() TA_REQ(get_lock());

  // Common implementation of Suspend and Kill.
  zx_status_t SuspendOrKillInternal(SuspendOrKillOp op) TA_EXCL(chainlock_transaction_token);

  // Returns true if it decides to kill the thread, which must be the
  // current thread. The thread_lock must be held when calling this
  // function.
  //
  // TODO: move this to CurrentThread, once that becomes a subclass of
  // Thread.
  bool CheckKillSignal() TA_REQ(get_lock());

  // These should only be accessed from the current thread.
  bool restricted_kick_pending() const {
    return (flags_ & THREAD_FLAG_RESTRICTED_KICK_PENDING) != 0;
  }
  void set_restricted_kick_pending(bool value) {
    if (value) {
      flags_ |= THREAD_FLAG_RESTRICTED_KICK_PENDING;
    } else {
      flags_ &= ~THREAD_FLAG_RESTRICTED_KICK_PENDING;
    }
  }

  __NO_RETURN void ExitLocked(int retcode) TA_REQ(get_lock());

  static void DumpAllLocked(const char* match_owner, bool full) TA_REQ(list_lock_);
  static void DumpTidLocked(zx_koid_t tid, bool full) TA_REQ(list_lock_);

 private:
  struct MigrateListTrait {
    static fbl::DoublyLinkedListNodeState<Thread*>& node_state(Thread& thread) {
      return thread.migrate_list_node_;
    }
  };
  using MigrateList = fbl::DoublyLinkedListCustomTraits<Thread*, MigrateListTrait>;

  static inline DECLARE_SPINLOCK(Thread) list_lock_ TA_ACQ_BEFORE(lock_);

  // The global list of threads with migrate functions.
  static MigrateList migrate_list_ TA_GUARDED(list_lock_);

  Canary canary_;

  // These fields are among the most active in the thread. They are grouped
  // together near the front to improve cache locality.
  __NO_UNIQUE_ADDRESS mutable fbl::NullLock scheduler_variable_lock_;
  unsigned int flags_{};
  ktl::atomic<uint32_t> signals_;
  SchedulerState scheduler_state_ TA_GUARDED(get_lock());
  SchedulerQueueState scheduler_queue_state_ TA_GUARDED(scheduler_variable_lock_);
  WaitQueueCollection::ThreadState wait_queue_state_ TA_GUARDED(get_lock());
  fbl::DoublyLinkedList<OwnedWaitQueue*> owned_wait_queues_ TA_GUARDED(get_lock());
  TaskState task_state_;
  PreemptionState preemption_state_;
  MemoryAllocationState memory_allocation_state_;

  // Must only be accessed by "this" thread.
  RestrictedStatePtr restricted_state_;

#if WITH_LOCK_DEP
  // state for runtime lock validation when in thread context
  lockdep::ThreadLockState lock_state_;
#endif

  // The current address space this thread is associated with.
  // This can be null if this is a kernel thread.
  // See the comments near active_aspace() for more details.
  RelaxedAtomic<VmAspace*> aspace_{nullptr};

  // Saved by SignalPolicyException() to store the type of policy error, and
  // passed to exception disptach in ProcessPendingSignals().
  uint32_t extra_policy_exception_code_ TA_GUARDED(get_lock()) = 0;
  uint32_t extra_policy_exception_data_ TA_GUARDED(get_lock()) = 0;

  // Is this thread allowed to own wait queues?  Set to false by a thread as
  // it exits, but before it reaches the DEAD state.
  bool can_own_wait_queues_ TA_GUARDED(get_lock()) = true;

  // Strong reference to user thread if one exists for this thread.
  // In the common case freeing Thread will also free ThreadDispatcher when this
  // reference is dropped.
  fbl::RefPtr<ThreadDispatcher> user_thread_;

  // When user_thread_ is set, these values are copied from ThreadDispatcher and
  // its parent ProcessDispatcher. Kernel threads maintain an independent tid.
  zx_koid_t tid_ = KernelObjectId::Generate();
  zx_koid_t pid_ = ZX_KOID_INVALID;

  // Architecture-specific stuff.
  arch_thread arch_{};

  KernelStack stack_;

  // This is used by dispatcher.cc:SafeDeleter.
  void* recursive_object_deletion_list_ = nullptr;

  // This always includes the trailing NUL.
  char name_[ZX_MAX_NAME_LEN]{};

  // Buffering for Debuglog output.
  Linebuffer linebuffer_;

  // The flow id allocated before blocking on the last lock.
  RelaxedAtomic<uint64_t> lock_flow_id_{0};

  // Indicates whether user register state (debug, vector, fp regs, etc.) has been saved to the
  // arch_thread_t as part of thread suspension / exception handling.
  //
  // When a user thread is suspended or generates an exception (synthetic or architectural) that
  // might be observed by another process, we save user register state to the thread's arch_thread_t
  // so that it may be accessed by a debugger.  Upon leaving a suspended or exception state, we
  // restore user register state.
  bool user_state_saved_ TA_GUARDED(get_lock()){false};

  // For threads with migration functions, indicates whether a migration is in progress. When true,
  // the migrate function has been called with Before but not yet with After.
  //
  // TODO(johngro): What to do about this?  Should it be protected by the
  // thread's lock?  Does the thread's lock need to be held exclusively when a
  // thread's migrate function is called?
  bool migrate_pending_ TA_GUARDED(get_lock()){};

  // Provides a way to execute custom logic when a thread must be migrated between CPUs.
  MigrateFn migrate_fn_ TA_GUARDED(get_lock());

  // Provides a way to execute custom logic when a thread is context switched to or away from.
  ContextSwitchFn context_switch_fn_;

  // An Accessor for the thread's restartable sequence structure, if any.
  //
  // This is used to detect when a thread is preempted or enters a waiting state
  // while in a restartable sequence.  When this happens, the restartable sequence
  // is interrupted, and the thread will restart the sequence when it resumes.
  //
  // Kernel threads do not use restartable sequences, so this will always be !IsValid() for kernel
  // threads.
  //
  // Must be accessed only by the current thread.
  page_map::Accessor<zx_rseq_t> rseq_accessor_;

  // The sampling session this thread is associated with.
  static constexpr uint64_t kInvalidSamplingSession = -1;
  RelaxedAtomic<uint64_t> sampling_session_{kInvalidSamplingSession};

  // Used to track threads that have set |migrate_fn_|. This is used to migrate
  // threads before a CPU is taken offline.
  fbl::DoublyLinkedListNodeState<Thread*> migrate_list_node_ TA_GUARDED(list_lock_);

  // Node storage for existing on the global thread list.
  fbl::DoublyLinkedListNodeState<Thread*> thread_list_node_ TA_GUARDED(list_lock_);

  // Node storage for existing on the temporary batch unblock list.
  fbl::DoublyLinkedListNodeState<Thread*> unblock_list_node_ TA_GUARDED(get_lock());

  // Node storage for existing on the save state list.
  fbl::SinglyLinkedListNodeState<Thread*> save_state_list_node_ TA_GUARDED(get_lock());

  // State of the thread with respect to memory stalls.
  //
  // It can only be accessed by the thread's current CPU. Hence, preemption must
  // be disabled while accessing it.
  ThreadStallState memory_stall_state_ TA_GUARDED(preempt_disabled_token) =
      ThreadStallState::IgnoredKernelOnly;
};

// For the moment, the arch-specific current thread implementations need to come here, after the
// Thread definition. One of the arches needs to know the structure of Thread to compute the offset
// that the hardware pointer holds into Thread.
#include <arch/current_thread.h>
Thread* Thread::Current::Get() { return arch_get_current_thread(); }

extern "C" void arch_dump_thread(const Thread* t) TA_REQ_SHARED(t->get_lock());

class ThreadDumper {
 public:
  static void DumpLocked(const Thread* t, const char* match_owner, bool full)
      TA_REQ_SHARED(t->get_lock());
};

inline void Thread::DumpDuringPanic(bool full) const TA_NO_THREAD_SAFETY_ANALYSIS {
  ThreadDumper::DumpLocked(this, nullptr, full);
}

inline void Thread::DumpAllDuringPanic(const char* match_owner,
                                       bool full) TA_NO_THREAD_SAFETY_ANALYSIS {
  // Skip grabbing the lock if we are panic'ing
  DumpAllLocked(match_owner, full);
}

inline void Thread::DumpTidDuringPanic(zx_koid_t tid, bool full) TA_NO_THREAD_SAFETY_ANALYSIS {
  // Skip grabbing the lock if we are panic'ing
  DumpTidLocked(tid, full);
}

// Other thread-system bringup functions.
void thread_init_early();
void thread_secondary_cpu_entry() __NO_RETURN;

// Call the arch-specific signal handler.
extern "C" void arch_iframe_process_pending_signals(iframe_t* iframe);

// find a thread based on the thread id
// NOTE: used only for debugging, its a slow linear search through the
// global thread list
//
// Note: any thread pointer borrowed by this function is only guaranteed to be
// alive as long as the Thread::list_lock is held.  Once this is dropped, the
// thread could cease to exist at any time, and the pointer must be considered
// to be invalid.
Thread* thread_id_to_thread_slow(zx_koid_t tid) TA_REQ(Thread::get_list_lock());

// RAII helper that installs/removes an exception context and saves/restores user register state.
// The class operates on the current thread.
//
// When a thread takes an exception, this class is used to make user register state available to
// debuggers and exception handlers.
//
// Example Usage:
//
// {
//   ScopedThreadExceptionContext context(...);
//   HandleException();
// }
//
// Note, ScopedThreadExceptionContext keeps track of whether the state has already been saved so
// it's safe to nest them:
//
// void Foo() {
//   ScopedThreadExceptionContext context(...);
//   Bar();
// }
//
// void Bar() {
//   ScopedThreadExceptionContext context(...);
//   Baz();
// }
//
class ScopedThreadExceptionContext {
 public:
  explicit ScopedThreadExceptionContext(const arch_exception_context_t* context);
  ~ScopedThreadExceptionContext();
  DISALLOW_COPY_ASSIGN_AND_MOVE(ScopedThreadExceptionContext);

 private:
  Thread* thread_;
  const arch_exception_context_t* context_;
  bool need_to_remove_;
  bool need_to_restore_;
};

// RAII helper to enforce that a block of code does not allocate memory.
//
// See |Thread::Current::memory_allocation_state()|.
class ScopedMemoryAllocationDisabled {
 public:
  ScopedMemoryAllocationDisabled() { Thread::Current::memory_allocation_state().Disable(); }
  ~ScopedMemoryAllocationDisabled() { Thread::Current::memory_allocation_state().Enable(); }
  DISALLOW_COPY_ASSIGN_AND_MOVE(ScopedMemoryAllocationDisabled);
};

// AssertInWaitQueue asserts (as its name implies) that this thread is
// currently blocked in |wq|, and that the wait queue is locked.  This should
// be sufficient to establish (at runtime) that it is safe to allow
// read-access to the thread's members which are used to track the thread's
// state in the queue.
//
// TODO(johngro) : Link to something documenting the reasoning behind this.
// 1) Consider making these asserts extra-debug-only asserts, so that most
//    builds do not need to suffer the runtime overhead of using them.
// 2) Consider ways to limit the scope of the read-only capability to just the
//    wait queue state (and other things protected by the fact that we are
//    blocked in the wait queue) and nothing else.  While the wait queue state
//    cannot mutate while we exist in the wait queue (at least, not without
//    holding both the wait queue and thread's lock), other things can.
static inline void AssertInWaitQueue(const Thread& t, const WaitQueueBase& wq) TA_REQ(wq.get_lock())
    TA_ASSERT_SHARED(t.get_lock()) {
  [&]() TA_NO_THREAD_SAFETY_ANALYSIS {
    DEBUG_ASSERT(t.wait_queue_state().InWaitQueue());
    DEBUG_ASSERT(t.state() == THREAD_BLOCKED || t.state() == THREAD_BLOCKED_READ_LOCK);
    DEBUG_ASSERT(t.wait_queue_state().blocking_wait_queue() == &wq);
  }();
}

////////////////////////////////////////////////////////////////////////////////
//
// Here and below, we need to deal with static analysis.  Right now, we need to
// disable static analysis while we figure it out.
//
////////////////////////////////////////////////////////////////////////////////

// WaitQueue collection trait implementations.  While typically these would be
// implemented in-band in the trait class itself, these definitions must come
// last, after the definition Thread.  This is because the traits (defined in
// WaitQueueCollection) need to understand the layout of Thread in order to be
// able to access both scheduler state and wait queue state variables.
inline WaitQueueCollection::Key WaitQueueCollection::BlockedThreadTreeTraits::GetKey(
    const Thread& thread) TA_NO_THREAD_SAFETY_ANALYSIS {
  // TODO(johngro): consider extending FBL to support a "MultiWAVLTree"
  // implementation which would allow for nodes with identical keys, breaking
  // ties (under the hood) using pointer value.  This way, we would not need to
  // manifest our own pointer in GetKey or in our key type.
  return {thread.wait_queue_state().blocked_threads_tree_sort_key_,
          reinterpret_cast<uintptr_t>(&thread)};
}

inline fbl::WAVLTreeNodeState<Thread*>& WaitQueueCollection::BlockedThreadTreeTraits::node_state(
    Thread& thread) TA_NO_THREAD_SAFETY_ANALYSIS {
  return thread.wait_queue_state().blocked_threads_tree_node_;
}

inline SchedDuration WaitQueueCollection::MinRelativeDeadlineTraits::GetValue(const Thread& thread)
    TA_NO_THREAD_SAFETY_ANALYSIS {
  const auto& scheduler_state = thread.scheduler_state();
  if (scheduler_state.effective_profile().IsDeadline()) {
    return scheduler_state.effective_profile().deadline().deadline_ns;
  }
  return SchedDuration::Max();
}

inline SchedDuration WaitQueueCollection::MinRelativeDeadlineTraits::GetSubtreeBest(
    const Thread& thread) TA_NO_THREAD_SAFETY_ANALYSIS {
  return thread.wait_queue_state().subtree_min_deadline_;
}

inline bool WaitQueueCollection::MinRelativeDeadlineTraits::Compare(ValueType a, ValueType b)
    TA_NO_THREAD_SAFETY_ANALYSIS {
  return a < b;
}

inline void WaitQueueCollection::MinRelativeDeadlineTraits::AssignBest(
    Thread& thread, SchedDuration val) TA_NO_THREAD_SAFETY_ANALYSIS {
  thread.wait_queue_state().subtree_min_deadline_ = val;
}

inline void WaitQueueCollection::MinRelativeDeadlineTraits::ResetBest(Thread& thread)
    TA_NO_THREAD_SAFETY_ANALYSIS {
  // In a debug build, reset the subtree best as we leave the collection.
  // This can help to find bugs by allowing us to assert that the value is zero/max
  // during insertion, however it is not strictly needed in a production build
  // and can be skipped.
#ifdef DEBUG_ASSERT_IMPLEMENTED
  thread.wait_queue_state().subtree_min_deadline_ = SchedDuration::Max();
#endif  // DEBUG_ASSERT_IMPLEMENTED
}

inline void PreemptionState::AssertCapabilityHeld(
    concurrent::CapabilityToken<PreemptionState>::TagType) {
  DEBUG_ASSERT(Thread::Current::preemption_state().PreemptIsEnabled() == false);
}

inline SchedulerState::WaitQueueInheritedSchedulerState*
WaitQueueCollection::FindInheritedSchedulerStateStorage() TA_NO_THREAD_SAFETY_ANALYSIS {
  return threads_.is_empty()
             ? nullptr
             : &threads_.back().wait_queue_state().inherited_scheduler_state_storage_;
}

struct BrwLockOps {
  // Attempt to lock the set of threads needed for a BrwLock wake operation.  If
  // the first thread to wake is a writer, the set consists of just this thread.
  // If the first thread to wake is a reader, then the set to wake is all of the
  // readers we encounter until we hit another writer, or run out of threads to
  // wake.
  //
  // On success, the list of threads to wake is returned with each thread
  // locked, and with each thread prepared to unblock as if Dequeue had been
  // called (blocked_status has been set to OK, and blocking_wait_queue has been
  // cleared).
  //
  // On failure, no list is returned, and all threads will be unlocked and
  // waiting in the collection.
  struct LockForWakeResult {
    Thread::UnblockList list;
    uint32_t count{0};
  };

  static ktl::optional<LockForWakeResult> LockForWake(WaitQueue& queue, zx_instant_mono_t now)
      TA_REQ(chainlock_transaction_token, queue.get_lock());
};

struct WaitQueueLockOps {
  // Attempt to lock one or all threads for waking.
  //
  // When successful, removes the threads from the wait queue, moving them to a
  // temporary unblock list and setting their wake result in the process.
  // Callers may then perform any internal bookkeeping before dropping the queue
  // lock, and finally waking the threads using Scheduler::Unblock.
  //
  // Returns std::nullopt in the case that a backoff condition is encountered.
  static ktl::optional<Thread::UnblockList> LockForWakeAllInQueue(WaitQueue& queue,
                                                                  zx_status_t wait_queue_error)
      TA_REQ(chainlock_transaction_token, queue.get_lock());

  // Same as LockForWakeAllInQueue(), but does not dequeue the threads from the wait queue.
  static ktl::optional<Thread::UnblockList> LockForWakeAllInPlace(WaitQueue& queue)
      TA_REQ(chainlock_transaction_token, queue.get_lock());

  // Same as LockForWakeAllInPlace(), but only locks the first thread.
  static ktl::optional<Thread::UnblockList> LockForWakeOneInPlace(WaitQueue& queue)
      TA_REQ(chainlock_transaction_token, queue.get_lock());
};

// RAII class to mark sections of code as memory stalls.
//
// Nesting is not allowed. It is a programming error to instantiate this class
// if there exists another instance on the call stack.
class ScopedMemoryStall {
 public:
  ScopedMemoryStall();
  ~ScopedMemoryStall();

  // Disallow copying, assigning and moving.
  ScopedMemoryStall(const ScopedMemoryStall&) = delete;
  ScopedMemoryStall(ScopedMemoryStall&&) = delete;
  ScopedMemoryStall& operator=(const ScopedMemoryStall&) = delete;
  ScopedMemoryStall& operator=(ScopedMemoryStall&&) = delete;
};

#endif  // ZIRCON_KERNEL_INCLUDE_KERNEL_THREAD_H_
