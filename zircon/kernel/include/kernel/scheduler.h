// Copyright 2018 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT
#ifndef ZIRCON_KERNEL_INCLUDE_KERNEL_SCHEDULER_H_
#define ZIRCON_KERNEL_INCLUDE_KERNEL_SCHEDULER_H_

#include <lib/fit/function.h>
#include <lib/fxt/interned_string.h>
#include <lib/fxt/string_ref.h>
#include <lib/power-management/bandwidth-reservation-cache.h>
#include <lib/power-management/energy-model.h>
#include <lib/power-management/power-state.h>
#include <lib/power-management/types.h>
#include <lib/relaxed_atomic.h>
#include <lib/sched/affine.h>
#include <lib/zircon-internal/macros.h>
#include <platform.h>
#include <stdint.h>
#include <zircon/syscalls/scheduler.h>
#include <zircon/syscalls/system.h>
#include <zircon/types.h>

#include <fbl/intrusive_pointer_traits.h>
#include <fbl/intrusive_wavl_tree.h>
#include <fbl/wavl_tree_augmented_invariant_observer.h>
#include <ffl/fixed.h>
#include <kernel/dpc.h>
#include <kernel/mp.h>
#include <kernel/owned_wait_queue.h>
#include <kernel/scheduler_state.h>
#include <kernel/scheduler_tracing.h>
#include <kernel/spinlock.h>
#include <kernel/thread.h>
#include <kernel/wait.h>
#include <ktl/algorithm.h>
#include <ktl/atomic.h>
#include <ktl/optional.h>
#include <ktl/span.h>
#include <ktl/utility.h>

// Forward declarations.
struct percpu;
struct cpu_stats;

// Ensure this define has a value when not defined globally by the build system.
#ifndef SCHEDULER_TRACING_LEVEL
#define SCHEDULER_TRACING_LEVEL 0
#endif

// Ensure this define has a value when not defined globally by the build system.
#ifndef SCHEDULER_QUEUE_TRACING_ENABLED
#define SCHEDULER_QUEUE_TRACING_ENABLED false
#endif

// The processing rate of a CPU relative to the fastest CPU in the system.
using SchedProcessingRate = ffl::Fixed<int64_t, 31>;

// Make sure lib/power-management and the scheduler definition stay in sync.
static_assert(ktl::is_same_v<power_management::ProcessingRate, SchedProcessingRate>);

// Converts a userspace CPU performance scale to a SchedProcessingRate value.
//
// The returned value will always be greater than zero.
constexpr SchedProcessingRate ToSchedProcessingRate(zx_cpu_performance_scale_t value) {
  const size_t FractionaBits = sizeof(value.fractional_part) * 8;
  const SchedProcessingRate performance_scale = ffl::FromRaw<FractionaBits>(
      uint64_t{value.integral_part} << FractionaBits | value.fractional_part);
  return ktl::clamp(performance_scale, SchedProcessingRate::Epsilon(), SchedProcessingRate{1});
}

// Converts a SchedProcessingRate value to a userspace CPU performance scale.
constexpr zx_cpu_performance_scale_t ToUserPerformanceScale(SchedProcessingRate value) {
  using UserScale = ffl::Fixed<uint64_t, 32>;
  const UserScale user_scale{value};
  const uint64_t integral = user_scale.Integral().raw_value() >> UserScale::Format::FractionalBits;
  const uint64_t fractional = user_scale.Fraction().raw_value();
  return {.integral_part = static_cast<uint32_t>(integral),
          .fractional_part = static_cast<uint32_t>(fractional)};
}

// Implements fair and deadline scheduling algorithms and manages the associated
// per-CPU state.
class Scheduler {
 public:
  // Returns true if new scheduler wakeup accounting is enabled either by boot
  // option or gn arg.
  static bool EnableNewWakeupAccounting();

  // Helper type used to implement various PI operations. See
  // //zircon/kernel/kernel/scheduler_pi.cc to see how it is used.
  struct Pi;

  // Minimum fair time slice. Set to avoid excessive reschedule overhead.
  // TODO(eieio): This needs to be revised and/or scaled for lower OPPs.
  static constexpr SchedDuration kMinimumFairCapacity = SchedMs(1);

  // The default period for fair tasks when period expansion is not in effect.
  static constexpr SchedDuration kDefaultFairPeriod = SchedDefaultFairPeriod;

  // The minimum fair utilization before period expansion is needed.
  static constexpr SchedUtilization kMinFairUtilization = kMinimumFairCapacity / kDefaultFairPeriod;

  // Reciprocal of the default fair period to minimize runtime divisions in fair
  // utilization calculations.
  static constexpr SchedUtilization kReciprocalDefaultFairPeriod = 1 / kDefaultFairPeriod;

  // Ensure that the min fair utilization is in the range (0, 1].
  static_assert(0 < kMinimumFairCapacity && kMinimumFairCapacity <= kDefaultFairPeriod);

  // The threshold for cross-cluster work stealing. Queues with an estimated
  // runtime below this value are not stolen from if the target and destination
  // CPUs are in different logical clusters. In a performance-balanced system,
  // this tunable value approximates the cost of cross-cluster migration due to
  // cache misses, assuming a task has high cache affinity in its current
  // cluster. This tunable may be increased to limit cross-cluster spill over.
  static constexpr SchedDuration kInterClusterThreshold = SchedMs(2);

  // The threshold for early termination when searching for a CPU to place a
  // task. Queues with an estimated runtime below this value are sufficiently
  // unloaded. In a performance-balanced system, this tunable value approximates
  // the cost of intra-cluster migration due to cache misses, assuming a task
  // has high cache affinity with the last CPU it ran on. This tunable may be
  // increased to limit intra-cluster spill over.
  static constexpr SchedDuration kIntraClusterThreshold = SchedUs(25);

  // The per-CPU deadline utilization limit to attempt to honor when selecting a
  // CPU to place a task. It is up to userspace to ensure that the total set of
  // deadline tasks can honor this limit. Even if userspace ensures the total
  // set of deadline utilizations is within the total available processor
  // resources, when total utilization is high enough it may not be possible to
  // honor this limit due to the bin packing problem.
  static constexpr SchedUtilization kCpuUtilizationLimit{1};

  // The maximum deadline utilization permitted for a single thread. This limit
  // is applied when scaling the utilization of a deadline task to the relative
  // performance of a candidate target processor -- placing a task on a
  // processor that would cause the scaled thread utilization to exceed this
  // value is avoided if possible.
  static constexpr SchedUtilization kThreadUtilizationMax{1};

  // The adjustment rates of the exponential moving averages tracking the
  // expected runtimes of each thread.
  static constexpr ffl::Fixed<int32_t, 2> kExpectedRuntimeAlpha = ffl::FromRatio(1, 4);
  static constexpr ffl::Fixed<int32_t, 0> kExpectedRuntimeBeta = ffl::FromRatio(1, 1);

  Scheduler() = default;
  ~Scheduler() = default;

  Scheduler(const Scheduler&) = delete;
  Scheduler& operator=(const Scheduler&) = delete;

  // Accessors for total weight and number of runnable tasks.
  SchedWeight GetTotalWeight() const TA_EXCL(queue_lock_);
  size_t GetRunnableTasks() const TA_EXCL(queue_lock_);

  // Dumps the state of the run queue to the specified output target.  Note that
  // interrupts must be disabled in order to call this method.
  void Dump(FILE* output_target = stdout, bool queue_state_only = false) TA_EXCL(queue_lock_);

  // Dumps info about the currently active thread (if any).  Note that
  // interrupts must be disabled in order to call this method.
  void DumpActiveThread(FILE* output_target = stdout) TA_EXCL(queue_lock_);

  // Returns the number of the CPU this scheduler instance is associated with.
  cpu_num_t this_cpu() const { return this_cpu_; }

  // Returns the index of the logical cluster of the CPU this scheduler instance
  // is associated with.
  size_t cluster() const { return cluster_; }

  // Returns the lock-free value of the estimated queue time for the CPU this
  // scheduler is associated with.
  SchedDuration exported_queue_time_ns() const { return exported_queue_time_ns_.load(); }

  // Returns the lock-free value of the estimated deadline utilization for the
  // CPU this scheduler is associated with.
  SchedUtilization exported_deadline_utilization() const {
    return exported_deadline_utilization_.load();
  }

  // Returns the lock-free value of the estimated deadline utilization for the
  // CPU, clamped to the current rate limits, for the CPU this scheduler is
  // associated with.
  SchedUtilization exported_clamped_deadline_utilization() const {
    return exported_clamped_deadline_utilization_.load();
  }

  // Returns the lock-free value of the estimated fair utilization for the
  // CPU this scheduler is associated with.
  SchedUtilization exported_fair_utilization() const { return exported_fair_utilization_.load(); }

  // Returns the lock-free value of the estimated fair utilization for the
  // CPU, clamped to the current rate limits, for the CPU this scheduler is
  // associated with.
  SchedUtilization exported_clamped_fair_utilization() const {
    return exported_clamped_fair_utilization_.load();
  }

  // Returns the lock-free value of the total estimated utilization (deadline + fair)
  // for the CPU, clamped to the current rate limits, for the CPU this scheduler
  // is associated with.
  SchedUtilization exported_clamped_total_utilization() const {
    return exported_clamped_total_utilization_.load();
  }

  // Returns the lock-free value of the processing rate for the CPU this
  // scheduler is associated with.
  SchedProcessingRate exported_processing_rate() const { return exported_processing_rate_.load(); }

  // Returns the lock-free value of the max processing rate for the CPU this
  // scheduler is associated with.
  SchedProcessingRate exported_max_processing_rate() const {
    return exported_max_processing_rate_.load();
  }

  // Returns the processing rate of the CPU this scheduler instance is
  // associated with.
  SchedProcessingRate processing_rate() const TA_REQ(queue_lock_) {
    return power_level_control_.processing_rate();
  }

  // Returns the reciprocal processing rate of the CPU this scheduler instance
  // is associated with.
  SchedProcessingRate processing_rate_reciprocal() const TA_REQ(queue_lock_) {
    return power_level_control_.processing_rate_reciprocal();
  }

  int32_t runnable_task_count() const TA_REQ(queue_lock_) { return runnable_task_count_; }

  // Returns a pointer to the currently running thread, if any.
  //
  // TODO(johngro); This is not particularly safe.  As soon as we returned this
  // thread's pointer, it could (in theory) exit.  We probably need to require
  // that the scheduler's queue lock be held in order to peek at the active
  // thread member.
  Thread* active_thread() const TA_EXCL(queue_lock_) {
    Guard<MonitoredSpinLock, IrqSave> guard{&queue_lock_, SOURCE_TAG};
    return active_thread_;
  }

  // Public entry points.

