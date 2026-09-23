// Copyright 2016 The Fuchsia Authors
// Copyright (c) 2008-2014 Travis Geiselbrecht
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_INCLUDE_KERNEL_WAIT_H_
#define ZIRCON_KERNEL_INCLUDE_KERNEL_WAIT_H_

#include <lib/kconcurrent/chainlock.h>

#include <fbl/intrusive_wavl_tree.h>
#include <fbl/macros.h>
#include <fbl/wavl_tree_best_node_observer.h>
#include <kernel/deadline.h>
#include <kernel/preempt_disabled_token.h>
#include <kernel/scheduler_state.h>
#include <kernel/timer.h>

struct Thread;
class WaitQueueBase;

// When blocking this enum indicates the kind of resource ownership that is being waited for
// that is causing the block.
enum class ResourceOwnership {
  // Blocking is either not for any particular resource, or it is to wait for
  // exclusive access to a resource.
  Normal,
  // Blocking is happening whilst waiting for shared read access to a resource.
  Reader,
};

// Whether a block or a sleep can be interrupted.
enum class Interruptible : bool { No, Yes };

// Whether to force base profile inheritance.
enum class ForceInheritance : bool { No, Yes };

// The collection of threads currently blocked in a wait queue.
class WaitQueueCollection {
 private:
  // fwd decls
  struct BlockedThreadTreeTraits;
  struct MinRelativeDeadlineTraits;

 public:
  using PrimaryKey = SchedTime;
  static constexpr PrimaryKey kPrimaryKeyZero = PrimaryKey{0};

  using Key = ktl::pair<PrimaryKey, uintptr_t>;

  // Encapsulation of all the per-thread state for the WaitQueueCollection data structure.
  class ThreadState {
   public:
    ThreadState() = default;

    ~ThreadState();

    // Disallow copying.
    ThreadState(const ThreadState&) = delete;
    ThreadState& operator=(const ThreadState&) = delete;

    bool InWaitQueue() const { return blocked_threads_tree_node_.InContainer(); }

    zx_status_t BlockedStatus() const { return blocked_status_; }

    void Block(Thread* current_thread, Interruptible interruptible, zx_status_t status)
        TA_REQ(chainlock_transaction_token, ChainLockable::GetLock(*current_thread));

    void Unsleep(Thread* thread, zx_status_t status) TA_REQ(chainlock_transaction_token)
        TA_REL(ChainLockable::GetLock(*thread));

    void AssertNoOwnedWaitQueues() const {}

    void AssertNotBlocked() const {
      DEBUG_ASSERT(blocking_wait_queue_ == nullptr);
      DEBUG_ASSERT(!InWaitQueue());
    }

    WaitQueueBase* blocking_wait_queue() { return blocking_wait_queue_; }
    const WaitQueueBase* blocking_wait_queue() const { return blocking_wait_queue_; }
    Interruptible interruptible() const { return interruptible_; }

   private:
    // WaitQueues, WaitQueueCollections, and their List types, can
    // directly manipulate the contents of the per-thread state, for now.
    friend struct BrwLockOps;
    friend class OwnedWaitQueue;
    friend class Scheduler;
    friend class WaitQueue;
    friend class WaitQueueBase;
    friend class WaitQueueCollection;
    friend struct WaitQueueCollection::BlockedThreadTreeTraits;
    friend struct WaitQueueCollection::MinRelativeDeadlineTraits;

    // Dumping routines are allowed to see inside us.
    friend class ThreadDumper;

    // If blocked, a pointer to the WaitQueue the Thread is on.
    WaitQueueBase* blocking_wait_queue_ = nullptr;

    // Node state for existing in WaitQueueCollection::threads_
    fbl::WAVLTreeNodeState<Thread*> blocked_threads_tree_node_;

    // Primary key used for determining our position in the collection of
    // blocked threads. Pre-computed during insert in order to save a time
    // during insert, rebalance, and search operations.
    PrimaryKey blocked_threads_tree_sort_key_ = kPrimaryKeyZero;

