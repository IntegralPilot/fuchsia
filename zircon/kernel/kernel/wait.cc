// Copyright 2016 The Fuchsia Authors
// Copyright (c) 2008-2015 Travis Geiselbrecht
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "kernel/wait.h"

#include <lib/fit/defer.h>
#include <lib/kconcurrent/chainlock.h>
#include <lib/kconcurrent/chainlock_transaction.h>
#include <lib/ktrace.h>
#include <platform.h>
#include <trace.h>
#include <zircon/errors.h>

#include <kernel/auto_preempt_disabler.h>
#include <kernel/owned_wait_queue.h>
#include <kernel/scheduler.h>
#include <kernel/thread.h>
#include <kernel/timer.h>
#include <kernel/wait_queue_internal.h>
#include <ktl/algorithm.h>
#include <ktl/utility.h>

#include <ktl/enforce.h>

#define LOCAL_TRACE 0

#ifndef WAIT_QUEUE_DEPTH_TRACING_ENABLED
#define WAIT_QUEUE_DEPTH_TRACING_ENABLED false
#endif

// Tell the static analyzer that we are allowed to read variables protected by a
// thread's lock, but don't actually perform any runtime checks. Sadly, the
// encapsulation and separation of the WaitQueueCollection from the actual
// WaitQueue makes it difficult to provide any stronger runtime checks.
//
// This is almost certainly *not* the function you should be looking for or
// using.  It is used exclusively by WaitQueueCollection code which needs to be
// able to examine the state of threads which are current in the collection
// itself.
//
// See the definition of MarkInWaitQueue for details, but the idea here is it
// is only safe to examine this state (without actually holding the thread's
// lock) while the thread exists in a wait queue, and only when holding the wait
// queue's lock.  It is *not* safe to mutate and of the thread's guarded members
// without actually holding the thread's lock.  In the limited number of places
// where we use this method, it should be obvious from context that the thread
// itself is a member of the wait queue (typically, it is used while enumerating
// the current members of the queue).
inline void MarkInWaitQueue(const Thread& t) TA_ASSERT_SHARED(t.get_lock()) {}

static inline void WqTraceDepth(const WaitQueueCollection* collection, uint32_t depth) {
  if constexpr (WAIT_QUEUE_DEPTH_TRACING_ENABLED) {
    KTrace::Probe(KTrace::Context::Cpu, "wq_depth"_intern, reinterpret_cast<uint64_t>(collection),
                  static_cast<uint64_t>(depth));
  }
}

// add expensive code to do a full validation of the wait queue at various entry points
// to this module.
#define WAIT_QUEUE_VALIDATION (0 || (LK_DEBUGLEVEL > 2))

// Wait queues come in 2 flavors (traditional and owned) which are distinguished
// using the magic number.  When DEBUG_ASSERT checking the magic number, check
// against both of the possible valid magic numbers.
#define DEBUG_ASSERT_MAGIC_CHECK(_queue)                                  \
  DEBUG_ASSERT_MSG(((_queue)->magic_ == WaitQueue::kMagic) ||             \
                       ((_queue)->magic_ == OwnedWaitQueue::kOwnedMagic), \
                   "magic 0x%08x", ((_queue)->magic_))

// There are a limited number of operations which should never be done on a
// WaitQueue which happens to be an OwnedWaitQueue.  Specifically, blocking.
// Blocking on an OWQ should always go through the OWQ specific
// BlockAndAssignOwner.  Add a macro to check for that as well.
#define DEBUG_ASSERT_MAGIC_AND_NOT_OWQ(_queue)                                                     \
  do {                                                                                             \
    DEBUG_ASSERT_MSG(((_queue)->magic_ != OwnedWaitQueue::kOwnedMagic),                            \
                     "This operation should not be performed against the WaitQueue "               \
                     "API, use the OwnedWaitQueue API intead.");                                   \
    DEBUG_ASSERT_MSG(((_queue)->magic_ == WaitQueue::kMagic), "magic 0x%08x", ((_queue)->magic_)); \
  } while (false)

