// Copyright 2016 The Fuchsia Authors
// Copyright (c) 2008-2014 Travis Geiselbrecht
// Copyright (c) 2012-2012 Shantanu Gupta
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

/**
 * @file
 * @brief  Mutex functions
 *
 * @defgroup mutex Mutex
 * @{
 */

#include "kernel/mutex.h"

#include <assert.h>
#include <debug.h>
#include <inttypes.h>
#include <lib/affine/ratio.h>
#include <lib/affine/utils.h>
#include <lib/arch/intrin.h>
#include <lib/counters.h>
#include <lib/fxt/interned_string.h>
#include <lib/fxt/string_ref.h>
#include <lib/kconcurrent/chainlock_transaction.h>
#include <lib/ktrace.h>
#include <lib/zircon-internal/ktrace.h>
#include <lib/zircon-internal/macros.h>
#include <platform.h>
#include <trace.h>
#include <zircon/errors.h>
#include <zircon/time.h>
#include <zircon/types.h>

#include <kernel/auto_preempt_disabler.h>
#include <kernel/lock_trace.h>
#include <kernel/owned_wait_queue_pool.h>
#include <kernel/scheduler.h>
#include <kernel/spin_tracing.h>
#include <kernel/task_runtime_timers.h>
#include <kernel/thread.h>
#include <ktl/type_traits.h>

#include <ktl/enforce.h>

#define LOCAL_TRACE 0

namespace {

enum class KernelMutexTracingLevel {
  None,       // No tracing is ever done.  All code drops out at compile time.
  Contested,  // Trace events are only generated when mutexes are contested.
  All         // Trace events are generated for all mutex interactions.
};

// By default, kernel mutex tracing is disabled.
template <KernelMutexTracingLevel = KernelMutexTracingLevel::None, typename = void>
class KTracer;

template <>
class KTracer<KernelMutexTracingLevel::None> {
 public:
  KTracer() = default;
  void KernelMutexUncontestedAcquire(const void* mutex_id) {}
  void KernelMutexUncontestedRelease(const void* mutex_id) {}
  void KernelMutexBlock(const void* mutex_id, Thread* blocker, uint32_t waiter_count) {}
  void KernelMutexWake(const void* mutex_id, Thread* new_owner, uint32_t waiter_count) {}
};

template <KernelMutexTracingLevel Level>
class KTracer<Level, ktl::enable_if_t<(Level == KernelMutexTracingLevel::Contested) ||
                                      (Level == KernelMutexTracingLevel::All)>> {
 public:
  KTracer() : ts_(KTrace::Timestamp()) {}

  void KernelMutexUncontestedAcquire(const void* mutex_id) {
    if constexpr (Level == KernelMutexTracingLevel::All) {
      KernelMutexTrace("mutex_acquire"_intern, mutex_id, nullptr, 0);
    }
  }

  void KernelMutexUncontestedRelease(const void* mutex_id) {
    if constexpr (Level == KernelMutexTracingLevel::All) {
      KernelMutexTrace("mutex_release"_intern, mutex_id, nullptr, 0);
    }
  }

  void KernelMutexBlock(const void* mutex_id, const Thread* blocker, uint32_t waiter_count) {
    KernelMutexTrace("mutex_block"_intern, mutex_id, blocker, waiter_count);
  }

  void KernelMutexWake(const void* mutex_id, const Thread* new_owner, uint32_t waiter_count) {
    KernelMutexTrace("mutex_release"_intern, mutex_id, new_owner, waiter_count);
  }

 private:
  void KernelMutexTrace(const fxt::InternedString& event_name, const void* mutex_id,
                        const Thread* t, uint32_t waiter_count) {
    if (KTrace::CategoryEnabled("kernel:sched"_category)) {
      auto tid_type = fxt::StringRef{(t == nullptr                  ? "none"_intern
                                      : t->user_thread() == nullptr ? "kernel_mode"_intern
                                                                    : "user_mode"_intern)};

      fxt::Argument mutex_id_arg{"mutex_id"_intern,
                                 fxt::Pointer(reinterpret_cast<uintptr_t>(mutex_id))};
      fxt::Argument tid_name_arg{"tid"_intern,
                                 fxt::Koid(t == nullptr ? ZX_KOID_INVALID : t->tid())};
      fxt::Argument tid_type_arg{"tid_type"_intern, tid_type};
      fxt::Argument wait_count_arg{"waiter_count"_intern, waiter_count};

      KTrace::EmitComplete(
          "kernel:sched"_category, fxt::StringRef{event_name}, ts_, ts_ + 50,
          KTrace::Context::Thread,
          ktl::make_tuple(mutex_id_arg, tid_name_arg, tid_type_arg, wait_count_arg));
    }
  }

  const zx_instant_boot_ticks_t ts_;
};
}  // namespace

