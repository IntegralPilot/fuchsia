// Copyright 2016 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "object/futex_context.h"

#include <assert.h>
#include <lib/kconcurrent/chainlock.h>
#include <lib/kconcurrent/chainlock_transaction.h>
#include <lib/ktrace.h>
#include <lib/zircon-internal/macros.h>
#include <trace.h>
#include <zircon/types.h>

#include <fbl/intrusive_hash_table.h>
#include <fbl/null_lock.h>
#include <kernel/auto_preempt_disabler.h>
#include <kernel/lock_trace.h>
#include <kernel/mutex.h>
#include <kernel/scheduler.h>
#include <object/process_dispatcher.h>
#include <object/thread_dispatcher.h>

#define LOCAL_TRACE 0

#ifndef FUTEX_TRACING_ENABLED
#define FUTEX_TRACING_ENABLED false
#endif

namespace {  // file scope only

constexpr bool kFutexBlockTracingEnabled = FUTEX_BLOCK_TRACING_ENABLED;
// Gets a reference to the thread that the user is asserting is the new owner of
// the futex.  The thread must have the same futex context as the caller since
// futexes may not be owned by threads with different futex contexts.  In addition,
// the new potential owner thread must have been started.  Threads which have not
// started yet may not be the owner of a futex.
//
// Do this before we enter any potentially blocking locks.  Right now, this
// operation can block on BRW locks involved in protecting the global handle
// table, and the penalty for doing so can be severe due to other issues.
// Until these are resolved, we would rather pay the price to do validation
// here instead of while holding the lock.
//
// This said, we cannot bail out with an error just yet.  We need to make it
// into the futex's lock and perform futex state validation first.  See Bug
// #34382 for details.
zx_status_t ValidateFutexOwner(zx_handle_t new_owner_handle,
                               fbl::RefPtr<ThreadDispatcher>* thread_dispatcher) {
  if (new_owner_handle == ZX_HANDLE_INVALID) {
    return ZX_OK;
  }
  auto up = ProcessDispatcher::GetCurrent();
  zx_status_t status = up->handle_table().GetDispatcherWithRightsNoPolicyCheck(
      new_owner_handle, 0, thread_dispatcher, nullptr);
  if (status != ZX_OK) {
    return status;
  }

  // Make sure that the proposed owner of the futex shares our futex context,
  // and that it has been started.
  const auto& new_owner = *thread_dispatcher;
  if ((&new_owner->process()->futex_context() != &up->futex_context()) ||
      !new_owner->HasStarted()) {
    thread_dispatcher->reset();
    return ZX_ERR_INVALID_ARGS;
  }

  // TODO(johngro): Is this where we signal to the lower level thread that it
  // cannot exit and free itself just yet because we are considering making this
  // thread an owner.  Perhaps thread ref counting needs to come back?
  //
  // If the thread is already DEAD or DYING, don't bother attempting to assign
  // it as a new owner for the futex.
  if (new_owner->IsDyingOrDead()) {
    thread_dispatcher->reset();
  }
  return ZX_OK;
}

inline zx_status_t ValidateFutexPointer(user_in_ptr<const zx_futex_t> value_ptr) {
  if (reinterpret_cast<uintptr_t>(value_ptr.get()) % sizeof(zx_futex_t) != 0) {
    return ZX_ERR_INVALID_ARGS;
  }
  if (FutexId(value_ptr) == FutexId::Null()) {
    return ZX_ERR_INVALID_ARGS;
  }
  return ZX_OK;
}

}  // namespace

void FutexContext::WakeHook::OnWakeOrRequeue(Thread& t) {
  // Why is this safe?  Because there are two places that a blocking
  // futex ID gets set.  By a thread while it is running and enters
  // FutexWait, or when a thread has been successfully dequeued from an
  // OWQ and is in the process of being woken or requeued.
  //
  // For a thread to block in any queue (FutexWait), owned or not, it must
  // hold its lock exclusively, and will drop it as it finishes the operation
  // and its CPU switches to a different thread.  By the time it is possible
  // for us to hold the thread's lock and observe it in the blocked state, any
  // mutations to the blocking futex id must have "happened before" we were
  // able to observe and wake the thread.
  //
  // Likewise, to unblock the thread during the wake operation, we need to be
  // holding thread's lock exclusively (as we are doing here).  In order for
  // the thread to run again after this, it must obtain its own lock as it
  // becomes rescheduled, meaning that any mutations we perform to the
  // blocking_futex_id must be visible to it.
  //
  // TODO(johngro): Consider simply moving the blocking futex ID down to
  // the kernel thread level of things in order to simplify all of this
  // reasoning?  That way, it could easily just be protected by the thread's
  // lock.
  if (t.user_thread() != nullptr) {
    t.user_thread()->blocking_futex_id_ = FutexId::Null();
    LOCK_TRACE_FLOW_END("contend_futex", t.lock_flow_id());
  }
}

