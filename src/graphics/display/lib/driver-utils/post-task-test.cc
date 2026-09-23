// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/lib/driver-utils/post-task.h"

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async/dispatcher.h>
#include <lib/fit/function.h>
#include <lib/zx/result.h>
#include <lib/zx/time.h>
#include <threads.h>
#include <zircon/assert.h>
#include <zircon/compiler.h>
#include <zircon/errors.h>
#include <zircon/status.h>

#include <atomic>
#include <cstdio>
#include <memory>
#include <thread>

#include <fbl/auto_lock.h>
#include <fbl/condition_variable.h>
#include <fbl/mutex.h>
#include <gtest/gtest.h>

#include "src/lib/testing/predicates/status.h"

namespace display {

namespace {

TEST(CallFromDestructorTest, CallsCallback) {
  bool callback_called = false;
  {
    CallFromDestructor cleanup([&] { callback_called = true; });
    EXPECT_FALSE(callback_called);
  }
  EXPECT_TRUE(callback_called);
}

TEST(CallFromDestructorTest, CallsCallbackAsCapture) {
  static constexpr size_t kCaptureSize = 16;
  bool callback_called = false;
  {
    fit::inline_callback<void(), kCaptureSize> cleanup_in_captures(
        [&, cleanup = CallFromDestructor([&] { callback_called = true; })] {});
    EXPECT_FALSE(callback_called);
  }
  EXPECT_TRUE(callback_called);
}

TEST(CallFromDestructorTest, MoveConstructor) {
  int callback_call_count = 0;
  {
    CallFromDestructor moved_from([&] { ++callback_call_count; });
    {
      CallFromDestructor moved_to = std::move(moved_from);
      EXPECT_EQ(callback_call_count, 0) << "Moving should not invoke the callback";
    }
    EXPECT_EQ(callback_call_count, 1)
        << "Destroying the moved-to instance should invoke the callback";
  }
  EXPECT_EQ(callback_call_count, 1)
      << "Destroying the moved-from instance should not invoke the callback";
}

TEST(CallFromDestructorTest, DoesNotCallCallbackAfterCallbackMovedFrom) {
  static constexpr size_t kCaptureSize = 16;
  bool callback_called = false;
  fit::inline_callback<void(), kCaptureSize> moved_to;
  {
    fit::inline_callback<void(), kCaptureSize> moved_from(
        [&, cleanup = CallFromDestructor([&] { callback_called = true; })] {});
    moved_to = std::move(moved_from);
    EXPECT_FALSE(callback_called);
  }
  EXPECT_FALSE(callback_called);
}

// Blocks a thread until an action is completed, usually on another thread.
class Barrier {
 public:
  Barrier() = default;
  Barrier(const Barrier&) = delete;
  Barrier(Barrier&&) = delete;
  Barrier& operator=(const Barrier&) = delete;
  Barrier& operator=(Barrier&&) = delete;
  ~Barrier() = default;

  // Signals the barrier.
  //
  // Must be called at most once.
  void Signal() {
    fbl::AutoLock lock(&mutex_);
    ZX_ASSERT(!signaled_);
    signaled_ = true;
    signal_.Signal();
  }

  // Blocks until the barrier is signaled.
  void WaitUntilSignaled() {
    fbl::AutoLock lock(&mutex_);
    while (!signaled_) {
      signal_.Wait(&mutex_);
    }
  }

 private:
  fbl::Mutex mutex_;
  fbl::ConditionVariable signal_ __TA_GUARDED(mutex_);
  bool signaled_ __TA_GUARDED(mutex_) = false;
};

class PostTaskTest : public testing::TestWithParam<bool> {
 public:
  PostTaskTest()
      : is_loop_on_main_thread_(GetParam()),
        main_thread_(thrd_current()),
        loop_(LoopConfig()),
        dispatcher_(*loop_.dispatcher()) {}
  ~PostTaskTest() override = default;

  void SetUp() override {
    if (is_loop_on_main_thread_) {
      loop_thread_ = main_thread_;
      return;
    }
    ASSERT_OK(loop_.StartThread("PostTaskTest-dispatcher", &loop_thread_));
  }

  void TearDown() override {
    loop_.Shutdown();
    if (loop_shutdown_poll_thread_ != nullptr) {
      loop_shutdown_poll_thread_->join();
    }
  }