Mutex::~Mutex() {
  magic_.Assert();
  DEBUG_ASSERT(!arch_blocking_disallowed());

  // Wait until we are absolutely certain (with acquire semantics) that the
  // inner OWQ lock has been fully released before proceeding with destruction.
  // See the comment at the end of ReleaseContendedMutex for more details as to
  // why this is important.
  OwnedWaitQueue* queue;
  while ((queue = wait_.Peek()) != nullptr && !queue->get_lock().is_unlocked()) {
    arch::Yield();
  }

  if (LK_DEBUGLEVEL > 0) {
    if (val() != STATE_FREE) {
      Thread* h = holder();
      panic("~Mutex(): thread %p (%s) tried to destroy locked mutex %p, locked by %p (%s)\n",
            Thread::Current::Get(), Thread::Current::Get()->name(), this, h, h->name());
    }
  }

  val_.store(STATE_FREE, ktl::memory_order_relaxed);
}

// By parametrizing on whether we're going to set a timeslice extension or not
// we can shave a few cycles.
template <bool TimesliceExtensionEnabled>
bool Mutex::AcquireCommon(zx_duration_t spin_max_duration,
                          TimesliceExtension<TimesliceExtensionEnabled> timeslice_extension) {
  magic_.Assert();
  DEBUG_ASSERT(!arch_blocking_disallowed());
  DEBUG_ASSERT(arch_num_spinlocks_held() == 0);

  Thread* const current_thread = Thread::Current::Get();
  const uintptr_t new_mutex_state = reinterpret_cast<uintptr_t>(current_thread);

  {
    // Record whether we set a timeslice extension for later return/rollback. Is marked unused for
    // when there is no timeslice extension.
    bool set_extension = false;
    if constexpr (TimesliceExtensionEnabled) {
      // To ensure there is no gap between acquiring the mutex, and setting the timeslice extension,
      // we optimistically set the timeslice extension first. Should acquiring the mutex fail we
      // will roll it back.
      set_extension =
          current_thread->preemption_state().SetTimesliceExtension(timeslice_extension.value);
    }

    // Fast path: The mutex is unlocked and uncontested. Try to acquire it immediately.
    //
    // We use the weak form of compare exchange here, which is faster on some
    // architectures (e.g. aarch64). In the rare case it spuriously fails, the slow
    // path will handle it.
    uintptr_t old_mutex_state = STATE_FREE;
    if (likely(val_.compare_exchange_weak(old_mutex_state, new_mutex_state,
                                          ktl::memory_order_acquire, ktl::memory_order_relaxed))) {
      RecordInitialAssignedCpu();

      // TODO(maniscalco): Is this the right place to put the KTracer?  Seems like
      // it should be the very last thing we do.
      //
      // Don't bother to update the ownership of the wait queue. If another thread
      // attempts to acquire the mutex and discovers it to be already locked, it
      // will take care of updating the wait queue ownership.
      KTracer{}.KernelMutexUncontestedAcquire(this);

      return set_extension;
    }
    if constexpr (TimesliceExtensionEnabled) {
      if (set_extension) {
        current_thread->preemption_state().ClearTimesliceExtension();
      }
    }
  }

  return AcquireContendedMutex(spin_max_duration, current_thread, timeslice_extension);
}

