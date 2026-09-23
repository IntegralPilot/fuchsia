// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/lib/fxl/test/test_settings.h"

#include <lib/syslog/cpp/log_message_impl.h>
#include <lib/syslog/cpp/log_settings.h>
#include <stdlib.h>
#include <unistd.h>

#include <fbl/unique_fd.h>
#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "sdk/lib/syslog/cpp/log_level.h"
#include "src/lib/fxl/command_line.h"

namespace fxl {
namespace {
using ::testing::StrEq;

// Saves and restores global state impacted by |SetTestSettings| before and
// after each test, so that this test suite may be run without impacting
// customized test parameters set by the user (for other tests in the same run).
class TestSettingsFixture : public ::testing::Test {
 public:
  TestSettingsFixture()
      : old_severity_(fuchsia_logging::GetMinLogSeverity()),
        old_stderr_(dup(STDERR_FILENO)),
        random_seed_(getenv("TEST_LOOP_RANDOM_SEED")) {}
  ~TestSettingsFixture() {
    fuchsia_logging::LogSettingsBuilder builder;
    builder.WithMinLogSeverity(old_severity_).BuildAndInitialize();
    dup2(old_stderr_.get(), STDERR_FILENO);
    if (random_seed_) {
      setenv("TEST_LOOP_RANDOM_SEED", random_seed_, /*overwrite=*/true);
    } else {
      unsetenv("TEST_LOOP_RANDOM_SEED");
    }
  }

 private:
  fuchsia_logging::RawLogSeverity old_severity_;
  fbl::unique_fd old_stderr_;
  char *random_seed_;
};

// Test that --test_loop_seed sets TEST_LOOP_RANDOM_SEED.
// Because FXL is cross-platform, we cannot test that the environment variable
// correctly propagates the random seed to the test loop, which is
// Fuchsia-specific. This propagation test is performed in
// //src/lib/testing/loop_fixture/test_loop_fixture_unittest.cc instead.
TEST_F(TestSettingsFixture, RandomSeed) {
  EXPECT_TRUE(SetTestSettings(CommandLineFromInitializerList({"argv0", "--test_loop_seed=1"})));
  EXPECT_THAT(getenv("TEST_LOOP_RANDOM_SEED"), StrEq("1"));
  const char *argv[] = {"argv0", "--test_loop_seed=2", nullptr};
  EXPECT_TRUE(SetTestSettings(2, argv));
  EXPECT_THAT(getenv("TEST_LOOP_RANDOM_SEED"), StrEq("2"));
}

}  // namespace
}  // namespace fxl
