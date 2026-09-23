// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/userboot/testing/launcher.h>
#include <lib/zx/time.h>
#include <zircon/types.h>

#include <gtest/gtest.h>

namespace userboot::testing {

zx::result<int64_t> WaitForTermination(zx::unowned_process process) {
  zx_signals_t pending;
  zx::result<> result =
      zx::make_result(process->wait_one(ZX_PROCESS_TERMINATED, zx::time::infinite(), &pending));
  EXPECT_TRUE(result.is_ok()) << "Wait for ZX_PROCESS_TERMINATED: " << result.status_string();
  if (result.is_error()) {
    return result.take_error();
  }

  zx_info_process_t info;
  result =
      zx::make_result(process->get_info(ZX_INFO_PROCESS, &info, sizeof(info), nullptr, nullptr));
  EXPECT_TRUE(result.is_ok()) << "ZX_INFO_PROCESS: " << result.status_string();
  if (result.is_error()) {
    return result.take_error();
  }

  return zx::ok(info.return_code);
}

}  // namespace userboot::testing