    // The minimum relative deadline of this node's subtree, if any.
    SchedDuration subtree_min_deadline_{SchedDuration::Max()};

    // Return code if woken up abnormally from suspend, sleep, or block.
    zx_status_t blocked_status_ = ZX_OK;

    // Are we allowed to be interrupted on the current thing we're blocked/sleeping on?
    Interruptible interruptible_ = Interruptible::No;

    // Storage used by an OwnedWaitQueue, but held within a thread
    // instance, while that thread is blocked in the wait queue.
    SchedulerState::WaitQueueInheritedSchedulerState inherited_scheduler_state_storage_{};
  };

  constexpr WaitQueueCollection() {}
  ~WaitQueueCollection() {
    Validate();
    DEBUG_ASSERT(threads_.is_empty());
  }

  void Validate() const {
    // TODO(johngro): We could perform a more rigorous check of the two maintained
    // invariants of the threads_ collection, however we probably only want to do
    // so if kSchedulerExtraInvariantValidation is true.
  }

  // Passthrus for the underlying container's size and is_empty methods.
  uint32_t Count() const { return static_cast<uint32_t>(threads_.size()); }
  bool IsEmpty() const { return threads_.is_empty(); }

  // The current minimum inheritable relative deadline of the set of blocked threads.
  SchedDuration MinInheritableRelativeDeadline() const;

  Thread& PeekOnlyThread() {
    DEBUG_ASSERT_MSG(threads_.size() == 1, "Expected size 1, not %zu", threads_.size());
    return threads_.front();
  }

  // Peek at the first Thread in the collection.
  Thread* PeekFront() { return threads_.is_empty() ? nullptr : &threads_.front(); }
  const Thread* PeekFront() const { return const_cast<WaitQueueCollection*>(this)->PeekFront(); }

  inline SchedulerState::WaitQueueInheritedSchedulerState* FindInheritedSchedulerStateStorage();

  // Add the Thread into its sorted location in the collection.
  void Insert(Thread* thread) TA_REQ(ChainLockable::GetLock(*thread));

  // Remove the Thread from the collection.
  void Remove(Thread* thread) TA_REQ(ChainLockable::GetLock(*thread));

  // Either lock every thread in the collection, or failed with
  // ChainLock::Result::Backoff, releasing any locks which were obtained in the
  // process. ASSERTs if any cycles are detected by the ChainLock. Used by
  // WaitQueue WakeAll.  Note that it is not possible to statically annotated
  // this, and needs to be used with extreme care.  If LockAll returns success,
  // it is critical that the caller (eventually) drops all of the locks.
  ChainLock::Result LockAll() TA_REQ(chainlock_transaction_token);

  // Accessor for the underlying thread collection.
  const auto& threads() const { return threads_; }
  auto& threads() { return threads_; }

  // Disallow copying and moving.
  WaitQueueCollection(const WaitQueueCollection&) = delete;
  WaitQueueCollection& operator=(const WaitQueueCollection&) = delete;

  WaitQueueCollection(WaitQueueCollection&&) = delete;
  WaitQueueCollection& operator=(WaitQueueCollection&&) = delete;

 private:
  struct BlockedThreadTreeTraits {
    static Key GetKey(const Thread& thread);
    static bool LessThan(Key a, Key b) { return a < b; }
    static bool EqualTo(Key a, Key b) { return a == b; }
    static fbl::WAVLTreeNodeState<Thread*>& node_state(Thread& thread);
  };

  struct MinRelativeDeadlineTraits {
    // WAVLTreeBestNodeObserver template API
    using ValueType = SchedDuration;
    static ValueType GetValue(const Thread& thread);
    static ValueType GetSubtreeBest(const Thread& thread);
    static bool Compare(ValueType a, ValueType b);
    static void AssignBest(Thread& thread, ValueType val);
    static void ResetBest(Thread& thread);
  };