  // See the comment in Thread::CreateEtc for why InitializeThread does not take
  // a stance on whether or not we should be in an active chain-lock
  // transaction. On the surface, demanding that we hold a threads lock without
  // also demanding that we also be involved in an active chain-lock transaction
  // is an apparent contradiction of some fundamental invariants (how can a
  // chain lock be held without being in a transaction?), but when used
  // exclusively from Thread::CreateEtc, it should be OK.
  static void InitializeThread(Thread* thread, const SchedulerState::BaseProfile& profile)
      TA_REQ(thread->get_lock());
  static void InitializeFirstThread(Thread* thread)
      TA_REQ(chainlock_transaction_token, thread->get_lock());
  static void RemoveFirstThread(Thread* thread)
      TA_REQ(chainlock_transaction_token, thread->get_lock());
  static void Block(Thread* current_thread)
      TA_REQ(chainlock_transaction_token, current_thread->get_lock());
  static void Yield(Thread* current_thread)
      TA_REQ(chainlock_transaction_token, current_thread->get_lock());

  // Indicates the type of preemption for diagnostic/tracing purposes.
  enum class PreemptType : uint8_t {
    Irq,
    FlushPending,
    Reschedule,
  };
  static fxt::StringRef<fxt::RefType::kId> ToStringRef(PreemptType preempt_type);

  // No chainlocks should be held when calling preempt.  The thread's lock will
  // be obtained unconditionally in the process.
  //
  // If holding a spinlock, the preemption will be deferred via self-IPI until
  // all spinlock are released and interrupts are re-enabled.
  static void Preempt(PreemptType preempt_type) TA_EXCL(chainlock_transaction_token);

  static void Reschedule(Thread* current_thread)
      TA_REQ(chainlock_transaction_token, current_thread->get_lock());
  static void RescheduleInternal(Thread* current_thread)
      TA_REQ(chainlock_transaction_token, current_thread->get_lock());

  // Finish unblocking a thread.  This function expects the thread to be locked
  // when called.  It will:
  //
  // 1) Select a scheduler compatible with the thread.
  // 2) Place the thread into that scheduler's run queue.
  // 3) Drop the thread's lock.
  // 4) Reschedule if needed/permitted.
  static void Unblock(Thread* thread) TA_REQ(chainlock_transaction_token)
      TA_REL(thread->get_lock());

  // Unblock list expects to receive a list of threads, all of whose
  // locks are currently held.  It will drop each thread's lock after
  // successfully assigning it to a scheduler.
  static void Unblock(Thread::UnblockList thread_list) TA_REQ(chainlock_transaction_token);

  // Similar to Unblock, except that it uses different target CPU selection
  // criteria to enable a "synchronous wake", if possible.
  //
  // A synchronous wake is a sequence of operations where the current thread
  // wakes another thread, updates the woken thread's scheduling parameters, and
  // then blocks, such that, under compatible conditions, the woken thread
  // replaces the current thread as the actively running thread on the CPU in
  // the _same reschedule operation_. A correctly executed synchronous wake
  // optimizes queuing latency in synchronous wake-and-wait operations, such as
  // channel calls.
  //
  // Due to the structure of run queues and wait queues, and their respective
  // locking protocols, a synchronous wake cannot be implemented as a single
  // scheduler operation. Instead, some variation of the following sequence of
  // operations must be executed to achieve the same effect:
  //
  // 1. Prevent immediate rescheduling in step 2 using an appropriate mechanism,
  //    or combination of mechanisms, for the use case (i.e. spinlock + irq
  //    disable, preempt disable, eager resched disable).
  // 2. Unblock the target thread and place it on the same CPU as the current
  //    thread, if possible.
  // 3. Update the target thread's scheduling parameters to transfer bandwidth
  //    and/or urgency from the current to the target thread, if possible.
  // 4. Block the current thread and reschedule the local CPU, and remote CPU if
  //    applicable.
  //
  // This method implements step 2 of the synchronous wake sequence. Note that
  // the normal unblock may be used in place of this method without impacting
  // correctness. However, suboptimal task placement can occur at step 2 and
  // multiple reschedules may be necessary in steps 3 and 4.
  static void UnblockSynchronous(Thread* thread) TA_REQ(chainlock_transaction_token)
      TA_REL(thread->get_lock());

  // UnblockIdle is used in the process of creation of the idle thread.  It
  // simply asserts that the thread is (in fact) flagged as the idle thread, and
  // that it has hard affinity for exactly one CPU (its CPU).  It then sets the
  // thread to be ready, and ensures that its curr_cpu_ state is correct.
  static void UnblockIdle(Thread* idle_thread)
      TA_REQ(chainlock_transaction_token, idle_thread->get_lock());

  // Migrates any threads that can run on other CPUs off of the current CPU.
  static void MigrateUnpinnedThreads();

  // TimerTick is called when the preemption timer for a CPU has fired.
  //
  // This function is logically private and should only be called by timer.cc.
  static void TimerTick(SchedTime now);

  // Releases the lock held by the previous and current threads after a context
  // switch. This must be called by trampoline routines at some point before
  // jumping to the current thread's entry point.
  static void TrampolineLockHandoff();

  // Called when the base profile of |thread| has been changed by a program. The
  // thread must be either running or runnable when this method is called. If
  // the thread had been blocked instead, the change of the base profile would
  // have been managed at the WaitQueue level, perhaps eventually propagating to
  // the scheduler via UpstreamThreadBaseProfileChanged.
  static void ThreadBaseProfileChanged(Thread& thread)
      TA_REQ(chainlock_transaction_token, ChainLockable::GetLock(thread), preempt_disabled_token);

  // Called when the base profile of a thread (|upstream|) which exists upstream
  // of |target| has changed its base profile.  It is possible for the rules
  // regarding scheduling penalties base profile changes to be slightly
  // different for when a thread's effective profile changes as a result of its
  // own base profile changing, vs the base profile of a thread which exists
  // upstream of it in a PI graph.  Because of this, the two scenarios are
  // handled separately using two different methods.
  template <typename TargetType>
  static void UpstreamThreadBaseProfileChanged(const Thread& upstream, TargetType& target)
      TA_REQ(chainlock_transaction_token, ChainLockable::GetLock(upstream),
             ChainLockable::GetLock(target), preempt_disabled_token);

  // Called when a thread (|upstream|) blocks in an OwnedWaitQueue, and the
  // target of that OWQ (|target|) is running or runnable.  At the point that
  // this method is called, propagation of the static profile pressure
  // parameters (weight, deadline utilization, and minimum deadline) should have
  // already been propagated to |target|, and its EffectiveProfile should be
  // dirty.  This method is called in order to update the dynamic scheduling
  // parameters (start time, finish time, time slice remaining) of |target| as
  // needed because of the join operation.
  template <typename UpstreamType, typename TargetType>
  static void JoinNodeToPiGraph(const UpstreamType& upstream, TargetType& target,
                                ForceInheritance force_inheritance)
      TA_REQ(chainlock_transaction_token, ChainLockable::GetLock(upstream),
             ChainLockable::GetLock(target), preempt_disabled_token);

  // Called when a thread (|upstream|) unblocks from an OwnedWaitQueue and
  // leaves the PI graph whose running or runnable target is |target|.
  //
  // At the point that this method is called, propagation of the static profile
  // pressure parameters (weight, deadline utilization, and minimum deadline)
  // should have already been propagated to |target|, and its EffectiveProfile
  // should be dirty.  This method is called in order to update the dynamic
  // scheduling parameters (start time, finish time, time slice remaining) in
  // both |target| and |upstream| as needed because of the split operation.
  template <typename UpstreamType, typename TargetType>
  static void SplitNodeFromPiGraph(UpstreamType& upstream, TargetType& target,
                                   const SchedulerState::EffectiveProfile* old_target_ep = nullptr)
      TA_REQ(chainlock_transaction_token, ChainLockable::GetLock(upstream),
             ChainLockable::GetLock(target), preempt_disabled_token);

  // Updates the performance scales of the requested CPUs and returns the
  // effective values in place, which may be different than the requested values
  // if they are below the minimum safe values for the respective CPUs.
  //
  // Requires info.size() <= num CPUs.
  static void UpdateProcessingRates(ktl::span<zx_cpu_performance_info_t> info) TA_EXCL(queue_lock_);

  // Gets the performance scales of up to info.size() CPUs. Returns the last
  // values requested by userspace, even if they have not yet taken effect.
  //
  // Requires info.size() <= num CPUs.
  static void GetPerformanceScales(ktl::span<zx_cpu_performance_info_t> info) TA_EXCL(queue_lock_);

  // Gets the default performance scales of up to info.size() CPUs. Returns the
  // initial values determined by the system topology, or 1.0 when no topology
  // is available.
  //
  // Requires info.size() <= num CPUs.
  static void GetDefaultPerformanceScales(ktl::span<zx_cpu_performance_info_t> info)
      TA_EXCL(queue_lock_);

  // Updates the performance limits of the requested CPUs.
  //
  // Requires limits.size() <= num CPUs.
  static void UpdateProcessingLimits(ktl::span<zx_cpu_perf_limit_t> limits) TA_EXCL(queue_lock_);

  // Gets the processing limits of up to limits.size() CPUs.
  //
  // Requires limits.size() <= num CPUs.
  static void GetProcessingLimits(ktl::span<zx_cpu_perf_limit_t> limits) TA_EXCL(queue_lock_);

  // Returns the CPU stats for the given CPU number, forward projected to
  // account for idle or normalized busy time elapsed since the last relevant
  // state change.
  static cpu_stats GetProjectedCpuStats(cpu_num_t cpu_num) TA_EXCL(queue_lock_);

  // Get the mask of valid CPUs that thread may run on. If a new mask
  // is set, the thread will be migrated to satisfy the new constraint.
  static cpu_mask_t SetCpuAffinity(Thread& thread, cpu_mask_t affinity, AffinityType affinity_type)
      TA_EXCL(chainlock_transaction_token, thread.get_lock());

  // Run the passed callable while holding the specified scheduler's lock.
  // Currently used only by the threadload debug function in kernel/debug.cc
  template <typename Callable>
    requires requires(Scheduler& scheduler, Callable callable) { callable(scheduler); }
  static auto RunInLockedScheduler(cpu_num_t which_cpu, Callable callable) {
    // The call do Get will eventually DEBUG_ASSERT if our cpu ID is out of range.
    Scheduler& scheduler = *Get(which_cpu);
    Guard<MonitoredSpinLock, IrqSave> queue_guard{&scheduler.queue_lock_, SOURCE_TAG};
    return callable(scheduler);
  }

  // Run the passed callable while holding the current CPU's scheduler's lock.
  // Used by routines in thread.cc who need to change the current thread's
  // state, but need to be holding the current scheduler's lock in order to do
  // so.
  template <typename Callable>
    requires requires(Scheduler& scheduler, Callable callable) { callable(scheduler); }
  static auto RunInLockedCurrentScheduler(Callable callable) {
    DEBUG_ASSERT(arch_ints_disabled());
    Scheduler& scheduler = *Get();
    Guard<MonitoredSpinLock, NoIrqSave> queue_guard{&scheduler.queue_lock_, SOURCE_TAG};
    return callable(scheduler);
  }