// Wait queues are building blocks that other locking primitives use to handle
// blocking threads.
void WaitQueueBase::TimeoutHandler(Timer* timer, zx_instant_mono_t now, void* arg) {
  Thread& thread = *(static_cast<Thread*>(arg));
  thread.canary().Assert();

  // In order to wake the thread with a TIMED_OUT error, we need to hold the
  // entire PI chain starting from the thread, and ending at the target of the
  // PI graph that the thread is blocked in.  It is possible, however, that the
  // thread has already woken up and is attempting to cancel the timeout
  // handler.  So, use the Timer's TryLockOrCancel to lock the first node in the
  // chain (the thread), being prepared to abort the operation if we are getting
  // canceled.  Then proceed to lock the rest of the chain, backing off and
  // retrying as needed.
  //
  // TODO(johngro): Optimize the case where the thread being unblocked has a
  // non-inheritable profile.  We don't need to lock the entire PI chain if
  // there will be no PI consequences to deal with.  It should be sufficient to
  // simply lock the thread and the wait queue following it, and remove the
  // thread from the queue.
  const auto try_lock_or_cancel = [timer](ChainLock& lock) TA_TRY_ACQ(true, lock) {
    return timer->TrylockOrCancel(lock) == ZX_OK;
  };

  // If the timeout has not been cancelled, handle the following:
  // 1. The thread is still sitting in its wait queue, it has not been
  //    explicitly unblocked yet. Its state must be either BLOCKED or
  //    BLOCKED_READ_LOCK. Finish locking, and then unblock the thread.
  // 2. The thread has been explicitly unblocked, but it has not become
  //    scheduled yet. The thread will have no blocking wait queue, and its
  //    state must be READY. It will eventually become scheduled, and as it
  //    unwinds, it will attempt to cancel this timer (which is currently on
  //    its stack).
  const auto do_transaction = [&]()
                                  TA_REQ(chainlock_transaction_token,
                                         preempt_disabled_token) -> ChainLockTransaction::Result<> {
    ChainLockGuard thread_guard{thread.get_lock(), ChainLockGuard::Defer};
    if (!thread_guard.TryAcquireWith(try_lock_or_cancel)) {
      return ChainLockTransaction::Done;
    }

    WaitQueueBase* wq = thread.wait_queue_state().blocking_wait_queue();
    if (wq == nullptr) {
      DEBUG_ASSERT(thread.state() == THREAD_READY);
      return ChainLockTransaction::Done;
    }

    DEBUG_ASSERT((thread.state() == THREAD_BLOCKED) ||
                 (thread.state() == THREAD_BLOCKED_READ_LOCK));
    if (OwnedWaitQueue::LockPiChain(thread) == ChainLock::Result::Backoff) {
      return ChainLockTransaction::Action::Backoff;
    }
    wq->get_lock().AssertAcquired();

    ChainLockTransaction::Finalize();
    thread_guard.Unguard();
    wq->UnblockThread(&thread, ZX_ERR_TIMED_OUT);
    return ChainLockTransaction::Done;
  };
  ChainLockTransaction::UntilDone(PreemptDisableAndIrqSaveOption,
                                  CLT_TAG("WaitQueue::TimeoutHandler"), do_transaction);
}

// Remove a thread from a wait queue, maintain the wait queue's internal count,
// and update the WaitQueue specific bookkeeping in the thread in the process.
void WaitQueueBase::Dequeue(Thread* t, zx_status_t wait_queue_error) {
  DEBUG_ASSERT(t != nullptr);
  AssertInWaitQueue(*t, *this);

  collection_.Remove(t);
  t->wait_queue_state().blocked_status_ = wait_queue_error;
  t->wait_queue_state().blocking_wait_queue_ = nullptr;
}

SchedDuration WaitQueueCollection::MinInheritableRelativeDeadline() const {
  if (threads_.is_empty()) {
    return SchedDuration::Max();
  }

  const Thread& root_thread = *threads_.root();
  MarkInWaitQueue(root_thread);

  return root_thread.wait_queue_state().subtree_min_deadline_;
}

