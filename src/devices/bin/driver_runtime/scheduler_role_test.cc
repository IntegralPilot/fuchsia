// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async/cpp/irq.h>
#include <lib/async/cpp/task.h>
#include <lib/async/cpp/wait.h>
#include <lib/driver/testing/cpp/driver_runtime.h>
#include <lib/fdf/cpp/dispatcher.h>
#include <lib/fdf/cpp/env.h>
#include <lib/sync/cpp/completion.h>
#include <lib/zx/interrupt.h>

#include "src/devices/bin/driver_runtime/dispatcher.h"
#include "src/devices/bin/driver_runtime/dispatcher_coordinator.h"
#include "src/devices/bin/driver_runtime/runtime_test_case.h"
#include "src/devices/bin/driver_runtime/thread_context.h"

class ThreadPoolTest : public RuntimeTestCase {
 public:
  void SetUp() override;
  void TearDown() override;

  void InitTestDispatcher(std::string_view scheduler_role,
                          fdf::SynchronizedDispatcher::Options options = {});

  driver_runtime::Dispatcher* runtime_dispatcher() { return dispatcher_.get()->GetDispatcher(); }

 protected:
  fdf_testing::internal::DriverRuntimeEnv runtime_env;

  fdf::SynchronizedDispatcher dispatcher_;
  libsync::Completion shutdown_completion_;
};

void ThreadPoolTest::SetUp() {
  // Make sure each test starts with exactly one thread.
  driver_runtime::GetDispatcherCoordinator().Reset();
  ASSERT_EQ(ZX_OK, driver_runtime::GetDispatcherCoordinator().Start(0));
}

void ThreadPoolTest::TearDown() {
  if (dispatcher_.get() != nullptr) {
    dispatcher_.ShutdownAsync();
    shutdown_completion_.Wait();
  }
}

void ThreadPoolTest::InitTestDispatcher(std::string_view scheduler_role,
                                        fdf::SynchronizedDispatcher::Options options) {
  ZX_ASSERT(dispatcher_.get() == nullptr);

  auto fake_driver = CreateFakeDriver();
  auto shutdown_handler = [&](fdf_dispatcher_t* dispatcher) { shutdown_completion_.Signal(); };
  auto dispatcher = fdf_env::DispatcherBuilder::CreateSynchronizedWithOwner(
      fake_driver, options, "dispatcher", shutdown_handler, scheduler_role);
  ASSERT_FALSE(dispatcher.is_error());
  dispatcher_ = *std::move(dispatcher);
}

TEST_F(ThreadPoolTest, NoSchedulerRole) {
  constexpr std::string_view kSchedulerRole = "";
  ASSERT_NO_FATAL_FAILURE(InitTestDispatcher(kSchedulerRole));

  libsync::Completion task_completion;
  ASSERT_OK(async::PostTask(dispatcher_.async_dispatcher(), [&] {
    auto role_profile_status = thread_context::GetRoleProfileStatus();
    ASSERT_FALSE(role_profile_status.has_value());

    task_completion.Signal();
  }));
  task_completion.Wait();
}

TEST_F(ThreadPoolTest, WithSchedulerRole) {
  constexpr std::string_view kSchedulerRole = "fuchsia.default";
  ASSERT_NO_FATAL_FAILURE(InitTestDispatcher(kSchedulerRole));

  libsync::Completion task_completion;
  ASSERT_OK(async::PostTask(dispatcher_.async_dispatcher(), [&] {
    auto role_profile_status = thread_context::GetRoleProfileStatus();
    ASSERT_TRUE(role_profile_status.has_value());
    ASSERT_OK(role_profile_status.value());

    task_completion.Signal();
  }));
  task_completion.Wait();
}

TEST_F(ThreadPoolTest, BadSchedulerRole) {
  constexpr std::string_view kSchedulerRole = "fuchsia.test-role:not-found";
  ASSERT_NO_FATAL_FAILURE(InitTestDispatcher(kSchedulerRole));

  libsync::Completion task_completion;
  ASSERT_OK(async::PostTask(dispatcher_.async_dispatcher(), [&] {
    auto role_profile_status = thread_context::GetRoleProfileStatus();
    ASSERT_TRUE(role_profile_status.has_value());
    ASSERT_NE(ZX_OK, *role_profile_status);

    task_completion.Signal();
  }));
  task_completion.Wait();
}