  void RecordCaptureDestruction() {
    captures_destroyed_.store(true, std::memory_order_relaxed);
    capture_destruction_thread_.store(thrd_current(), std::memory_order_relaxed);
  }
  bool CapturesDestroyed() { return captures_destroyed_.load(std::memory_order_relaxed); }
  thrd_t CapturesDestructionThread() {
    return capture_destruction_thread_.load(std::memory_order_relaxed);
  }

  // Causes WaitForAsyncWorkToBeDone() to return.
  void AsyncWorkDone() {
    ZX_ASSERT(thrd_equal(loop_thread_, thrd_current()));
    async_work_barrier_.Signal();

    if (is_loop_on_main_thread_) {
      loop_.Quit();
    }
  }

  // Blocks until AsyncWorkDone() is called.
  void WaitForAsyncWorkToBeDone() {
    ZX_ASSERT(thrd_equal(main_thread_, thrd_current()));
    if (is_loop_on_main_thread_) {
      zx_status_t loop_run_status = loop_.Run();
      if (loop_run_status != ZX_ERR_CANCELED) {
        fprintf(stderr, "async::Loop::Run() returned unexpected status: %s\n",
                zx_status_get_string(loop_run_status));
      }
    }

    async_work_barrier_.WaitUntilSignaled();
  }

  // Blocks until `loop_` shuts down.
  //
  // This relies on the main thread having called PollForLoopShutdown().
  void WaitForLoopShutdown() {
    ZX_ASSERT(thrd_equal(loop_thread_, thrd_current()));

    // When the loop is on the main thread, WaitForAsyncWorkToBeDone() returns
    // after task processing is complete. So, the main thread cannot call
    // Shutdown() if a task blocks.
    if (is_loop_on_main_thread_) {
      return;
    }
    loop_shutdown_barrier_.WaitUntilSignaled();
  }

  // Spins up a thread that fires the signal for WaitForLoopShutdown().
  void PollForLoopShutdown() {
    ZX_ASSERT(thrd_equal(main_thread_, thrd_current()));
    ZX_ASSERT(loop_shutdown_poll_thread_ == nullptr);

    // Capturing `this` is safe because TearDown() will join the spawned thread,
    // ensuring that the thread does not outlive `this`.
    loop_shutdown_poll_thread_ = std::make_unique<std::thread>([this] {
      while (loop_.GetState() != ASYNC_LOOP_SHUTDOWN) {
        zx::nanosleep(zx::deadline_after(zx::msec(1)));
      }
      loop_shutdown_barrier_.Signal();
    });
  }

  const async_loop_config_t* LoopConfig() const {
    return is_loop_on_main_thread_ ? &kAsyncLoopConfigAttachToCurrentThread
                                   : &kAsyncLoopConfigNeverAttachToThread;
  }

 protected:
  const bool is_loop_on_main_thread_;
  const thrd_t main_thread_;
  async::Loop loop_;
  async_dispatcher_t& dispatcher_;

  // Populated in SetUp().
  thrd_t loop_thread_;

  Barrier async_work_barrier_;
  Barrier loop_shutdown_barrier_;
  std::unique_ptr<std::thread> loop_shutdown_poll_thread_;

