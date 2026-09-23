// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_RUNTIME_TESTING_RUNTIME_INTERNAL_WAIT_FOR_H_
#define LIB_DRIVER_RUNTIME_TESTING_RUNTIME_INTERNAL_WAIT_FOR_H_

#include <lib/driver/runtime/testing/cpp/internal/wait_for.h>
#include <lib/fdf/testing.h>
#include <lib/fit/function.h>
#include <lib/zx/result.h>

namespace fdf_internal {

zx::result<> IfExistsRunUnmanagedUntil(fit::function<bool()> condition) {
  while (!condition()) {
    auto status = fdf_testing_run_until_idle();
    if (status == ZX_OK) {
      continue;
    }

    // When |fdf_testing_run_until_idle| returns |ZX_ERR_BAD_STATE|, it means
    // the driver runtime is managing threads, in which case we cannot run
    // the loop manually. Defer to the caller to handle it (such as by blocking
    // on their condition directly without running the loop).
    if (status == ZX_ERR_BAD_STATE) {
      break;
    }

    return zx::error(status);
  }

  return zx::ok();
}

}  // namespace fdf_internal

#endif  // LIB_DRIVER_RUNTIME_TESTING_RUNTIME_INTERNAL_WAIT_FOR_H_
