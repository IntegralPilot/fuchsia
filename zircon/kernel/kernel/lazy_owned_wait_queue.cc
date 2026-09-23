// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/counters.h>
#include <lib/lazy_init/lazy_init.h>
#include <lib/object_cache.h>

#include <arch/ops.h>
#include <fbl/alloc_checker.h>
#include <kernel/lazy_owned_wait_queue.h>
#include <kernel/owned_wait_queue.h>
#include <kernel/owned_wait_queue_pool.h>
#include <kernel/spinlock.h>
#include <lk/init.h>

OwnedWaitQueuePool& GlobalOwqPoolAccess::get_pool() const { return OwnedWaitQueuePool::Get(); }

template <typename PoolAccess>
LazyOwnedWaitQueueImpl<PoolAccess>::~LazyOwnedWaitQueueImpl() {
  ASSERT_MSG(Peek() == nullptr, "Cannot destroy a LazyOwnedWaitQueue that has an OwnedWaitQueue.");
}

template <typename PoolAccess>
OwnedWaitQueue* LazyOwnedWaitQueueImpl<PoolAccess>::Get() {
  OwnedWaitQueue* current = ptr_.load(ktl::memory_order_acquire);
  if (current != nullptr) {
    return current;
  }

  Guard<SpinLock, IrqSave> guard{OwnedWaitQueuePoolLock::Get()};

  current = ptr_.load(ktl::memory_order_relaxed);
  if (current != nullptr) {
    return current;
  }

  OwnedWaitQueue* new_queue = this->get_pool().BorrowLocked();

  ptr_.store(new_queue, ktl::memory_order_release);
  return new_queue;
}

template <typename PoolAccess>
void LazyOwnedWaitQueueImpl<PoolAccess>::ReleaseLocked() {
  OwnedWaitQueue* ptr = Detach();
  if (ptr != nullptr) {
    this->get_pool().RecycleLocked(ptr);
  }
}

template <typename PoolAccess>
OwnedWaitQueue* LazyOwnedWaitQueueImpl<PoolAccess>::Detach() {
  return ptr_.exchange(nullptr, ktl::memory_order_acq_rel);
}

template class LazyOwnedWaitQueueImpl<GlobalOwqPoolAccess>;
template class LazyOwnedWaitQueueImpl<LocalOwqPoolAccess>;