TEST_F(ThreadPoolTest, AllowSyncCalls) {
  constexpr std::string_view kSchedulerRole = "fuchsia.default";
  ASSERT_NO_FATAL_FAILURE(
      InitTestDispatcher(kSchedulerRole, fdf::SynchronizedDispatcher::Options::kAllowSyncCalls));

  libsync::Completion task_completion;
  ASSERT_OK(async::PostTask(dispatcher_.async_dispatcher(), [&] {
    auto role_profile_status = thread_context::GetRoleProfileStatus();
    ASSERT_TRUE(role_profile_status.has_value());
    ASSERT_OK(role_profile_status.value());

    auto thread_pool = runtime_dispatcher()->thread_pool();

    ASSERT_EQ(thread_pool->scheduler_role(), kSchedulerRole);
    ASSERT_EQ(thread_pool->num_threads(), 1u);

    ASSERT_EQ(driver_runtime::GetDispatcherCoordinator().default_thread_pool()->num_threads(), 1u);

    task_completion.Signal();
  }));
  task_completion.Wait();
}

TEST_F(ThreadPoolTest, Shutdown) {
  constexpr std::string_view kSchedulerRole = "fuchsia.default";

  auto fake_driver = CreateFakeDriver();
  libsync::Completion shutdown_completion;
  auto shutdown_handler = [&](fdf_dispatcher_t* dispatcher) {
    auto role_profile_status = thread_context::GetRoleProfileStatus();
    ASSERT_TRUE(role_profile_status.has_value());
    ASSERT_OK(role_profile_status.value());

    shutdown_completion.Signal();
  };
  auto dispatcher = fdf_env::DispatcherBuilder::CreateSynchronizedWithOwner(
      fake_driver, {}, "dispatcher", shutdown_handler, kSchedulerRole);
  ASSERT_FALSE(dispatcher.is_error());

  dispatcher->ShutdownAsync();
  shutdown_completion.Wait();
}

class MultipleDispatchersThreadPoolTest : public RuntimeTestCase {
 public:
  void TearDown() override;

  zx::result<fdf::Unowned<fdf::SynchronizedDispatcher>> CreateTestDispatcher(
      std::string_view scheduler_role, fdf::SynchronizedDispatcher::Options options);

  driver_runtime::ThreadPool* GetThreadPool(fdf::Unowned<fdf::SynchronizedDispatcher>& dispatcher) {
    return dispatcher->get()->GetDispatcher()->thread_pool();
  }

 protected:
  std::vector<fdf::SynchronizedDispatcher> dispatchers_;
  libsync::Completion shutdown_completion_;

  fbl::Mutex lock_;
  uint32_t num_dispatchers_shutdown_ __TA_GUARDED(&lock_) = 0;
};

void MultipleDispatchersThreadPoolTest::TearDown() {
  for (auto& dispatcher : dispatchers_) {
    dispatcher.ShutdownAsync();
  }
  if (!dispatchers_.empty()) {
    shutdown_completion_.Wait();
  }
}

zx::result<fdf::Unowned<fdf::SynchronizedDispatcher>>
MultipleDispatchersThreadPoolTest::CreateTestDispatcher(
    std::string_view scheduler_role, fdf::SynchronizedDispatcher::Options options) {
  auto fake_driver = CreateFakeDriver();
  auto shutdown_handler = [&](fdf_dispatcher_t* dispatcher) {
    bool signal = false;
    {
      fbl::AutoLock lock(&lock_);
      num_dispatchers_shutdown_++;
      if (num_dispatchers_shutdown_ == dispatchers_.size()) {
        signal = true;
      }
    }
    if (signal) {
      shutdown_completion_.Signal();
    }
  };

  auto dispatcher = fdf_env::DispatcherBuilder::CreateSynchronizedWithOwner(
      fake_driver, options, "dispatcher", shutdown_handler, scheduler_role);
  if (dispatcher.is_error()) {
  }

  fdf::Unowned<fdf::SynchronizedDispatcher> unowned = dispatcher->borrow();
  dispatchers_.push_back(*std::move(dispatcher));
  return zx::ok(unowned);
}