void WaitQueueCollection::Insert(Thread* thread) {
  WqTraceDepth(this, Count() + 1);

  WaitQueueCollection::ThreadState& wq_state = thread->wait_queue_state();
  DEBUG_ASSERT(wq_state.blocked_threads_tree_sort_key_ == kPrimaryKeyZero);
  DEBUG_ASSERT(wq_state.subtree_min_deadline_ == SchedDuration::Max());

  const auto& sched_state = thread->scheduler_state();
  wq_state.blocked_threads_tree_sort_key_ = sched_state.finish_time();
  threads_.insert(thread);
}

void WaitQueueCollection::Remove(Thread* thread) {
  WqTraceDepth(this, Count() - 1);
  threads_.erase(*thread);

  // In a debug build, zero out the sort key now that we have left the
  // collection.  This can help to find bugs by allowing us to assert that the
  // value is zero during insertion, however it is not strictly needed in a
  // production build and can be skipped.
  WaitQueueCollection::ThreadState& wq_state = thread->wait_queue_state();
#ifdef DEBUG_ASSERT_IMPLEMENTED
  wq_state.blocked_threads_tree_sort_key_ = kPrimaryKeyZero;
#endif
}

ChainLock::Result WaitQueueCollection::LockAll() {
  for (Thread& t : threads_) {
    // If we hit a conflict, we need to unlock everything and start again,
    // unwinding completely out of this function as we go.
    //
    // Note that this method relies on the enumeration order of the collection
    // being deterministic, which should always be the case for a fbl::WAVLTree.
    if (!t.get_lock().AcquireOrBackoff()) {
      for (Thread& unlock_me : threads_) {
        if (unlock_me.get_lock().MarkNeedsReleaseIfHeld()) {
          unlock_me.get_lock().Release();
        } else {
          break;
        }
      }
      return ChainLock::Result::Backoff;
    }
    t.get_lock().MarkReleased();
  }

  return ChainLock::Result::Ok;
}

void WaitQueueBase::ValidateQueue() {
  DEBUG_ASSERT_MAGIC_CHECK(this);
  collection_.Validate();
}

////////////////////////////////////////////////////////////////////////////////
//
// Begin user facing API
//
////////////////////////////////////////////////////////////////////////////////

/**
 * @brief  Block until a wait queue is notified, ignoring existing signals
 *         in |signal_mask|.
 *
 * This function puts the current thread at the end of a wait
 * queue and then blocks until some other thread wakes the queue
 * up again.
 *
 * @param  deadline       The time at which to abort the wait
 * @param  slack          The amount of time it is acceptable to deviate from deadline
 * @param  signal_mask    Mask of existing signals to ignore
 * @param  reason         Reason for the block
 * @param  interruptible  Whether the block can be interrupted
 *
 * If the deadline is zero, this function returns immediately with
 * ZX_ERR_TIMED_OUT.  If the deadline is ZX_TIME_INFINITE, this function
 * waits indefinitely.  Otherwise, this function returns with
 * ZX_ERR_TIMED_OUT when the deadline elapses.
 *
 * @return ZX_ERR_TIMED_OUT on timeout, else returns the return
 * value specified when the queue was woken by wait_queue_wake_one().
 */
zx_status_t WaitQueue::BlockEtc(Thread* const current_thread, const Deadline& deadline,
                                uint signal_mask, ResourceOwnership reason,
                                Interruptible interruptible) {
  DEBUG_ASSERT(current_thread == Thread::Current::Get());
  DEBUG_ASSERT_MAGIC_AND_NOT_OWQ(this);
  DEBUG_ASSERT(current_thread->state() == THREAD_RUNNING);

  if (WAIT_QUEUE_VALIDATION) {
    ValidateQueue();
  }

  zx_status_t res = BlockEtcPreamble(current_thread, deadline, signal_mask, reason, interruptible);

  // After BlockEtcPreamble has completed, we need to drop the WaitQueue's lock.
  // At this point, we are committed.  Either the thread successfully joined the
  // wait queue and needs to descend into the scheduler to actually block
  // (handled by BlockEtcPostamble), or it failed to join the queue (because of
  // a timeout, or something similar).  Either way, we should no longer be
  // holding the queue lock.
  get_lock().Release();

  // If we failed to join the queue, propagate the error up (while still holding
  // the thread's lock).  Otherwise, enter the scheduler while holding the
  // thread's lock.  The scheduler will drop the thread's lock during the final
  // context switch, and will re-acquire it once the thread becomes runnable
  // again and unwinds.
  if (res != ZX_OK) {
    return res;
  }

  return BlockEtcPostamble(current_thread, deadline);
}

