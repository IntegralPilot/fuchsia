// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "minimal_driver_runtime.h"

#include <lib/fdf/env.h>

namespace network::tun {

zx::result<std::unique_ptr<MinimalDriverRuntime>> MinimalDriverRuntime::Create() {
  // Call this before creating the MinimalDriverRuntime object. This ensures that fdf_env_reset is
  // only called if fdf_env_start succeeds.
  zx_status_t status = fdf_env_start(0);
  if (status != ZX_OK) {
    return zx::error(status);
  }

  std::unique_ptr<MinimalDriverRuntime> runtime(new MinimalDriverRuntime);

  fdf_env_register_driver_entry(runtime.get());

  return zx::ok(std::move(runtime));
}

MinimalDriverRuntime::~MinimalDriverRuntime() {
  fdf_env_register_driver_exit();
  fdf_env_reset();
}

}  // namespace network::tun