  // Run the passed callable while holding the specified thread's scheduler's
  // lock, if any. Used by SetMigrateFn in thread.cc.
  template <typename Callable>
  static auto RunInThreadsSchedulerLocked(Thread* thread, Callable callable)
      TA_REQ(thread->get_lock()) {
    const cpu_num_t thread_cpu = thread->scheduler_state().curr_cpu();
    if (thread_cpu != INVALID_CPU) {
      Scheduler& scheduler = *Get(thread_cpu);
      Guard<MonitoredSpinLock, NoIrqSave> queue_guard{&scheduler.queue_lock_, SOURCE_TAG};
      return callable();
    }
    return callable();
  }

  // Set/clears the current cpu's bit in the global mp_active_mask.  Holds the
  // scheduler's queue lock while the bit is changed, enforcing the invariant
  // that a scheduler's active bit can only ever change while holding that
  // scheduler's lock.
  static void SetCurrCpuActive(bool is_active) {
    Scheduler& scheduler = *Get();
    Guard<MonitoredSpinLock, IrqSave> guard{&scheduler.queue_lock_, SOURCE_TAG};
    scheduler.SetIsActive(is_active);
  }

  // Peek at the current active mask for all schedulers.
  //
  // Note that this is just a lockless atomic load.  Baring special
  // circumstances, the set of active schedulers can change at any time.
  static cpu_mask_t PeekActiveMask() { return active_schedulers_.load(ktl::memory_order_relaxed); }

  // Peek at the current active mask state for a given CPU's scheduler.
  //
  // Note that this is just a lockless atomic load.  Baring special
  // circumstances, the set of active schedulers can change at any time.
  static bool PeekIsActive(cpu_num_t cpu) {
    const cpu_mask_t mask = cpu_num_to_mask(cpu);
    return (PeekActiveMask() & mask) != 0;
  }

  // Accessors for the "idle" state mask; similar to the active state mask.
  void SetIdle(bool is_idle) const {
    const cpu_mask_t mask = cpu_num_to_mask(this_cpu_);
    if (is_idle) {
      idle_schedulers_.fetch_or(mask, ktl::memory_order_relaxed);
    } else {
      idle_schedulers_.fetch_and(~mask, ktl::memory_order_relaxed);
    }
  }
  static cpu_mask_t PeekIdleMask() { return idle_schedulers_.load(ktl::memory_order_relaxed); }
  static bool PeekIsIdle(cpu_num_t cpu) { return (PeekIdleMask() & cpu_num_to_mask(cpu)) != 0; }

  // Accumulates processor idle time.
  void AddProcessorIdleTime(SchedDuration idle_time) {
    processor_idle_time_ns_.fetch_add(idle_time.raw_value(), ktl::memory_order_relaxed);
  }

  // Clears any accumulated processor idle time.
  void ResetProcessorIdleTime() { processor_idle_time_ns_.store(0, ktl::memory_order_relaxed); }

  // Returns the accumulated processor idle time for assertions/diagnostics.
  SchedDuration processor_idle_time() const {
    return SchedDuration{processor_idle_time_ns_.load(ktl::memory_order_relaxed)};
  }

  // Reschedules the given CPU mask, applying any updated bookkeeping that may
  // affect the currently running threads on those CPUs.
  inline static void RescheduleCpus(cpu_mask_t cpu_mask) TA_EXCL(queue_lock_);

  using PowerDomain = power_management::PowerDomain;
  using PowerDomainSet = power_management::PowerDomainSet;

  // Sets the power domain set for this scheduler instance, returning the previous set.
  PowerDomainSet ExchangePowerDomainSet(PowerDomainSet domain_set) TA_EXCL(queue_lock_) {
    Guard<MonitoredSpinLock, IrqSave> guard{&queue_lock_, SOURCE_TAG};
    PowerDomainSet previous = power_level_control_.ExchangePowerDomainSet(ktl::move(domain_set));

    // Update the cross-processor shadow value of the max processing rate to match the current power
    // domain value.
    exported_max_processing_rate_ = power_level_control_.clamped_max_processing_rate();

    return previous;
  }

  // Sets the power domain set for all CPUs.
  static void SetPowerDomainSet(PowerDomainSet power_domain_set);

  // Updates the current power level with the value reported by the power level
  // controller.
  static zx::result<cpu_mask_t> UpdatePowerLevel(uint32_t domain_id, uint64_t controller_id,
                                                 power_management::ControlInterface interface,
                                                 uint64_t arg);

  // Updates the current power level for this CPU with the value reported by the power level
  // controller. Called by kernel tests and sys_system_set_processor_power_state.
  zx::result<> UpdateActivePowerLevel(uint8_t power_level) TA_EXCL(queue_lock_) {
    Guard<MonitoredSpinLock, IrqSave> guard{&queue_lock_, SOURCE_TAG};
    return power_level_control_.UpdateActivePowerLevel(power_level);
  }

  // Returns the current active power level, if any.
  ktl::optional<uint8_t> GetActivePowerLevel() const TA_EXCL(queue_lock_) {
    Guard<MonitoredSpinLock, IrqSave> guard{&queue_lock_, SOURCE_TAG};
    return power_level_control_.active_power_level();
  }

  // Sends a power level request through this scheduler's power level controller for testing. This
  // method is called from thread and IRQ context to ensure that the underlying machinery can be
  // safely called within Block, Unblock, and other scheduling operations.
  //
  // Returns true if the request was sent, false otherwise.
  bool RequestPowerLevelForTesting(uint8_t power_level) TA_EXCL(queue_lock_);

  // Returns the current power domain set for this scheduler instance.
  PowerDomainSet GetPowerDomainSetForTesting() TA_EXCL(queue_lock_) {
    Guard<MonitoredSpinLock, IrqSave> guard{&queue_lock_, SOURCE_TAG};
    return power_level_control_.power_domain_set();
  }

  // Returns the current power domain for this scheduler instance.
  fbl::RefPtr<PowerDomain> GetPowerDomainForTesting() TA_EXCL(queue_lock_) {
    Guard<MonitoredSpinLock, IrqSave> guard{&queue_lock_, SOURCE_TAG};
    return power_level_control_.domain();
  }

  // Returns the current active power coefficient.
  ktl::optional<uint64_t> GetActivePowerCoefficientNwForTesting() const TA_EXCL(queue_lock_) {
    Guard<MonitoredSpinLock, IrqSave> guard{&queue_lock_, SOURCE_TAG};
    return power_level_control_.active_power_coefficient_nw();
  }

  // Returns the max idle power coefficient.
  ktl::optional<uint64_t> GetMaxIdlePowerCoefficientNwForTesting() const TA_EXCL(queue_lock_) {
    Guard<MonitoredSpinLock, IrqSave> guard{&queue_lock_, SOURCE_TAG};
    return power_level_control_.max_idle_power_coefficient_nw();
  }

  // Const accessor for static analysis.
  const auto& queue_lock_cap() const TA_RET_CAP(queue_lock_) { return queue_lock_; }

 private:
  friend struct Thread;

  // Allow percpu to init our cpu number and performance scale.
  friend struct percpu;
  // Load balancer test.
  friend struct LoadBalancerTestAccess;
  // Allow tests to modify our state.
  friend class LoadBalancerTest;
  // Diagnostics used by LockupDetector
  friend void DumpSchedulerActiveThreadDetails(Scheduler& scheduler);

  // EffectiveProfile and BaseProfile are both inner classes of SchedulerState which
  // are also frequently used by the methods in Scheduler.  Make a couple of handy
  // private aliases to keep us from needing to say
  // SchedulerState::(Effective|Base)Profile in all of the Scheduler methods.
  using EffectiveProfile = SchedulerState::EffectiveProfile;
  using BaseProfile = SchedulerState::BaseProfile;

  using Disposition = SchedulerQueueState::Disposition;

  using SavedState = concurrent::ChainLockTransactionCommon::SavedState;

  static void LockHandoffInternal(SavedState saved_state, Thread* current_thread)
      TA_REQ(chainlock_transaction_token, current_thread->get_lock());

  // Sets the initial values of the CPU performance scales for this Scheduler
  // instance.
  void InitializeProcessingRate(SchedProcessingRate scale);

  static inline void RescheduleMask(cpu_mask_t cpus_to_reschedule_mask);
  static void PreemptLocked(Thread* current_thread, PreemptType preempt_type)
      TA_REQ(chainlock_transaction_token, current_thread->get_lock());

  // Unconditionally perform an expensive check of as many scheduler invariants
  // as we can.  Usually, this is not called directly; the ValidateInvariants
  // version (which is gated on a default-off build option) is preferred.
  void ValidateInvariantsUnconditional() const TA_REQ(queue_lock_, chainlock_transaction_token);
  void ValidateInvariants() const TA_REQ(queue_lock_, chainlock_transaction_token) {
    if constexpr (kSchedulerExtraInvariantValidation) {
      ValidateInvariantsUnconditional();
    }
  }

  // Runs the migration functions of threads in the save_state_list_, placing
  // the threads in their target RunQueues when done.
  //
  // Note that this method is called while holding the special "sched token"
  // used during reschedule operations. As a result, it is an absolute
  // requirement that any threads we lock in this method are not discoverable
  // and lockable from another scheduler that is also holding the "sched token."
  // Failure to meet this requirement will result in deadlock.
  void ProcessSaveStateList() TA_REQ(chainlock_transaction_token);

  // Returns true if the given thread needs to be migrated from one CPU to another.
  static bool NeedsMigration(Thread* thread)
      TA_REQ(chainlock_transaction_token, thread->get_lock());

  // Lockup detector needs to be able to hold a scheduler's lock in order to
  // print diagnostics
  auto& queue_lock() TA_RET_CAP(queue_lock_) { return queue_lock_; }

  // Specifies how to associate a thread with a Scheduler instance, update
  // metadata, and whether/where to place the thread in a run queue.
  enum class Placement {
    // Selects a place in the queue based on the current insertion time and
    // thread weight or deadline.
    Insertion,

    // Selects a place in the queue based on the original insertion time and
    // the updated (inherited or changed) weight or deadline on the same CPU.
    Adjustment,

    // Selects a place in the queue based on the original insertion time and
    // the updated time slice due to being preempted by another thread.
    Preemption,

    // Selects a place in the queue based on the insertion time in the original
    // queue adjusted for the new queue.
    Migration,

    // Updates the metadata to account for a stolen thread that was just taken
    // from a different queue. This is distinct from Migration in that the
    // thread is not also enqueued, it is run immediately by the new CPU.
    Association,
  };

  // Returns a tracing string ref for the give placement value.
  static fxt::StringRef<fxt::RefType::kId> ToStringRef(Placement placement);

