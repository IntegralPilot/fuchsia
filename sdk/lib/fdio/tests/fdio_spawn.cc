// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/fdio/io.h>
#include <lib/fdio/spawn.h>
#include <lib/zx/process.h>
#include <lib/zx/socket.h>
#include <unistd.h>
#include <zircon/processargs.h>

#include <string>

#include <zxtest/zxtest.h>

namespace {

void RunSpawnTest(const char* path, const char** argv, const std::string& expected) {
  int fd;
  zx::socket socket;
  zx_status_t status = fdio_pipe_half(&fd, socket.reset_and_get_address());
  ASSERT_OK(status);

  fdio_spawn_action_t action = {
      .action = FDIO_SPAWN_ACTION_TRANSFER_FD,
      .fd = {.local_fd = fd, .target_fd = STDOUT_FILENO},
  };

  zx::process process;
  char err_msg[FDIO_SPAWN_ERR_MSG_MAX_LENGTH];
  int flags = FDIO_SPAWN_CLONE_ALL & ~FDIO_SPAWN_CLONE_STDIO;
  status = fdio_spawn_etc(ZX_HANDLE_INVALID, flags, path, argv, nullptr, 1, &action,
                          process.reset_and_get_address(), err_msg);
  ASSERT_OK(status, "fdio_spawn_etc failed: %s", err_msg);

  status = process.wait_one(ZX_PROCESS_TERMINATED, zx::time::infinite(), nullptr);
  ASSERT_OK(status);

  zx_info_process_t info;
  status = process.get_info(ZX_INFO_PROCESS, &info, sizeof(info), nullptr, nullptr);
  ASSERT_OK(status);
  ASSERT_EQ(info.return_code, 0);

  std::string output;
  char buf[1024];
  while (true) {
    size_t actual;
    status = socket.read(0, buf, sizeof(buf), &actual);
    if (status == ZX_OK) {
      output.append(buf, actual);
    } else if (status == ZX_ERR_PEER_CLOSED) {
      break;
    } else {
      ASSERT_OK(status);
    }
  }

  ASSERT_EQ(output, expected);
}

TEST(FdioSpawnTest, ShebangAbsoluteArgv0) {
  const char* path = "/pkg/bin/shebang_test_script";
  const char* argv[] = {path, "arg1", "arg2", nullptr};
  std::string expected =
      "/pkg/bin/echo-args\n"
      "/pkg/bin/shebang_test_script\n"
      "arg1\n"
      "arg2\n";
  RunSpawnTest(path, argv, expected);
}

TEST(FdioSpawnTest, ShebangRelativeArgv0) {
  const char* path = "/pkg/bin/shebang_test_script";
  const char* argv[] = {"shebang_test_script", "arg1", "arg2", nullptr};
  std::string expected =
      "/pkg/bin/echo-args\n"
      "/pkg/bin/shebang_test_script\n"
      "arg1\n"
      "arg2\n";
  RunSpawnTest(path, argv, expected);
}

}  // namespace
