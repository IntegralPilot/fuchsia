// Copyright 2018 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "kernel/brwlock.h"

#include <lib/affine/ratio.h>
#include <lib/affine/utils.h>
#include <lib/arch/intrin.h>
#include <lib/kconcurrent/chainlock.h>
#include <lib/kconcurrent/chainlock_transaction.h>
#include <lib/zircon-internal/macros.h>

#include <kernel/auto_preempt_disabler.h>
#include <kernel/lock_trace.h>
#include <kernel/preempt_disabled_token.h>
#include <kernel/task_runtime_timers.h>
#include <ktl/limits.h>

#include <ktl/enforce.h>

namespace internal {

template <BrwLockEnablePi PI>
BrwLock<PI>::~BrwLock() {
  DEBUG_ASSERT(ktl::atomic_ref(state_.state_).load(ktl::memory_order_relaxed) == 0);
}

template <BrwLockEnablePi PI>
ktl::optional<BlockOpLockDetails<PI>> BrwLock<PI>::LockForBlock() {
  Thread* const current_thread = Thread::Current::Get();

  if constexpr (PI == BrwLockEnablePi::Yes) {
    Thread* const new_owner = ktl::atomic_ref(state_.writer_).load(ktl::memory_order_relaxed);
    OwnedWaitQueue::BAAOLockingDetails lock_details;
    if (wait_.TryLockForBAAOOperationLocked(current_thread, new_owner, lock_details)) {
      current_thread->get_lock().MarkReleased();
      return BlockOpLockDetails<PI>{lock_details};
    }
  } else {
    if (current_thread->get_lock().AcquireOrBackoff()) {
      current_thread->get_lock().MarkReleased();
      return BlockOpLockDetails<PI>{};
    }
  }

  return ktl::nullopt;
}

template <BrwLockEnablePi PI>
void BrwLock<PI>::Block(Thread* const current_thread, const BlockOpLockDetails<PI>& lock_details,
                        bool write) {
  zx_status_t ret;

  auto reason = write ? ResourceOwnership::Normal : ResourceOwnership::Reader;

  DEBUG_ASSERT(current_thread == Thread::Current::Get());
  const uint64_t flow_id = current_thread->TakeNextLockFlowId();
  LOCK_TRACE_FLOW_BEGIN("contend_rwlock", flow_id);

  if constexpr (PI == BrwLockEnablePi::Yes) {
    // Changes in ownership of node in a PI graph have the potential to affect
    // the running/runnable status of multiple threads at the same time.
    // Because of this, the OwnedWaitQueue class requires that we disable eager
    // rescheduling in order to optimize a situation where we might otherwise
    // send multiple (redundant) IPIs to the same CPUs during one PI graph
    // mutation.
    //
    // Note that this is an optimization, not a requirement.  What _is_ a
    // requirement is that we keep preemption disabled during the PI
    // propagation.  Currently, all of the invariants of OwnedWaitQueues and PI
    // graphs are protected by a single global "thread lock" which must be held
    // when a thread calls into the scheduler.  If a change to a PI graph would
    // cause the current scheduler to choose a different thread to run on that
    // CPU, however, the current thread will be preempted, and the ownership of
    // the thread lock will be transferred to the newly selected thread.  As the
    // new thread unwinds, it is going to drop the thread lock and return to
    // executing, leaving the first thread in the middle of what was supposed to
    // be an atomic operation.
    //
    // Because if this, it is critically important that local preemption be
    // disabled (at a minimum) when mutating a PI graph.  In the case that a
    // thread eventually blocks, the OWQ code will make sure that all invariants
    // will be restored before the thread finally blocks (and eventually wakes
    // and unwinds).
    AnnotatedAutoEagerReschedDisabler eager_resched_disabler;
    ret = wait_.BlockAndAssignOwnerLocked(current_thread, Deadline::infinite(), lock_details,
                                          reason, Interruptible::No);
    current_thread->get_lock().Release();
  } else {
    AnnotatedAutoPreemptDisabler preempt_disable;
    ret = wait_.BlockEtc(current_thread, Deadline::infinite(), 0, reason, Interruptible::No);
    current_thread->get_lock().Release();
  }

  if (unlikely(ret < ZX_OK)) {
    panic(
        "BrwLock<%d>::Block: Block returned with error %d lock %p, thr %p, "
        "sp %p\n",
        static_cast<bool>(PI), ret, this, Thread::Current::Get(), __GET_FRAME());
  }
}

// A callback class used to select which threads we want to wake during a wake
// operation. Generally, if the first waiter is a writer, we wake just the
// writer and assign ownership to them. Otherwise, we wake as many readers as we
// can and stop either when we run out, or when we encounter the first writer.
class WakeOpHooks : public OwnedWaitQueue::IWakeRequeueHook {
 public:
  enum class Operation { None, WakeWriter, WakeReaders };
  WakeOpHooks() = default;