/**
 * @brief  Wake up one thread sleeping on a wait queue
 *
 * This function removes one thread (if any) from the head of the wait queue and
 * makes it executable.  The new thread will be placed in the run queue.
 *
 * @param wait_queue_error  The return value which the new thread will receive
 * from wait_queue_block().
 *
 * @return  Whether a thread was woken
 */
bool WaitQueue::WakeOne(zx_status_t wait_queue_error) {
  // In order to wake the next thread from the WaitQueue, we need to hold two
  // locks.  First we must hold the WaitQueue's lock in order to select the
  // "best" thread to wake, and then we need to obtain the best thread's lock in
  // order to actually remove it from the queue and transfer it to a scheduler
  // via Scheduler::Unblock.
  //
  // Note that we explicitly disable local rescheduling using a preempt-disabler
  // before calling into the scheduler.  This is not just an optimization; we
  // can't be holding the local thread's lock if/when a local reschedule needs
  // to happen as this would set up a situation where we were holding the woken
  // thread's lock, and then needed to be holding the current thread's lock as
  // well. This is not an easy thing to do; we would have had to lock the
  // current thread as part of the lock chain before making any changes to the
  // actual state.
  //
  // Turning off local preemption before obtaining any locks avoids this issue.
  // We can now call into the scheduler holding whatever locks we want (fewer is
  // better, however), confident that there will be no local rescheduling until
  // after the preempt disabler has gone out of scope, after we have dropped all
  // of our locks.
  //
  const auto do_transaction = [&]() TA_REQ(
                                  chainlock_transaction_token,
                                  preempt_disabled_token) -> ChainLockTransaction::Result<bool> {
    ChainLockGuard guard{get_lock()};

    // Note(johngro): No one should ever calling wait_queue_wake_one on an
    // instance of an OwnedWaitQueue.  OwnedWaitQueues need to deal with
    // priority inheritance, and all wake operations on an OwnedWaitQueue should
    // be going through their interface instead.
    DEBUG_ASSERT(magic_ == kMagic);
    if (WAIT_QUEUE_VALIDATION) {
      ValidateQueue();
    }

    // Now that we are holding the queue lock, attempt to lock the thread's pi
    // lock.  Be prepared to drop all locks and retry the operation if we
    // fail.
    Thread* t = PeekFront();
    if (t) {
      if (!t->get_lock().AcquireOrBackoff()) {
        return ChainLockTransaction::Action::Backoff;
      }
      ChainLockTransaction::Finalize();

      // Remove the thread from the queue and drop the queue lock.  We no longer
      // need to hold it once the thread has been removed.
      Dequeue(t, wait_queue_error);
      guard.Release();

      // Finally, call into the scheduler to finish unblocking the thread.  We
      // need to continue to hold the thread's lock while we do this, the
      // scheduler will drop the lock for us once it has found a CPU for the
      // thread and added it to the proper scheduler instance.
      Scheduler::Unblock(t);
      return true;
    }

    return false;
  };
  return ChainLockTransaction::UntilDone(PreemptDisableAndIrqSaveOption,
                                         CLT_TAG("WaitQueue::WakeOne"), do_transaction);
}

