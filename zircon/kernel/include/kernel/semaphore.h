// Copyright 2017 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_INCLUDE_KERNEL_SEMAPHORE_H_
#define ZIRCON_KERNEL_INCLUDE_KERNEL_SEMAPHORE_H_

#include <lib/kconcurrent/chainlock.h>
#include <lib/kconcurrent/chainlock_transaction.h>
#include <lib/zircon-internal/macros.h>
#include <stdint.h>
#include <zircon/types.h>

#include <kernel/thread.h>
#include <kernel/wait.h>
#include <ktl/atomic.h>

// A basic counting semaphore used to control access to a shared resource.
//
// Think of Semaphore as a gatekeeper that allows a certain number of threads to
// pass through a gate so they can access some resource.
//
// Threads queue up at the gate by calling Wait and then block until the
// gatekeeper lets them through or they give up and leave the queue because of
// timeout or thread signal.  In the case of timeout or thread signal, the
// waiter may not proceed to the resource.
//
// Calling Post tells the gatekeeper that they may immediately admit one through
// the gate (if there's a waiter in the queue) or admit one in the future once a
// waiter has queued up.
class Semaphore {
 public:
  explicit Semaphore(int64_t initial_count = 0) : count_(initial_count) {}

  Semaphore(const Semaphore&) = delete;
  Semaphore(Semaphore&&) = delete;
  Semaphore& operator=(const Semaphore&) = delete;
  ~Semaphore() = default;

  // Unblocks a single thread if any are waiting.  If none are waiting, this
  // operation logically increments the count such that a future call to |Wait|
  // may return without blocking.
  //
  // |Post| has release memory order semantics and synchronizes with |Wait|.
  //
  // |queue_to_own| is the OwnedWaitQueue that the unblocked thread will take
  // ownership of. If this is not-null, then we will perform an assign-owner
  // operation on the queue. If there is no thread waiting, then no
  // assign-owner operation will occur.
  void Post(OwnedWaitQueue* queue_to_own = nullptr) TA_EXCL(chainlock_transaction_token);

  // If the count is positive, decrement the count and return ZX_OK.  Otherwise,
  // decrement the count and wait (uninterruptibly) until some other thread
  // wakes us via |Post|.
  //
  // |Wait| has acquire memory order semantics and synchronizes with |Post|.
  zx_status_t Wait() TA_EXCL(chainlock_transaction_token) {
    return WaitInternal(Deadline::infinite(), Interruptible::No);
  }

  // Just like Wait(), but interruptible.  In addition to ZX_OK, the return
  // value can be ZX_ERR_TIMED_OUT if the deadline had passed or one of the
  // ZX_ERR_INTERNAL_INTR errors if the thread had a signal delivered.
  //
  // |Wait| has acquire memory order semantics and synchronizes with |Post|.
  zx_status_t Wait(const Deadline& deadline) TA_EXCL(chainlock_transaction_token) {
    return WaitInternal(deadline, Interruptible::Yes);
  }

  // Observe the current internal count of the semaphore.
  //
  // This should only be used for testing/diagnostic purposes.
  int64_t count() const { return count_.load(ktl::memory_order_relaxed); }

  // Observe the current internal count of waiters.
  //
  // This should only be used for testing/diagnostic purposes.
  uint64_t num_waiters() const TA_EXCL(chainlock_transaction_token, waitq_.get_lock()) {
    SingleChainLockGuard guard{IrqSaveOption, waitq_.get_lock(), CLT_TAG("Semaphore::num_waiters")};
    return waitq_.Count();
  }

 private:
  zx_status_t WaitInternal(const Deadline& deadline, Interruptible interruptible);

  // This class has two fields that must be kept in sync, a count and a
  // WaitQueue.  There are some rules for keeping them in sync.  See the
  // implementation comments of |Post| and |Wait| for details.
  //
  // When |count_| is greater than zero, a call to |Wait| will decrement the
  // count and "fall through" without blocking.
  //
  // When |count_| is zero or negative, a call to |Wait| will decrement |count_|
  // and block.  Note, a negative value does not necessarily imply there are
  // currently waiters in the |waitq_|.  It's possible to have a negative count
  // and no waiters because a |Wait| can timeout or be interrupted (e.g. by
  // thread kill or thread suspend).  The next call to |Post| after an
  // interrupted |Wait| will update the |count_|.
  //
  // Many methods on |waitq_| require the caller to be holding the WaitQueue's
  // lock. In order to reduce lock contention and improve scalability, the
  // |Post| and |Wait| operations are designed to only acquire the lock when
  // necessary.  In particular:
  //
  // 1. |Post| does not need to access |waitq_| if it knows that no threads are
  // blocked (i.e. when |count_| is non-negative).
  //
  // 2. |Wait| does not need to access the |waitq_| if the caller does not need
  // to block (i.e. when |count_| is greater than zero).
  //
  // It's critical that the lock protecting |waitq_| be held when transitioning
  // |count_| from negative to non-negative or from non-negative to negative.
  ktl::atomic<int64_t> count_;
  WaitQueue waitq_;
};

#endif  // ZIRCON_KERNEL_INCLUDE_KERNEL_SEMAPHORE_H_