  using BlockedThreadTree =
      fbl::WAVLTree<Key, Thread*, BlockedThreadTreeTraits, fbl::DefaultObjectTag,
                    fbl::SizeOrder::Constant, BlockedThreadTreeTraits,
                    fbl::WAVLTreeBestNodeObserver<MinRelativeDeadlineTraits>>;
  BlockedThreadTree threads_;
};

// Base class for wait queue types, providing common functionality.
class WaitQueueBase : public ChainLockable {
 public:
  uint32_t magic() const { return magic_; }
  bool IsEmpty() const TA_REQ_SHARED(get_lock()) { return collection_.IsEmpty(); }
  uint32_t Count() const TA_REQ_SHARED(get_lock()) { return collection_.Count(); }

  // Remove a specific thread out of the wait queue it's blocked on, and deal
  // with any PI side effects.  Note: when calling this function:
  //
  // 1) The thread |t| must be actively blocked in the WaitQueue instance
  //    indicated by |this|.
  // 2) In addition to |t|'s lock, all of the ChainLocks downstream of |t|
  //    (starting from |t| and ending at the target of |t|'s PI graph) must be
  //    held.  Note that we are only able to statically assert that the first
  //    two of these locks; |t|'s lock, and the lock of the wait queue |t| is
  //    currently blocked in.
  // 3) During the call UnblockThread, all of the locks identified in #2 (above)
  //    will be released.
  zx_status_t UnblockThread(Thread* t, zx_status_t wait_queue_error)
      TA_REL(get_lock(), ChainLockable::GetLock(*t))
          TA_REQ(chainlock_transaction_token, preempt_disabled_token);

 protected:
  friend struct BrwLockOps;
  friend struct WaitQueueLockOps;
  friend class Scheduler;

  explicit constexpr WaitQueueBase(uint32_t magic) : magic_(magic) {}
  ~WaitQueueBase();

  Thread* PeekFront() TA_REQ(get_lock()) { return collection_.PeekFront(); }
  const Thread* PeekFront() const TA_REQ(get_lock()) { return collection_.PeekFront(); }

  inline zx_status_t BlockEtcPreamble(Thread* current_thread, const Deadline& deadline,
                                      uint signal_mask, ResourceOwnership reason,
                                      Interruptible interruptible)
      TA_REQ(get_lock(), ChainLockable::GetLock(*current_thread));

  inline zx_status_t BlockEtcPostamble(Thread* current_thread, const Deadline& deadline)
      TA_EXCL(get_lock())
          TA_REQ(chainlock_transaction_token, ChainLockable::GetLock(*current_thread));

  void Dequeue(Thread* t, zx_status_t wait_queue_error)
      TA_REQ(get_lock(), ChainLockable::GetLock(*t));

  void ValidateQueue() TA_REQ_SHARED(get_lock());

  static void TimeoutHandler(Timer* timer, zx_instant_mono_t now, void* arg);

  // Recompute the effective profile of a thread which is known to be blocked in
  // this wait queue, reordering the thread in the queue collection as needed.
  //
  // This method does not deal with the consequences of profile inheritance, and
  // should only ever be called from the scheduler's PI update code.
  void UpdateBlockedThreadEffectiveProfile(Thread& t) TA_REQ(get_lock(), t);

  uint32_t magic_;
  WaitQueueCollection collection_ TA_GUARDED(get_lock());
};

// A basic wait queue for blocking and unblocking threads without resource
// ownership semantics.
class WaitQueue : public WaitQueueBase {
 public:
  static constexpr uint32_t kMagic = fbl::magic("wait");

  constexpr WaitQueue() : WaitQueueBase(kMagic) {}

  WaitQueue(WaitQueue&) = delete;
  WaitQueue(WaitQueue&&) = delete;
  WaitQueue& operator=(WaitQueue&) = delete;
  WaitQueue& operator=(WaitQueue&&) = delete;

  // Expose direct access to PeekFront.
  using WaitQueueBase::PeekFront;