  // Returns the current system time as a SchedTime value.
  static SchedTime CurrentTime() { return SchedTime{current_mono_time()}; }

  // A coherent pair of corresponding time/ticks values in the monotonic and
  // boot timelines.
  struct SchedMonoTimeAndBootTicks {
    SchedTime mono_time;
    zx_instant_boot_ticks_t boot_ticks;
  };

  // Returns the current monotonic time / boot ticks using coherent samples of
  // the respective timelines.
  // TODO(eieio): Look into making this non-static and requiring the queue lock
  // of the respective scheduler to ensure this is always called with the queue
  // lock held.
  static SchedMonoTimeAndBootTicks CurrentMonoTimeAndBootTicks() {
    InstantMonoTimeAndBootTicks mono_time_and_boot_ticks = current_mono_time_and_boot_ticks();
    return {.mono_time = SchedTime{mono_time_and_boot_ticks.mono_time},
            .boot_ticks = mono_time_and_boot_ticks.boot_ticks};
  }

  // Returns the Scheduler instance for the current CPU.
  static Scheduler* Get();

  // Returns the Scheduler instance for the given CPU.
  static Scheduler* Get(cpu_num_t cpu);

  // Find an appropriate CPU for a thread to run on, then lock that scheduler,
  // confirm that it is still active, and invoke the user-supplied callback with
  // the scheduler's queue_lock held.
  //
  // This method may need to search for a target CPU for the thread more than
  // once if the initially chosen scheduler becomes deactivated after it was
  // selected, but before it was locked.
  template <typename Callable>
  static cpu_num_t FindActiveSchedulerForThread(Thread* thread, Callable action)
      TA_REQ_SHARED(thread->get_lock()) {
    while (true) {
      const cpu_num_t target_cpu = FindTargetCpu(thread, UnblockType::Normal);
      Scheduler* const target = Get(target_cpu);
      Guard<MonitoredSpinLock, NoIrqSave> target_queue_guard{&target->queue_lock_, SOURCE_TAG};

      if (target->IsSchedulerActiveLocked()) {
        action(thread, target);
        return target_cpu;
      }

      IncFindTargetCpuRetriesKcounter();
    }
  }

  // Indicates how to select the target CPU for the woken task. This flag is a
  // hint to enable more optimal CPU placement for "synchronous wake"
  // operations, but is not required for correctness. See UnblockSynchronous for
  // more details.
  enum class UnblockType : bool {
    // Indicates that the target CPU selection process should place the woken
    // task based solely on its own scheduling properties and last CPU history.
    Normal,

    // Indicates that the target CPU selection process should place the task
    // based on both the woken and current threads' scheduling properties,
    // particularly the CPU and urgency of the current thread, such that, under
    // compatible conditions, the woken thread can replace the current thread on
    // a CPU as it blocks in the same reschedule operation.
    Synchronous,
  };

  // A helper function used by FindActiveSchedulerForThread.  Returns a CPU to
  // run the given thread on, but does not lock that scheduler, meaning that the
  // selected scheduler can become de-activated before the caller can lock it.
  //
  // Typically, users will want to use FindActiveSchedulerForThread instead,
  // which will supply a locked scheduler which is guaranteed to be active.
  //
  static cpu_num_t FindTargetCpu(Thread* thread, UnblockType unblock_type)
      TA_REQ_SHARED(thread->get_lock()) TA_EXCL(queue_lock_);

  // Increment the kcounter which tracks the number of times that an extra
  // attempt to find an active scheduler was needed during
  // FindActiveSchedulerForThread.  Sadly, the method cannot increment the
  // counter directly because of the way that kcounters need to be declared, in
  // a translation unit in an anonymous namespace, with no ability to fwd
  // declare the counter.
  static void IncFindTargetCpuRetriesKcounter();

  // Change this scheduler's CPU's active state. We require that the scheduler's
  // queue_lock be held during the operation. This enforces the invariant that
  // "the scheduler's active bit only changes while holding the scheduler's
  // queue lock".
  //
  // This means that things like "FindTargetCpu" are free to check the global
  // active mask to see whether or not we _think_ that a given scheduler is
  // likely to be active for selection purposes, but when it comes time to
  // actually add a thread to a scheduler's queues, we check the active state of
  // the scheduler's assigned CPU from within the safety of the queue lock using
  // IsLockedSchedulerActive()
  void SetIsActive(bool is_active) const TA_REQ(queue_lock_) {
    const cpu_mask_t mask = cpu_num_to_mask(this_cpu_);
    if (is_active) {
      active_schedulers_.fetch_or(mask, ktl::memory_order_relaxed);
    } else {
      active_schedulers_.fetch_and(~mask, ktl::memory_order_relaxed);
    }
    arch_set_blocking_disallowed(!is_active);
  }

  // Check to see if |this| scheduler is currently active while holding its
  // queue_lock, ensuring that it will stay in its current state until after we
  // drop the lock.
  bool IsSchedulerActiveLocked() const TA_REQ(queue_lock_) { return PeekIsActive(this_cpu_); }

  // Indicates whether the CPU stats being updated is for a busy or idle time
  // segment.
  enum class CpuStatsSegmentType : bool {
    Busy,
    Idle,
  };

  // Returns the segment type inferred from whether the give thread is an idle
  // thread or not.
  static CpuStatsSegmentType ToCpuStatsSegmentType(const Thread& thread) {
    return thread.IsIdle() ? CpuStatsSegmentType::Idle : CpuStatsSegmentType::Busy;
  }

  // Updates a segment of the cpu stats idle or busy timelines.
  //
  // CRITICAL: See implementation comment for coherency rules!
  void UpdateCpuStatsSegment(SchedTime now, CpuStatsSegmentType stats_segment_type,
                             SchedProcessingRate segment_processing_rate) const TA_REQ(queue_lock_);

  using EndTraceCallback = fit::inline_function<void(), sizeof(void*)>;

  // CPUs to reschedule after changes to active schedulers.
  struct RescheduleTargets {
    // The CPU that the thread was scheduled on.
    cpu_num_t target_cpu;
    // Other CPUs that should be rescheduled. This is typically due to power
    // level changes.
    cpu_mask_t cpus_to_reschedule;

    cpu_mask_t Mask() const { return cpu_num_to_mask(target_cpu) | cpus_to_reschedule; }
  };

  // Common logic for unblock API.
  static RescheduleTargets UnblockCommon(Thread* thread, SchedTime now, UnblockType unblock_type)
      TA_REQ(chainlock_transaction_token, thread->get_lock());

  // Common logic for reschedule API.
  void RescheduleCommon(Thread* current_thread, EndTraceCallback end_outer_trace = nullptr)
      TA_EXCL(queue_lock_) TA_REQ(chainlock_transaction_token, current_thread->get_lock());

  // A utility result type to help avoid missing updates to a thread's
  // start/finish times when dequeuing from a run queue with a variable
  // timeline.
  class DequeueResult {
   public:
    // Constructs an empty result.
    DequeueResult() = default;

    // Constructs a result without a pending update to start/finish times. If
    // the thread argument is nullptr an empty result is constructed.
    explicit(false) DequeueResult(Thread* thread) : thread_{thread} {}

    // Constructs a result with a pending update to start/finish times. The
    // thread argument MUST NOT be nullptr and UpdateThreadTimeline() must be
    // called with the thread's lock held to apply the pending update to
    // start/finish times before the result is destroyed or reassigned.
    DequeueResult(Thread* thread, SchedTime mono_start_time_ns, SchedTime mono_finish_time_ns)
        : thread_{thread},
          mono_start_time_ns_{mono_start_time_ns},
          mono_finish_time_ns_{mono_finish_time_ns} {
      DEBUG_ASSERT(thread_ != nullptr);
      DEBUG_ASSERT(mono_start_time_ns != 0 || mono_finish_time_ns != 0);
    }

    // Destroys the result, asserting that there is no pending update the
    // thread's start/finish times.
    ~DequeueResult() {
      DEBUG_ASSERT_MSG(!is_updated_pending(),
                       "DequeueResult destroyed with a pending timeline update!");
    }

    // The result type is move-only.
    DequeueResult(const DequeueResult&) = delete;
    DequeueResult& operator=(const DequeueResult&) = delete;

    // Moves the given result into this instance, asserting that the current
    // instance does not have a pending update to the thread's start/finish
    // times.
    DequeueResult(DequeueResult&& other) { *this = ktl::move(other); }
    DequeueResult& operator=(DequeueResult&& other) {
      if (this != &other) {
        DEBUG_ASSERT_MSG(!is_updated_pending(),
                         "DequeueResult destroyed with a pending timeline update!");
        thread_ = nullptr;
        ktl::swap(thread_, other.thread_);
        ktl::swap(mono_start_time_ns_, other.mono_start_time_ns_);
        ktl::swap(mono_finish_time_ns_, other.mono_finish_time_ns_);
      }
      return *this;
    }

    // Assigns this instance from a thread without a pending update to the
    // thread's start/finish times, asserting that the previous thread, if any,
    // does not have a pending update.
    DequeueResult& operator=(Thread* thread) {
      *this = DequeueResult{thread};
      return *this;
    }

    // Updates the thread's start/finish times while holding the thread's lock.
    // May be called unconditionally and more than once, whether or not there
    // are pending updates.
    void UpdateThreadTimeline() TA_REQ(thread()->get_lock()) {
      if (is_updated_pending()) {
        thread()->scheduler_state().start_time_ = mono_start_time_ns_;
        thread()->scheduler_state().finish_time_ = mono_finish_time_ns_;
        mono_start_time_ns_ = mono_finish_time_ns_ = SchedTime{0};
      }
    }

    // Returns true if thread's timeline needs to be updated by calling
    // UpdateThreadTimeline.
    bool is_updated_pending() const {
      return mono_start_time_ns_ != 0 || mono_finish_time_ns_ != 0;
    }

    // Contextually evaluates to true if the result is not empty.
    explicit operator bool() const { return thread_ != nullptr; }

    Thread* thread() { return thread_; }
    const Thread* thread() const { return thread_; }

   private:
    Thread* thread_{nullptr};
    SchedTime mono_start_time_ns_{0};
    SchedTime mono_finish_time_ns_{0};
  };

  // Evaluates the schedule and returns the thread that should execute.
  DequeueResult EvaluateNextThread(SchedTime now, Thread* current_thread, bool current_is_migrating,
                                   Guard<MonitoredSpinLock, NoIrqSave>& queue_guard)
      TA_REQ(chainlock_transaction_token, queue_lock_, current_thread->get_lock());

  // Updates the thread's start/finish times and capacity if the current
  // activation has expired.
  void UpdateActivation(Thread* thread, SchedTime now) TA_REQ(queue_lock_, thread->get_lock());

