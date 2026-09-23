// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/unittest/unittest.h>

#include <kernel/lazy_owned_wait_queue.h>
#include <kernel/owned_wait_queue.h>
#include <kernel/owned_wait_queue_pool.h>

namespace {

bool test_lazy_wrapper() {
  BEGIN_TEST;

  // Ensure local pool has capacity for our test.
  OwnedWaitQueuePool stack_pool;
  EXPECT_EQ(ZX_OK, stack_pool.Grow());

  {
    LocalLazyOwnedWaitQueue lazy(stack_pool);
    EXPECT_NULL(lazy.Peek());

    OwnedWaitQueue* q;
    {
      q = lazy.Get();
    }
    EXPECT_NONNULL(q);
    EXPECT_EQ(q, lazy.Peek());

    // Second Get should return same pointer.
    {
      EXPECT_EQ(q, lazy.Get());
    }

    {
      Guard<SpinLock, IrqSave> guard{OwnedWaitQueuePoolLock::Get()};
      lazy.ReleaseLocked();
    }
    EXPECT_NULL(lazy.Peek());
  }

  {
    Guard<SpinLock, IrqSave> guard{OwnedWaitQueuePoolLock::Get()};
    stack_pool.BorrowLocked();
  }

  END_TEST;
}

bool test_concurrent_get() {
  BEGIN_TEST;

  OwnedWaitQueuePool stack_pool;
  constexpr int kNumThreads = 4;
  // We need to ensure the local pool has enough items for all threads to potentially
  // Borrow() once, even though only one execution will win the race.
  // The losers will Recycle() their queues.
  for (int i = 0; i < kNumThreads; ++i) {
    EXPECT_EQ(ZX_OK, stack_pool.Grow());
  }

  {
    LocalLazyOwnedWaitQueue lazy(stack_pool);
    Thread* threads[kNumThreads];
    OwnedWaitQueue* results[kNumThreads];

    struct WorkerArgs {
      LocalLazyOwnedWaitQueue* lazy;
      OwnedWaitQueue** result_slot;
    };

    WorkerArgs args[kNumThreads];

    for (int i = 0; i < kNumThreads; ++i) {
      args[i] = {&lazy, &results[i]};
      threads[i] = Thread::Create(
          "lazy_race_get_worker",
          [](void* arg) -> int {
            auto* a = static_cast<WorkerArgs*>(arg);
            *a->result_slot = a->lazy->Get();
            return 0;
          },
          &args[i], DEFAULT_PRIORITY);
      ASSERT_NONNULL(threads[i]);
      threads[i]->Resume();
    }

    // Join all.
    for (int i = 0; i < kNumThreads; ++i) {
      int ret = 0;
      EXPECT_EQ(ZX_OK, threads[i]->Join(&ret, ZX_TIME_INFINITE));
    }

    // Verify all got the same queue.
    OwnedWaitQueue* winner = results[0];
    EXPECT_NONNULL(winner);
    for (int i = 1; i < kNumThreads; ++i) {
      EXPECT_EQ(winner, results[i]);
    }

    EXPECT_EQ(winner, lazy.Peek());
    {
      Guard<SpinLock, IrqSave> guard{OwnedWaitQueuePoolLock::Get()};
      lazy.ReleaseLocked();
    }
  }

  {
    Guard<SpinLock, IrqSave> guard{OwnedWaitQueuePoolLock::Get()};
    for (int i = 0; i < kNumThreads; ++i) {
      stack_pool.BorrowLocked();
    }
  }

  END_TEST;
}

bool test_pool_stress() {
  BEGIN_TEST;

  OwnedWaitQueuePool stack_pool;
  constexpr int kNumThreads = 256;
  constexpr int kIterations = 100000;

  // Pre-seed the local pool so threads don't starve immediately if they borrow.
  for (int i = 0; i < kNumThreads; ++i) {
    EXPECT_EQ(ZX_OK, stack_pool.Grow());
  }

  Thread* threads[kNumThreads];

  for (int i = 0; i < kNumThreads; ++i) {
    threads[i] = Thread::Create(
        "pool_stress_worker",
        [](void* arg) -> int {
          auto* pool = static_cast<OwnedWaitQueuePool*>(arg);
          for (int j = 0; j < kIterations; ++j) {
            LocalLazyOwnedWaitQueue lazy(*pool);
            lazy.Get();
            Guard<SpinLock, IrqSave> guard{OwnedWaitQueuePoolLock::Get()};
            lazy.ReleaseLocked();
          }
          return 0;
        },
        &stack_pool, DEFAULT_PRIORITY);
    ASSERT_NONNULL(threads[i]);
    threads[i]->Resume();
  }

  for (int i = 0; i < kNumThreads; ++i) {
    int ret = 0;
    EXPECT_EQ(ZX_OK, threads[i]->Join(&ret, ZX_TIME_INFINITE));
  }

  {
    Guard<SpinLock, IrqSave> guard{OwnedWaitQueuePoolLock::Get()};
    for (int i = 0; i < kNumThreads; ++i) {
      stack_pool.BorrowLocked();
    }
  }

  END_TEST;
}

bool test_pool_counters() {
  BEGIN_TEST;

  {
    OwnedWaitQueuePool stack_pool;

    int64_t initial = stack_pool.GetExcessCapacityForTest();

    stack_pool.Grow();
    int64_t after_grow = stack_pool.GetExcessCapacityForTest();

    EXPECT_EQ(initial, after_grow);

    stack_pool.Shrink();

    int64_t after_shrink = stack_pool.GetExcessCapacityForTest();
    EXPECT_EQ(after_grow + 1, after_shrink);

    // Empty the pool so fbl::DoublyLinkedList doesn't assert upon destruction.
    // The pool was expanded by 1 from Grow() and recycled. So Borrow out the queue and leak it
    // or properly destruct it if Needed.
    Guard<SpinLock, IrqSave> guard{OwnedWaitQueuePoolLock::Get()};
    OwnedWaitQueue* queue = stack_pool.BorrowLocked();
    if (queue) {
      // Free it properly or rely on the cache to eventually clean it.
      // We don't have to recycle it back since we're intentionally draining it to bypass
      // DoublyLinkedList::~DoublyLinkedList() 'is_empty()' assertion.
    }
  }

  END_TEST;
}

}  // namespace

UNITTEST_START_TESTCASE(owned_wait_queue_pool_tests)
UNITTEST("lazy_wrapper", test_lazy_wrapper)
UNITTEST("concurrent_get", test_concurrent_get)
UNITTEST("pool_stress", test_pool_stress)
UNITTEST("pool_counters", test_pool_counters)
UNITTEST_END_TESTCASE(owned_wait_queue_pool_tests, "owned_wait_queue_pool",
                      "Tests for OwnedWaitQueuePool and LazyOwnedWaitQueue")