void FutexContext::RequeueHook::OnWakeOrRequeue(Thread& t) {
  if (t.user_thread() != nullptr) {
    t.user_thread()->blocking_futex_id_ = new_id;
  }

  // As we requeue threads, we need to transfer their pending operation
  // counts from the FutexState that they went to sleep on, over to the
  // FutexState they are being requeued to.
  //
  // Sadly, this needs to be done from within the context of the requeued
  // thread's lock.  Failure to do this means that it would be possible for us
  // to requeue a thread from futex A over to futex B, then have that thread
  // time out from the futex before we have move the pending operation
  // references from A to B.  If the thread manages wake up and attempts to
  // drop its pending operation count on futex B before we have transferred
  // the count, it would result in a bookkeeping error.
  requeue_futex_ref.TakeRefs(&wake_futex_ref, 1);
}

struct ResetBlockingFutexIdState {
  ResetBlockingFutexIdState() = default;

  // No move, no copy.
  ResetBlockingFutexIdState(const ResetBlockingFutexIdState&) = delete;
  ResetBlockingFutexIdState(ResetBlockingFutexIdState&&) = delete;
  ResetBlockingFutexIdState& operator=(const ResetBlockingFutexIdState&) = delete;
  ResetBlockingFutexIdState& operator=(ResetBlockingFutexIdState&&) = delete;

  uint32_t count = 0;
};

struct SetBlockingFutexIdState {
  explicit SetBlockingFutexIdState(FutexId new_id) : id(new_id) {}

  // No move, no copy.
  SetBlockingFutexIdState(const SetBlockingFutexIdState&) = delete;
  SetBlockingFutexIdState(SetBlockingFutexIdState&&) = delete;
  SetBlockingFutexIdState& operator=(const SetBlockingFutexIdState&) = delete;
  SetBlockingFutexIdState& operator=(SetBlockingFutexIdState&&) = delete;

  const FutexId id;
  uint32_t count = 0;
};

FutexContext::FutexState::~FutexState() {}

FutexContext::FutexContext() : active_futexes_(fbl::HashTableOption::DelayedInit) { LTRACE_ENTRY; }

FutexContext::~FutexContext() {
  LTRACE_ENTRY;

  // All of the threads should have removed themselves from wait queues and
  // destroyed themselves by the time the process has exited.
  DEBUG_ASSERT(active_futexes_.is_empty());
  DEBUG_ASSERT(free_futexes_.is_empty());
}

zx_status_t FutexContext::Init(ProcessType type) {
  size_t num_buckets;
  switch (type) {
    case ProcessType::Regular:
      num_buckets = 37;
      break;
    case ProcessType::Shared:
      // A FutexContext used by a group of shared processes tends to have *many* active futexes.
      num_buckets = 1031;
      break;
    default:
      panic("unknown ProcessType %u", static_cast<uint32_t>(type));
  };

  fbl::AllocChecker ac;
  auto storage = ktl::make_unique<ActiveFutexMap::BucketType[]>(&ac, num_buckets);
  if (!ac.check()) {
    return ZX_ERR_NO_MEMORY;
  }

  {
    Guard<SpinLock, IrqSave> pool_lock_guard{&pool_lock_};
    active_futexes_.Init(ktl::move(storage), num_buckets);
  }

  return ZX_OK;
}

zx_status_t FutexContext::GrowFutexStatePool() {
  fbl::AllocChecker ac;
  ktl::unique_ptr<FutexState> new_state1{new (&ac) FutexState};
  if (!ac.check()) {
    return ZX_ERR_NO_MEMORY;
  }

  ktl::unique_ptr<FutexState> new_state2{new (&ac) FutexState};
  if (!ac.check()) {
    return ZX_ERR_NO_MEMORY;
  }

  Guard<SpinLock, IrqSave> pool_lock_guard{&pool_lock_};
  free_futexes_.push_front(ktl::move(new_state1));
  free_futexes_.push_front(ktl::move(new_state2));
  return ZX_OK;
}