  // Adds a thread to the run queue tree. The thread must be active on this
  // CPU.
  // TODO(https://fxbug.dev/428694298): Remove total_runtime_ns parameter.
  void QueueThread(Thread* thread, Placement placement, SchedTime now = SchedTime{0},
                   SchedDuration total_runtime_ns = SchedDuration{0})
      TA_REQ(queue_lock_, thread->get_lock());

  // Removes the thread at the head of the first eligible run queue, or attempt
  // to steal a thread from another scheduler if we have no eligible thread in
  // our current run queues.  If none of these attempts succeed, we will fall
  // back on returning this scheduler's Idle thread.
  //
  // In all cases, the thread being returned will not be locked, and will not be
  // a member of any current run queue.  One of the following *must* happen to
  // the thread after this.
  //
  // 1) The thread is added back to one of these scheduler's run queues (this
  //    is common during a PI interaction involving the thread).
  // 2) The thread is flagged as "reschedule_in_progress" in its scheduler-owned
  //    variables, before becoming the active thread in its scheduler (after
  //    dropping the scheduler lock and re-obtaining the thread's lock
  //    exclusively).  This happens in RescheduleCommon when EvaluateNextThread
  //    chooses a new thread to run.
  // 3) The thread is flagged as "steal_in_progress" after being removed from
  //    another scheduler's run queue, but before being locked exclusively and
  //    re-inserted into its new scheduler's run queue.
  //
  // Note that these rules apply to *all* the versions of Dequeue listed below.
  DequeueResult DequeueThread(SchedTime now, Guard<MonitoredSpinLock, NoIrqSave>& queue_guard)
      TA_REQ(chainlock_transaction_token, queue_lock_);

  // Removes the thread at the head of the fair run queue and returns it.
  DequeueResult DequeueFairThread(SchedTime eligible_time) TA_REQ(queue_lock_);

  // Removes the eligible fair thread with a finish time earlier than the given
  // finish time and returns it or nullptr if one does not exist.
  DequeueResult DequeueEarlierFairThread(SchedTime eligible_time, SchedTime finish_time)
      TA_REQ(queue_lock_);

  // Removes the eligible deadline thread with the earliest finish time in the
  // deadline run queue and returns it.
  DequeueResult DequeueDeadlineThread(SchedTime eligible_time) TA_REQ(queue_lock_);

  // Removes the eligible deadline thread with a finish time earlier than the
  // given reference thread's finish time and criticality and returns it or
  // nullptr if one does not exist.
  DequeueResult DequeueEarlierDeadlineThread(SchedTime eligible_time,
                                             const Thread* reference_thread) TA_REQ(queue_lock_)
      TA_REQ_SHARED(reference_thread->get_lock());

  // Returns the eligible fair thread in the run queue with a finish time
  // earlier than the given finish time, or nullptr if one does not exist.
  //
  // See the note in |FindEarliestEligibleThread| for lifecycle rules for the
  // returned pointer (if any)
  Thread* FindEarlierFairThread(SchedTime eligible_time, SchedTime finish_time) TA_REQ(queue_lock_);

  // Returns the eligible deadline thread in the run queue with a finish time
  // earlier than the given reference thread's finish time and criticality, or
  // nullptr if one does not exist.
  //
  // See the note in |FindEarliestEligibleThread| for lifecycle rules for the
  // returned pointer (if any)
  Thread* FindEarlierDeadlineThread(SchedTime eligible_time, const Thread* reference_thread)
      TA_REQ(queue_lock_) TA_REQ_SHARED(reference_thread->get_lock());

  // Attempts to steal work from other busy CPUs. Returns nullptr if no work was
  // stolen, otherwise returns a pointer to the stolen thread that is partially
  // associated with the local Scheduler instance.
  DequeueResult StealWork(SchedTime now, SchedProcessingRate processing_rate)
      TA_REQ(chainlock_transaction_token) TA_EXCL(queue_lock_);

  // Returns the time that the next deadline task will become eligible or infinite
  // if there are no ready deadline tasks.
  SchedTime GetNextEligibleTime() const TA_REQ(queue_lock_);

  // Calculates the timeslice of the thread based on the current run queue
  // state.
  SchedDuration CalculateTimeslice(const Thread* thread) TA_REQ(queue_lock_)
      TA_REQ_SHARED(thread->get_lock());

  // Returns the completion time clamped to the start of the earliest deadline
  // thread that will become eligible in that time frame.
  SchedTime ClampToDeadline(SchedTime completion_time) TA_REQ(queue_lock_);

  // Returns the completion time clamped to the start of the earliest deadline
  // thread that will become eligible in that time frame and also has an earlier
  // finish time than the given reference thread, accounting for criticality.
  SchedTime ClampToEarlierDeadline(SchedTime completion_time, const Thread* reference_thread)
      TA_REQ(queue_lock_) TA_REQ_SHARED(reference_thread->get_lock());

  // Updates the timeslice of the thread based on the current run queue state.
  // Returns the absolute deadline for the next time slice, which may be earlier
  // than the completion of the time slice if other threads could preempt the
  // given thread before the time slice is exhausted.
  SchedTime NextThreadTimeslice(Thread* thread, SchedTime now)
      TA_REQ(queue_lock_, thread->get_lock());

  // Updates the fair bandwidth scheduling period based on the number of active
  // threads.
  void UpdateFairBandwidthPeriod(SchedTime now) TA_REQ(queue_lock_);

  static void SetThreadMigrateFnInternal(Thread* thread, Thread::MigrateFn migrate_fn)
      TA_REQ(thread->get_lock(), Thread::get_list_lock());

  // Makes a thread active on this CPU's scheduler and inserts it into the
  // run queue tree.
  void Insert(SchedTime now, Thread* thread, Placement placement = Placement::Insertion)
      TA_REQ(queue_lock_, thread->get_lock(), thread->get_scheduler_variable_lock());

  // Removes the thread from this CPU's scheduler to move it to another CPU
  // (i.e. stealing or migration). This method does not require the thread to be
  // locked exclusively, and consequently leaves a stale value for the thread's
  // current CPU id. The stale value is later corrected when the thread is
  // locked exclusively and added to the new target CPU by calling Insert.
  void Remove(SchedTime now, Thread* thread, cpu_num_t stolen_by = INVALID_CPU)
      TA_REQ(queue_lock_, thread->get_scheduler_variable_lock()) TA_REQ_SHARED(thread->get_lock());

  // Removes the thread from this CPU's scheduler and updates the current CPU
  // id. The thread must not be in the run queue.
  void RemoveThreadLocked(SchedTime now, Thread* thread)
      TA_REQ(queue_lock_, thread->get_lock(), thread->get_scheduler_variable_lock());

  // Removes a specific thread from its current RunQueue.  Note that the thread
  // must currently exist in its queue, it is an error otherwise.
  void EraseFromQueue(Thread* thread) TA_REQ(queue_lock_, thread->get_scheduler_variable_lock())
      TA_REQ(thread->get_lock()) {
    DEBUG_ASSERT(thread->scheduler_queue_state().in_queue());
    SchedulerState& state = thread->scheduler_state();

    const bool is_fair = state.discipline() == SchedDiscipline::Fair;
    const bool is_critical = state.effective_profile().is_critical();
    RunQueue& queue = is_fair ? fair_run_queue_
                              : (is_critical ? critical_deadline_run_queue_ : deadline_run_queue_);
    queue.erase(*thread);

    if (is_fair) {
      VariableToMonotonicInPlace(state.start_time_);
      VariableToMonotonicInPlace(state.finish_time_);
    }
  }

  // Adjusts the given fair thread's bandwidth and dynamic parameters for
  // changes in bandwidth demand since it was last scheduled.
  void AdjustFairBandwidth(Thread* thread, SchedTime now) TA_REQ(queue_lock_, thread->get_lock());

  // Returns true if there is at least one eligible deadline thread in the
  // run queue.
  bool IsDeadlineThreadEligible(SchedTime eligible_time) TA_REQ(queue_lock_) {
    for (const RunQueue* queue : {&critical_deadline_run_queue_, &deadline_run_queue_}) {
      if (!queue->is_empty()) {
        const Thread& front = queue->front();
        AssertInRunQueue(front);
        if (front.scheduler_state().start_time() <= eligible_time) {
          return true;
        }
      }
    }
    return false;
  }

  // Returns true if there is at least one eligible fair thread in the run
  // queue.
  bool IsFairThreadEligible(SchedTime eligible_time) TA_REQ(queue_lock_) {
    if (fair_run_queue_.is_empty()) {
      return false;
    }

    const SchedTime variable_eligible_time = MonotonicToVariable(eligible_time);

    const Thread& front = fair_run_queue_.front();
    AssertInRunQueue(front);
    return front.scheduler_state().start_time() <= variable_eligible_time;
  }

  // Updates the total expected runtime estimator and exports the atomic shadow
  // variable for cross-CPU readers.
  inline void UpdateTotalExpectedRuntime(SchedDuration delta_ns) TA_REQ(queue_lock_);

  // Updates to total deadline utilization estimator and exports the atomic
  // shadow variable for cross-CPU readers.
  inline void UpdateTotalDeadlineUtilization(SchedUtilization delta_ns) TA_REQ(queue_lock_);

  // Updates the processing rate and exports the atomic shadow variable for
  // cross-CPU readers.
  inline bool UpdateProcessingRate(zx_instant_boot_ticks_t boot_ticks) TA_REQ(queue_lock_);

  // Computes the estimated energy consumed since the last reschedule and
  // updates the current thread and CPU energy accumulators.
  void UpdateEstimatedEnergyConsumption(Thread* current_thread, SchedMonoTimeAndBootTicks now,
                                        SchedDuration actual_runtime_ns)
      TA_REQ(current_thread->get_lock(), queue_lock_);

  // Returns and resets the accumulated processor idle time.
  SchedDuration TakeProcessorIdleTime() {
    return SchedDuration{processor_idle_time_ns_.exchange(0, ktl::memory_order_relaxed)};
  }

  // Utilities to scale up or down the given value by the performance scale of the CPU.
  template <typename T>
  inline T ScaleUp(T value) const TA_REQ(queue_lock_);
  template <typename T>
  inline T ScaleDown(T value) const TA_REQ(queue_lock_);

  // Update trace counters which track the total number of runnable threads for a CPU
  inline void TraceTotalRunnableThreads() const TA_REQ(queue_lock_);

  // Returns a new flow id when flow tracing is enabled, zero otherwise.
  inline static uint64_t NextFlowId();

  // Traits type to adapt the WAVLTree to Thread with node state in the
  // scheduler_state member.
  struct TaskTraits {
    using KeyType = SchedulerState::KeyType;
    static KeyType GetKey(const Thread& thread) TA_NO_THREAD_SAFETY_ANALYSIS {
      return thread.scheduler_state().key();
    }
    static bool LessThan(KeyType a, KeyType b) { return a < b; }
    static bool EqualTo(KeyType a, KeyType b) { return a == b; }
    static auto& node_state(Thread& thread) TA_NO_THREAD_SAFETY_ANALYSIS {
      return thread.scheduler_queue_state().run_queue_node();
    }
  };

