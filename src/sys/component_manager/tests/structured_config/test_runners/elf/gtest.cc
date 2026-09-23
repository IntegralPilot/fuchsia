// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <gtest/gtest.h>

#include "src/sys/component_manager/tests/structured_config/test_runners/elf/receiver_config.h"

namespace {
TEST(ScTest, CheckValues) {
  receiver_config::Config c = receiver_config::Config::TakeFromStartupHandle();
  ASSERT_EQ(c.my_flag(), true);
  ASSERT_EQ(c.my_uint8(), 255);
}

TEST(ScTest, CreateOwnConfig) {
  receiver_config::Config c;
  c.my_uint8() = 5;
  ASSERT_EQ(c.my_uint8(), 5);
}
}  // namespace