  // Used by RecordCaptureDestruction() and CapturesDestroyed*().
  std::atomic<bool> captures_destroyed_ = false;
  std::atomic<thrd_t> capture_destruction_thread_;
};

TEST_P(PostTaskTest, RunsCallback) {
  static constexpr size_t kCaptureSize = 16;
  std::atomic<bool> callback_called = false;
  zx::result<> post_task_result = PostTask<kCaptureSize>(dispatcher_, [&]() {
    callback_called.store(true, std::memory_order_relaxed);
    AsyncWorkDone();
  });
  ASSERT_OK(post_task_result);

  WaitForAsyncWorkToBeDone();
  EXPECT_TRUE(callback_called.load(std::memory_order_relaxed));
}

TEST_P(PostTaskTest, DoesNotRunCallbackSynchronously) {
  static constexpr size_t kCaptureSize = 16;

  // Stall the loop until `loop_barrier` is signaled. This gives us some time to
  // check state between the time we issue the second PostTask() call and the
  // time the loop processes the task.
  Barrier loop_barrier;
  zx::result<> post_stall_result =
      PostTask<kCaptureSize>(dispatcher_, [&]() { loop_barrier.WaitUntilSignaled(); });
  ASSERT_OK(post_stall_result);

  std::atomic<bool> callback_called = false;
  zx::result<> post_task_result = PostTask<kCaptureSize>(dispatcher_, [&]() {
    callback_called.store(true, std::memory_order_relaxed);
    AsyncWorkDone();
  });
  ASSERT_OK(post_task_result);
  EXPECT_FALSE(callback_called.load(std::memory_order_relaxed));

  loop_barrier.Signal();
  WaitForAsyncWorkToBeDone();
  EXPECT_TRUE(callback_called.load(std::memory_order_relaxed));
}

TEST_P(PostTaskTest, CaptureDestructionOnCallbackRun) {
  static constexpr size_t kCaptureSize = 32;

  Barrier loop_barrier;
  zx::result<> post_stall_result =
      PostTask<kCaptureSize>(dispatcher_, [&]() { loop_barrier.WaitUntilSignaled(); });
  ASSERT_OK(post_stall_result);

  std::atomic<bool> captures_destroyed_during_callback_call = false;
  zx::result<> post_task_result = PostTask<kCaptureSize>(
      dispatcher_, [&, _ = CallFromDestructor([&] { RecordCaptureDestruction(); })]() {
        captures_destroyed_during_callback_call.store(CapturesDestroyed(),
                                                      std::memory_order_relaxed);
      });
  ASSERT_OK(post_task_result);
  EXPECT_FALSE(CapturesDestroyed());

  // AsyncWorkDone() must be called in a separate task, because the test covers
  // the CallFromDestructor callback that runs after the main task's callback
  // returns.
  std::atomic<bool> captures_destroyed_during_done_call = true;
  zx::result<> post_done_result = PostTask(dispatcher_, [&]() {
    captures_destroyed_during_done_call.store(CapturesDestroyed(), std::memory_order_relaxed);
    AsyncWorkDone();
  });
  ASSERT_OK(post_done_result);
  EXPECT_FALSE(CapturesDestroyed());

  loop_barrier.Signal();
  WaitForAsyncWorkToBeDone();
  EXPECT_FALSE(captures_destroyed_during_callback_call.load(std::memory_order_relaxed));
  EXPECT_TRUE(captures_destroyed_during_done_call.load(std::memory_order_relaxed));

  ASSERT_TRUE(CapturesDestroyed());
  EXPECT_TRUE(thrd_equal(loop_thread_, CapturesDestructionThread()));
}

TEST_P(PostTaskTest, PostAfterShutdown) {
  static constexpr size_t kCaptureSize = 16;
  loop_.Shutdown();

  std::atomic<bool> callback_called = false;
  zx::result<> post_task_result = PostTask<kCaptureSize>(
      dispatcher_, [&]() { callback_called.store(true, std::memory_order_relaxed); });
  EXPECT_STATUS(post_task_result, zx::error(ZX_ERR_BAD_STATE));
  EXPECT_FALSE(callback_called.load(std::memory_order_relaxed));
}

TEST_P(PostTaskTest, CaptureDestructionOnPostAfterShutdown) {
  static constexpr size_t kCaptureSize = 16;
  loop_.Shutdown();

  zx::result<> post_task_result = PostTask<kCaptureSize>(
      dispatcher_, [&, _ = CallFromDestructor([&] { RecordCaptureDestruction(); })]() {});
  EXPECT_STATUS(post_task_result, zx::error(ZX_ERR_BAD_STATE));

  ASSERT_TRUE(CapturesDestroyed());
  EXPECT_TRUE(thrd_equal(main_thread_, CapturesDestructionThread()));
}

TEST_P(PostTaskTest, ShutdownAfterPostBeforeCall) {
  static constexpr size_t kCaptureSize = 16;

  // Stall the loop until Shutdown is called(). This ensures that Shutdown()
  // runs before the loop processes the "main" task posted below.
  PollForLoopShutdown();
  zx::result<> post_stall_result = PostTask<kCaptureSize>(dispatcher_, [&] {
    AsyncWorkDone();
    WaitForLoopShutdown();
  });
  ASSERT_OK(post_stall_result);

  std::atomic<bool> callback_called = false;
  zx::result<> post_task_result = PostTask<kCaptureSize>(
      dispatcher_, [&]() { callback_called.store(true, std::memory_order_relaxed); });
  ASSERT_OK(post_task_result);
  EXPECT_FALSE(callback_called.load(std::memory_order_relaxed));

  WaitForAsyncWorkToBeDone();
  loop_.Shutdown();
  EXPECT_FALSE(callback_called.load(std::memory_order_relaxed));
}

TEST_P(PostTaskTest, CaptureDestructionOnShutdownAfterPostBeforeCall) {
  static constexpr size_t kCaptureSize = 16;

  PollForLoopShutdown();
  zx::result<> post_stall_result = PostTask<kCaptureSize>(dispatcher_, [&] {
    AsyncWorkDone();
    WaitForLoopShutdown();
  });
  ASSERT_OK(post_stall_result);

  zx::result<> post_task_result = PostTask<kCaptureSize>(
      dispatcher_, [&, _ = CallFromDestructor([&] { RecordCaptureDestruction(); })]() {});
  ASSERT_OK(post_task_result);
  EXPECT_FALSE(CapturesDestroyed());

  WaitForAsyncWorkToBeDone();
  loop_.Shutdown();
  ASSERT_TRUE(CapturesDestroyed());
  EXPECT_TRUE(thrd_equal(loop_thread_, CapturesDestructionThread()));
}

INSTANTIATE_TEST_SUITE_P(, PostTaskTest, testing::Values(true, false));

class PostTaskStateTest : public PostTaskTest {};

TEST_P(PostTaskStateTest, DiscardingUnusedInstanceDoesNotCrash) {
  static constexpr size_t kCaptureSize = 16;
  auto post_task_state = std::make_unique<PostTaskState<kCaptureSize>>();
  post_task_state.reset();
}

TEST_P(PostTaskStateTest, RunsCallback) {
  static constexpr size_t kCaptureSize = 16;
  auto post_task_state = std::make_unique<PostTaskState<kCaptureSize>>();

  std::atomic<bool> callback_called = false;
  zx::result<> post_task_result = PostTask(std::move(post_task_state), dispatcher_, [&]() {
    callback_called.store(true, std::memory_order_relaxed);
    AsyncWorkDone();
  });
  ASSERT_OK(post_task_result);

  WaitForAsyncWorkToBeDone();
  EXPECT_TRUE(callback_called.load(std::memory_order_relaxed));
}

TEST_P(PostTaskStateTest, DoesNotRunCallbackSynchronously) {
  static constexpr size_t kCaptureSize = 16;
  auto post_stall_state = std::make_unique<PostTaskState<kCaptureSize>>();
  auto post_task_state = std::make_unique<PostTaskState<kCaptureSize>>();

  // Stall the loop until `loop_barrier` is signaled. This gives us some time to
  // check state between the time we issue the second PostTask() call and the
  // time the loop processes the task.
  Barrier loop_barrier;
  zx::result<> post_stall_result = PostTask(std::move(post_stall_state), dispatcher_,
                                            [&]() { loop_barrier.WaitUntilSignaled(); });
  ASSERT_OK(post_stall_result);

  std::atomic<bool> callback_called = false;
  zx::result<> post_task_result = PostTask(std::move(post_task_state), dispatcher_, [&]() {
    callback_called.store(true, std::memory_order_relaxed);
    AsyncWorkDone();
  });
  ASSERT_OK(post_task_result);
  EXPECT_FALSE(callback_called.load(std::memory_order_relaxed));

  loop_barrier.Signal();
  WaitForAsyncWorkToBeDone();
  EXPECT_TRUE(callback_called.load(std::memory_order_relaxed));
}

TEST_P(PostTaskStateTest, CaptureDestructionOnCallbackRun) {
  static constexpr size_t kCaptureSize = 32;
  auto post_stall_state = std::make_unique<PostTaskState<kCaptureSize>>();
  auto post_task_state = std::make_unique<PostTaskState<kCaptureSize>>();
  auto post_done_state = std::make_unique<PostTaskState<kCaptureSize>>();

  Barrier loop_barrier;
  zx::result<> post_stall_result =
      PostTask<kCaptureSize>(dispatcher_, [&]() { loop_barrier.WaitUntilSignaled(); });
  ASSERT_OK(post_stall_result);

  std::atomic<bool> captures_destroyed_during_callback_call = false;
  zx::result<> post_task_result =
      PostTask(std::move(post_task_state), dispatcher_,
               [&, _ = CallFromDestructor([&] { RecordCaptureDestruction(); })]() {
                 captures_destroyed_during_callback_call.store(CapturesDestroyed(),
                                                               std::memory_order_relaxed);
               });
  ASSERT_OK(post_task_result);
  EXPECT_FALSE(CapturesDestroyed());

  // AsyncWorkDone() must be called in a separate task, because the test covers
  // the CallFromDestructor callback that runs after the main task's callback
  // returns.
  std::atomic<bool> captures_destroyed_during_done_call = true;
  zx::result<> post_done_result = PostTask(std::move(post_done_state), dispatcher_, [&]() {
    captures_destroyed_during_done_call.store(CapturesDestroyed(), std::memory_order_relaxed);
    AsyncWorkDone();
  });
  ASSERT_OK(post_done_result);
  EXPECT_FALSE(CapturesDestroyed());

  loop_barrier.Signal();
  WaitForAsyncWorkToBeDone();
  EXPECT_FALSE(captures_destroyed_during_callback_call.load(std::memory_order_relaxed));
  EXPECT_TRUE(captures_destroyed_during_done_call.load(std::memory_order_relaxed));

  ASSERT_TRUE(CapturesDestroyed());
  EXPECT_TRUE(thrd_equal(loop_thread_, CapturesDestructionThread()));
}

TEST_P(PostTaskStateTest, PostAfterShutdown) {
  static constexpr size_t kCaptureSize = 16;
  auto post_task_state = std::make_unique<PostTaskState<kCaptureSize>>();
  loop_.Shutdown();

  std::atomic<bool> callback_called = false;
  zx::result<> post_task_result = PostTask(std::move(post_task_state), dispatcher_, [&]() {
    callback_called.store(true, std::memory_order_relaxed);
  });
  EXPECT_STATUS(post_task_result, zx::error(ZX_ERR_BAD_STATE));
  EXPECT_FALSE(callback_called.load(std::memory_order_relaxed));
}

TEST_P(PostTaskStateTest, CaptureDestructionOnSuccessfulRunOnPostAfterShutdown) {
  static constexpr size_t kCaptureSize = 16;
  auto post_task_state = std::make_unique<PostTaskState<kCaptureSize>>();
  loop_.Shutdown();

  zx::result<> post_task_result =
      PostTask(std::move(post_task_state), dispatcher_,
               [&, _ = CallFromDestructor([&] { RecordCaptureDestruction(); })]() {});
  EXPECT_STATUS(post_task_result, zx::error(ZX_ERR_BAD_STATE));
  ASSERT_TRUE(CapturesDestroyed());
  EXPECT_TRUE(thrd_equal(main_thread_, CapturesDestructionThread()));
}

TEST_P(PostTaskStateTest, ShutdownAfterPostBeforeCall) {
  static constexpr size_t kCaptureSize = 16;
  auto post_stall_state = std::make_unique<PostTaskState<kCaptureSize>>();
  auto post_task_state = std::make_unique<PostTaskState<kCaptureSize>>();

  // Stall the loop until Shutdown is called(). This ensures that Shutdown()
  // runs before the loop processes the "main" task posted below.
  PollForLoopShutdown();
  zx::result<> post_stall_result = PostTask(std::move(post_stall_state), dispatcher_, [&] {
    AsyncWorkDone();
    WaitForLoopShutdown();
  });
  ASSERT_OK(post_stall_result);

  std::atomic<bool> callback_called = false;
  zx::result<> post_task_result = PostTask(std::move(post_task_state), dispatcher_, [&]() {
    callback_called.store(true, std::memory_order_relaxed);
  });
  ASSERT_OK(post_task_result);

  WaitForAsyncWorkToBeDone();
  loop_.Shutdown();
  EXPECT_FALSE(callback_called.load(std::memory_order_relaxed));
}

TEST_P(PostTaskStateTest, CaptureDestructionOnShutdownAfterPostBeforeCall) {
  static constexpr size_t kCaptureSize = 16;
  auto post_stall_state = std::make_unique<PostTaskState<kCaptureSize>>();
  auto post_task_state = std::make_unique<PostTaskState<kCaptureSize>>();

  PollForLoopShutdown();
  zx::result<> post_stall_result = PostTask(std::move(post_stall_state), dispatcher_, [&] {
    AsyncWorkDone();
    WaitForLoopShutdown();
  });
  ASSERT_OK(post_stall_result);

  zx::result<> post_task_result =
      PostTask(std::move(post_task_state), dispatcher_,
               [&, _ = CallFromDestructor([&] { RecordCaptureDestruction(); })]() {});
  ASSERT_OK(post_task_result);
  EXPECT_FALSE(CapturesDestroyed());

  WaitForAsyncWorkToBeDone();
  loop_.Shutdown();
  ASSERT_TRUE(CapturesDestroyed());
  EXPECT_TRUE(thrd_equal(loop_thread_, CapturesDestructionThread()));
}

INSTANTIATE_TEST_SUITE_P(, PostTaskStateTest, testing::Values(true, false));

}  // namespace

}  // namespace display
