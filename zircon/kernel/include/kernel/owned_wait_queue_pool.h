// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_INCLUDE_KERNEL_OWNED_WAIT_QUEUE_POOL_H_
#define ZIRCON_KERNEL_INCLUDE_KERNEL_OWNED_WAIT_QUEUE_POOL_H_

#include <lib/object_cache.h>
#include <zircon/types.h>

#include <fbl/intrusive_double_list.h>
#include <fbl/macros.h>
#include <kernel/lazy_owned_wait_queue.h>
#include <kernel/owned_wait_queue.h>
#include <kernel/spinlock.h>
#include <ktl/atomic.h>

// Global pool for OwnedWaitQueue objects.
class OwnedWaitQueuePool {
 public:
  OwnedWaitQueuePool() : cache_(0) {}

  // Returns the global singleton instance.
  static OwnedWaitQueuePool& Get();

  // Increases the pool size by one if the number of active threads exceeds the current pool
  // capacity.
  //
  // May block the caller.
  //
  // Must be called during Thread construction to ensure pool invariants are upheld.
  //
  // Returns ZX_ERR_NO_MEMORY if allocation fails.
  zx_status_t Grow();

  // Decreases the active thread count.
  //
  // Must be called during Thread destruction or resource cleanup.
  //
  // Note that this does not currently shrink the size of the pool, it shrinks the number of
  // active threads tracked by the pool. This allows new threads in the future to reuse already
  // allocated OwnedWaitQueue's.
  void Shrink();

  // Acquires the pool lock for external synchronization.
  //
  // If this lock will be held at the same time as OwnedWaitQueue::get_lock(), then
  // OwnedWaitQueue's lock must be acquired first.
  SpinLock* GetLock() TA_RET_CAP(OwnedWaitQueuePoolLock::Get()) {
    return reinterpret_cast<SpinLock*>(OwnedWaitQueuePoolLock::Get());
  }

  // Recycle a queue back to the pool.
  //
  // The queue must be in a clean state (no owner, no waiters).
  void RecycleLocked(OwnedWaitQueue* queue) TA_REQ(OwnedWaitQueuePoolLock::Get());

  // Borrow a queue from the pool.
  OwnedWaitQueue* BorrowLocked() TA_REQ(OwnedWaitQueuePoolLock::Get());

  // Returns the value of the excess capacity OWQ kcounter.
  int64_t GetExcessCapacityForTest();

 private:
  object_cache::ObjectCache<OwnedWaitQueue> cache_;

  // Use a DoublyLinkedList so that we can reuse OWQ's existing storage and mitigate contention
  // from optimistic concurrency by pushing/popping from opposite ends.
  // OwnedWaitQueue::AssertSafeForPooling() is used to ensure that we can safely reuse the storage.
  fbl::DoublyLinkedList<OwnedWaitQueue*> free_list_ __TA_GUARDED(OwnedWaitQueuePoolLock::Get());

  ktl::atomic<uint32_t> active_threads_{0};
  ktl::atomic<uint32_t> total_allocated_{0};
};

#endif  // ZIRCON_KERNEL_INCLUDE_KERNEL_OWNED_WAIT_QUEUE_POOL_H_