template <bool TimesliceExtensionEnabled>
__NO_INLINE bool Mutex::AcquireContendedMutex(
    zx_duration_t spin_max_duration, Thread* current_thread,
    TimesliceExtension<TimesliceExtensionEnabled> timeslice_extension) {
  LOCK_TRACE_DURATION("Mutex::AcquireContended", ("name", class_name_ref()));

  // It looks like the mutex is most likely contested (at least, it was when we
  // just checked). Enter the adaptive mutex spin phase, where we spin on the
  // mutex hoping that the thread which owns the mutex is running on a different
  // CPU, and will release the mutex shortly.
  //
  // If we manage to acquire the mutex during the spin phase, we can simply
  // exit, having achieved our goal.  Otherwise, there are 3 reasons we may end
  // up terminating the spin phase and dropping into a block operation.
  //
  // 1) We exceed the system's configured |spin_max_duration|.
  // 2) The mutex is marked as CONTESTED, meaning that at least one other thread
  //    has dropped out of its spin phase and blocked on the mutex.
  // 3) We think that there is a reasonable chance that the owner of this mutex
  //    was assigned to the same core that we are running on.
  //
  // Notes about #3:
  //
  // In order to implement this behavior, the Mutex class maintains a variable
  // called |maybe_acquired_on_cpu_|.  This is the system's best guess as to
  // which CPU the owner of the mutex may currently be assigned to. The value of
  // the variable is set when a thread successfully acquires the mutex, and
  // cleared when the thread releases the mutex later on.
  //
  // This behavior is best effort; the guess is just a guess and could be wrong
  // for several legitimate reasons.  The owner of the mutex will assign the
  // variable to the value of the CPU is it running on immediately after it
  // successfully mutates the mutex state to indicate that it owns the mutex.
  //
  // A spinning thread my observe:
  // 1) A value of INVALID_CPU, either because of weak memory ordering, or
  //    because the thread was preempted after updating the mutex state, but
  //    before recording the assigned CPU guess.
  // 2) An incorrect value of the assigned CPU, again either because of weak
  //    memory ordering, or because the thread either moved to a different CPU
  //    or blocked after the guess was recorded.
  //
  // So, it is possible to keep spinning when we probably shouldn't, and also
  // possible to drop out of a spin when we might want to stay in it.
  //
  // TODO(https://fxbug.dev/42109976): Optimize cache pressure of spinners and default spin max.

  const uintptr_t new_mutex_state = reinterpret_cast<uintptr_t>(current_thread);

  // Make sure that we don't leave this scope with preemption disabled.  If
  // we've got a timeslice extension, we're going to disable preemption while
  // spinning to ensure that we can't get "preempted early" if we end up
  // acquiring the mutex in the spin phase.  However, if a preemption becomes
  // pending while spinning, we'll briefly enable then disable preemption to
  // allow a reschedule.
  AutoPreemptDisabler preempt_disabler(AutoPreemptDisabler::Defer);
  if constexpr (TimesliceExtensionEnabled) {
    preempt_disabler.Disable();
  }

  // Remember the last call to current_ticks.
  zx_instant_mono_ticks_t now_ticks = current_mono_ticks();
  spin_tracing::Tracer<kSchedulerLockSpinTracingEnabled> spin_tracer{now_ticks};

  const affine::Ratio time_to_ticks = timer_get_ticks_to_time_ratio().Inverse();
  const zx_instant_mono_ticks_t spin_until_ticks =
      affine::utils::ClampAdd(now_ticks, time_to_ticks.Scale(spin_max_duration));
  do {
    uintptr_t old_mutex_state = STATE_FREE;
    // Attempt to acquire the mutex by swapping out "STATE_FREE" for our current thread.
    //
    // We use the strong form of compare exchange here because strong is easier
    // to reason about and we're already in the slow path so any performance
    // advantage of weak is negligible.
    if (likely(val_.compare_exchange_strong(old_mutex_state, new_mutex_state,
                                            ktl::memory_order_acquire,
                                            ktl::memory_order_relaxed))) {
      spin_tracer.Finish(spin_tracing::FinishType::kLockAcquired, this->encoded_lock_id());
      RecordInitialAssignedCpu();

      // Same as above in the fastest path: leave accounting to later contending
      // threads.
      KTracer{}.KernelMutexUncontestedAcquire(this);

      if constexpr (TimesliceExtensionEnabled) {
        return Thread::Current::preemption_state().SetTimesliceExtension(timeslice_extension.value);
      }
      return false;
    }

    // Stop spinning if the mutex is or becomes contested. All spinners convert
    // to blocking when the first one reaches the max spin duration.
    if (old_mutex_state & STATE_FLAG_CONTESTED) {
      break;
    }

    {
      // Stop spinning if it looks like we might be running on the same CPU which
      // was assigned to the owner of the mutex.
      //
      // Note: The accuracy of |curr_cpu_num| depends on whether preemption is
      // currently enabled or not and whether we re-enable it below.
      const cpu_num_t curr_cpu_num = arch_curr_cpu_num();
      if (curr_cpu_num == maybe_acquired_on_cpu_.load(ktl::memory_order_relaxed)) {
        break;
      }

      if constexpr (TimesliceExtensionEnabled) {
        // If this CPU has a preemption pending, briefly enable then disable
        // preemption to give this CPU a chance to reschedule.
        const cpu_mask_t curr_cpu_mask = cpu_num_to_mask(arch_curr_cpu_num());
        if ((Thread::Current::preemption_state().preempts_pending() & curr_cpu_mask) != 0) {
          // Reenable preemption to trigger a local reschedule and then disable it again.
          preempt_disabler.Enable();
          preempt_disabler.Disable();
        }
      }
    }

    // Give the arch a chance to relax the CPU.
    arch::Yield();
    now_ticks = current_mono_ticks();
  } while (now_ticks < spin_until_ticks);

  // Capture the end-of-spin timestamp for our spin tracer, but do not finish
  // the event just yet. We don't actually know if we are going to block or not
  // yet; we have one last chance to grab the lock after we obtain a few more
  // spinlocks.  Once we have dropped into the final locks, we should be able to
  // produce our spin-record using the timestamp explicitly recorded here.
  spin_tracing::SpinTracingTimestamp spin_end_ts{};

  if ((LK_DEBUGLEVEL > 0) && unlikely(this->IsHeld())) {
    panic("Mutex::Acquire: thread %p (%s) tried to acquire mutex %p it already owns.\n",
          current_thread, current_thread->name(), this);
  }

  ContentionTimer timer(current_thread, now_ticks);

  // Blocking in an OwnedWaitQueue requires that preemption be disabled.
  //
  // TODO(johngro); should we find a way to transfer the responsibility for
  // preempt disabling to the CLT so preemption can be properly re-enabled
  // during a back-off and relax event?
  preempt_disabler.Disable();
  const auto do_transaction =
      [&]() TA_REQ(chainlock_transaction_token) -> ChainLockTransaction::Result<bool> {
    preempt_disabled_token.AssertHeld();
    // We contended with someone else, will probably need to block. Acquire an OWQ and then
    // hold its lock while we check if we need to block.
    OwnedWaitQueue* queue = wait_.Get();

    // It is possible that at this point the pointer has been detached by another thread
    // in order to recycle the OWQ. We know that the OWQ is not going to be freed (see
    // OwnedWaitQueuePool::Shrink) so it is safe to acquire the lock. We will release it
    // below if we lost the race and the OWQ was recycled.
    queue->get_lock().AcquireFirstInChain();

    if (likely(wait_.Peek() == queue)) {
      // At this point, we know that `queue` is not going to be recycled out from under us
      // as long as we hold the OWQ's lock since `queue` would have been detached before we
      // were able to acquire the OWQ's lock.
    } else {
      // We were racing with a thread that was the last waiter in the middle of releasing the
      // mutex and they recycled the OWQ. Try again.
      queue->get_lock().Release();
      return ChainLockTransaction::Action::Backoff;
    }

    // Check if the contested flag is currently set. The contested flag can only be changed
    // whilst the OWQ's lock is held, so we know we aren't racing with anyone here. This
    // is just an optimization and allows us to avoid redundantly doing the atomic OR.
    uintptr_t old_mutex_state = val_.load(ktl::memory_order_relaxed);
    if (unlikely(!(old_mutex_state & STATE_FLAG_CONTESTED))) {
      // Set the queued flag to indicate that we're blocking.
      //
      // We may find the old state was |STATE_FREE| if we raced with the
      // holder as they dropped the mutex. We use acquire+release semantics
      // here to make sure we do not reorder before the acquisition of the OWQ.
      old_mutex_state = val_.fetch_or(STATE_FLAG_CONTESTED, ktl::memory_order_acq_rel);

      if (unlikely(old_mutex_state == STATE_FREE)) {
        // Since we set the contested flag we know that there are no waiters and
        // no one is able to perform fast path acquisition. Therefore we can
        // just take the mutex, and remove the queued flag.  Relaxed semantic
        // are fine here, we already observed the state with acquire semantics
        // during the fetch_or.
        ChainLockTransaction::Finalize();
        val_.store(new_mutex_state, ktl::memory_order_relaxed);
        RecordInitialAssignedCpu();

        // We were the only waiter and we successfully acquired the mutex so we
        // don't need the queue anymore. We need to detach the queue from LazyOWQ
        // before releasing the OWQ's lock, otherwise other threads might try to
        // block on the queue that we're about to recycle.
        {
          Guard<SpinLock, NoIrqSave> guard{OwnedWaitQueuePoolLock::Get()};
          OwnedWaitQueue* detached = wait_.Detach();
          DEBUG_ASSERT(detached == queue);
          queue->get_lock().Release();
          OwnedWaitQueuePool::Get().RecycleLocked(detached);
        }

        spin_tracer.Finish(spin_tracing::FinishType::kLockAcquired, this->encoded_lock_id(),
                           spin_end_ts);

        if constexpr (TimesliceExtensionEnabled) {
          return Thread::Current::preemption_state().SetTimesliceExtension(
              timeslice_extension.value);
        }

        return false;
      }
    }

    spin_tracer.Finish(spin_tracing::FinishType::kBlocked, this->encoded_lock_id(), spin_end_ts);

    // Whether or not we just set the contested flag in our state, this mutex
    // currently has an owner.  Extract the owner and make sure we assign it to
    // our owned wait queue.
    Thread* const cur_owner = holder_from_val(old_mutex_state);
    DEBUG_ASSERT(cur_owner != nullptr);
    DEBUG_ASSERT(cur_owner != current_thread);

    // Attempt to obtain the locks we need to block in the mutex.
    OwnedWaitQueue::BAAOLockingDetails details;
    if (!queue->TryLockForBAAOOperationLocked(current_thread, cur_owner, details)) {
      // We failed to lock what we needed to lock in order to block.  Drop our
      // queue lock so that we can try again, but do _not_ put the lock state
      // back to the way it was before.  We just observed that the lock was
      // owned by someone, and while doing so we may have set the contested
      // flag.  We want to preserve the invariant: "once the contested flag has
      // been set (while holding the queue lock), only the owner of the lock is
      // allowed to clear the flag".
      queue->get_lock().Release();
      return ChainLockTransaction::Action::Backoff;
    }

    // Ok, we are now committed.  Our state has been updated to indicate the
    // proper owner of the mutex, and we hold all of the locks we need in order
    // to block.  Drop into the block operation, releasing all of the locks we
    // hold as we do so.
    ChainLockTransaction::Finalize();

    // Log the trace data now that we are committed to blocking.
    const uint64_t flow_id = current_thread->TakeNextLockFlowId();
    LOCK_TRACE_FLOW_BEGIN("contend_mutex", flow_id);
    KTracer{}.KernelMutexBlock(this, cur_owner, queue->Count() + 1);

    // Block the thread.  This will drop all of the PI related locks we have
    // been holding up until now.
    preempt_disabler.AssertDisabled();
    zx_status_t ret =
        queue->BlockAndAssignOwnerLocked(current_thread, Deadline::infinite(), details,
                                         ResourceOwnership::Normal, Interruptible::No);

    if (unlikely(ret < ZX_OK)) {
      // mutexes are not interruptible and cannot time out, so it
      // is illegal to return with any error state.
      panic("Mutex::Acquire: wait queue block returns with error %d m %p, thr %p, sp %p\n", ret,
            this, current_thread, __GET_FRAME());
    }

    // Someone woke us up, we should be the holder of the mutex now.
    DEBUG_ASSERT(current_thread == holder());

    // We need to manually drop our current thread's lock.
    // BlockAndAssignedOwnerLocked released all of the other required locks for
    // us, and the current thread's lock was actually released when we finally
    // blocked, but it was re-obtained for us when we unblocked and became
    // re-scheduled.
    current_thread->get_lock().Release();

    if constexpr (TimesliceExtensionEnabled) {
      return Thread::Current::preemption_state().SetTimesliceExtension(timeslice_extension.value);
    }
    return false;
  };
  return ChainLockTransaction::UntilDone(IrqSaveOption, CLT_TAG("Mutex::AcquireContendedMutex"),
                                         do_transaction);
}