// Same as WakeOne, but called with the queue's lock already held.  This call
// can fail (returning nullopt) in the case that a Backoff error is encountered,
// and we need to drop the queue's lock before starting again.  This version of
// WakeOne will continue to hold the queue's lock for the entire operation,
// instead of dropping it as soon as it has the target thread locked and removed
// from the queue.
ktl::optional<bool> WaitQueue::WakeOneLocked(zx_status_t wait_queue_error) {
  // Note(johngro): See the note in wake_one.  On one should ever be calling
  // this method on an OwnedWaitQueue
  DEBUG_ASSERT(magic_ == kMagic);
  if (WAIT_QUEUE_VALIDATION) {
    ValidateQueue();
  }

  // Check to see if there is a thread we want to wake.
  if (Thread* t = PeekFront(); t != nullptr) {
    // There is!  Try to lock it so we can actually wake it up.  If we can't, we
    // will need to unwind to allow the caller to release our lock before trying
    // again.
    if (!t->get_lock().AcquireOrBackoff()) {
      return ktl::nullopt;
    }

    // We now have all of the locks we need for the wake operation to proceed.
    // Make sure we mark the active ChainLockTransaction as finalized.
    ChainLockTransaction::ActiveRef().Finalize();

    // Remove the thread from the queue and unblock it.  We cannot drop the
    // queue lock yet, our caller needs us to hold onto it.  The scheduler will
    // take care of dropping the thread's lock for us after it has found a CPU
    // for the thread and added it to the proper scheduler.
    Dequeue(t, wait_queue_error);
    Scheduler::Unblock(t);
    return true;
  }

  // No one to wake up.
  return false;
}

/**
 * @brief  Wake all threads sleeping on a wait queue
 *
 * This function removes all threads (if any) from the wait queue and
 * makes them executable.  The new threads will be placed at the head of the
 * run queue.
 *
 * @param wait_queue_error  The return value which the new thread will receive
 * from wait_queue_block().
 *
 * @return  The number of threads woken
 */
void WaitQueue::WakeAll(zx_status_t wait_queue_error) {
  // WakeAll is a bit tricky.  In order to preserve the behavior as it was
  // before the global ThreadLock was removed, we need to obtain a set of locks
  // which includes not just the WaitQueue's lock, but also all of the locks of
  // all of the threads currently waiting in the queue.  This might evetually
  // become a contention bottleneck in some situations; the only good thing which
  // can be said here is that fair nature of the ChainLock means that we will
  // _eventually_ succeed.
  //
  // TODO(johngro): Figure out if it is OK to relax the previous behavior of
  // wake all.  In the past, at the instant we held the ThreadLock, we fixed a
  // set of threads who were in our WaitQueue, and those were the threads that
  // we were going to wake.
  //
  // In an effort to increase concurrency, we could adopt an approach where only
  // one thread's lock was held at any given point in time, minimizing the
  // chance of conflict.  The downside to this approach is that after
  // successfully locking and waking one thread, if we need to wake a second
  // thread, we may need to back off, dropping the queue lock in the process.
  // The result of this is that our WakeAll operation is no longer atomic.  It
  // just wake threads (including threads who happen to join the WaitQueue
  // during the WakeAll operation) until it finally observes zero threads in the
  // queue.

  // TODO(johngro): Should we suppress remote rescheduling for this operation
  // too?  Seems like it might be wise given that we could be waking a lot of
  // threads.  Then again, we drop the thread's locks as soon as we can, so it
  // might not be required.
  const auto do_transaction = [&]()
                                  TA_REQ(chainlock_transaction_token,
                                         preempt_disabled_token) -> ChainLockTransaction::Result<> {
    get_lock().AcquireFirstInChain();

    // Note(johngro): See the note in wake_one.  On one should ever be calling
    // this method on an OwnedWaitQueue
    DEBUG_ASSERT(magic_ == kMagic);
    if (WAIT_QUEUE_VALIDATION) {
      ValidateQueue();
    }

    // If the collection is empty, there is nothing left to do.
    if (collection_.IsEmpty()) {
      get_lock().Release();
      return ChainLockTransaction::Done;
    }

    ktl::optional<Thread::UnblockList> maybe_unblock_list =
        WaitQueueLockOps::LockForWakeAllInQueue(*this, wait_queue_error);
    // Whether we locked our threads and got a list back or not, we can drop the queue lock.  We are
    // going to either unblock our threads, or loop back around to try again.
    get_lock().Release();

    if (maybe_unblock_list.has_value()) {
      // Now that we have all of the threads locked, we are committed to the
      // WakeAll operation and can unblock our threads. Unlike the standard
      // WakeAll, we need to continue to hold onto the queue lock to satisfy our
      // caller's requirements.
      ChainLockTransaction::Finalize();
      Scheduler::Unblock(ktl::move(maybe_unblock_list).value());
    }
    return ChainLockTransaction::Done;
  };
  ChainLockTransaction::UntilDone(PreemptDisableAndIrqSaveOption, CLT_TAG("WaitQueue::WakeAll"),
                                  do_transaction);
}

