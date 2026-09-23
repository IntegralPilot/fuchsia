// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <sys/ioctl.h>
#include <sys/stat.h>
#include <sys/sysmacros.h>
#include <termios.h>
#include <unistd.h>

#include <algorithm>
#include <cerrno>
#include <cstring>
#include <string>
#include <string_view>
#include <vector>

#include <fbl/unique_fd.h>
#include <gmock/gmock.h>
#include <gtest/gtest.h>
#include <linux/capability.h>
#include <linux/dm-ioctl.h>

#include "src/lib/files/file.h"
#include "src/lib/fxl/strings/split_string.h"
#include "src/lib/fxl/strings/string_number_conversions.h"
#include "src/starnix/tests/selinux/userspace/util.h"
#include "src/starnix/tests/syscalls/cpp/capabilities_helper.h"
#include "src/starnix/tests/syscalls/cpp/syscall_matchers.h"
#include "src/starnix/tests/syscalls/cpp/test_helper.h"

extern std::string DoPrePolicyLoadWork() { return "minimal_policy"; }

namespace {

// Helper to ensure /dev/mapper/control exists in the test environment.
void EnsureDeviceMapperControlExists() {
  struct stat st;
  if (stat("/dev/mapper/control", &st) == 0) {
    return;  // Already exists
  }

  // Create /dev/mapper directory
  mkdir("/dev/mapper", 0755);  // Ignore error, might exist

  int major = 10;   // Default misc
  int minor = 236;  // Default device-mapper minor

  std::string content;
  if (files::ReadFileToString("/sys/class/misc/device-mapper/dev", &content)) {
    int parsed_major, parsed_minor;
    if (sscanf(content.c_str(), "%d:%d", &parsed_major, &parsed_minor) == 2) {
      major = parsed_major;
      minor = parsed_minor;
    }
  } else if (files::ReadFileToString("/proc/misc", &content)) {
    for (std::string_view line :
         fxl::SplitString(content, "\n", fxl::kTrimWhitespace, fxl::kSplitWantNonEmpty)) {
      std::vector<std::string_view> tokens =
          fxl::SplitString(line, " ", fxl::kTrimWhitespace, fxl::kSplitWantNonEmpty);
      if (tokens.size() >= 2 && tokens[1] == "device-mapper") {
        int parsed_minor;
        if (fxl::StringToNumberWithError(tokens[0], &parsed_minor)) {
          minor = parsed_minor;
          major = 10;
          break;
        }
      }
    }
  }

  ASSERT_THAT(mknod("/dev/mapper/control", S_IFCHR | 0600, makedev(major, minor)),
              ::testing::AnyOf(SyscallSucceeds(), SyscallFailsWithErrno(EEXIST)));
}

// Initializes a dm_ioctl struct with the standard version and size,
// and optionally copies a device name.
dm_ioctl InitDmIoctl(std::string_view name = {}) {
  dm_ioctl io = {};
  io.version[0] = DM_VERSION_MAJOR;
  io.version[1] = DM_VERSION_MINOR;
  io.version[2] = DM_VERSION_PATCHLEVEL;
  io.data_size = sizeof(io);
  if (!name.empty()) {
    size_t copy_len = std::min(name.size(), sizeof(io.name) - 1);
    name.copy(io.name, copy_len);
  }
  return io;
}

class DeviceMapperTest : public ::testing::Test {
 protected:
  void SetUp() override {
    if (!test_helper::HasCapability(CAP_SYS_ADMIN)) {
      GTEST_SKIP() << "Need CAP_SYS_ADMIN to run this test";
    }

    EnsureDeviceMapperControlExists();
    ctrl_fd_ = fbl::unique_fd(SAFE_SYSCALL(open("/dev/mapper/control", O_RDWR)));
  }

  void TearDown() override {
    if (ctrl_fd_.is_valid()) {
      dm_ioctl rm = InitDmIoctl("test-unprivileged-cpp");
      ioctl(ctrl_fd_.get(), DM_DEV_REMOVE, &rm);
    }
  }

