// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async-loop/cpp/loop.h>
#include <lib/fit/defer.h>
#include <lib/sync/cpp/completion.h>
#include <lib/zx/time.h>

#include <latch>
#include <list>
#include <semaphore>
#include <thread>

#include <gtest/gtest.h>
#include <sdk/lib/async_patterns/cpp/internal/task_queue.h>

#include "src/lib/testing/predicates/status.h"

namespace {

using async_patterns::internal::Task;
using async_patterns::internal::TaskQueue;
using async_patterns::internal::TaskQueueHandle;

TEST(TaskQueue, MustStopBeforeDestruction) {
  ASSERT_DEATH(
      {
        async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);
        TaskQueue queue(loop.dispatcher(), "");
      },
      "");
}

TEST(TaskQueue, Add) {
  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);
  TaskQueue queue(loop.dispatcher(), "");
  bool called = false;
  queue.Add(Task::Box([&] { called = true; }));
  EXPECT_OK(loop.RunUntilIdle());
  EXPECT_TRUE(called);
  queue.Stop();
}

TEST(TaskQueue, StopBeforeAdd) {
  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);
  TaskQueue queue(loop.dispatcher(), "");
  queue.Stop();
  for (size_t i = 0; i < 3; i++) {
    bool called = false;
    queue.Add(Task::Box([&] { called = true; }));
    EXPECT_OK(loop.RunUntilIdle());
    EXPECT_FALSE(called);
  }
}

TEST(TaskQueue, StopAfterAdd) {
  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);
  TaskQueue queue(loop.dispatcher(), "");
  bool called = false;
  queue.Add(Task::Box([&] { called = true; }));
  queue.Stop();
  EXPECT_OK(loop.RunUntilIdle());
  EXPECT_FALSE(called);
}

TEST(TaskQueue, DispatcherShutdownBeforeAdd) {
  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);
  TaskQueue queue(loop.dispatcher(), "");
  loop.Shutdown();
  for (size_t i = 0; i < 3; i++) {
    bool called = false;
    bool dropped = false;
    queue.Add(Task::Box([&, on_drop = fit::defer([&] { dropped = true; })] { called = true; }));
    EXPECT_FALSE(called);
    EXPECT_TRUE(dropped);
  }
  queue.Stop();
}

TEST(TaskQueue, DispatcherShutdownAfterAdd) {
  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);
  TaskQueue queue(loop.dispatcher(), "");
  bool called = false;
  bool dropped = false;
  queue.Add(Task::Box([&, on_drop = fit::defer([&] { dropped = true; })] { called = true; }));
  loop.Shutdown();
  EXPECT_FALSE(called);
  EXPECT_TRUE(dropped);
  queue.Stop();
}

TEST(TaskQueue, AddFromOtherThread) {
  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);
  auto queue = std::make_shared<TaskQueue>(loop.dispatcher(), "");

  libsync::Completion added;
  libsync::Completion called;

  std::thread::id main_thread_id = std::this_thread::get_id();
  std::optional<std::thread::id> task_thread_id;

  std::thread t([handle = TaskQueueHandle(queue), &added, &called, &task_thread_id] {
    handle.Add([&called, &task_thread_id] {
      task_thread_id.emplace(std::this_thread::get_id());
      called.Signal();
    });
    added.Signal();
  });

  added.Wait();
  EXPECT_FALSE(called.signaled());

  EXPECT_OK(loop.RunUntilIdle());
  EXPECT_TRUE(called.signaled());

  EXPECT_EQ(main_thread_id, task_thread_id.value());

  queue->Stop();
  t.join();
}

struct TaskAdderThreadPool {
 public:
  static constexpr size_t kNumThreads = 50;
  static constexpr size_t kMinimumNumberOfTasks = kNumThreads * 1000;

  // If the scheduler doesn't schedule the main thread as much, we might OOM
  // the system as the worker threads keep queuing tests. This is a backstop
  // against that. Assuming each task is 64 bytes, we should not use more than
  // 256 MiB of RAM.
  static constexpr size_t kMaximumNumberOfOutstandingTasks = 256 * 1024 * 1024 / 64;

  explicit TaskAdderThreadPool(const TaskQueueHandle& handle) {
    for (size_t i = 0; i < kNumThreads; i++) {
      threads_.emplace_back([this, handle] {
        entered_.count_down();
        start_.Wait();
        for (;;) {
          outstanding_tasks_.acquire();
          handle.Add([this, on_dropped = fit::defer([this] { dropped_count_.fetch_add(1); }),
                      destroyed = fit::defer([this] { outstanding_tasks_.release(); })]() mutable {
            ran_count_++;
            on_dropped.cancel();
          });
          if (quit_.load()) {
            return;
          }
        }
      });
    }

    entered_.wait();
    start_.Signal();
  }

  ~TaskAdderThreadPool() {
    quit_.store(true);
    for (auto& t : threads_) {
      t.join();
    }
  }

  size_t ran_count() const { return ran_count_; }
  const std::atomic_size_t& dropped_count() const { return dropped_count_; }

 private:
  std::latch entered_{kNumThreads};
  libsync::Completion start_;
  std::atomic_bool quit_{false};
  size_t ran_count_ = 0;
  std::atomic_size_t dropped_count_{0};
  std::counting_semaphore<> outstanding_tasks_{kMaximumNumberOfOutstandingTasks};
  std::list<std::thread> threads_;
};

// A mini-stress test to attempt to exercise as many thread interleavings as possible.
//
// Worth-noting this test does not use RunUntilIdle. When many threads are posting
// work into the loop, RunUntilIdle may live lock where it is forever processing
// new tasks.
TEST(TaskQueue, AddFromManyThreads) {
  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);
  auto queue = std::make_shared<TaskQueue>(loop.dispatcher(), "");

  TaskAdderThreadPool pool{TaskQueueHandle{queue}};

  // Run tasks in batches for a while.
  for (;;) {
    loop.Run(zx::time::infinite_past(), /* once */ true);
    EXPECT_EQ(pool.dropped_count().load(), 0u);
    if (pool.ran_count() >= TaskAdderThreadPool::kMinimumNumberOfTasks) {
      break;
    }
  }

  queue->Stop();

  // Observe enough tasks dropped for a while.
  for (;;) {
    loop.Run(zx::time::infinite_past(), /* once */ true);
    if (pool.dropped_count().load() >= TaskAdderThreadPool::kMinimumNumberOfTasks) {
      break;
    }
  }
}

TEST(TaskQueue, AddFromManyThreadsButDispatcherShutsDown) {
  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);
  auto queue = std::make_shared<TaskQueue>(loop.dispatcher(), "");

  TaskAdderThreadPool pool{TaskQueueHandle{queue}};

  // Run tasks in batches for a while.
  for (;;) {
    loop.Run(zx::time::infinite_past(), /* once */ true);
    EXPECT_EQ(pool.dropped_count().load(), 0u);
    if (pool.ran_count() >= TaskAdderThreadPool::kMinimumNumberOfTasks) {
      break;
    }
  }

  loop.Shutdown();

  // Observe enough tasks dropped for a while.
  for (;;) {
    zx::nanosleep(zx::deadline_after(zx::usec(1)));
    if (pool.dropped_count().load() >= TaskAdderThreadPool::kMinimumNumberOfTasks) {
      break;
    }
  }

  queue->Stop();
}

}  // namespace
