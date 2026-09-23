// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_INCLUDE_KERNEL_LAZY_OWNED_WAIT_QUEUE_H_
#define ZIRCON_KERNEL_INCLUDE_KERNEL_LAZY_OWNED_WAIT_QUEUE_H_

#include <zircon/types.h>

#include <fbl/intrusive_double_list.h>
#include <fbl/macros.h>
#include <kernel/owned_wait_queue.h>
#include <kernel/spinlock.h>
#include <ktl/atomic.h>

class OwnedWaitQueue;
class OwnedWaitQueuePool;

// Global lock for OwnedWaitQueuePool.
DECLARE_SINGLETON_SPINLOCK(OwnedWaitQueuePoolLock);

// Global pool accessor policy.
struct GlobalOwqPoolAccess {
  OwnedWaitQueuePool& get_pool() const;
};

// Local pool accessor policy for tests.
struct LocalOwqPoolAccess {
  constexpr explicit LocalOwqPoolAccess(OwnedWaitQueuePool* pool = nullptr) : pool_(pool) {}
  OwnedWaitQueuePool* pool_;
  OwnedWaitQueuePool& get_pool() const { return *pool_; }
};

// A wrapper class for OwnedWaitQueue that lazily borrows a queue from a pool
// when necessary.
template <typename PoolAccess = GlobalOwqPoolAccess>
class LazyOwnedWaitQueueImpl : private PoolAccess {
 public:
  constexpr LazyOwnedWaitQueueImpl() = default;

  constexpr explicit LazyOwnedWaitQueueImpl(OwnedWaitQueuePool& pool)
      // Limit this constructor to use by the LocalLazyOwnedWaitQueue that is used in tests.
    requires(sizeof(PoolAccess) > 1)
      : PoolAccess(&pool) {}

  DISALLOW_COPY_ASSIGN_AND_MOVE(LazyOwnedWaitQueueImpl);

  ~LazyOwnedWaitQueueImpl();

  // Returns a pointer to an OwnedWaitQueue, borrowing from the pool if necessary.
  //
  // Callers should only call this when they are preparing to use the returned queue to block.
  // Callers are responsible for coordinating amongst each other so that the last caller on each
  // LazyOwnedWaitQueue to *unblock* after calling this calls Detach() or ReleaseLocked() to
  // recycle the queue to the pool.
  //
  // Thread-safe. Will not block but may acquire the pool spinlock.
  OwnedWaitQueue* Get();

  // Releases the underlying OwnedWaitQueue back to the pool if it exists.
  //
  // Will not block.
  void ReleaseLocked() TA_REQ(OwnedWaitQueuePoolLock::Get());

  // Detaches the underlying queue and returns it. Caller takes ownership and is responsible
  // for calling OwnedWaitQueuePool::RecycleLocked() on the returned queue when done.
  //
  // Will not block.
  OwnedWaitQueue* Detach();

  // Returns the current pointer without allocating. Will neither block nor acquire the pool
  // spinlock. Thread-safe.
  OwnedWaitQueue* Peek() { return ptr_.load(ktl::memory_order_acquire); }

 private:
  ktl::atomic<OwnedWaitQueue*> ptr_{nullptr};
};

using LazyOwnedWaitQueue = LazyOwnedWaitQueueImpl<GlobalOwqPoolAccess>;
static_assert(sizeof(LazyOwnedWaitQueue) == sizeof(void*),
              "LazyOwnedWaitQueue must be exactly pointer-sized.");
using LocalLazyOwnedWaitQueue = LazyOwnedWaitQueueImpl<LocalOwqPoolAccess>;

#endif  // ZIRCON_KERNEL_INCLUDE_KERNEL_LAZY_OWNED_WAIT_QUEUE_H_
