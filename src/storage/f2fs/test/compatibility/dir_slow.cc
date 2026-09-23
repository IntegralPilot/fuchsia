// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/f2fs/test/compatibility/compatibility.h"

namespace f2fs {
namespace {

using DirSlowCompatibilityTest = F2fsGuestTest;

TEST_F(DirSlowCompatibilityTest, DirSlowWidthTestLinuxToFuchsia) {
  // Mkdir on Linux
  constexpr int kDirWidth = 10000;
  const std::string dirname_prefix = "dir";
  const size_t num_digits = std::to_string(kDirWidth - 1).size();
  {
    GetEnclosedGuest().GetLinuxOperator().Mkfs();
    GetEnclosedGuest().GetLinuxOperator().Mount();

    auto umount = fit::defer([&] { GetEnclosedGuest().GetLinuxOperator().Umount(); });

    std::string dir_name = std::string(kLinuxPathPrefix) + dirname_prefix + "{00.." +
                           std::to_string(kDirWidth - 1) + "}";
    GetEnclosedGuest().GetLinuxOperator().Mkdir(dir_name, 0644);
  }

  // Check on Fuchsia
  {
    GetEnclosedGuest().GetFuchsiaOperator().Fsck();
    GetEnclosedGuest().GetFuchsiaOperator().Mount();

    auto umount = fit::defer([&] { GetEnclosedGuest().GetFuchsiaOperator().Umount(); });

    for (int width = 0; width < kDirWidth; ++width) {
      std::string width_string = std::to_string(width);
      int leading_zero = num_digits - std::min(num_digits, width_string.size());
      std::string dir_name = dirname_prefix + std::string(leading_zero, '0') + width_string;
      auto dir =
          GetEnclosedGuest().GetFuchsiaOperator().Open(dir_name, O_RDONLY | O_DIRECTORY, 0644);
      ASSERT_TRUE(dir->IsValid());
    }
  }
}

TEST_F(DirSlowCompatibilityTest, DirSlowWidthTestFuchsiaToLinux) {
  // Mkdir on Fuchsia
  constexpr int kDirWidth = 10000;
  const std::string dirname_prefix = "dir";
  const size_t num_digits = std::to_string(kDirWidth - 1).size();
  {
    GetEnclosedGuest().GetFuchsiaOperator().Mkfs();
    GetEnclosedGuest().GetFuchsiaOperator().Mount();

    auto umount = fit::defer([&] { GetEnclosedGuest().GetFuchsiaOperator().Umount(); });

    for (int width = 0; width < kDirWidth; ++width) {
      std::string width_string = std::to_string(width);
      int leading_zero = num_digits - std::min(num_digits, width_string.size());
      std::string dir_name = dirname_prefix + std::string(leading_zero, '0') + width_string;
      GetEnclosedGuest().GetFuchsiaOperator().Mkdir(dir_name, 0644);
    }
  }

  // Check on Linux
  {
    GetEnclosedGuest().GetLinuxOperator().Fsck();
    GetEnclosedGuest().GetLinuxOperator().Mount();

    auto umount = fit::defer([&] { GetEnclosedGuest().GetLinuxOperator().Umount(); });

    const std::string common_command = "find " +
                                       GetEnclosedGuest().GetLinuxOperator().ConvertPath("//") +
                                       " -type d -name \"" + dirname_prefix + "*\"";

    // Check created number of children
    std::string command = common_command + " | wc -l | tr -d \"\\n\"";
    std::string result;
    GetEnclosedGuest().GetLinuxOperator().ExecuteWithAssert({command}, &result);
    ASSERT_EQ(result, std::to_string(kDirWidth));

    // Check name of the first child after sort
    int leading_zero = num_digits - std::min(num_digits, std::to_string(0).size());
    std::string dir_name = std::string(kLinuxPathPrefix) + dirname_prefix +
                           std::string(leading_zero, '0') + std::to_string(0);

    command = common_command + " | sort | head -n 1 | tr -d \"\\n\"";
    result.clear();
    GetEnclosedGuest().GetLinuxOperator().ExecuteWithAssert({command}, &result);
    ASSERT_EQ(result, GetEnclosedGuest().GetLinuxOperator().ConvertPath(dir_name));

    // Check name of the last child after sort
    dir_name = std::string(kLinuxPathPrefix) + dirname_prefix + std::to_string(kDirWidth - 1);

    command = common_command + " | sort | tail -n 1 | tr -d \"\\n\"";
    result.clear();
    GetEnclosedGuest().GetLinuxOperator().ExecuteWithAssert({command}, &result);
    ASSERT_EQ(result, GetEnclosedGuest().GetLinuxOperator().ConvertPath(dir_name));
  }
}

}  // namespace
}  // namespace f2fs
