// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/fdf/cpp/env.h>

#include <perftest/perftest.h>

int main(int argc, char** argv) {
  if (zx_status_t status = fdf_env_start(0); status != ZX_OK) {
    return status;
  }
  return perftest::PerfTestMain(argc, argv, "fuchsia.fidl_microbenchmarks");
}