void FutexContext::ShrinkFutexStatePool() {
  ktl::unique_ptr<FutexState> state1, state2;
  {  // Do not let the futex state become released inside of the lock.
    Guard<SpinLock, IrqSave> pool_lock_guard{&pool_lock_};
    DEBUG_ASSERT(free_futexes_.is_empty() == false);
    state1 = free_futexes_.pop_front();
    state2 = free_futexes_.pop_front();
  }
}

// NullableDispatcherGuard is a mutex guard type.  Its purpose is to allow the use of clang
// thread-safety static analysis in situations where we have a (possibly null) pointer to a
// ThreadDispatcher that we want to lock, but only when the pointer is non-null.  When the pointer
// is null, we lie to the compiler about holding its lock.
class TA_SCOPED_CAP FutexContext::NullableDispatcherGuard {
 public:
  explicit NullableDispatcherGuard(ThreadDispatcher* t) TA_ACQ(t->get_lock()) : t_{t} {
    if (t_ != nullptr) {
      should_clear_ = t_->get_lock()->lock().Acquire();
    }
  }

  ~NullableDispatcherGuard() TA_REL() { Release(); }

  NullableDispatcherGuard(const NullableDispatcherGuard&) = delete;
  NullableDispatcherGuard(NullableDispatcherGuard&&) = delete;
  NullableDispatcherGuard& operator=(const NullableDispatcherGuard&) = delete;
  NullableDispatcherGuard& operator=(NullableDispatcherGuard&&) = delete;

  void Release() TA_REL() {
    if (t_ != nullptr) {
      t_->get_lock()->lock().Release(should_clear_);
      t_ = nullptr;
    }
  }

 private:
  ThreadDispatcher* t_;
  CriticalMutex::ShouldClear should_clear_ = CriticalMutex::ShouldClear::No;
};

