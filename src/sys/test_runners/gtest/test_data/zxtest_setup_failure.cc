// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <zxtest/zxtest.h>

namespace {

class SetupFailingTest : public zxtest::Test {
 protected:
  void SetUp() override { ASSERT_TRUE(false); }
};

TEST_F(SetupFailingTest, Test) { ASSERT_TRUE(true); }

class SetupFailingTestSuite : public zxtest::Test {
 protected:
  static void SetUpTestSuite() { ASSERT_TRUE(false); }
};

TEST_F(SetupFailingTestSuite, Test) { ASSERT_TRUE(true); }

}  // namespace