ktl::optional<uint32_t> WaitQueue::WakeAllLocked(zx_status_t wait_queue_error) {
  // Note(johngro): See the note in wake_one.  On one should ever be calling
  // this method on an OwnedWaitQueue
  DEBUG_ASSERT(magic_ == kMagic);
  if (WAIT_QUEUE_VALIDATION) {
    ValidateQueue();
  }

  // If the collection is empty, there is nothing left to do.
  if (collection_.IsEmpty()) {
    ChainLockTransaction::ActiveRef().Finalize();
    return 0u;
  }

  const uint32_t count = collection_.Count();
  ktl::optional<Thread::UnblockList> maybe_unblock_list =
      WaitQueueLockOps::LockForWakeAllInQueue(*this, wait_queue_error);
  if (maybe_unblock_list.has_value()) {
    // Now that we have all of the threads locked, we are committed to the
    // WakeAll operation and can unblock our threads. Unlike the standard
    // WakeAll, we need to continue to hold onto the queue lock to satisfy our
    // caller's requirements.
    ChainLockTransaction::ActiveRef().Finalize();
    Scheduler::Unblock(ktl::move(maybe_unblock_list).value());
    return count;
  } else {
    return ktl::nullopt;
  }
}

void WaitQueue::DequeueThread(Thread* t, zx_status_t wait_queue_error) {
  DEBUG_ASSERT_MAGIC_CHECK(this);

  if (WAIT_QUEUE_VALIDATION) {
    ValidateQueue();
  }

  Dequeue(t, wait_queue_error);
}

void WaitQueue::MoveThread(WaitQueue* source, WaitQueue* dest, Thread* t) {
  DEBUG_ASSERT_MAGIC_AND_NOT_OWQ(source);
  DEBUG_ASSERT_MAGIC_AND_NOT_OWQ(dest);

  if (WAIT_QUEUE_VALIDATION) {
    source->ValidateQueue();
    dest->ValidateQueue();
  }

  DEBUG_ASSERT(t != nullptr);
  AssertInWaitQueue(*t, *source);
  DEBUG_ASSERT(source->collection_.Count() > 0);

  source->collection_.Remove(t);
  dest->collection_.Insert(t);
  t->wait_queue_state().blocking_wait_queue_ = dest;
}

/**
 * @brief  Tear down a wait queue
 *
 * This panics if any threads were waiting on this queue, because that
 * would indicate a race condition for most uses of wait queues.  If a
 * thread is currently waiting, it could have been scheduled later, in
 * which case it would have called Block() on an invalid wait
 * queue.
 */
WaitQueueBase::~WaitQueueBase() {
  DEBUG_ASSERT_MAGIC_CHECK(this);

  const uint32_t count = collection_.Count();
  if (count != 0) {
    panic("~WaitQueue() called on non-empty WaitQueue, count=%u, magic=0x%08x\n", count, magic_);
  }

  magic_ = 0;
}