  // Block on a wait queue.
  // The returned status is whatever the caller of WaitQueue::Wake_*() specifies.
  // A deadline other than Deadline::infinite() will abort at the specified time
  // and return ZX_ERR_TIMED_OUT. A deadline in the past will immediately return.
  zx_status_t Block(Thread* const current_thread, const Deadline& deadline,
                    Interruptible interruptible) TA_REL(get_lock())
      TA_REQ(chainlock_transaction_token, ChainLockable::GetLock(*current_thread)) {
    return BlockEtc(current_thread, deadline, 0, ResourceOwnership::Normal, interruptible);
  }

  // Block on a wait queue with a zx_instant_mono_t-typed deadline.
  zx_status_t Block(Thread* const current_thread, zx_instant_mono_t deadline,
                    Interruptible interruptible) TA_REL(get_lock())
      TA_REQ(chainlock_transaction_token, ChainLockable::GetLock(*current_thread)) {
    return BlockEtc(current_thread, Deadline::no_slack(deadline), 0, ResourceOwnership::Normal,
                    interruptible);
  }

  // Block on a wait queue, ignoring existing signals in |signal_mask|.
  // The returned status is whatever the caller of WaitQueue::Wake_*() specifies, or
  // ZX_ERR_TIMED_OUT if the deadline has elapsed or is in the past.
  // This will never timeout when called with a deadline of Deadline::infinite().
  zx_status_t BlockEtc(Thread* current_thread, const Deadline& deadline, uint signal_mask,
                       ResourceOwnership reason, Interruptible interruptible) TA_REL(get_lock())
      TA_REQ(chainlock_transaction_token, ChainLockable::GetLock(*current_thread));

  // Release one or more threads from the wait queue.
  // wait_queue_error = what WaitQueue::Block() should return for the blocking thread.
  //
  // Returns true if a thread was woken, and false otherwise.
  bool WakeOne(zx_status_t wait_queue_error) TA_EXCL(chainlock_transaction_token, get_lock());
  void WakeAll(zx_status_t wait_queue_error) TA_EXCL(chainlock_transaction_token, get_lock());

  // Locked versions of the wake calls.  These calls are going to need to obtain
  // locks for each of the threads woken, which could result in needing to back
  // off and start the operation again. Each routine returns a
  // std::optional (bool or u32).
  //
  // ++ If the optional holds a value, then the value is the number of threads
  //    which were woken, and the operation succeeded.
  // ++ Otherwise, the operation failed with a Backoff error and the queue's
  //    lock needs to be dropped before trying again.
  //
  // It is assumed that forming a lock cycle should be impossible.  If such a
  // cycle is detected (like, if the caller was holding the lock of one of the
  // thread's blocked in this queue at the time of the call) it will trigger a
  // DEBUG_ASSERT.
  //
  // Either way, unlike the non-Locked versions of these routines, the wait
  // queue's lock is held for the duration of the operation, instead of being
  // release as soon as possible (during the call to SchedulerUnlock)
  ktl::optional<bool> WakeOneLocked(zx_status_t wait_queue_error)
      TA_REQ(chainlock_transaction_token, get_lock(), preempt_disabled_token);
  ktl::optional<uint32_t> WakeAllLocked(zx_status_t wait_queue_error)
      TA_REQ(chainlock_transaction_token, get_lock(), preempt_disabled_token);

  // Dequeue the specified thread and set its blocked_status.  Do not actually
  // schedule the thread to run.
  void DequeueThread(Thread* t, zx_status_t wait_queue_error)
      TA_REQ(get_lock(), ChainLockable::GetLock(*t));

 protected:
  explicit constexpr WaitQueue(uint32_t magic) : WaitQueueBase(magic) {}

  // Move the specified thread from the source wait queue to the dest wait queue.
  static void MoveThread(WaitQueue* source, WaitQueue* dest, Thread* t)
      TA_REQ(source->get_lock(), dest->get_lock(), ChainLockable::GetLock(*t));
};

#endif  // ZIRCON_KERNEL_INCLUDE_KERNEL_WAIT_H_
