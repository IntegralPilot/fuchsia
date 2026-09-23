// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/fdio/spawn.h>
#include <lib/zx/process.h>
#include <lib/zx/time.h>

#include <string>

#include <zxtest/zxtest.h>

namespace {

TEST(NullNamespaceTest, NullNamespace) {
  const char* argv[] = {"/pkg/bin/null-namespace-child", nullptr};
  zx::process process;
  ASSERT_OK(fdio_spawn(/*job=*/ZX_HANDLE_INVALID, FDIO_SPAWN_CLONE_STDIO | FDIO_SPAWN_DEFAULT_LDSVC,
                       argv[0], argv, process.reset_and_get_address()));
  ASSERT_OK(process.wait_one(ZX_TASK_TERMINATED, zx::time::infinite(), nullptr));
  zx_info_process_t proc_info;
  ASSERT_OK(process.get_info(ZX_INFO_PROCESS, &proc_info, sizeof(proc_info), nullptr, nullptr));
  ASSERT_EQ(0, proc_info.return_code);
}

}  // namespace
