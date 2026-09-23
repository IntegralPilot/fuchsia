// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/fdf/cpp/env.h>

#include <gtest/gtest.h>

int main(int argc, char** argv) {
  if (zx_status_t status = fdf_env_start(0); status != ZX_OK) {
    return status;
  }

  // Setting this flag to true causes googletest to *generate* and log the random seed.
  GTEST_FLAG_SET(shuffle, true);

  testing::InitGoogleTest(&argc, argv);
  return RUN_ALL_TESTS();
}