TEST_F(MultipleDispatchersThreadPoolTest, ManyDispatchers) {
  constexpr std::string_view kSchedulerRole = "fuchsia.default";
  constexpr std::string_view kBadSchedulerRole = "fuchsia.test-role:not-found";
  constexpr uint32_t kNumDispatchers = 5;

  driver_runtime::ThreadPool* want_thread_pool = nullptr;

  for (uint32_t i = 0; i < kNumDispatchers; i++) {
    auto dispatcher = CreateTestDispatcher(kSchedulerRole, {});
    ASSERT_OK(dispatcher.status_value());

    auto thread_pool = GetThreadPool(*dispatcher);
    if (want_thread_pool == nullptr) {
      want_thread_pool = thread_pool;
    } else {
      ASSERT_EQ(want_thread_pool, thread_pool);
    }

    libsync::Completion task_completion;
    ASSERT_OK(async::PostTask(dispatcher->async_dispatcher(), [&] {
      auto role_profile_status = thread_context::GetRoleProfileStatus();
      ASSERT_TRUE(role_profile_status.has_value());
      ASSERT_OK(role_profile_status.value());

      task_completion.Signal();
    }));
    task_completion.Wait();
  }

  {
    auto dispatcher = CreateTestDispatcher(kBadSchedulerRole, {});
    ASSERT_OK(dispatcher.status_value());

    auto thread_pool = GetThreadPool(*dispatcher);
    ASSERT_NE(want_thread_pool, thread_pool);
  }
}

TEST_F(MultipleDispatchersThreadPoolTest, NumThreads) {
  constexpr std::string_view kSchedulerRole = "fuchsia.default";

  {
    auto dispatcher = CreateTestDispatcher(kSchedulerRole, {});
    ASSERT_OK(dispatcher.status_value());
    ASSERT_EQ(1u, GetThreadPool(*dispatcher)->num_threads());
  }
  {
    auto dispatcher = CreateTestDispatcher(kSchedulerRole, {});
    ASSERT_OK(dispatcher.status_value());
    ASSERT_EQ(1u, GetThreadPool(*dispatcher)->num_threads());
  }
  {
    auto dispatcher =
        CreateTestDispatcher(kSchedulerRole, fdf::SynchronizedDispatcher::Options::kAllowSyncCalls);
    ASSERT_OK(dispatcher.status_value());
    ASSERT_EQ(2u, GetThreadPool(*dispatcher)->num_threads());
  }
}

TEST_F(MultipleDispatchersThreadPoolTest, NumThreadsAllowSyncCalls) {
  constexpr std::string_view kSchedulerRole = "fuchsia.default";

  // adding sync call dispatchers should always spawn a new thread.
  {
    auto dispatcher =
        CreateTestDispatcher(kSchedulerRole, fdf::SynchronizedDispatcher::Options::kAllowSyncCalls);
    ASSERT_OK(dispatcher.status_value());
    ASSERT_EQ(1u, GetThreadPool(*dispatcher)->num_threads());
  }
  {
    auto dispatcher =
        CreateTestDispatcher(kSchedulerRole, fdf::SynchronizedDispatcher::Options::kAllowSyncCalls);
    ASSERT_OK(dispatcher.status_value());
    ASSERT_EQ(2u, GetThreadPool(*dispatcher)->num_threads());
  }
  // adding a no-sync-calls dispatcher should add a thread if it's the first one.
  {
    auto dispatcher = CreateTestDispatcher(kSchedulerRole, {});
    ASSERT_OK(dispatcher.status_value());
    ASSERT_EQ(3u, GetThreadPool(*dispatcher)->num_threads());
  }
  // but adding more should not.
  {
    auto dispatcher = CreateTestDispatcher(kSchedulerRole, {});
    ASSERT_OK(dispatcher.status_value());
    ASSERT_EQ(3u, GetThreadPool(*dispatcher)->num_threads());
  }
}
