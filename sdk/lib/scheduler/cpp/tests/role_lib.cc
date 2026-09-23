// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/scheduler/role.h>
#include <lib/zx/event.h>
#include <zircon/errors.h>
#include <zircon/threads.h>

#include <condition_variable>
#include <mutex>
#include <thread>

#include <zxtest/zxtest.h>

namespace {

TEST(RoleApi, SetThreadRole) {
  // Test setting a role on the current thread.
  {
    EXPECT_OK(fuchsia_scheduler::SetRoleForThisThread("test.core.a"));
    EXPECT_EQ(ZX_ERR_NOT_FOUND, fuchsia_scheduler::SetRoleForThisThread("test.nonexistent.role"));
  }

  // Test setting a role on another thread.
  {
    std::mutex lock;
    bool done = false;
    std::condition_variable condition;

    std::thread thread{[&] {
      std::unique_lock<std::mutex> guard{lock};
      condition.wait(guard, [&] { return done; });
    }};

    const zx::unowned_thread thread_handle{native_thread_get_zx_handle(thread.native_handle())};

    EXPECT_OK(fuchsia_scheduler::SetRoleForThread(thread_handle->borrow(), "test.core.a"));
    EXPECT_EQ(ZX_ERR_NOT_FOUND, fuchsia_scheduler::SetRoleForThread(thread_handle->borrow(),
                                                                    "test.nonexistent.role"));

    {
      std::unique_lock<std::mutex> guard{lock};
      done = true;
    }

    condition.notify_all();
    thread.join();
  }
}

TEST(RoleApi, SetThreadRoleInput) {
  {
    std::vector<fuchsia_scheduler::RoleParameter> input_params{{.name = "name", .value = "ernie"}};
    zx::result args = fuchsia_scheduler::SetRoleForThisThread("test.core.input", input_params);
    ASSERT_OK(args.status_value());
    std::vector<fuchsia_scheduler::RoleParameter> expected_output{
        {.name = "name", .value = "bert"}};
    EXPECT_EQ(*args, expected_output);
  }
  {
    std::vector<fuchsia_scheduler::RoleParameter> input_params{{.name = "name", .value = "oscar"}};
    zx::result args = fuchsia_scheduler::SetRoleForThisThread("test.core.input", input_params);
    ASSERT_OK(args.status_value());
    std::vector<fuchsia_scheduler::RoleParameter> expected_output{
        {.name = "name", .value = "grouch"}};
    EXPECT_EQ(*args, expected_output);
  }

  {
    std::vector<fuchsia_scheduler::RoleParameter> input_params;
    EXPECT_EQ(
        ZX_ERR_NOT_FOUND,
        fuchsia_scheduler::SetRoleForThisThread("test.core.input", input_params).status_value());
  }
  {
    std::vector<fuchsia_scheduler::RoleParameter> input_params{{.name = "name", .value = "elmo"}};
    EXPECT_EQ(
        ZX_ERR_NOT_FOUND,
        fuchsia_scheduler::SetRoleForThisThread("test.core.input", input_params).status_value());
  }
}

TEST(RoleApi, SetThreadRoleOutput) {
  std::vector<fuchsia_scheduler::RoleParameter> input_params;
  std::vector<fuchsia_scheduler::RoleParameter> expected_output{
      {.name = "output1", .value = 2},
      {.name = "output2", .value = 5.7},
      {.name = "output3", .value = "deadbeef"},
  };
  {
    zx::result args = fuchsia_scheduler::SetRoleForThisThread("test.core.output", input_params);
    ASSERT_OK(args.status_value());

    EXPECT_EQ(*args, expected_output);

    EXPECT_EQ(ZX_ERR_NOT_FOUND,
              fuchsia_scheduler::SetRoleForThisThread("test.nonexistent.role", input_params)
                  .status_value());
  }

  // Test setting a role on another thread.
  {
    std::mutex lock;
    bool done = false;
    std::condition_variable condition;

    std::thread thread{[&] {
      std::unique_lock<std::mutex> guard{lock};
      condition.wait(guard, [&] { return done; });
    }};

    const zx::unowned_thread thread_handle{native_thread_get_zx_handle(thread.native_handle())};

    zx::result args = fuchsia_scheduler::SetRoleForThread(thread_handle->borrow(),
                                                          "test.core.output", input_params);
    ASSERT_OK(args.status_value());
    EXPECT_EQ(*args, expected_output);

    EXPECT_EQ(ZX_ERR_NOT_FOUND, fuchsia_scheduler::SetRoleForThread(
                                    thread_handle->borrow(), "test.nonexistent.role", input_params)
                                    .status_value());

    {
      std::unique_lock<std::mutex> guard{lock};
      done = true;
    }

    condition.notify_all();
    thread.join();
  }
}

TEST(RoleApi, SetMemoryRoleRoot) {
  EXPECT_OK(fuchsia_scheduler::SetRoleForRootVmar("test.core.a.memory"));
  EXPECT_EQ(ZX_ERR_NOT_FOUND, fuchsia_scheduler::SetRoleForRootVmar("test.nonexistent.role"));
}

TEST(RoleApi, SetMemoryRoleNested) {
  zx::vmar nested;
  uintptr_t child_addr;
  ASSERT_EQ(zx::vmar::root_self()->allocate(ZX_VM_CAN_MAP_READ, 0, 4096, &nested, &child_addr),
            ZX_OK);
  EXPECT_OK(fuchsia_scheduler::SetRoleForVmar(nested.borrow(), "test.core.a.memory"));

  ASSERT_EQ(zx::vmar::root_self()->allocate(ZX_VM_CAN_MAP_READ, 0, 4096, &nested, &child_addr),
            ZX_OK);
  EXPECT_EQ(ZX_ERR_NOT_FOUND,
            fuchsia_scheduler::SetRoleForVmar(nested.borrow(), "test.nonexistent.role"));
}

}  // anonymous namespace