// FutexWait verifies that the integer pointed to by |value_ptr| still equals
// |current_value|. If the test fails, FutexWait returns FAILED_PRECONDITION.
// Otherwise it will block the current thread until the |deadline| passes, or
// until the thread is woken by a FutexWake or FutexRequeue operation on the
// same |value_ptr| futex.
zx_status_t FutexContext::FutexWait(user_in_ptr<const zx_futex_t> value_ptr,
                                    zx_futex_t current_value, zx_handle_t new_futex_owner,
                                    const Deadline& deadline) {
  LTRACE_ENTRY;

  // As we may need to perform fault resolution later, ensure our caller is not holding any locks.
  lockdep::AssertNoLocksHeld();

  // Make sure the futex pointer is following the basic rules.
  zx_status_t result = ValidateFutexPointer(value_ptr);
  if (result != ZX_OK) {
    return result;
  }

  fbl::RefPtr<ThreadDispatcher> futex_owner_thread;
  const zx_status_t validator_status = ValidateFutexOwner(new_futex_owner, &futex_owner_thread);

  Thread* current_core_thread = Thread::Current::Get();
  ThreadDispatcher* current_thread = current_core_thread->user_thread();
  FutexId futex_id(value_ptr);
  {
    // Obtain the FutexState for the ID we are interested in, activating a free
    // futex state in the process if needed.  This operation should never fail
    // (there should always be a FutexState available to us).
    //
    FutexState::PendingOpRef futex_ref = ActivateFutex(futex_id);
    DEBUG_ASSERT(futex_ref != nullptr);

    // Now that we have a hold of the FutexState, enter the futex specific lock
    // and validate the user-mode futex state.
    //
    // FutexWait() checks that the address value_ptr still contains
    // current_value, and if so it sleeps awaiting a FutexWake() on value_ptr.
    // Those two steps must together be atomic with respect to FutexWake().  If
    // a FutexWake() operation could occur between them, a user-land mutex
    // operation built on top of futexes would have a race condition that could
    // miss wakeups.
    //
    // Note that we disable involuntary preemption while we are inside of this
    // lock.  The price of blocking while holding this lock is high, and we
    // should not (in theory) _ever_ be inside of this lock for very long at
    // all. The vast majority of the time, we just need validate the state,
    // then trade this lock for the thread lock, and then block.  Even if we are
    // operating at the very end of our slice, it is best to disable preemption
    // until we manage to join the wait queue, or abort because of state
    // validation issues.
    while (1) {
      // The lock ordering in this loop body is a bit subtle. Here are the locks we acquire in the
      // order we acquire them, along with some rationale as to why they're acquired and released
      // in that order:
      //
      // 1. We start by acquiring the futex_owner_thread's ThreadDispatcher lock, if the futex
      //    owner is not nullptr. We need to hold this lock to verify that the thread has not died
      //    during this Wait operation. It needs to be acquired before the FutexState's spinlock,
      //    as acquiring a mutex while holding a spinlock is not allowed.
      // 2. Then, we acquire the FutexState's spinlock. We need to hold this lock to verify that the
      //    value of the futex hasn't changed during this Wait operation. Acquiring this spinlock
      //    requires disabling interrupts. We do so explicitly with an `InterruptDisableGuard`
      //    instead of using the `IrqSave` option in the `Guard` because we acquire the thread lock
      //    later in this method, and we want IRQs to be disabled for this entire duration.
      // 3. We then validate the futex's state and the futex owner's state.
      // 4. Once validation is complete, we observe the "core thread" of the proposed new owner.
      //    This is the Thread* member of the ThreadDispatcher.  If there is still a valid proposed
      //    new owner, we will obtain the dispatcher's "core thread" lock, which will ensure that
      //    the underlying Thread cannot exit even after we drop the ThreadDispatcher lock.
      // 5. Next, we can obtain the set of ChainLocks needed to block the thread an assign our
      //    (optional) new owner.
      // 6. Once we have these locks, we can drop the FutexState lock and proceed with the block
      //    operation.
      NullableDispatcherGuard futex_owner_guard(futex_owner_thread.get());
      InterruptDisableGuard irqd;
      Guard<SpinLock, NoIrqSave> futex_state_guard{&futex_ref->lock_};

      // Sanity check, bookkeeping should not indicate that we are blocked on
      // a futex at this point in time.
      DEBUG_ASSERT(current_thread->blocking_futex_id_ == FutexId::Null());

      int value;
      UserCopyCaptureFaultsResult copy_result = arch_copy_from_user_capture_faults(
          &value, value_ptr.get(), sizeof(int), CopyContext::kBlockingNotAllowed);
      if (copy_result.status != ZX_OK) {
        // At this point we are committed to either returning from the function, or restarting the
        // loop, so we:
        // 1. Drop the futex state lock.
        // 2. Re-enable interrupts.
        // 3. Drop the futex owner lock.
        // 4. Either return with an error or resolve the fault and continue the loop.
        futex_state_guard.Release();
        irqd.Reenable();
        futex_owner_guard.Release();
        if (auto fault = copy_result.fault_info) {
          result = Thread::Current::SoftFault(fault->pf_va, fault->pf_flags);
          if (result != ZX_OK) {
            return result;
          }
          continue;
        }
        return copy_result.status;
      }

      if (value != current_value) {
        return ZX_ERR_BAD_STATE;
      }

      if (validator_status != ZX_OK) {
        if (validator_status == ZX_ERR_BAD_HANDLE) {
          [[maybe_unused]] auto res =
              ProcessDispatcher::GetCurrent()->EnforceBasicPolicy(ZX_POL_BAD_HANDLE);
        }
        return validator_status;
      }

      // Now that the futex state has been validated, we can validate the state of the futex owner
      // thread (if one exists). We must do this after validating the futex state to avoid the race
      // condition outlined in https://fxbug.dev/42109683.
      // TODO(johngro): explain how the core thread observation keeps the Thread* core thread valid.
      ThreadDispatcher::CoreThreadObservation new_owner_observation;
      if (futex_owner_thread != nullptr) {
        // When attempting to wait, the new owner of the futex (if any) may not be
        // the thread which is attempting to wait.
        if (futex_owner_thread.get() == ThreadDispatcher::GetCurrent()) {
          return ZX_ERR_INVALID_ARGS;
        }

        // If we have a valid new owner, then verify that this thread is not already
        // waiting on the target futex.
        if (futex_owner_thread->blocking_futex_id_ == futex_id) {
          return ZX_ERR_INVALID_ARGS;
        }

        // We need to resolve |futex_owner_thread|'s Thread*.  First, we must revalidate that
        // |futex_owner_thread| is not dead or dying as it may have changed state during the
        // |SoftFault| call above.
        if (!futex_owner_thread->IsDyingOrDeadLocked()) {
          new_owner_observation = futex_owner_thread->ObserveCoreThread();
        }
      }

      // We have now passed all of our checks and are committed to becoming
      // blocked.  We are currently holding a few different locks/guards, but do
      // not yet hold the set of chain locks we will need to perform the final
      // block and assign owner operation.  We will exchange the spinlocks we
      // are holding for the ChainLocks we need, but _before_ we can do that, we
      // need to release the "futex_owner_guard", which is holding a mutex (if
      // we still have a valid proposed owner).
      //
      // We need to release this mutex before we start obtaining ChainLocks; if
      // we release the mutex and discover that we need to wake a waiter, we
      // need to make sure we are not already in the middle of a
      // ChainLockTransaction (since we will need to conduct a transaction in
      // order to unblock the next waiter)
      //
      // We also need to make sure that local rescheduling (at least) has been
      // disabled.  We are currently holding spinlocks, and even if we do end up
      // unblocking another thread, we cannot reschedule until we drop those
      // spinlocks.
      //
      // So, disable all rescheduling for now, both local and "eager
      // rescheduling" (meaning we will hold off on sending IPIs just now).  We
      // are going to block, and when we do, all of the pending reschedule
      // operations (local and remote) will be automatically flushed for us.
      AnnotatedAutoEagerReschedDisabler eager_resched_disabler;
      futex_owner_guard.Release();  // This is now OK since we have suppressed
                                    // local rescheduling.

      // Now we can go ahead and obtain the ChainLocks we need, and once we have
      // them, drop the spinlocks we are holding.
      Thread* const current_kernel_thread = Thread::Current::Get();

      // StateSaver handles the relax operation during backoff.
      ChainLockTransaction::StateSaver<ChainLockTransaction::StateOptions::NoIrqSave> state_saver;

      // Create a bare transaction to avoid tripping over all of the existing guards and auto state
      // save/restore RAII objects above.
      // TODO(eieio): Figure out how to refactor this method into a pattern that is amedable to the
      // standard chain lock transaction pattern.
      ChainLockTransaction transaction =
          ChainLockTransaction::MakeBareTransaction(CLT_TAG("FutexContext::FutexWaitInternal"));

      Thread* const new_owner = new_owner_observation.core_thread();
      OwnedWaitQueue::BAAOLockingDetails locking_details;
      while (!futex_ref->waiters_.LockForBAAOOperationOrBackoff(current_kernel_thread, new_owner,
                                                                locking_details)) {
        // No other chain locks are held. Simply try again until the locks are obtained.
        transaction.AssertNumLocksHeld(0);
        state_saver.Relax(transaction);
      }

      transaction.Finalize();

      // We now have all of the locks we need in order to block our thread.
      // We can now drop the:
      //
      // 1) New owner core lock (if any).  We have the new prospective owner
      //    Thread's ChainLock held, it cannot exit out from under us at this
      //    point.
      // 2) The futex state guard.  We have the futex's OwnedWaitQueue
      //    ChainLock, so it is not possible for another thread to wake
      //    threads from the Futex's OWQ before this thread manages to become
      //    blocked.
      new_owner_observation.Release();
      futex_state_guard.Release();

      // Record the futex ID of the thread we are about to block on.
      current_thread->blocking_futex_id_ = futex_id;

      // Mark that we are now blocked-by-futex.
      ThreadDispatcher::AutoBlocked by(ThreadDispatcher::Blocked::FUTEX);

      // Finally, drop into the block operation.  This will take care of
      // releasing the queue's lock, as well as any of the locks involved in
      // ownership propagation.  The current thread's lock will be held right
      // up until the point where it is context switched away from, when it
      // finally will be dropped.  It will be re-obtained later on after the
      // thread has woken and become rescheduled, and we will need to drop it
      // ourselves as we unwind.
      {
        ktrace::Scope block_tracer = KTRACE_BEGIN_SCOPE_ENABLE(
            kFutexBlockTracingEnabled, "kernel:sched", "futex_block", ("futex_id", futex_id.get()),
            ("blocking_tid", new_owner ? new_owner->tid() : ZX_KOID_INVALID));

        LOCK_TRACE_DURATION("Futex::Wait", ("futex_id", ktrace::Pointer{futex_id.get()}));

        const uint64_t flow_id = current_core_thread->TakeNextLockFlowId();
        LOCK_TRACE_FLOW_BEGIN("contend_futex", flow_id);

        result = futex_ref->waiters_.BlockAndAssignOwnerLocked(
            current_kernel_thread, deadline, locking_details, ResourceOwnership::Normal,
            Interruptible::Yes);
      }

      // Do _not_ allow the PendingOpRef helper to release our pending op
      // reference.  Having just woken up, either the thread which woke us will
      // have released our pending op reference, or we will need to revalidate
      // _which_ futex we were waiting on (because of FutexRequeue) and manage the
      // release of the reference ourselves.
      futex_ref.CancelRef();

      // We came out of the block operation holding only our current thread's
      // chain lock.  Drop our own lock and break out of the retry loop.
      current_kernel_thread->get_lock().Release();
      break;
    }
  }

  // If we were woken by another thread, then our block result will be ZX_OK.
  // We know that the thread has handled releasing our pending op reference, and
  // has reset our blocking futex ID to zero.  No special action should be
  // needed by us at this point.
  if (result == ZX_OK) {
    // The FutexWake operation should have already cleared our blocking
    // futex ID.
    DEBUG_ASSERT(current_thread->blocking_futex_id_ == FutexId::Null());
    return ZX_OK;
  }

  // If the result is not ZX_OK, then additional actions may be required by
  // us.  This could be because
  //
  // 1) We hit the deadline (ZX_ERR_TIMED_OUT)
  // 2) We were killed (ZX_ERR_INTERNAL_INTR_KILLED)
  // 3) We were suspended (ZX_ERR_INTERNAL_INTR_RETRY)
  //
  // In any one of these situations, it is possible that we were the last
  // waiter in our FutexState and need to return the FutexState to the free
  // pool as a result.  To complicate things just a bit further, becuse of
  // zx_futex_requeue, the futex that we went to sleep on may not be the futex
  // we just woke up from.  We need to find the futex we were blocked by, and
  // release our pending op reference to it (potentially returning the
  // FutexState to the free pool in the process).
  DEBUG_ASSERT(current_thread->blocking_futex_id_ != FutexId::Null());

  FutexState::PendingOpRef futex_ref = FindActiveFutex(current_thread->blocking_futex_id_);
  current_thread->blocking_futex_id_ = FutexId::Null();
  LOCK_TRACE_FLOW_END("contend_futex", current_core_thread->lock_flow_id());
  DEBUG_ASSERT(futex_ref != nullptr);

  // Record the fact that we are holding an extra reference.  The first
  // reference was placed on the FutexState at the start of this method as we
  // fetched the FutexState from the pool.  This reference was not removed by a
  // waking thread because we just timed out, or were killed/suspended.
  //
  // The second reference was just added during the FindActiveFutex (above).
  //
  futex_ref.SetExtraRefs(1);

  // Deal with ownership of the futex.  It is possible that we were the last
  // thread waiting on the futex, but that the futex's wait queue still has an
  // owner assigned.  We need to obtain the locks all of the proper locks, then
  // (if the waiters_ OWQ has no threads blocked in it) reset the owner.
  //
  // Note: We should not need the actual FutexState lock at this point in time.
  // We know that the FutexState cannot disappear out from under us (we are
  // holding two pending operation references), and once we are inside of the
  // thread lock, we no that no new threads can join the wait queue.  If there
  // is a thread racing with us to join the queue, then it will go ahead and
  // explicitly update ownership as it joins the queue once it has made it
  // inside of the thread lock.
  futex_ref->waiters_.ResetOwnerIfNoWaiters();
  return result;
}

