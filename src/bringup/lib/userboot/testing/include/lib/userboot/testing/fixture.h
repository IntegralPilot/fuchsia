// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_BRINGUP_LIB_USERBOOT_TESTING_INCLUDE_LIB_USERBOOT_TESTING_FIXTURE_H_
#define SRC_BRINGUP_LIB_USERBOOT_TESTING_INCLUDE_LIB_USERBOOT_TESTING_FIXTURE_H_

#include <lib/elfldltl/testing/test-pipe-reader.h>
#include <lib/zx/process.h>

#include <gtest/gtest.h>

#include "launcher.h"

namespace userboot::testing {

// This is a gtest test fixture for tests that each launch a userboot program.
class Fixture : public ::testing::Test {
 protected:
  static void SetUpTestSuite();
  static void TearDownTestSuite();
  void SetUp() override;

  // A single userboot::testing::Launcher instance is used for all tests.
  static Launcher& launcher() { return launcher_; }

  // Launch a userboot program, thereafter available in process().
  void Launch(zx::vmo vmo, std::vector<zx::handle> handles);
  void Launch(const char* file, std::vector<zx::handle> handles);

  zx::process& process() { return process_; }

  // Wait for the process to exit.
  zx::result<int64_t> Wait();

  // Collect the log from the process.
  std::string FinishLog();

 private:
  fbl::unique_fd TakeLogFd();

  static Launcher launcher_;

  userboot::testing::TestJob job_;
  zx::process process_;
  elfldltl::testing::TestPipeReader log_;
  fbl::unique_fd log_fd_;
};

}  // namespace userboot::testing

#endif  // SRC_BRINGUP_LIB_USERBOOT_TESTING_INCLUDE_LIB_USERBOOT_TESTING_FIXTURE_H_
