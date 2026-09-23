// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/fdio/spawn.h>
#include <lib/zx/process.h>

#include <thread>

#include <gtest/gtest.h>
#include <src/lib/testing/loop_fixture/real_loop_fixture.h>

#include "zircon/syscalls/object.h"

class FakeClockDeathTest : public gtest::RealLoopFixture {};

TEST_F(FakeClockDeathTest, CrashWhenGettingUTCClock) {
  // This would normally have been a death test. However, because the library
  // we are testing also takes part in setting up the death test, we can not
  // use existing EXPECT_DEATH facility directly, since the setup code would
  // fail to set up the death test.
  //
  // Reusing the example from `fdio_atexit.cc` tests.

  const char* argv[] = {"/pkg/bin/death_bin", nullptr};

  zx::process process;
  char err_msg[FDIO_SPAWN_ERR_MSG_MAX_LENGTH];
  ASSERT_EQ(ZX_OK, fdio_spawn_etc(ZX_HANDLE_INVALID, FDIO_SPAWN_CLONE_ALL, argv[0], argv, nullptr,
                                  0, nullptr, process.reset_and_get_address(), err_msg));

  ASSERT_EQ(ZX_OK, process.wait_one(ZX_TASK_TERMINATED, zx::time::infinite(), nullptr));

  zx_info_process_t proc_info;
  ASSERT_EQ(ZX_OK,
            process.get_info(ZX_INFO_PROCESS, &proc_info, sizeof(proc_info), nullptr, nullptr));
  // Crashed.
  ASSERT_EQ(proc_info.return_code, ZX_TASK_RETCODE_EXCEPTION_KILL);
}