  // This hook is called during the locking phase of the wake operation, and
  // allows us to determine which threads we want to wake based on their current
  // state.  Returning true selects a thread for waking, and continues the
  // enumeration of threads (if any).  Returning false will stop the
  // enumeration.
  //
  // Note that we have not committed to waking any threads yet.  We first need
  // to obtain all of the locks needed for the wake operation.  If we fail to
  // obtain any of the locks, we are going to need to back off from the wake
  // operation and try again after releasing our queue lock.
  bool Allow(const Thread& t) TA_REQ_SHARED(t.get_lock()) override {
    // We have not considered any threads yet, so we don't know if we are going
    // to be waking a single writer, or a set of readers.  Figure this out now.
    if (op_ == Operation::None) {
      // If the first thread is blocked for writing, then we are waking a single
      // writer.  Record this and approve this thread.
      if (t.state() == THREAD_BLOCKED) {
        op_ = Operation::WakeWriter;
        return true;
      }

      // If not writing then we must be blocked for reading
      DEBUG_ASSERT(t.state() == THREAD_BLOCKED_READ_LOCK);
      op_ = Operation::WakeReaders;
    }

    // If we are waking readers, and the next thread is a reader, accept it,
    // otherwise stop.
    if (op_ == Operation::WakeReaders) {
      return (t.state() == THREAD_BLOCKED_READ_LOCK);
    }

    // We must be waking a single writer.  Reject the next thread
    // and stop the enumeration.
    DEBUG_ASSERT(op_ == Operation::WakeWriter);
    return false;
  }

  Operation op() const { return op_; }