  // Observer that maintains the subtree invariant min_finish_time and
  // min_weight as nodes are added to and removed from the run queue.
  struct SubtreeInvariantTraits {
    using SubtreeInvariants = SchedulerState::SubtreeInvariants;

    static SubtreeInvariants GetNodeValue(const Thread& node) TA_NO_THREAD_SAFETY_ANALYSIS {
      const SchedulerState& scheduler_state = node.scheduler_state();
      const EffectiveProfile& effective_profile = scheduler_state.effective_profile();
      const SchedWeight min_weight = effective_profile.weight_or(SchedWeight::Max());

      return {.min_finish_time = scheduler_state.finish_time_, .min_weight = min_weight};
    }

    static SubtreeInvariants GetSubtreeValue(const Thread& node) TA_NO_THREAD_SAFETY_ANALYSIS {
      return node.scheduler_state().subtree_invariants_;
    }

    static SubtreeInvariants CombineValues(SubtreeInvariants subtree, SubtreeInvariants node) {
      return {.min_finish_time = ktl::min(subtree.min_finish_time, node.min_finish_time),
              .min_weight = ktl::min(subtree.min_weight, node.min_weight)};
    }

    static void SetSubtreeValue(Thread& node,
                                SubtreeInvariants value) TA_NO_THREAD_SAFETY_ANALYSIS {
      node.scheduler_state().subtree_invariants_ = value;
    }

    static void ResetSubtreeValue(Thread& target) {}
  };

  using SubtreeObserver = fbl::WAVLTreeAugmentedInvariantObserver<SubtreeInvariantTraits>;

  // Alias of the WAVLTree type for the run queue.
  using RunQueue = fbl::WAVLTree<TaskTraits::KeyType, Thread*, TaskTraits, fbl::DefaultObjectTag,
                                 fbl::SizeOrder::Constant, TaskTraits, SubtreeObserver>;

  // Finds the next eligible thread in the given run queue.
  //
  // Note: Any Thread* returned by this method may only be used as long as the
  // queue lock is held. Dropping the queue lock allows the thread to be removed
  // from the scheduler, and potentially exit, destroying the Thread and
  // invalidating the thread pointer in the process.
  Thread* FindEarliestEligibleThread(RunQueue* run_queue, SchedTime eligible_time)
      TA_REQ(queue_lock_);

  // Finds the next eligible thread in the given run queue that also passes the
  // given predicate.
  //
  // See the note in |FindEarliestEligibleThread| for lifecycle rules for the
  // returned pointer (if any)
  template <typename Predicate>
  Thread* FindEarliestEligibleThread(RunQueue* run_queue, SchedTime eligible_time,
                                     Predicate&& predicate) TA_REQ(queue_lock_);

  // Emits queue event tracers for trace-based scheduler performance analysis.
  inline void TraceThreadQueueEvent(const fxt::InternedString& name, const Thread* thread) const
      TA_REQ(queue_lock_) TA_REQ_SHARED(thread->get_lock());

  // Returns true if the given thread is fair scheduled.
  static bool IsFairThread(const Thread* thread) TA_REQ_SHARED(thread->get_lock()) {
    return thread->scheduler_state().effective_profile_.IsFair();
  }

  // Returns true if the given thread is deadline scheduled.
  static bool IsDeadlineThread(const Thread* thread) TA_REQ_SHARED(thread->get_lock()) {
    return thread->scheduler_state().effective_profile_.IsDeadline();
  }

  // Returns true if the given thread is critical deadline scheduled.
  static bool IsCriticalDeadlineThread(const Thread* thread) TA_REQ_SHARED(thread->get_lock()) {
    return thread->scheduler_state().effective_profile_.is_critical();
  }

  // Returns true if the given thread's time slice is adjustable under changes to
  // the fair scheduler demand on the CPU.
  static bool IsThreadAdjustable(const Thread* thread) TA_REQ_SHARED(thread->get_lock()) {
    // Checking the thread state avoids unnecessary adjustments on a thread that
    // is no longer competing.
    return !thread->IsIdle() && IsFairThread(thread) && thread->state() == THREAD_READY;
  }

  // Returns the thread with the earliest finish time. Either or both threads
  // may be nullptr. If there is tie between primary and secondary, primary is
  // returned.
  const Thread* EarliestDeadlineThread(const Thread* primary, const Thread* secondary) const
      TA_REQ(queue_lock_) {
    if (primary == nullptr) {
      return secondary;
    }
    if (secondary == nullptr) {
      return primary;
    }

    AssertInScheduler(*primary);
    AssertInScheduler(*secondary);
    if (primary->scheduler_state().finish_time() <= secondary->scheduler_state().finish_time()) {
      return primary;
    }

    return secondary;
  }

  // Called by code paths that encounter a thread in the READY state with a
  // disposition other than Enqueued. Updates a kcounter to track the rate of
  // updates that race with lock juggling.
  static void CountUpdateInTransition();

  // Protects run queues and associated metadata for this Scheduler instance.
  // The queue lock is the bottom most lock in the system for the CPU it is
  // associated with: no other locks may be nested within a queue lock. A queue
  // lock may be acquired across CPUs, however, the no nesting rule still
  // applies.
  //
  // Note: while the queue_lock_ is the bottom-most lock in the system from the
  // scheduler's perspective, it is not (formally) a leaf-lock right now.
  // Currently, there is one place in the system (DumpThreadLocked) where the
  // queue_lock_ needs to be held, while fprintf is called.
  //
  // This happens when the lockup-detector is unhappy, and asking the Scheduler
  // to print information about its run queue state.  Most of the printf targets
  // which could be passed to the scheduler also involve holding locks.  If the
  // queue_lock_ is flagged as a leaf lock, and lockdep is enabled, and the
  // lockup-detector attempt to report a hang, it will cause lock-dep to
  // complain about obtaining a lock while holding a leaf-lock, which will cause
  // more printing, which causes more complaints, etc...
  //
  // See https://fxbug.dev/42070437 for more details.
  mutable DECLARE_SPINLOCK_WITH_TYPE(Scheduler, MonitoredSpinLock) queue_lock_
      TA_ACQ_AFTER(Thread::get_list_lock());

  // Alias of the queue lock wrapper type.
  using QueueLock = decltype(queue_lock_);

  // Simplified static assertion on the wrapped queue lock type to avoid the more verbose template
  // parameters required by lockdep::AssertHeld.
  static void AssertHeld(const QueueLock& lock) TA_ASSERT(lock) TA_ASSERT(lock.lock()) {
    lock.lock().AssertHeld();
  }

  // Asserts that a given thread belongs to |this| Scheduler instance.
  // Requires holding the scheduler's queue_lock_.
  //
  void AssertInScheduler(const Thread& thread) const TA_REQ(queue_lock_)
      TA_ASSERT_SHARED(thread.get_lock())
          TA_ASSERT(thread.get_scheduler_variable_lock()) TA_NO_THREAD_SAFETY_ANALYSIS {
    // TODO(johngro): Come back here and remove the `&t == active_thread_`
    // clause once other system-wide invariants have been cleaned up.  See
    // https://fxbug.dev/340551498 for details.
    DEBUG_ASSERT_MSG(thread.scheduler_state().curr_cpu() == this_cpu() ||
                         thread.scheduler_queue_state().stolen_by() == this_cpu() ||
                         &thread == active_thread_,
                     "Thread %lu expected to be a member of scheduler %u, but curr_cpu says %u",
                     thread.tid(), this_cpu_, thread.scheduler_state().curr_cpu());
  }

  // It would be nice to be able to make a stronger assertion here (that the
  // thread was not just in a run queue, but the specific run queue we expect it
  // to be in), but unfortunately, there is no inexpensive way of doing that.
  //
  // Thankfully, most of the time that we need to invoke an assertion like this,
  // it is pretty obvious from the context that the assertion is true.  It might
  // be reasonably to (someday) consider removing even this weak runtime check,
  // and just leaving this as a static analysis workaround, and nothing more.
  void AssertInRunQueue(const Thread& t) const TA_REQ(queue_lock_) TA_ASSERT_SHARED(t.get_lock())
      TA_ASSERT(t.get_scheduler_variable_lock()) TA_NO_THREAD_SAFETY_ANALYSIS {
    DEBUG_ASSERT((t.scheduler_state().curr_cpu_ == this_cpu_) &&
                 (t.scheduler_queue_state().in_queue()));
  }

  // Add a couple of small no-op helpers which inform the static analyzer of
  // some properties which are very difficult to apply to the lambdas used in
  // the functional programming patterns below.  Neither of these methods
  // actually _check_ anything, so they must be used with *extreme caution*.
  //
  // They should only ever be used when the locking invariants are trivially
  // verifiable from the immediate context.
  //
  // MarkHasSchedulerAccess tells the static analyzer that we are exclusively
  // holding a scheduler's queue lock.  It's primary use case is in lambda's
  // which are declared inside of the scope of a queue_lock_ guard, and are used
  // for some sort of functional programming pattern.
  //
  // MarkHasOwnedThreadAccess tells the static analyzer that we have exclusive
  // access to a thread's scheduler-variables, and shared (read-only) access to
  // variables protected by a thread's lock (eg; the scheduler_state). The
  // conditions for this operation are that we are holding the queue_lock_ for
  // the scheduler that the thread is currently a member of (see
  // AssertInScheduler, above).
  //
  // MarkInFindActiveSchedulerForThreadCbk tells the static analyzer that we are
  // in the callback for an FindActiveSchedulerForThread operation, meaning that
  // we must hold both the thread's lock and the queue_lock for the thread and
  // scheduler involved in the operation.
  //
  // See StealWork for a practical example of where these methods are useful.
  //
  static void MarkHasSchedulerAccess(const Scheduler& s) TA_ASSERT(s.queue_lock_) {}
  static void MarkHasOwnedThreadAccess(const Thread& t) TA_ASSERT(t.get_scheduler_variable_lock())
      TA_ASSERT_SHARED(t.get_lock()) {}
  static void MarkInFindActiveSchedulerForThreadCbk(const Thread& t, const Scheduler& s)
      TA_ASSERT(t.get_scheduler_variable_lock()) TA_ASSERT(t.get_lock()) TA_ASSERT(s.queue_lock_) {}

  // A mask of all of the currently active scheduler's in the system.  An
  // "active" scheduler is one who will accept threads to be executed.  An
  // "in-active" scheduler will not, and is typically the scheduler for a CPU
  // which is in the process of being taken offline.
  //
  // Note that to change the active state of a scheduler for CPU X (eg; to
  // toggle the state of bit X in the active_schedulers_ mask) that scheduler's
  // queue_lock_ must be held.  Therefore, observations of the schedulers'
  // active mask must typically be assumed to be volatile.  As soon as the mask
  // is observed, it can immediately change.
  static inline ktl::atomic<cpu_mask_t> active_schedulers_{0};

