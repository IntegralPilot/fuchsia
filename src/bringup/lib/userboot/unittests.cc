// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/userboot/testing/fixture.h>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

namespace {

using ::testing::HasSubstr;

using UserbootTests = userboot::testing::Fixture;

TEST_F(UserbootTests, BasicLaunch) {
  ASSERT_NO_FATAL_FAILURE(Launch("/pkg/test/userboot-lib-static-pie-test", {}));

  auto result = Wait();
  ASSERT_TRUE(result.is_ok());
  EXPECT_EQ(*result, 0);

  std::string log = FinishLog();
  EXPECT_THAT(log, HasSubstr("Hello from userland!")) << log;
}

TEST_F(UserbootTests, BasicRustLaunch) {
  ASSERT_NO_FATAL_FAILURE(Launch("/pkg/bin/userboot-lib-rust-static-pie-test", {}));

  auto result = Wait();
  ASSERT_TRUE(result.is_ok());
  EXPECT_EQ(*result, 0);

  std::string log = FinishLog();
  EXPECT_THAT(log, HasSubstr("Hello from Rust userland!")) << log;
}

}  // namespace
