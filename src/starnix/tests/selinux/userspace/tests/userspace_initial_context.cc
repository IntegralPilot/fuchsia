// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <gtest/gtest.h>

#include "src/starnix/tests/selinux/userspace/util.h"

extern std::string DoPrePolicyLoadWork() { return "userspace_initial_context_policy"; }

namespace {

class UserspaceInitialContextTest : public ::testing::Test {
 protected:
  void SetUp() override {
    constexpr char kPolicyCap[] = "userspace_initial_context";
    ASSERT_TRUE(IsPolicyCapSupported(kPolicyCap));
    ASSERT_TRUE(IsPolicyCapEnabled(kPolicyCap));
  }
};

TEST_F(UserspaceInitialContextTest, InitSidMapped) {
  auto result = ReadFile("/sys/fs/selinux/initial_contexts/init");
  ASSERT_TRUE(result.is_ok());
  auto stripped = RemoveTrailingNul(result.value());
  ASSERT_TRUE(stripped.is_ok());
  EXPECT_EQ(stripped.value(), "system_u:unconfined_r:test_init_t:s0");
}

}  // namespace