  fbl::unique_fd ctrl_fd_;
};

TEST_F(DeviceMapperTest, CreateDeviceRequiresSysAdmin) {
  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([&]() {
    // Drop all capabilities to become unprivileged.
    test_helper::DropAllCapabilities();

    ASSERT_FALSE(test_helper::HasSysAdmin());

    dm_ioctl io = InitDmIoctl("test-unprivileged-cpp");

    // Attempt to create the device. This must fail with EACCES because
    // device-mapper checks for CAP_SYS_ADMIN and explicitly returns EACCES
    // (Permission denied) when it is missing, rather than the more common EPERM.
    EXPECT_THAT(ioctl(ctrl_fd_.get(), DM_DEV_CREATE, &io), SyscallFailsWithErrno(EACCES));
  });

  // Assert that the child test logic passed.
  ASSERT_TRUE(helper.WaitForChildren());
}

TEST_F(DeviceMapperTest, VersionRequiresSysAdmin) {
  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([&]() {
    // Drop all capabilities to become unprivileged.
    test_helper::DropAllCapabilities();

    ASSERT_FALSE(test_helper::HasSysAdmin());

    dm_ioctl io = InitDmIoctl();

    // Attempt to get version.
    // Device-mapper requires CAP_SYS_ADMIN even for the version ioctl,
    // so it fails with EACCES when run without capabilities.
    EXPECT_THAT(ioctl(ctrl_fd_.get(), DM_VERSION, &io), SyscallFailsWithErrno(EACCES));
  });

  // Assert that the child test logic passed.
  ASSERT_TRUE(helper.WaitForChildren());
}

TEST_F(DeviceMapperTest, ListDevicesRequiresSysAdmin) {
  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([&]() {
    test_helper::DropAllCapabilities();
    ASSERT_FALSE(test_helper::HasSysAdmin());

    dm_ioctl io = InitDmIoctl();

    EXPECT_THAT(ioctl(ctrl_fd_.get(), DM_LIST_DEVICES, &io), SyscallFailsWithErrno(EACCES));
  });

  ASSERT_TRUE(helper.WaitForChildren());
}

TEST_F(DeviceMapperTest, ListVersionsRequiresSysAdmin) {
  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([&]() {
    test_helper::DropAllCapabilities();
    ASSERT_FALSE(test_helper::HasSysAdmin());

    dm_ioctl io = InitDmIoctl();

    EXPECT_THAT(ioctl(ctrl_fd_.get(), DM_LIST_VERSIONS, &io), SyscallFailsWithErrno(EACCES));
  });

  ASSERT_TRUE(helper.WaitForChildren());
}

TEST_F(DeviceMapperTest, VfsIoctlDoesNotRequireSysAdmin) {
  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([&]() {
    test_helper::DropAllCapabilities();
    ASSERT_FALSE(test_helper::HasSysAdmin());

    // FIONBIO is a standard file ioctl handled by the VFS and does not require CAP_SYS_ADMIN.
    int val = 0;
    EXPECT_THAT(ioctl(ctrl_fd_.get(), FIONBIO, &val), SyscallSucceeds());
  });

  ASSERT_TRUE(helper.WaitForChildren());
}

TEST_F(DeviceMapperTest, NonDmIoctlRequiresSysAdmin) {
  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([&]() {
    test_helper::DropAllCapabilities();
    ASSERT_FALSE(test_helper::HasSysAdmin());

    // The device mapper driver checks CAP_SYS_ADMIN at the very beginning of ioctl handling
    // before inspecting the command. Therefore, even non-DM ioctls like TCGETS that reach
    // the driver return EACCES when invoked without CAP_SYS_ADMIN.
    struct termios t;
    EXPECT_THAT(ioctl(ctrl_fd_.get(), TCGETS, &t), SyscallFailsWithErrno(EACCES));
  });

  ASSERT_TRUE(helper.WaitForChildren());
}

}  // namespace
