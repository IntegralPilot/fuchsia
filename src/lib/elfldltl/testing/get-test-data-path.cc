// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <stdlib.h>

#include <cerrno>
#include <cstring>
#include <filesystem>
#include <string>
#include <string_view>

#ifdef __APPLE__
#include <mach-o/dyld.h>
#endif

#include <lib/elfldltl/testing/get-test-data.h>

#include <gtest/gtest.h>

namespace elfldltl::testing {

std::filesystem::path GetTestDataPath(std::string_view filename) {
  std::filesystem::path path;
#ifdef __linux__
  char self_path[PATH_MAX];
  path.append(realpath("/proc/self/exe", self_path)).remove_filename();
#elif defined(__Fuchsia__)
  path.append("/pkg/data");
#elif defined(__APPLE__)
  uint32_t length = PATH_MAX;
  char self_path[PATH_MAX];
  char self_path_symlink[PATH_MAX];
  _NSGetExecutablePath(self_path_symlink, &length);
  path.append(realpath(self_path_symlink, self_path)).remove_filename();
#else
#error unknown platform.
#endif
  return path / "test_data/elfldltl" / filename;
}

#ifndef __Fuchsia__
// See get-test-lib.cc for the Fuchsia case; elsewhere this is a normal open.
fbl::unique_fd TryGetTestLib(std::string_view libname) {
  std::string path = GetTestDataPath(libname);
  fbl::unique_fd fd{open(path.c_str(), O_RDONLY)};
  // If the fd is not valid, expect that it is due to the file not found; any
  // other kind of error is unexpected.
  if (!fd) {
    EXPECT_EQ(errno, ENOENT) << "elfldltl::testing::TryGetTestLib(\"" << libname << "\"): " << path
                             << ": " << strerror(errno);
  }
  return fd;
}

fbl::unique_fd GetTestLib(std::string_view libname) {
  auto fd = TryGetTestLib(libname);
  EXPECT_TRUE(fd) << "elfldltl::testing::GetTestLib(\"" << libname
                  << "\"): " << GetTestDataPath(libname) << " not found";
  return fd;
}
#endif

}  // namespace elfldltl::testing