zx_status_t FutexContext::FutexWake(user_in_ptr<const zx_futex_t> value_ptr, uint32_t wake_count,
                                    OwnerAction owner_action) {
  LTRACE_ENTRY;
  zx_status_t result;

  // Make sure the futex pointer is following the basic rules.
  result = ValidateFutexPointer(value_ptr);
  if (result != ZX_OK) {
    return result;
  }

  // Try to find an active futex with the specified ID.  If we cannot find one,
  // then we are done.  This wake operation had no threads to wake.
  FutexId futex_id(value_ptr);
  FutexState::PendingOpRef futex_ref = FindActiveFutex(futex_id);
  if (futex_ref == nullptr) {
    return ZX_OK;
  }

  // We found an "active" futex, meaning its pending operation count was
  // non-zero when we went looking for it.  Now enter the FutexState specific
  // lock and see if there are any actual waiters to wake up.
  {
    // Optimize lock contention by delaying local/remote reschedules until the mutex is released.
    AnnotatedAutoEagerReschedDisabler eager_resched_disabler;
    Guard<SpinLock, IrqSave> guard{&futex_ref->lock_};

    // Now actually wake up the threads. OwnedWakeQueue will handle the
    // ownership bookkeeping for us.
    {
      LOCK_TRACE_DURATION("Futex::Wake", ("futex_id", ktrace::Pointer{futex_id.get()}));

      // Attempt to wake |wake_count| threads.  Count the number of thread that
      // we have successfully woken, and assign each of their blocking futex IDs
      // to 0 as we go.  We need an accurate count in order to properly adjust
      // the pending operation ref count on our way out of this function.
      const OwnedWaitQueue::WakeThreadsResult wake_result = [&]() {
        WakeHook hook;
        if (owner_action == OwnerAction::RELEASE) {
          return futex_ref->waiters_.WakeThreads(wake_count, hook);
        } else {
          DEBUG_ASSERT(owner_action == OwnerAction::ASSIGN_WOKEN);
          DEBUG_ASSERT(wake_count == 1);
          return futex_ref->waiters_.WakeThreadAndAssignOwner(hook);
        }
      }();

      // Adjust the number of pending operation refs we are about to release.  In
      // addition to the ref we were holding when we started the wake operation, we
      // are also now responsible for the refs which were being held by each of the
      // threads which we have successfully woken.  Those threads are exiting along
      // the FutexWait hot-path, and they have expected us to manage their
      // blocking_futex_id and pending operation references for them.
      futex_ref.SetExtraRefs(wake_result.woken);
    }
  }

  return ZX_OK;
}

