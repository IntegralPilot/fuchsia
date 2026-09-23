// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_LD_TEST_LD_LOAD_TESTS_POSIX_BASE_H_
#define LIB_LD_TEST_LD_LOAD_TESTS_POSIX_BASE_H_

#include <filesystem>

#include <fbl/unique_fd.h>

#include "ld-load-tests-base.h"

namespace ld::testing {

class LdLoadTestsPosixBase : public LdLoadTestsBase {
 public:
 protected:
  void LoadTestDir(std::string_view executable_name);

  const std::filesystem::path& test_dir_path() const { return test_dir_path_; }

  int test_dir() const { return test_dir_.get(); }

  // This checks the test_dir() contents against Needed() expectations.
  void CheckNeededLibs();

 private:
  std::filesystem::path test_dir_path_;
  fbl::unique_fd test_dir_;
};

}  // namespace ld::testing

#endif  // LIB_LD_TEST_LD_LOAD_TESTS_POSIX_BASE_H_
