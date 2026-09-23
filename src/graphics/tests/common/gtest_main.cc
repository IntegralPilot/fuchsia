// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <gtest/gtest.h>

// Must be provided by the test based on its config.
std::string DisabledTestPattern();

int main(int argc, char** argv) {
  // Setting this flag to true causes googletest to *generate* and log the random seed.
  GTEST_FLAG_SET(shuffle, true);

  testing::InitGoogleTest(&argc, argv);

  std::string disabled_test_pattern = DisabledTestPattern();
  if (!disabled_test_pattern.empty()) {
    std::string current_filter = GTEST_FLAG_GET(filter);
    GTEST_FLAG_SET(filter, current_filter + "-" + disabled_test_pattern);
  }
  return RUN_ALL_TESTS();
}