zx_status_t FutexContext::FutexRequeue(user_in_ptr<const zx_futex_t> wake_ptr, uint32_t wake_count,
                                       int current_value, OwnerAction owner_action,
                                       user_in_ptr<const zx_futex_t> requeue_ptr,
                                       uint32_t requeue_count,
                                       zx_handle_t new_requeue_owner_handle) {
  LTRACE_ENTRY;
  zx_status_t result;

  // As we may need to perform fault resolution later, ensure our caller is not holding any locks.
  lockdep::AssertNoLocksHeld();

  // Make sure the futex pointers are following the basic rules.
  result = ValidateFutexPointer(wake_ptr);
  if (result != ZX_OK) {
    return result;
  }

  result = ValidateFutexPointer(requeue_ptr);
  if (result != ZX_OK) {
    return result;
  }

  FutexId wake_id(wake_ptr);
  FutexId requeue_id(requeue_ptr);

  if (wake_id == requeue_id) {
    return ZX_ERR_INVALID_ARGS;
  }

  // Validate the proposed new owner outside of any FutexState locks, but take
  // no action just yet.  See the comment in FutexWait for details.
  fbl::RefPtr<ThreadDispatcher> requeue_owner_thread;
  const zx_status_t validator_status =
      ValidateFutexOwner(new_requeue_owner_handle, &requeue_owner_thread);

  // Find the FutexState for the wake and requeue futexes.
  Guard<SpinLock, IrqSave> ref_lookup_guard{&pool_lock_};
  FutexState::PendingOpRef wake_futex_ref = ActivateFutexLocked(wake_id);
  FutexState::PendingOpRef requeue_futex_ref = ActivateFutexLocked(requeue_id);

  DEBUG_ASSERT(wake_futex_ref != nullptr);
  DEBUG_ASSERT(requeue_futex_ref != nullptr);

  // Manually release the ref lookup guard.  While we would typically do this
  // using scope, the PendingOpRefs need to live outside of just the locking
  // scope.  We cannot declare the PendingOpRefs outside of the scope because we
  // do not allow default construction of PendingOpRefs, nor do we allow move
  // assignment.  This is done on purpose; pending op refs should only ever be
  // constructed during lookup operations, and they really should not be moved
  // around.  We need to have a move constructor, but there is no reason for a
  // move assignment.
  ref_lookup_guard.Release();

  while (1) {
    // See the comment in FutexWait about the structure of lock ordering in this method.
    NullableDispatcherGuard requeue_owner_guard(requeue_owner_thread.get());
    AnnotatedAutoEagerReschedDisabler eager_resched_disabler;
    GuardMultiple<2, SpinLock, IrqSave> futex_guards{&wake_futex_ref->lock_,
                                                     &requeue_futex_ref->lock_};

    // Validate the futex storage state.
    int value;
    UserCopyCaptureFaultsResult copy_result = arch_copy_from_user_capture_faults(
        &value, wake_ptr.get(), sizeof(int), CopyContext::kBlockingNotAllowed);
    if (copy_result.status != ZX_OK) {
      // At this point we are committed to either returning from the function, or restarting the
      // loop, so we can drop the locks and resched disable that are local to this loop iteration.
      futex_guards.Release();
      requeue_owner_guard.Release();
      eager_resched_disabler.Enable();
      if (auto fault = copy_result.fault_info) {
        result = Thread::Current::SoftFault(fault->pf_va, fault->pf_flags);
        if (result != ZX_OK) {
          return result;
        }
        continue;
      }
      return copy_result.status;
    }

    if (value != current_value) {
      return ZX_ERR_BAD_STATE;
    }

    // If owner validation failed earlier, then bail out now (after we have passed the state check).
    if (validator_status != ZX_OK) {
      if (validator_status == ZX_ERR_BAD_HANDLE) {
        [[maybe_unused]] auto res =
            ProcessDispatcher::GetCurrent()->EnforceBasicPolicy(ZX_POL_BAD_HANDLE);
      }
      return validator_status;
    }

    Thread* new_requeue_owner = nullptr;
    if (requeue_owner_thread) {
      // Verify that the thread we are attempting to make the requeue target's
      // owner (if any) is not waiting on either the wake futex or the requeue
      // futex.
      if ((requeue_owner_thread->blocking_futex_id_ == wake_id) ||
          (requeue_owner_thread->blocking_futex_id_ == requeue_id)) {
        return ZX_ERR_INVALID_ARGS;
      }

      // We need to resolve |requeue_owner_thread|'s Thread*.  First, we must revalidate that
      // |requeue_owner_thread| is not dead or dying as it may have changed state during the
      // |SoftFault| call above.
      //
      // Once we've verified it's not dead or dying, we must ensure the Thread* remains valid.  See
      // related comment below just after |new_owner_guard| is released.
      if (!requeue_owner_thread->IsDyingOrDeadLocked()) {
        new_requeue_owner = requeue_owner_thread->core_thread_;
      }
    }

    // Now that all of our sanity checks are complete, it is time to do the
    // actual manipulation of the various wait queues.
    {
      DEBUG_ASSERT(wake_futex_ref != nullptr);

      // TODO(johngro): we are holding the proposed new owner's
      // ThreadDispatcher; do we need to, or is it possible to drop it?

      WakeHook wake_hook;
      RequeueHook requeue_hook{wake_futex_ref, requeue_futex_ref, requeue_id};
      OwnedWaitQueue::WakeOption wake_option = (owner_action == OwnerAction::RELEASE)
                                                   ? OwnedWaitQueue::WakeOption::None
                                                   : OwnedWaitQueue::WakeOption::AssignOwner;

      const OwnedWaitQueue::WakeThreadsResult wake_threads_result =
          wake_futex_ref->waiters_.WakeAndRequeue(requeue_futex_ref->waiters_, new_requeue_owner,
                                                  wake_count, requeue_count, wake_hook,
                                                  requeue_hook, wake_option);

      // Now, if we successfully woke any threads from the wake_futex, then we need
      // to adjust the number of references we are holding by that number of
      // threads.  They are on the hot-path out of FutexWake, and we are responsible
      // for their pending op refs.
      wake_futex_ref.SetExtraRefs(wake_threads_result.woken);
    }
    // If we got to here then we have no user copy faults that need retrying, so we should break out
    // of the infinite loop.
    break;
  }

  // Now just return.  The futex states will return to the pool as needed.
  return ZX_OK;
}