  // A mask of all of the currently idle scheduler's in the system.  An
  // "idle" scheduler is one which last selected the idle/power thread to run,
  // while a "busy" scheduler is one which is running a non-idle thread.
  //
  // Like the active mask, the idle mask is volatile and subject to change at
  // any time barring special circumstances.
  static inline ktl::atomic<cpu_mask_t> idle_schedulers_{0};

  // Flow id counter for sched_latency flow events.
  inline static RelaxedAtomic<uint64_t> next_flow_id_{1};

  // The run queue of fair scheduled threads ready to run, but not currently running.
  TA_GUARDED(queue_lock_)
  RunQueue fair_run_queue_;

  // The run queue of deadline scheduled threads ready to run, but not currently running.
  TA_GUARDED(queue_lock_)
  RunQueue deadline_run_queue_;

  // The run queue of critical deadline scheduled threads ready to run, but not currently running.
  TA_GUARDED(queue_lock_)
  RunQueue critical_deadline_run_queue_;

  // The total critical deadline utilization of threads associated with this CPU.
  SchedUtilization critical_deadline_utilization_{0};

  // The list of threads that need to run the Save stage of their migrate functions.
  //
  // There are several invariants that must be maintained when interacting with this
  // list:
  // 1. Other CPUs can put threads into this save_state_list_, but only this Scheduler (and thus
  //    this CPU) can remove threads from it. Removal is currently handled in ProcessSaveStateList.
  // 2. Interrupts must be disabled while removing threads from this list, effectively ensuring
  //    that only one call to ProcessSaveStateList can be made at a time on a given CPU.
  // 3. Threads cannot be destroyed until they are removed from this list. This is guaranteed
  //    today by the fact that dead threads clean themselves up when they are scheduled, which is
  //    not possible if the thread is in this list.
  TA_GUARDED(queue_lock_)
  Thread::SaveStateList save_state_list_;

  // Pointer to the thread actively running on this CPU.
  TA_GUARDED(queue_lock_)
  Thread* active_thread_{nullptr};

  TA_GUARDED(queue_lock_)
  SchedWeight active_thread_weight_{SchedWeight::Max()};

  // Count of the threads running on this CPU, including threads in the run
  // queue and the currently running thread. Does not include the idle thread.
  TA_GUARDED(queue_lock_)
  int32_t runnable_task_count_{0};

  // Total weights of threads running on this CPU, including threads in the
  // run queue and the currently running thread. Does not include the idle
  // thread.
  TA_GUARDED(queue_lock_)
  SchedWeight weight_total_{0};

  // The value of |weight_total_| when the current thread was scheduled.
  // Provides a reference for determining whether the total weights changed
  // since the last reschedule.
  SchedWeight scheduled_weight_total_{0};

  // Alias of the affine transform type used for variable <-> monotonic timeline
  // conversions.
  using Affine = sched::Affine<10>;

  // The affine transform for converting between monotonic and variable
  // timelines for the fair run queue.
  TA_GUARDED(queue_lock_)
  Affine fair_affine_transform_;

  TA_GUARDED(queue_lock_)
  SchedDuration fair_period_{kDefaultFairPeriod};

  TA_GUARDED(queue_lock_)
  SchedUtilization reciprocal_fair_period_{kReciprocalDefaultFairPeriod};

  // Utilities to streamline affine transformations.
  constexpr SchedTime MonotonicToVariable(SchedTime t) const TA_REQ(queue_lock_) {
    return fair_affine_transform_.MonotonicToVariable(t);
  }
  constexpr SchedTime VariableToMonotonic(SchedTime v) const TA_REQ(queue_lock_) {
    return fair_affine_transform_.VariableToMonotonic(v);
  }
  constexpr void MonotonicToVariableInPlace(SchedTime& ref) const TA_REQ(queue_lock_) {
    fair_affine_transform_.MonotonicToVariableInPlace(ref);
  }
  constexpr void VariableToMonotonicInPlace(SchedTime& ref) const TA_REQ(queue_lock_) {
    fair_affine_transform_.VariableToMonotonicInPlace(ref);
  }

  // The system time since the last update to the global virtual time.
  TA_GUARDED(queue_lock_)
  SchedTime last_update_time_ns_{0};

  // Accumulates the time spent in an idle state since the last reschedule.
  ktl::atomic<zx_duration_mono_t> processor_idle_time_ns_{0};

  // The system time that the current time slice started.
  SchedTime start_of_current_time_slice_ns_{0};

  // The system time that the current thread should be preempted. Initialized to
  // ZX_TIME_INFINITE to pass the assertion now < target_preemption_time_ns_ (or
  // else the current time slice is expired) on the first entry into the
  // scheduler.
  SchedTime target_preemption_time_ns_{ZX_TIME_INFINITE};

  // The sum of the expected runtimes of all active threads on this CPU. This
  // value is an estimate of the average queuing time for this CPU, given the
  // current set of active threads.
  TA_GUARDED(queue_lock_)
  SchedDuration total_expected_runtime_ns_{0};

  // The CPU this scheduler instance is associated with.
  // NOTE: This member is not initialized to prevent clobbering the value set
  // by sched_early_init(), which is called before the global ctors that
  // initialize the rest of the members of this class.
  // TODO(eieio): Figure out a better long-term solution to determine which
  // CPU is associated with each instance of this class. This is needed by
  // non-static methods that are called from arbitrary CPUs, namely Insert().
  cpu_num_t this_cpu_;

  // The index of the logical cluster this CPU belongs to. CPUs with the same
  // logical cluster index have the best chance of good cache affinity with
  // respect to load distribution decisions.
  size_t cluster_{0};

  // Flag indicating that this Scheduler's CPU is in the process of becoming
  // inactive.  When true, threads who have a migration function which must be
  // run on this CPU are permitted to join this CPU's ready queue, even if the
  // CPU has already been flagged as inactive.  The thread who is in charge of
  // deactivating the CPU will make sure to run the migration function of the
  // thread on this CPU before finishing the deactivation process.
  ktl::atomic<bool> cpu_deactivating_{false};

  // Values exported for lock-free access across CPUs. These are mirrors of the
  // members of the same name without the exported_ prefix. This avoids
  // unnecessary atomic loads when updating the values using arithmetic
  // operations on the local CPU. These values are atomically readonly to other
  // CPUs.
  // TODO(eieio): Look at cache line alignment for these members to optimize
  // cache performance.
  RelaxedAtomic<SchedDuration> exported_queue_time_ns_{SchedNs(0)};
  RelaxedAtomic<SchedUtilization> exported_deadline_utilization_{SchedUtilization{0}};
  RelaxedAtomic<SchedUtilization> exported_clamped_deadline_utilization_{SchedUtilization{0}};
  RelaxedAtomic<SchedUtilization> exported_fair_utilization_{SchedUtilization{0}};
  RelaxedAtomic<SchedUtilization> exported_clamped_fair_utilization_{SchedUtilization{0}};
  RelaxedAtomic<SchedUtilization> exported_clamped_total_utilization_{SchedUtilization{0}};
  RelaxedAtomic<SchedProcessingRate> exported_processing_rate_{SchedProcessingRate{1}};
  RelaxedAtomic<SchedProcessingRate> exported_max_processing_rate_{SchedProcessingRate{1}};

  // The thread which ran just before this thread was scheduled.  Used by
  // Scheduler::LockHandoff to release the previous thread's lock after a
  // context switch operation has fully completed.
  Thread* previous_thread_{nullptr};

  // Utility type that tracks values related to the thread being placed when
  // finding a target CPU for a waking or migrating thread.
  class ThreadDemand;

  // Utility type that tracks values used for comparing candidate thread
  // placements when finding a target CPU for a waking or migrating thread.
  class CandidatePlacement;

  // Track bandwidth reservations for up to 8 recently blocked threads.
  static constexpr size_t kMaxBandwidthReservations = 8;
  using BandwidthReservationCache =
      power_management::BandwidthReservationCache<kMaxBandwidthReservations>;

  TA_GUARDED(queue_lock_)
  BandwidthReservationCache bandwidth_reservation_cache_;

  // PowerLevelControl encapsulates the CPU energy model and power level control interface for
  // energy aware scheduling.
  //
  // Instances of PowerLevelControl are protected by Scheduler::queue_lock_.
  class PowerLevelControl {
   public:
    explicit PowerLevelControl(Scheduler* scheduler) : scheduler_{scheduler} {}

    // Sets/resets the power domain set associated with this scheduler, returning the previous set.
    PowerDomainSet ExchangePowerDomainSet(PowerDomainSet new_domain_set) {
      PowerDomainSet previous_domain_set =
          power_state_.UpdatePowerDomainSet(ktl::move(new_domain_set), cpu());

      // Clear the request to ensure that a DPC racing with a domain change cannot latch a pending
      // request intended for the previous domain and the new domain ref pointer together.
      pending_update_request_.reset();

      // Reset the processing rate using either the current rate for the domain or the default.
      if (const SchedProcessingRate rate = power_state_.active_processing_rate(); rate > 0) {
        updated_processing_rate_ = rate;
      } else {
        updated_processing_rate_ = default_processing_rate_;
      }

      // Set the active power coefficient from the power state config.
      active_power_coefficient_nw_ = power_state_.active_power_coefficient_nw();

      // Set the max processing rate for admission control comparisons.
      max_processing_rate_ =
          domain() ? domain()->model().max_processing_rate() : default_processing_rate_;

      return previous_domain_set;
    }

    // Called by the power level controller server (e.g. the userspace component servicing the
    // kernel power level control interface) for the domain associated with this scheduler to
    // acknowledge that a power level request has been completed.
    zx::result<> UpdateActivePowerLevel(uint8_t power_level) {
      ktrace::Scope trace = LOCAL_KTRACE_BEGIN_SCOPE(COMMON, "UpdateActivePowerLevel",
                                                     ("cpu", cpu()), ("power_level", power_level));

      const zx::result result = power_state_.UpdateActivePowerLevel(power_level);
      if (result.is_ok()) {
        updated_processing_rate_ = power_state_.active_processing_rate();
      }

      trace = KTRACE_END_SCOPE(
          ("processing_rate", Round<uint32_t>(1000 * processing_rate_)),
          ("updated_processing_rate", Round<uint32_t>(1000 * updated_processing_rate_)));

      return result;
    }

    // Sets the processing rate on systems that don't provide an energy model and manage OPPs
    // exclusively from userspace. User requests are ignored if an energy model is set.
    void UserSetProcessingRate(SchedProcessingRate processing_rate) {
      DEBUG_ASSERT(processing_rate > 0);
      if (!domain()) {
        updated_processing_rate_ = processing_rate;
      }
    }

