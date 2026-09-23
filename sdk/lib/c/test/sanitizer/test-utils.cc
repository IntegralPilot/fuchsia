// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "test-utils.h"

#include <lib/elfldltl/testing/test-pipe-reader.h>
#include <lib/fdio/spawn.h>
#include <lib/fit/defer.h>
#include <lib/zx/job.h>
#include <lib/zx/process.h>

#include <array>
#include <filesystem>

namespace {

std::filesystem::path HelperPath(std::string_view name) {
  const char* root_dir = getenv("TEST_ROOT_DIR");
  if (!root_dir) {
    root_dir = "/pkg";
  }
  std::filesystem::path file(root_dir);
  file /= "bin";
  file /= name;
  EXPECT_TRUE(std::filesystem::exists(file))
      << '"' << file << "\" from TEST_ROOT_DIR=\"" << root_dir << '"';
  return file;
}

}  // namespace

HelperResult RunHelper(std::string_view name, std::vector<const char*> args,
                       std::vector<fdio_spawn_action_t> actions) {
  std::filesystem::path file = HelperPath(name);
  if (file.empty()) {
    return {};
  }
  args.insert(args.begin(), file.c_str());
  args.push_back(nullptr);

  HelperResult result;
  [&] {
    zx::job test_job;
    ASSERT_OK(zx::job::create(*zx::job::default_job(), 0, &test_job));
    auto kill_job = fit::defer([&test_job]() { ASSERT_OK(test_job.kill()); });

    elfldltl::testing::TestPipeReader out, err;
    fbl::unique_fd out_fd, err_fd;
    ASSERT_NO_FATAL_FAILURE(out.Init(out_fd));
    auto finish_out = fit::defer([&out, &result] { result.out = std::move(out).Finish(); });
    ASSERT_NO_FATAL_FAILURE(err.Init(err_fd));
    auto finish_err = fit::defer([&err, &result] { result.err = std::move(err).Finish(); });

    actions.append_range(std::to_array<fdio_spawn_action_t>({
        {.action = FDIO_SPAWN_ACTION_TRANSFER_FD,
         .fd = {.local_fd = out_fd.release(), .target_fd = STDOUT_FILENO}},
        {.action = FDIO_SPAWN_ACTION_TRANSFER_FD,
         .fd = {.local_fd = err_fd.release(), .target_fd = STDERR_FILENO}},
    }));

    zx::process child;
    char err_msg[FDIO_SPAWN_ERR_MSG_MAX_LENGTH] = "";
    ASSERT_OK(fdio_spawn_etc(test_job.get(), FDIO_SPAWN_DEFAULT_LDSVC, args.front(), args.data(),
                             nullptr, actions.size(), actions.data(), child.reset_and_get_address(),
                             err_msg))
        << err_msg;

    zx_signals_t signals;
    ASSERT_OK(child.wait_one(ZX_PROCESS_TERMINATED, zx::time::infinite(), &signals));
    ASSERT_TRUE(signals & ZX_PROCESS_TERMINATED);

    zx_info_process_t info;
    ASSERT_OK(child.get_info(ZX_INFO_PROCESS, &info, sizeof(info), nullptr, nullptr));
    result.exit = info.return_code;
  }();

  return result;
}