/**
 * @brief  Wake a specific thread from a specific wait queue
 *
 * TODO(johngro): Update this comment.  UnblockThread does not actually remove
 * the thread from the wait queue, it simply finishes the unblock operation,
 * propagating any PI effects and dropping the PI lock chain starting from the
 * wait queue in the processes.
 *
 * This function extracts a specific thread from a wait queue, wakes it, puts it
 * into a Scheduler's run queue, and does a reschedule if necessary.  Callers of
 * this function must be sure that they are holding the locks for all of the
 * nodes in the PI chain, starting from the thread, before calling the function.
 * Static analysis can only ensure that the thread and its immediately
 * downstream blocking wait queue are locked.
 *
 * @param t  The thread to wake
 * @param wait_queue_error  The return value which the new thread will receive from
 * wait_queue_block().
 *
 * @return ZX_ERR_BAD_STATE if thread was not in any wait queue.
 */
zx_status_t WaitQueueBase::UnblockThread(Thread* t, zx_status_t wait_queue_error) {
  DEBUG_ASSERT_MAGIC_CHECK(this);
  t->canary().Assert();

  if (WAIT_QUEUE_VALIDATION) {
    ValidateQueue();
  }

  // Remove the thread from the wait queue and deal with any PI propagation
  // required if the thread was removed from an owned wait queue. Finally,
  // formally unblock the thread, allowing it to join a run queue.
  Dequeue(t, wait_queue_error);

  if (OwnedWaitQueue* owq = OwnedWaitQueue::DowncastToOwq(this); owq != nullptr) {
    owq->get_lock().MarkHeld();
    // Get the effective profile of the owned wait queue before propagation,
    // which will recompute the effective profile of the queue. This is
    // necessary for the scheduler PI logic to determine how the wait queue's
    // effective profile changed.
    const SchedulerState::EffectiveProfile old_target_ep = owq->GetEffectiveProfile();
    OwnedWaitQueue::BeginPropagate(*t, *owq, OwnedWaitQueue::RemoveSingleEdgeOp,
                                   ForceInheritance::No, &old_target_ep);
    owq->UpdateSchedStateStorageThreadRemoved(*t);
  }

  // Now that any required propagation is complete, we can release all of the PI
  // chain locks starting from this wait queue.
  OwnedWaitQueue::UnlockPiChainCommon(*this);

  // The PI consequences of the unblock (if any) have been applied, and all of
  // the (previously) downstream PI chain has been unlocked.  Go ahead and
  // unblock the thread in the scheduler, which will will drop the unblocking thread's
  // lock for us.
  Scheduler::Unblock(t);
  return ZX_OK;
}

void WaitQueueBase::UpdateBlockedThreadEffectiveProfile(Thread& t) {
  t.canary().Assert();
  DEBUG_ASSERT_MAGIC_CHECK(this);
  // Note, we don't do this in order to establish shared access to the thread's
  // scheduler state.  We should already have the thread locked for exclusive
  // access.  We just do this as an additional consistency check.
  AssertInWaitQueue(t, *this);

  SchedulerState& state = t.scheduler_state();
  collection_.Remove(&t);
  state.RecomputeEffectiveProfile();
  collection_.Insert(&t);

  if (WAIT_QUEUE_VALIDATION) {
    ValidateQueue();
  }
}

