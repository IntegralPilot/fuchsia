// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/counters.h>
#include <lib/lazy_init/lazy_init.h>
#include <lib/object_cache.h>

#include <kernel/owned_wait_queue.h>
#include <kernel/owned_wait_queue_pool.h>
#include <kernel/spinlock.h>
#include <lk/init.h>

namespace {

KCOUNTER(owq_pool_excess_capacity, "owned_wait_queue.pool.excess_capacity")

lazy_init::LazyInit<OwnedWaitQueuePool, lazy_init::CheckType::Basic> g_pool;

void OwnedWaitQueuePoolInit(uint level) { g_pool.Initialize(); }

LK_INIT_HOOK(owq_pool, OwnedWaitQueuePoolInit, LK_INIT_LEVEL_THREADING - 1)

}  // namespace

OwnedWaitQueuePool& OwnedWaitQueuePool::Get() { return g_pool.Get(); }

zx_status_t OwnedWaitQueuePool::Grow() {
  kcounter_add(owq_pool_excess_capacity, -1);
  uint32_t active = active_threads_.fetch_add(1, ktl::memory_order_acq_rel) + 1;
  uint32_t capacity = total_allocated_.load(ktl::memory_order_acquire);

  if (likely(active <= capacity)) {
    return ZX_OK;
  }

  auto result = cache_.Allocate();
  if (result.is_error()) {
    active_threads_.fetch_sub(1, ktl::memory_order_release);
    kcounter_add(owq_pool_excess_capacity, 1);
    return result.status_value();
  }

  Guard<SpinLock, IrqSave> guard{OwnedWaitQueuePoolLock::Get()};
  RecycleLocked(result.value().release());
  total_allocated_.fetch_add(1, ktl::memory_order_release);
  kcounter_add(owq_pool_excess_capacity, 1);
  return ZX_OK;
}

void OwnedWaitQueuePool::Shrink() {
  uint32_t old_active = active_threads_.fetch_sub(1, ktl::memory_order_release);
  DEBUG_ASSERT_MSG(old_active > 0, "old_active must be greater than 0, got %u", old_active);
  kcounter_add(owq_pool_excess_capacity, 1);

  // This would be a reasonable place to free unneeded OWQs, but we lack a mechanism to guarantee
  // that no thread exit frees a queue from the pool that is still being used by a stale chainlock
  // transaction.
  // TODO(https://fxbug.dev/488166372) actually free excess OWQs
}

OwnedWaitQueue* OwnedWaitQueuePool::BorrowLocked() {
  OwnedWaitQueue* queue = free_list_.pop_front();
  ASSERT_MSG(queue != nullptr, "Pool cannot be empty.");
  return queue;
}

void OwnedWaitQueuePool::RecycleLocked(OwnedWaitQueue* queue) {
  DEBUG_ASSERT_MSG(queue != nullptr, "Cannot recycle a null queue.");
  queue->AssertSafeForPooling();

  // Pushing a recycled queue onto the back of the list means that we are much less likely to
  // immediately hand the same queue back out in a BorrowLocked call while another thread
  // still sees the pointer in its LazyOWQ. Mutex code checks that it still has the same
  // pointer in its LazyOWQ *after* acquiring the OWQ's lock which means that a just-recycled
  // OWQ's lock may be briefly acquired after being recycled before the caller realizes that the
  // OWQ pointer is stale. We don't want those accesses to contend with legitimate attempts to lock
  // the queue from other threads that just allocated the queue for themselves from the front of the
  // list.
  free_list_.push_back(queue);
}

int64_t OwnedWaitQueuePool::GetExcessCapacityForTest() {
  return owq_pool_excess_capacity.SumAcrossAllCpus();
}
