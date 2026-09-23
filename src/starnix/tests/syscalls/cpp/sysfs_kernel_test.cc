// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/lib/files/directory.h"
#include "src/lib/files/file.h"
#include "src/starnix/tests/syscalls/cpp/test_helper.h"

using testing::ContainsRegex;
using testing::IsSupersetOf;

class SysfsKernelTest : public ::testing::Test {
 public:
  void SetUp() override {
    // TODO(https://fxbug.dev/317285180) don't skip on baseline
    // Assume starnix always has /sys/kernel.
    if (!test_helper::IsStarnix() && access("/sys/kernel/wakeup_reasons", F_OK) == -1) {
      GTEST_SKIP() << "/sys/kernel/wakeup_reasons not available, skipping...";
    }
  }
};

TEST_F(SysfsKernelTest, KernelDirectoryContainsExpectedContents) {
  std::vector<std::string> files;
  EXPECT_TRUE(files::ReadDirContents("/sys/kernel", &files));
  EXPECT_THAT(files, IsSupersetOf({"tracing", "wakeup_reasons"}));
}

TEST_F(SysfsKernelTest, WakeupReasonsDirectoryContainsExpectedContents) {
  std::vector<std::string> files;
  EXPECT_TRUE(files::ReadDirContents("/sys/kernel/wakeup_reasons", &files));
  EXPECT_THAT(files, IsSupersetOf({"last_resume_reason", "last_suspend_time"}));
}

TEST_F(SysfsKernelTest, WakeupReasonsDirectoryContainDefaultsLastResumeReason) {
  std::string last_resume_reason_str;
  EXPECT_TRUE(files::ReadFileToString("/sys/kernel/wakeup_reasons/last_resume_reason",
                                      &last_resume_reason_str));
  EXPECT_THAT(last_resume_reason_str, ContainsRegex("^.*\n"));
}

TEST_F(SysfsKernelTest, WakeupReasonsDirectoryContainDefaultsLastSuspendTime) {
  std::string last_suspend_time_str;
  EXPECT_TRUE(files::ReadFileToString("/sys/kernel/wakeup_reasons/last_suspend_time",
                                      &last_suspend_time_str));
  EXPECT_THAT(last_suspend_time_str, ContainsRegex("^[0-9]+ [0-9]+\n"));
}
