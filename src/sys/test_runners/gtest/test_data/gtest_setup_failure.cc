// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <gtest/gtest.h>

namespace {

class SetupFailingTest : public ::testing::Test {
 protected:
  void SetUp() override { ASSERT_TRUE(false); }
};

TEST_F(SetupFailingTest, Test) { ASSERT_TRUE(true); }

}  // namespace