 private:
  Operation op_{Operation::None};
};

template <BrwLockEnablePi PI>
ktl::optional<ResourceOwnership> BrwLock<PI>::TryWake() {
  // If there is no one to wake by the time we make it into the queue's lock, we
  // should fail the operation, signaling to the caller that they should retry.
  if (wait_.IsEmpty()) {
    return ktl::nullopt;
  }

  if constexpr (PI == BrwLockEnablePi::Yes) {
    WakeOpHooks hooks{};
    ktl::optional<Thread::UnblockList> maybe_unblock_list =
        wait_.LockForWakeOperationLocked(ktl::numeric_limits<uint32_t>::max(), hooks);

    // We get back a list only if we succeeded in locking all of the threads we
    // want to wake. Otherwise, we need to back off and try again.
    if (!maybe_unblock_list.has_value()) {
      return ktl::nullopt;
    }

    // Great, we have locked all of the threads we need to lock.
    ChainLockTransaction::ActiveRef().Finalize();

    // Fix up our state, granting the lock to the set of readers, or single
    // writer, we are going to wake  before we actually perform the wake
    // operation.
    DEBUG_ASSERT(hooks.op() != WakeOpHooks::Operation::None);
    Thread::UnblockList& unblock_list = maybe_unblock_list.value();
    const OwnedWaitQueue::WakeOption wake_opt = (hooks.op() == WakeOpHooks::Operation::WakeWriter)
                                                    ? OwnedWaitQueue::WakeOption::AssignOwner
                                                    : OwnedWaitQueue::WakeOption::None;

    // TODO(johngro): optimize this; we should not be doing O(n) counts here, especially since we
    // already did one while obtaining the locks.
    uint32_t to_wake = static_cast<uint32_t>(unblock_list.size_slow());
    if (hooks.op() == WakeOpHooks::Operation::WakeWriter) {
      DEBUG_ASSERT(to_wake == 1);
      Thread* const new_owner = &unblock_list.front();

      ktl::atomic_ref(state_.writer_).store(new_owner, ktl::memory_order_relaxed);
      ktl::atomic_ref(state_.state_)
          .fetch_add(-kBrwLockWaiter + kBrwLockWriter, ktl::memory_order_acq_rel);
    } else {
      DEBUG_ASSERT(hooks.op() == WakeOpHooks::Operation::WakeReaders);
      DEBUG_ASSERT(to_wake >= 1);
      ktl::atomic_ref(state_.state_)
          .fetch_add((-kBrwLockWaiter + kBrwLockReader) * to_wake, ktl::memory_order_acq_rel);
    }

    for (const Thread& thread : unblock_list) {
      LOCK_TRACE_FLOW_END("contend_rwlock", thread.lock_flow_id());
    }

    // Now actually do the wake.
    [[maybe_unused]] const OwnedWaitQueue::WakeThreadsResult results =
        wait_.WakeThreadsLocked(ktl::move(maybe_unblock_list).value(), hooks, wake_opt);
    if (hooks.op() == WakeOpHooks::Operation::WakeWriter) {
      DEBUG_ASSERT(results.woken == 1);
      DEBUG_ASSERT(((results.still_waiting == 0) && (results.owner == nullptr)) ||
                   (results.owner != nullptr));
      return ResourceOwnership::Normal;
    } else {
      DEBUG_ASSERT(hooks.op() == WakeOpHooks::Operation::WakeReaders);
      DEBUG_ASSERT(results.woken >= 1);
      DEBUG_ASSERT(results.owner == nullptr);
      return ResourceOwnership::Reader;
    }
  } else {
    // Try to lock the set of threads we need to wake.  Either the next writer,
    // or the next contiguous span of readers.
    ktl::optional<BrwLockOps::LockForWakeResult> lock_result =
        BrwLockOps::LockForWake(wait_, current_mono_time());

    // If we failed to lock, backoff and try again.
    if (!lock_result.has_value()) {
      return ktl::nullopt;
    }

    // Great, we have locked all of the threads we need to lock.  Finalize our
    // transaction, then update our bookkeeping and send the threads off to the
    // scheduler to finish unblocking.
    ChainLockTransaction::ActiveRef().Finalize();
    DEBUG_ASSERT(lock_result->count > 0);
    DEBUG_ASSERT(!lock_result->list.is_empty());

    for (const Thread& thread : lock_result->list) {
      LOCK_TRACE_FLOW_END("contend_rwlock", thread.lock_flow_id());
    }

    Thread& first_wake = lock_result->list.front();
    first_wake.get_lock().AssertHeld();

    if (first_wake.state() == THREAD_BLOCKED_READ_LOCK) {
      ktl::atomic_ref(state_.state_)
          .fetch_add((-kBrwLockWaiter + kBrwLockReader) * lock_result->count,
                     ktl::memory_order_acq_rel);
      Scheduler::Unblock(ktl::move(lock_result->list));
      return ResourceOwnership::Reader;
    } else {
      DEBUG_ASSERT(first_wake.state() == THREAD_BLOCKED);
      DEBUG_ASSERT(lock_result->count == 1);
      ktl::atomic_ref(state_.state_)
          .fetch_add(-kBrwLockWaiter + kBrwLockWriter, ktl::memory_order_acq_rel);
      Scheduler::Unblock(ktl::move(lock_result->list));
      return ResourceOwnership::Normal;
    }
  }
}

template <BrwLockEnablePi PI>
void BrwLock<PI>::ContendedReadAcquire() {
  LOCK_TRACE_DURATION("ContendedReadAcquire", ("name", class_name_ref()));

  // Remember the last call to current_ticks.
  zx_instant_mono_ticks_t now_ticks = current_mono_ticks();
  Thread* const current_thread = Thread::Current::Get();
  ContentionTimer timer(current_thread, now_ticks);

  const zx_duration_t spin_max_duration = Mutex::SPIN_MAX_DURATION;
  const affine::Ratio time_to_ticks = timer_get_ticks_to_time_ratio().Inverse();
  const zx_instant_mono_ticks_t spin_until_ticks =
      affine::utils::ClampAdd(now_ticks, time_to_ticks.Scale(spin_max_duration));

  do {
    const uint64_t state = ktl::atomic_ref(state_.state_).load(ktl::memory_order_acquire);

    // If there are any waiters, implying another thread exhausted its spin phase on the same lock,
    // break out of the spin phase early.
    if (StateHasWaiters(state)) {
      break;
    }

    // If there are only readers now, return holding the lock for read, leaving the optimistic
    // reader count in place.
    if (!StateHasWriter(state)) {
      return;
    }

    // Give the arch a chance to relax the CPU.
    arch::Yield();
    now_ticks = current_mono_ticks();
  } while (now_ticks < spin_until_ticks);

  // Enter our wait queue's lock and figure out what to do next.  We don't really know what we need
  // to do yet, so we need to be prepared for needing to back off and try again.
  const auto do_transaction = [&]()
                                  TA_REQ(chainlock_transaction_token,
                                         preempt_disabled_token) -> ChainLockTransaction::Result<> {
    wait_.get_lock().AcquireFirstInChain();

    auto TryBlock = [&]() TA_REQ(chainlock_transaction_token) TA_REL(wait_.get_lock()) -> bool {
      ktl::optional<BlockOpLockDetails<PI>> maybe_lock_details = LockForBlock();

      if (maybe_lock_details.has_value()) {
        ChainLockTransaction::Finalize();
        current_thread->get_lock().AssertAcquired();
        Block(current_thread, ktl::move(maybe_lock_details).value(), false);
        return true;
      } else {
        // Back off and try again.
        wait_.get_lock().Release();
        return false;
      }
    };

    constexpr uint64_t kReaderToWaiter = kBrwLockWaiter - kBrwLockReader;
    constexpr uint64_t kWaiterToReader = kBrwLockReader - kBrwLockWaiter;

    // Before blocking, remove our optimistic reader from the count,
    // and put a waiter on there instead.
    const uint64_t prev =
        ktl::atomic_ref(state_.state_).fetch_add(kReaderToWaiter, ktl::memory_order_relaxed);

    if (StateHasWriter(prev)) {
      // Try to grab all of our needed locks and block.  If we succeed, return, otherwise loop
      // around and try again.
      if (TryBlock()) {
        return ChainLockTransaction::Done;
      }
      // Make sure to convert our waiter back to an optimistic reader
      ktl::atomic_ref(state_.state_).fetch_add(kWaiterToReader, ktl::memory_order_relaxed);
      return ChainLockTransaction::Action::Backoff;
    }

    // If we raced and there is in fact no one waiting then we can switch to
    // having the lock
    if (!StateHasWaiters(prev)) {
      ktl::atomic_ref(state_.state_).fetch_add(kWaiterToReader, ktl::memory_order_relaxed);
      wait_.get_lock().Release();
      return ChainLockTransaction::Done;
    }

    // If there are no current readers then we need to wake somebody up
    if (StateReaderCount(prev) == 1) {
      ktl::optional<ResourceOwnership> maybe_ownership = TryWake();
      const bool woke_someone{maybe_ownership.has_value()};

      // Convert our waiter count back to a reader count.  If we woke a writer instead of a reader,
      // then this is an "optimistic" count, and we need to drop our locks before looping around to
      // try again.  Otherwise, if we woke readers, then we are now a member of the reader pool and
      // can return.
      // drop the locks and loop back around to try again.
      if (woke_someone) {
        if (maybe_ownership.value() == ResourceOwnership::Reader) {
          ktl::atomic_ref(state_.state_).fetch_add(kWaiterToReader, ktl::memory_order_acquire);
          wait_.get_lock().Release();
          return ChainLockTransaction::Done;
        }
      }

      // Need to back off and try again.  If we successfully woke someone, then
      // our transaction was finalized and we need restart it before going
      // again.
      ktl::atomic_ref(state_.state_).fetch_add(kWaiterToReader, ktl::memory_order_relaxed);
      wait_.get_lock().Release();
      if (woke_someone) {
        // clt.Restart(CLT_TAG("BrwLock<PI>::ContendedReadAcquire (restart)"));
        return ChainLockTransaction::Action::Restart;
      }
      return ChainLockTransaction::Action::Backoff;
    }

    // So, at this point, we know that when we converted our optimistic reader count to a writer,
    // the state which was there indicated
    //
    // ++ There were no writers.
    // ++ There were waiters (or at least threads trying to block).
    // ++ There was at least one other thread trying to become a reader (their count is optimistic).
    //
    // Try to join the blocking pool.  Eventually the final optimistic reader should attempt to wake
    // the threads who have actually managed to block, waking either a writer or a set of readers as
    // needed.

    if (TryBlock()) {
      return ChainLockTransaction::Done;
    }

    // We failed to block, restore our optimistic reader count, drop our locks, and try again.
    ktl::atomic_ref(state_.state_).fetch_add(kWaiterToReader, ktl::memory_order_relaxed);
    return ChainLockTransaction::Action::Backoff;
  };
  ChainLockTransaction::UntilDone(EagerReschedDisableAndIrqSaveOption,
                                  CLT_TAG("BrwLock<PI>::ContendedReadAcquire"), do_transaction);
}

template <BrwLockEnablePi PI>
void BrwLock<PI>::ContendedWriteAcquire() {
  LOCK_TRACE_DURATION("ContendedWriteAcquire", ("name", class_name_ref()));

  // Remember the last call to current_ticks.
  zx_instant_mono_ticks_t now_ticks = current_mono_ticks();
  Thread* current_thread = Thread::Current::Get();
  ContentionTimer timer(current_thread, now_ticks);

  const zx_duration_t spin_max_duration = Mutex::SPIN_MAX_DURATION;
  const affine::Ratio time_to_ticks = timer_get_ticks_to_time_ratio().Inverse();
  const zx_instant_mono_ticks_t spin_until_ticks =
      affine::utils::ClampAdd(now_ticks, time_to_ticks.Scale(spin_max_duration));

  do {
    AcquireResult result = AtomicWriteAcquire(kBrwLockUnlocked, current_thread);

    // Acquire succeeded, return holding the lock.
    if (result) {
      return;
    }

    // If there are any waiters, implying another thread exhausted its spin phase on the same lock,
    // break out of the spin phase early.
    if (StateHasWaiters(result.state)) {
      break;
    }

    // Give the arch a chance to relax the CPU.
    arch::Yield();
    now_ticks = current_mono_ticks();
  } while (now_ticks < spin_until_ticks);

  // Enter our wait queue's lock and figure out what to do next.  We don't really know what we need
  // to do yet, so we need to be prepared for needing to back off and try again.
  const auto do_transaction = [&]()
                                  TA_REQ(chainlock_transaction_token,
                                         preempt_disabled_token) -> ChainLockTransaction::Result<> {
    wait_.get_lock().AcquireFirstInChain();

    auto TryBlock = [&]() TA_REQ(chainlock_transaction_token) TA_REL(wait_.get_lock()) -> bool {
      ktl::optional<BlockOpLockDetails<PI>> maybe_lock_details = LockForBlock();

      if (maybe_lock_details.has_value()) {
        // We got all of the locks we need, and are already marked as waiting.
        // Drop into the block operation, when we wake again, the lock should
        // have been assigned to us.
        ChainLockTransaction::Finalize();
        current_thread->get_lock().AssertAcquired();
        Block(current_thread, ktl::move(maybe_lock_details).value(), true);
        return true;
      } else {
        // Remove our speculative waiter count, then back off and try again.
        ktl::atomic_ref(state_.state_).fetch_sub(kBrwLockWaiter, ktl::memory_order_relaxed);
        wait_.get_lock().Release();
        return false;
      }
    };

    // There were either readers or writers just as we were attempting to
    // acquire the lock.  Now that we are on the slow path and have obtained the
    // queue lock, mark ourselves as a speculative waiter, then re-examine what
    // the state of the BRW lock was as we did so.
    const uint64_t prev =
        ktl::atomic_ref(state_.state_).fetch_add(kBrwLockWaiter, ktl::memory_order_relaxed);

    // If there were still readers or writers when we marked ourselves as
    // waiting, we need to try to block.  If we fail to block (because of a
    // locking conflict), we need to remove our speculative waiter count and try
    // again.  TryBlock has already dropped the queue lock.
    if (StateHasWriter(prev) || StateHasReaders(prev)) {
      if (TryBlock()) {
        return ChainLockTransaction::Done;
      }
      return ChainLockTransaction::Action::Backoff;
    }

    // OK, there were no readers, no writers.  Were there waiters when we tried
    // to add ourselves to the waiter pool?
    if (!StateHasWaiters(prev)) {
      // There were no waiters, we are done acquiring locks.
      ChainLockTransaction::Finalize();

      // Since we have flagged ourselves as a speculative waiter, no one else
      // can currently grab the lock for write without following the slow path
      // and obtaining the queue lock, which we currently hold.  So, we are free
      // to simply declare ourselves the owner of the BRW lock, convert our
      // waiter status to writer, then get out.
      if constexpr (PI == BrwLockEnablePi::Yes) {
        ktl::atomic_ref(state_.writer_).store(Thread::Current::Get(), ktl::memory_order_relaxed);
      }
      ktl::atomic_ref(state_.state_)
          .fetch_add(kBrwLockWriter - kBrwLockWaiter, ktl::memory_order_acq_rel);
      wait_.get_lock().Release();
      return ChainLockTransaction::Done;
    }

    // OK, it looks like there were no readers or writers, but there were
    // waiters who were not us.
    if (TryWake().has_value()) {
      // We succeeded in waking one or more threads, giving them the lock in the
      // process.  Since someone else now owns the lock we need to try to block.
      // If we succeed, then when we wake up, we should have been granted the
      // lock and can just return.
      //
      // Note that TryWake finalized our transaction after it succeeded in
      // obtaining the locks needed to wake up a thread.  We are still holding
      // the wait queue's lock, but we need to restart the transaction before
      // attempting to block (which will require holding a new set of locks).
      ChainLockTransaction::ActiveRef().Restart(
          CLT_TAG("BrwLock<PI>::ContendedWriteAcquire (restart)"));
      if (TryBlock()) {
        return ChainLockTransaction::Done;
      }

      // We didn't manage to block, but TryBlock has removed our speculative
      // wait count and dropped the queue lock for us, we don't need to do it
      // here.
    } else {
      // We failed to wake anyone, remove our speculative wait count, drop the
      // queue lock, and try again.
      ktl::atomic_ref(state_.state_).fetch_sub(kBrwLockWaiter, ktl::memory_order_relaxed);
      wait_.get_lock().Release();
    }
    return ChainLockTransaction::Action::Backoff;
  };
  ChainLockTransaction::UntilDone(PreemptDisableAndIrqSaveOption,
                                  CLT_TAG("BrwLock<PI>::ContendedWriteAcquire"), do_transaction);
}

template <BrwLockEnablePi PI>
void BrwLock<PI>::WriteRelease() {
  canary_.Assert();

#if LK_DEBUGLEVEL > 0
  if constexpr (PI == BrwLockEnablePi::Yes) {
    Thread* holder = ktl::atomic_ref(state_.writer_).load(ktl::memory_order_relaxed);
    Thread* ct = Thread::Current::Get();
    if (unlikely(ct != holder)) {
      panic(
          "BrwLock<PI>::WriteRelease: thread %p (%s) tried to release brwlock %p it "
          "doesn't "
          "own. Ownedby %p (%s)\n",
          ct, ct->name(), this, holder, holder ? holder->name() : "none");
    }
  }
#endif

  // For correct PI handling we need to ensure that up until a higher priority
  // thread can acquire the lock we will correctly be considered the owner.
  // Other threads are able to acquire the lock *after* we call ReleaseWakeup,
  // prior to that we could be racing with a higher priority acquirer and it
  // could be our responsibility to wake them up, and so up until ReleaseWakeup
  // is called they must be able to observe us as the owner.
  //
  // If we hold off on changing writer_ till after ReleaseWakeup we will then be
  // racing with others who may be acquiring, or be granted the write lock in
  // ReleaseWakeup, and so we would have to CAS writer_ to not clobber the new
  // holder. CAS is much more expensive than just a 'store', so to avoid that
  // we instead disable preemption. Disabling preemption effectively gives us the
  // highest priority, and so it is fine if acquirers observe writer_ to be null
  // and 'fail' to treat us as the owner.
  if constexpr (PI == BrwLockEnablePi::Yes) {
    Thread::Current::preemption_state().PreemptDisable();

    ktl::atomic_ref(state_.writer_).store(nullptr, ktl::memory_order_relaxed);
  }
  uint64_t prev =
      ktl::atomic_ref(state_.state_).fetch_sub(kBrwLockWriter, ktl::memory_order_release);

  if (unlikely(StateHasWaiters(prev))) {
    LOCK_TRACE_DURATION("ContendedWriteRelease", ("name", class_name_ref()));
    // There are waiters, we need to wake them up
    ReleaseWakeup();
  }

  if constexpr (PI == BrwLockEnablePi::Yes) {
    Thread::Current::preemption_state().PreemptReenable();
  }
}

template <BrwLockEnablePi PI>
void BrwLock<PI>::ReleaseWakeup() {
  // Don't reschedule whilst we're waking up all the threads as if there are
  // several readers available then we'd like to get them all out of the wait
  // queue.
  //
  // See the comment in BrwLock<PI>::Block for why this eager reschedule
  // disabled is important.
  const auto do_transaction = [&]()
                                  TA_REQ(chainlock_transaction_token,
                                         preempt_disabled_token) -> ChainLockTransaction::Result<> {
    bool finished{false};
    wait_.get_lock().AcquireFirstInChain();

    const uint64_t count = ktl::atomic_ref(state_.state_).load(ktl::memory_order_relaxed);
    if (StateHasWaiters(count) && !StateHasWriter(count) && !StateHasReaders(count)) {
      // If TryWake succeeds, it will finalize our transaction for us once it
      // obtains all of the locks it needs.
      finished = TryWake().has_value();
    } else {
      ChainLockTransaction::Finalize();
      finished = true;
    }

    wait_.get_lock().Release();
    if (finished) {
      return ChainLockTransaction::Done;
    }

    return ChainLockTransaction::Action::Backoff;
  };
  ChainLockTransaction::UntilDone(EagerReschedDisableAndIrqSaveOption,
                                  CLT_TAG("BrwLock<PI>::ContendedReadAcquire"), do_transaction);
}

template class BrwLock<BrwLockEnablePi::Yes>;
template class BrwLock<BrwLockEnablePi::No>;

}  // namespace internal