// Get the KOID of the current owner of the specified futex, if any, or ZX_KOID_INVALID if there
// is no known owner.
zx_status_t FutexContext::FutexGetOwner(user_in_ptr<const zx_futex_t> value_ptr,
                                        user_out_ptr<zx_koid_t> koid_out) {
  zx_status_t result;

  // Make sure the futex pointer is following the basic rules.
  result = ValidateFutexPointer(value_ptr);
  if (result != ZX_OK) {
    return result;
  }

  // Attempt to find the futex.  If it is not in the active set, then there is no owner.
  zx_koid_t koid = ZX_KOID_INVALID;
  FutexId futex_id(value_ptr);
  FutexState::PendingOpRef futex_ref = FindActiveFutex(futex_id);

  // We found a FutexState in the active set.  It may have an owner, but we need
  // to enter the queue's lock in order to check.
  if (futex_ref != nullptr) {
    {  // explicit lock scope
      SingleChainLockGuard guard{IrqSaveOption, futex_ref->waiters_.get_lock(),
                                 CLT_TAG("FutexContext::FutexGetOwner")};

      if (const Thread* owner = futex_ref->waiters_.owner(); owner != nullptr) {
        // Any thread which owns a FutexState's wait queue *must* be a
        // user mode thread.
        DEBUG_ASSERT(owner->user_thread() != nullptr);
        koid = owner->user_thread()->get_koid();
      }
    }
  }

  return koid_out.copy_to_user(koid);
}
