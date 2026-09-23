// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "ld-load-tests-posix-base.h"

#include <fcntl.h>
#include <lib/elfldltl/testing/get-test-data.h>
#include <unistd.h>

#include <cstring>

#include <gtest/gtest.h>

namespace ld::testing {

void LdLoadTestsPosixBase::LoadTestDir(std::string_view executable) {
  ASSERT_FALSE(test_dir_) << "LoadTestDir called twice?";
  static const std::filesystem::path kTestDirParent =
      elfldltl::testing::GetTestDataPath("sdk/lib/ld/test/modules");
  test_dir_path_ = kTestDirParent / executable;
  test_dir_.reset(open(test_dir_path_.c_str(), O_RDONLY | O_DIRECTORY | O_CLOEXEC));
  EXPECT_TRUE(test_dir_) << test_dir_path_ << ": " << strerror(errno);
}

void LdLoadTestsPosixBase::CheckNeededLibs() {
  for (const auto& [name, found] : TakeNeededLibs()) {
    int error = faccessat(test_dir(), name.c_str(), R_OK, AT_EACCESS) < 0 ? errno : 0;
    if (found) {
      EXPECT_EQ(error, 0) << "missing " << name << ": " << strerror(errno);
    } else {
      EXPECT_EQ(error, ENOENT) << "expected-absent" << name << " got " << error << " ("
                               << strerror(errno) << ") but expected (" << ENOENT << ") ENOENT ("
                               << strerror(ENOENT) << ")";
    }
  }
}

}  // namespace ld::testing
