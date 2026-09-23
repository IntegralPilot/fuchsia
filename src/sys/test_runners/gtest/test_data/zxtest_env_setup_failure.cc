// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <zxtest/zxtest.h>

class FailingGlobalResource : public zxtest::Environment {
 public:
  void SetUp() override { ASSERT_TRUE(false); }

  void TearDown() override {}

 private:
};

TEST(SomeSuite, SomeTest) { ASSERT_TRUE(true); }

int main(int argc, char** argv) {
  zxtest::Runner::GetInstance()->AddGlobalTestEnvironment(
      std::make_unique<FailingGlobalResource>());
  return RUN_ALL_TESTS(argc, argv);
}