inline uintptr_t Mutex::TryRelease(Thread* current_thread) {
  // Try the fast path.  Assume that we are locked, but uncontested.
  // Success memory order is release: makes sure the lock state is ordered so
  // the next thread sees the content. Failure memory order is acquire: makes
  // sure we re-observe the state of the mutex before we try to wake up threads
  // and unwind the state.
  uintptr_t old_mutex_state = reinterpret_cast<uintptr_t>(current_thread);
  if (likely(val_.compare_exchange_strong(old_mutex_state, STATE_FREE, ktl::memory_order_release,
                                          ktl::memory_order_acquire))) {
    // We're done.  Since this mutex was uncontested, we know that we were
    // not receiving any priority pressure from the wait queue, and there is
    // nothing further to do.
    KTracer{}.KernelMutexUncontestedRelease(this);
    return STATE_FREE;
  }

  // The mutex is contended, return the current state of the mutex.
  return old_mutex_state;
}

__NO_INLINE void Mutex::ReleaseContendedMutex(Thread* current_thread, uintptr_t old_mutex_state) {
  LOCK_TRACE_DURATION("Mutex::ReleaseContended", ("name", class_name_ref()));
  OwnedWaitQueue::IWakeRequeueHook& default_hooks = OwnedWaitQueue::default_wake_hooks();

  // Lock our OWQ expecting to wake at most 1 thread.
  //
  // Disable preemption to prevent switching to the woken thread inside of
  // WakeThreads() if it is assigned to this CPU. If the woken thread is
  // assigned to a different CPU, the thread lock prevents it from observing
  // the inconsistent owner before the correct owner is recorded.
  const auto do_transaction = [&]()
                                  TA_REQ(chainlock_transaction_token,
                                         preempt_disabled_token) -> ChainLockTransaction::Result<> {
    // This mutex is contended, so it must have a queue.
    OwnedWaitQueue* const queue = wait_.Peek();
    DEBUG_ASSERT(queue != nullptr);

    Thread::UnblockList wake_me;
    if (!queue->LockForWakeOperationOrBackoff(1u, default_hooks, wake_me)) {
      return ChainLockTransaction::Action::Backoff;
    }

    ChainLockTransaction::Finalize();

    // Pre-flight checks.  In order to get here, the mutex's state should indicate
    // that we are the owner and that the mutex is currently contested.  This may
    // not actually turn out to be the state (perhaps the thread contesting the
    // mutex has timed out), but once the contested flag has been set, this should
    // be the only place it can be cleared.
    if (LK_DEBUGLEVEL > 0) {
      uintptr_t expected_state = reinterpret_cast<uintptr_t>(current_thread) | STATE_FLAG_CONTESTED;

      if (unlikely(old_mutex_state != expected_state)) {
        auto other_holder = reinterpret_cast<Thread*>(old_mutex_state & ~STATE_FLAG_CONTESTED);
        panic(
            "Mutex::ReleaseContendedMutex: consistency check failure.  Thread %p (%s) tried to release "
            "mutex %p.  Expected state (%lx) != observed state (%lx).  Other holder (%s)\n",
            current_thread, current_thread->name(), this, expected_state, old_mutex_state,
            other_holder ? other_holder->name() : "<none>");
      }
    }

    // If there is no one to wake, we should be able to assert that the OWQ has no
    // owner, and we should be able to we can simply set our state to FREE and get
    // out.  Otherwise, we need to change the state of the queue to indicate it is
    // now owned by the thread we are waking, wake the thread, and make sure that
    // it is declared as the owner of the mutex's OWQ.
    //
    // We need to make sure that we set the state before we wake the thread.  As
    // soon as the thread wakes, it is on the hotpath out, and it needs the
    // mutex's state to be updated already in order to perform operations such as
    // Release, or AssertHeld on the mutex.
    KTracer tracer;
    const bool do_wake = !wake_me.is_empty();
    uintptr_t new_mutex_state;
    Thread* new_owner;
    uint32_t still_waiting;

    if (do_wake) {
      DEBUG_ASSERT(queue->Count() >= 1);
      new_owner = &wake_me.front();
      new_mutex_state = reinterpret_cast<uintptr_t>(new_owner);
      still_waiting = queue->Count() - 1;
      if (still_waiting) {
        new_mutex_state |= STATE_FLAG_CONTESTED;
      }
    } else {
      DEBUG_ASSERT(queue->owner() == nullptr);
      new_owner = nullptr;
      still_waiting = 0;
      new_mutex_state = STATE_FREE;
    }

    // It should be safe to pass `new_owner` to the tracer at this point in time.
    // We have not woken the thread yet, and we hold its lock (as well as its
    // queue's lock) meaning that it cannot successfully be woken (and therefore
    // must still be alive) until after we release the lock.
    tracer.KernelMutexWake(this, new_owner, still_waiting);

    // Update our state with release semantics.
    if (unlikely(!val_.compare_exchange_strong(old_mutex_state, new_mutex_state,
                                               ktl::memory_order_release,
                                               ktl::memory_order_relaxed))) {
      panic("bad state (%lx != %lx) in mutex release %p, current thread %p\n",
            reinterpret_cast<uintptr_t>(current_thread) | STATE_FLAG_CONTESTED, old_mutex_state,
            this, current_thread);
    }

    // Now that our state is updated, actually perform our wake operation if we
    // need to, then drop the queue lock and get out.
    if (do_wake) {
      [[maybe_unused]] const OwnedWaitQueue::WakeThreadsResult wake_result =
          queue->WakeThreadsLocked(ktl::move(wake_me), default_hooks,
                                   OwnedWaitQueue::WakeOption::AssignOwner);
      DEBUG_ASSERT(wake_result.woken == 1);
      if (wake_result.still_waiting > 0) {
        DEBUG_ASSERT(wake_result.owner == new_owner);
        DEBUG_ASSERT(queue->owner() == new_owner);
      } else {
        DEBUG_ASSERT(wake_result.owner == nullptr);
        DEBUG_ASSERT(queue->owner() == nullptr);
      }
      LOCK_TRACE_FLOW_END("contend_mutex", new_owner->lock_flow_id());
    }

    // Finally release the OWQ's lock and we are finished.  Note: there are a few
    // tricky lifecycle issues here.  Immediately after the compare and exchange
    // strong (above), we have either set our Mutex state to be FREE, or to be
    // owned by the thread that we are about to wake.  Either immediately after we
    // set the state to FREE, or immediately after we wake the thread we assigned
    // ownership to, it is possible for the Mutex to become destroyed, even if
    // external code bounces through a Mutex acquire/release cycle before
    // destroying it (If there is no contention on the lock at that point, other
    // threads will not synchronize on the wait queue's lock before allowing the
    // object to become destroyed).
    //
    // We prevent this potential use after free by spinning in the Mutex's
    // destructor until it observes the inner queue lock as being unlocked state
    // before proceeding with destruction.  This will guarantee that any thread at
    // this point in the code will hold off destruction until it is past the point
    // where it will ever touch the object's memory. Additionally, it guarantees
    // that anything destroying the object is certain to see all of the changes
    // made to the inner OWQ before destruction is allowed to start.
    if (still_waiting == 0) {
      // We were the last waiter, so we are now responsible for detaching and recycling
      // the OWQ from the LazyOWQ.
      //
      // We must ensure that no other thread can observe the OWQ as being attached to the
      // LazyOWQ after we've released the OWQ's lock. If they did, they might try to
      // block on it while it is being placed back in the pool.
      //
      // This means:
      //
      // 1. acquire the pool lock while still holding OWQ's lock
      // 2. detach the OWQ from the LazyOWQ
      // 3. release the OWQ's lock
      // 4. recycle the OWQ
      // 5. release the pool lock
      Guard<SpinLock, NoIrqSave> guard{OwnedWaitQueuePoolLock::Get()};

      OwnedWaitQueue* detached = wait_.Detach();
      // The queue can't have changed from under us in the process of unblocking, since the queue
      // can only change when the last waiter stops contending on the Mutex. Since we're just now
      // unblocking the queue must be the same it was when ReleaseContendedMutex was first called.
      DEBUG_ASSERT(detached == queue);

      queue->get_lock().Release();

      OwnedWaitQueuePool::Get().RecycleLocked(detached);
    } else {
      // Not recycling, just drop the lock.
      queue->get_lock().Release();
    }

    return ChainLockTransaction::Done;
  };
  ChainLockTransaction::UntilDone(PreemptDisableAndIrqSaveOption,
                                  CLT_TAG("Mutex::ReleaseContendedMutex"), do_transaction);
}

void Mutex::ReleaseInternal() {
  magic_.Assert();
  DEBUG_ASSERT(!arch_blocking_disallowed());
  Thread* current_thread = Thread::Current::Get();

  ClearInitialAssignedCpu();

  if (const uintptr_t old_mutex_state = TryRelease(current_thread); old_mutex_state != STATE_FREE) {
    ReleaseContendedMutex(current_thread, old_mutex_state);
  }
}

// Explicit instantiations since it's not defined in the header.
template bool Mutex::AcquireCommon(zx_duration_t spin_max_duration, TimesliceExtension<false>);
template bool Mutex::AcquireCommon(zx_duration_t spin_max_duration, TimesliceExtension<true>);