    // Sets the processing rate limits for the processor. Returns true if the
    // current active power level needs to be updated in response to the new
    // limits.
    //
    // The specified min is allowed to be greater than the max for future
    // latency vs. power tradeoff management, which will be documented when
    // implemented. Until that time, the max takes precedence in the case where
    // min > max.
    bool UserSetProcessingRateLimits(SchedProcessingRate min, SchedProcessingRate max);

    // Returns the given demand clamped to the current rate limits.
    //
    // Note that the minimum limit can be greater than the maximum limit, in
    // which case the maximum limit takes precedence to correctly support using
    // the maximum for thermal mitigation.
    SchedUtilization ClampDemand(SchedUtilization demand) const {
      return ktl::clamp(demand, ktl::min(processing_rate_limit_min_, processing_rate_limit_max_),
                        processing_rate_limit_max_);
    }

    // Sets the initial processing rate from topology data during early boot.
    void TopologySetDefaultProcessingRate(SchedProcessingRate processing_rate) {
      DEBUG_ASSERT(processing_rate > 0);
      DEBUG_ASSERT(!domain());
      processing_rate_ = processing_rate;
      max_processing_rate_ = processing_rate;
      default_processing_rate_ = processing_rate;
      updated_processing_rate_ = processing_rate;
      processing_rate_reciprocal_ = 1 / processing_rate;
    }

    // Returns true if the given power level is a valid active power level.
    bool IsValidActivePowerLevel(uint8_t power_level) const {
      return power_state_.IsValidActivePowerLevel(power_level);
    }

    // Updates the current processing rate, reciprocal, and active power coefficient to the updated
    // values and returns the new processing rate. Called by RescheduleCommon after accounting is
    // performed for the time spent at the previous rate.
    SchedProcessingRate UpdateProcessingRate() {
      if (is_processing_rate_update_pending()) {
        processing_rate_ = updated_processing_rate_;
        processing_rate_reciprocal_ = 1 / processing_rate_;
        active_power_coefficient_nw_ = power_state_.active_power_coefficient_nw();
        if (!domain()) {
          max_processing_rate_ = processing_rate_;
        }
      }
      return processing_rate_;
    }

    // Updates the normalized utilization for this processor and the total
    // normalized utilization for the domain it belongs to with the given delta.
    // Returns the updated utilization for this processor.
    SchedUtilization UpdateNormalizedUtilization(SchedUtilization delta) {
      return power_state_.UpdateUtilization(delta);
    }

    // Pends a power level request that will later be flushed by
    // SendPendingPowerLevelRequest*.
    bool PendPowerLevelRequest(uint8_t power_level);

    // Pends a power level request compatible with the given rate that will
    // later be flushed by SendPendingPowerLevelRequest*.
    bool PendPowerLevelRequestForRate(SchedProcessingRate processing_rate);

    // Re-evaluates the power level requirements for processors in the same
    // domain and pends a power level request if the processing rate should be
    // adjusted.
    void ReevaluateCurrentPowerLevel();

    // Sends a pending power level request immediately using the registered
    // power level controller.
    //
    // Because this operation needs to update bookkeeping on other CPUs that are
    // currently protected by their respective queue locks, the local CPU's
    // queue lock must be temporarily released, which could result in an
    // incomplete critical section if local invariants are changed by a remote
    // processor. Care must be taken to ensure the caller's invariants are
    // either maintained or re-validated across this operation.
    //
    // Returns the mask of CPUs that should be rescheduled immediately.
    cpu_mask_t SendPendingPowerLevelRequest(Guard<MonitoredSpinLock, NoIrqSave>& queue_guard);

    // Returns true if power control is enabled.
    bool is_enabled() const { return power_state_.is_enabled(); }

    // Returns the power domain set.
    const PowerDomainSet& power_domain_set() const { return power_state_.power_domain_set(); }

    // Returns the power domain associated with this scheduler.
    const fbl::RefPtr<PowerDomain>& domain() const { return power_state_.domain(); }

    // Returns the id of the domain associated with this scheduler.
    ktl::optional<uint32_t> domain_id() const { return power_state_.domain_id(); }

    // Returns the current active power level.
    ktl::optional<uint8_t> active_power_level() const { return power_state_.active_power_level(); }

    // Returns the power coefficient for the current active power level.
    uint64_t active_power_coefficient_nw() const { return active_power_coefficient_nw_; }

    // Returns the power coefficient of the maximum idle power level (e.g. clock gating).
    uint64_t max_idle_power_coefficient_nw() const {
      return power_state_.max_idle_power_coefficient_nw();
    }

    // Returns the normalized deadline utilization of this processor.
    SchedUtilization normalized_deadline_utilization() const {
      return power_state_.normalized_utilization();
    }

    // Returns the normalized deadline utilization of this processor,
    // clamped to the current performance limits.
    SchedUtilization clamped_deadline_utilization() const {
      return ClampDemand(normalized_deadline_utilization());
    }

    // Returns the fair demand of this processor based on total expected runtime
    // of active fair threads.
    SchedUtilization fair_demand() const TA_NO_THREAD_SAFETY_ANALYSIS {
      return scheduler().total_expected_runtime_ns_ * kReciprocalDefaultFairPeriod;
    }

    // Returns the fair demand of this processor, clamped to the current
    // performance limits.
    SchedUtilization clamped_fair_demand() const TA_NO_THREAD_SAFETY_ANALYSIS {
      return ClampDemand(fair_demand());
    }

    // Returns the total demand (deadline + fair) of this processor.
    SchedUtilization total_demand() const {
      return normalized_deadline_utilization() + fair_demand();
    }

    // Returns the total normalized utilization demand of this processor,
    // clamped to the current performance limits.
    SchedUtilization clamped_total_demand() const { return ClampDemand(total_demand()); }

    // Returns the normalized deadline utilization of the domain this processor belongs to.
    SchedUtilization domain_deadline_utilization() const {
      return power_state_.total_normalized_utilization();
    }

    // Returns the normalized processing rate of this processor at its current active power level.
    SchedProcessingRate processing_rate() const { return processing_rate_; }

    // Returns the reciprocal of the normalized processing rate.
    SchedProcessingRate processing_rate_reciprocal() const { return processing_rate_reciprocal_; }

    // Returns the potentially pending update to the normalized processing rate. This value is set
    // whenever the power level controller acknowledges completion of a power level change request.
    SchedProcessingRate updated_processing_rate() const { return updated_processing_rate_; }

    // Returns the maximum processing rate of this processor. Initially set from the CPU topology
    // data and updated whenever the energy model is set/updated by userspace.
    SchedProcessingRate max_processing_rate() const { return max_processing_rate_; }

    // Returns the maximum processing rate of this processor, clamped to the
    // current performance limits.
    SchedProcessingRate clamped_max_processing_rate() const {
      return ktl::min(max_processing_rate_, processing_rate_limit_max_);
    }

    // Returns the minimum processing rate limit of this processor. Initially
    // set to 0.0 and updated by userspace to effect performance limits policy.
    SchedProcessingRate processing_rate_limit_min() const { return processing_rate_limit_min_; }

    // Returns the maximum processing rate limit of this processor. Initially
    // set to 1.0 and updated by userspace to effect performance limits policy.
    SchedProcessingRate processing_rate_limit_max() const { return processing_rate_limit_max_; }

    // Returns the processing rate of the active power level immediately before
    // the current power level. This rate is the lower bound of the performance
    // capacity of the current active power level. Returns a rate of 0.0 when
    // the current power level is the lowest possible active power level.
    SchedProcessingRate preceding_processing_rate() const {
      return power_state_.preceding_active_processing_rate();
    }

    // Returns the target processing rate of this processor (the in-flight
    // desired rate if a transition is pending, or the current processing rate).
    SchedProcessingRate target_processing_rate() const {
      return domain() ? power_state_.target_processing_rate() : processing_rate_;
    }

    // Returns true if there is a pending request to change the active power level.
    bool has_pending_request() const { return pending_update_request_.has_value(); }

    // Returns whether the kernel scheduler should send power level update requests through the
    // control interface to handle utilization changes.
    bool scheduler_control_enabled() const { return power_state_.scheduler_control_enabled(); }

    // Returns the processing rate of the active power level immediately
    // preceding the target power level (the lower bound of the target power level).
    SchedProcessingRate preceding_target_processing_rate() const {
      return domain() ? power_state_.preceding_target_processing_rate()
                      : preceding_processing_rate();
    }

    // Returns true if the targeted active power level has an active power level
    // preceding it (i.e. is not the lowest active power level).
    bool has_preceding_target_power_level() const {
      return domain() ? power_state_.has_preceding_target_power_level()
                      : preceding_processing_rate() > 0;
    }

    // Returns true if the clamped demand is above the processing rate of the
    // target active power level.
    bool processing_rate_should_increase() const {
      const SchedUtilization clamped_demand = clamped_total_demand();
      return clamped_demand > target_processing_rate();
    }

    // Returns true if the clamped demand is at or below the processing rate of
    // the preceding target active power level.
    bool processing_rate_should_decrease() const {
      const SchedUtilization clamped_demand = clamped_total_demand();
      return has_preceding_target_power_level() &&
             clamped_demand <= preceding_target_processing_rate();
    }

    // Returns true if the clamped demand is outside the processing rate range
    // of the target active power level.
    bool processing_rate_should_change() const {
      return processing_rate_should_decrease() || processing_rate_should_increase();
    }

    // Returns true if there is a pending update to the processing rate that has not yet been
    // processed by the scheduler. Checked by RescheduleCommon after accounting has been updated for
    // the runtime segment covering the previous processing rate.
    bool is_processing_rate_update_pending() const {
      return processing_rate_ != updated_processing_rate_;
    }

   private:
    Scheduler& scheduler() { return *scheduler_; }
    const Scheduler& scheduler() const { return *scheduler_; }
    cpu_num_t cpu() const { return scheduler().this_cpu(); }

    SchedProcessingRate processing_rate_{1};
    SchedProcessingRate processing_rate_reciprocal_{1};
    SchedProcessingRate updated_processing_rate_{1};
    SchedProcessingRate default_processing_rate_{1};
    SchedProcessingRate max_processing_rate_{1};

    // The min and max effective processing rate for this processor. These
    // values influence both the actual processing rate of the power domain the
    // processor belongs to and the effective processing rate of this specific
    // processor.
    SchedProcessingRate processing_rate_limit_min_{0};
    SchedProcessingRate processing_rate_limit_max_{1};

    uint64_t active_power_coefficient_nw_{0};

    power_management::PowerState power_state_;
    ktl::optional<power_management::PowerLevelUpdateRequest> pending_update_request_;
    Scheduler* const scheduler_;
  };

  TA_GUARDED(queue_lock_)
  PowerLevelControl power_level_control_{this};
};

extern "C" {
cpu_mask_t cpp_scheduler_peek_active_mask();
}  // extern "C"

#endif  // ZIRCON_KERNEL_INCLUDE_KERNEL_SCHEDULER_H_
