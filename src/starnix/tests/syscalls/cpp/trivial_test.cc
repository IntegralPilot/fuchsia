// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <gtest/gtest.h>

TEST(TrivialTest, Control) { EXPECT_TRUE(true); }

TEST(TrivialTest, ExitKilledBySignal) {
  ASSERT_DEATH([]() { __builtin_trap(); }(), "");
}

TEST(TrivialTest, ExitWithCode) {
  EXPECT_EXIT([]() { exit(5); }(), testing::ExitedWithCode(5), "");
}