ktl::optional<BrwLockOps::LockForWakeResult> BrwLockOps::LockForWake(WaitQueue& queue,
                                                                     zx_instant_mono_t now) {
  DEBUG_ASSERT_MAGIC_AND_NOT_OWQ(&queue);
  LockForWakeResult result;
  Thread* t;

  while ((t = queue.collection_.PeekFront()) != nullptr) {
    // Figure out of this thread a reader or a writer.  Note; the odd use of a
    // lambda here is to work around issues with the static analyzer.  We cannot
    // assert that we have read access to a capability in a function, and later
    // on assert that we have exclusive access (after having obtained the
    // capability exclusively).
    const bool is_reader = [&]() TA_REQ(queue.get_lock()) {
      AssertInWaitQueue(*t, queue);
      DEBUG_ASSERT((t->state() == THREAD_BLOCKED) || (t->state() == THREAD_BLOCKED_READ_LOCK));
      return (t->state() == THREAD_BLOCKED_READ_LOCK);
    }();

    // We should stop if we have already selected threads (meaning we are waking
    // readers), and this thread is not a reader.
    if ((result.count != 0) && (is_reader == false)) {
      return result;
    }

    // Looks like we want to wake this thread.  Try to lock it.
    if (!t->get_lock().AcquireOrBackoff()) {
      // Put each of the threads back where they belong before returning
      // nothing, unlocking them as we go.  Make sure to set the thread's
      // blocking wait queue pointer back to ourselves.
      while (!result.list.is_empty()) {
        Thread* return_to_queue = result.list.pop_front();
        return_to_queue->get_lock().AssertAcquired();
        DEBUG_ASSERT(return_to_queue->wait_queue_state().blocking_wait_queue_ == nullptr);
        return_to_queue->wait_queue_state().blocking_wait_queue_ = &queue;
        queue.collection_.Insert(return_to_queue);
        return_to_queue->get_lock().Release();
      }
      return ktl::nullopt;
    }

    // We got the thread's lock.  Move it from the collection to our list of
    // threads to wake, clearing the blocking queue and setting the block result
    // as we go.
    queue.DequeueThread(t, ZX_OK);
    result.list.push_back(t);
    ++result.count;

    t->get_lock().MarkReleased();

    // If we just locked a writer for wake, we are done.  We can only wake one writer at a time.
    if (is_reader == false) {
      DEBUG_ASSERT(result.count == 1);
      return result;
    }
  }

  // Out of threads to lock.  Whatever we have so far is our result.
  return result;
}

ktl::optional<Thread::UnblockList> WaitQueueLockOps::LockForWakeAllInPlace(WaitQueue& queue) {
  DEBUG_ASSERT_MAGIC_AND_NOT_OWQ(&queue);

  // Try to lock all of the threads, backing off if we have to.
  if (const ChainLock::Result res = queue.collection_.LockAll();
      res == ChainLock::Result::Backoff) {
    return ktl::nullopt;
  }

  // Now that we have all of the threads locked, we are committed to the
  // WakeAll operation, and we can add the threads to an UnblockList.
  //
  // TODO(https://fxbug.dev/473385363): The in-place operations prevent us from collapsing the
  // storage requirements of an UnblockList and WaitQueueCollection.
  Thread::UnblockList unblock_list;
  for (Thread& t : queue.collection_.threads()) {
    t.get_lock().AssertHeld();
    unblock_list.push_back(&t);
  }

  return unblock_list;
}

ktl::optional<Thread::UnblockList> WaitQueueLockOps::LockForWakeAllInQueue(
    WaitQueue& queue, zx_status_t wait_queue_error) {
  // TODO(johngro): Optimize this.  By removing all of the elements of the
  // collection one at a time, we are paying a rebalancing price we really
  // should not have to pay, since we are going to eventually remove all of
  // the elements, so maintaining balance is not really important.
  ktl::optional<Thread::UnblockList> unblock_list = LockForWakeAllInPlace(queue);
  if (unblock_list.has_value()) {
    for (Thread& t : *unblock_list) {
      t.get_lock().AssertHeld();
      queue.Dequeue(&t, wait_queue_error);
    }
  }
  return unblock_list;
}

ktl::optional<Thread::UnblockList> WaitQueueLockOps::LockForWakeOneInPlace(WaitQueue& queue) {
  DEBUG_ASSERT_MAGIC_AND_NOT_OWQ(&queue);

  if (Thread* t = queue.collection_.PeekFront(); t != nullptr) {
    if (!t->get_lock().AcquireOrBackoff()) {
      return ktl::nullopt;
    }

    // TODO(https://fxbug.dev/473385363): The in-place operations prevent us from collapsing the
    // storage requirements of an UnblockList and WaitQueueCollection.
    Thread::UnblockList unblock_list;
    unblock_list.push_back(t);
    t->get_lock().MarkReleased();

    return unblock_list;
  }

  return Thread::UnblockList{};
}
