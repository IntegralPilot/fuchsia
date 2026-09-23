// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <sys/mount.h>
#include <sys/stat.h>
#include <sys/sysmacros.h>

#include <string>

#include <fbl/unique_fd.h>
#include <gtest/gtest.h>
#include <linux/capability.h>

#include "src/lib/files/file.h"
#include "src/lib/fxl/strings/string_printf.h"
#include "src/starnix/tests/selinux/userspace/util.h"
#include "src/starnix/tests/syscalls/cpp/capabilities_helper.h"
#include "src/starnix/tests/syscalls/cpp/syscall_matchers.h"
#include "src/starnix/tests/syscalls/cpp/test_helper.h"

extern std::string DoPrePolicyLoadWork() { return "overlayfs_policy"; }

namespace {

class OverlayFsTest : public ::testing::Test {
 protected:
  static void SetUpTestSuite() {
    // The unshare() call will isolate the mount namespaces for the running
    // test process. This allows the Linux-based tests to execute syscalls with
    // root permissions, without fear of messing the environment up. While the
    // Starnix tests don't strictly need to unshare, it's beneficial to run the
    // same test binaries on Linux and on Starnix so we can be sure the semantics
    // match. As a side effect, this means that the mounted directories will not
    // be viewable in traditional ways, e.g. ffx component explore.
    ASSERT_THAT(unshare(CLONE_NEWNS), SyscallSucceeds());
  }

  void SetUp() override {
    ASSERT_TRUE(test_helper::HasSysAdmin());

    ASSERT_FALSE(temp_dir_.path().empty());

    overlay_ = temp_dir_.path() + "/overlay";
    ASSERT_THAT(mkdir(overlay_.c_str(), 0700), SyscallSucceeds());

    lower_ = temp_dir_.path() + "/lower";
    upper_base_ = temp_dir_.path() + "/upper_base";

    upper_ = upper_base_ + "/upper";
    work_ = upper_base_ + "/work";

    // Create the underlying directories with the appropriate labels.
    {
      auto resetter =
          ScopedTaskAttrResetter::SetTaskAttr("fscreate", "test_u:object_r:test_overlay_file_t:s0");
      ASSERT_THAT(mkdir(lower_.c_str(), 0700), SyscallSucceeds());
      ASSERT_THAT(mkdir(upper_base_.c_str(), 0700), SyscallSucceeds());
      ASSERT_THAT(mkdir(upper_.c_str(), 0700), SyscallSucceeds());
      ASSERT_THAT(mkdir(work_.c_str(), 0700), SyscallSucceeds());
    }
  }

  void TearDown() override {
    // TODO: https://fxbug.dev/493818838 - This can flakily fail with EBUSY if a test, or subprocess
    // does not explicitly close all FDs it has opened on the overlay filesystem before completing.
    ASSERT_THAT(umount(overlay_.c_str()), SyscallSucceeds());
  }

  void Mount() { MountWith("test_u:test_r:test_overlayfs_mounter_t:s0"); }

  void MountWith(const char* mounter_label) {
    std::string options = fxl::StringPrintf("lowerdir=%s,upperdir=%s,workdir=%s", lower_.c_str(),
                                            upper_.c_str(), work_.c_str());
    ASSERT_TRUE(RunSubprocessAs(mounter_label, [&] {
      ASSERT_THAT(
          mount(nullptr, overlay_.c_str(), "overlay", MS_NOATIME | MS_NOSUID, options.c_str()),
          SyscallSucceeds());
    }));
  }

  test_helper::ScopedTempDir temp_dir_;

  std::string lower_;
  std::string upper_;
  std::string upper_base_;
  std::string work_;
  std::string overlay_;
};

// Generate an audit log of the checks that should be performed when mounting the overlay FS.
TEST_F(OverlayFsTest, PermissiveMountChecks) { ASSERT_NO_FATAL_FAILURE(Mount()); }

// Verify that the overlay FS root node uses the upper directory's root node xattrs.
TEST_F(OverlayFsTest, RootLabelMatchesUpper) {
  auto enforce = ScopedEnforcement::SetEnforcing();

  // Re-label the upper directory to a more specific label for this test.
  ASSERT_THAT(SetLabel(upper_, "test_u:object_r:test_overlayfs_upper_file_t:s0"),
              SyscallResultIsOk());

  ASSERT_NO_FATAL_FAILURE(Mount());

  EXPECT_EQ(GetLabel(overlay_), GetLabel(upper_));
}

// Verify that a file can be read from the lower FS if both caller and mounter can read it.
TEST_F(OverlayFsTest, ReadSucceedsWithPermissions) {
  auto enforce = ScopedEnforcement::SetEnforcing();
  ASSERT_TRUE(files::WriteFile(lower_ + "/file", "lower_data"));
  ASSERT_THAT(SetLabel(lower_ + "/file", "test_u:object_r:test_overlay_file_t:s0"),
              SyscallResultIsOk());
  ASSERT_NO_FATAL_FAILURE(Mount());

  EXPECT_TRUE(RunSubprocessAs("test_u:test_r:test_overlayfs_caller_t:s0", [&] {
    std::string content;
    EXPECT_TRUE(files::ReadFileToString(overlay_ + "/file", &content));
    EXPECT_EQ(content, "lower_data");
  }));
}

// Verify that if the caller cannot write to a file then it won't be copied-up.
TEST_F(OverlayFsTest, CopyUpOnlyAfterAccessCheck) {
  auto enforce = ScopedEnforcement::SetEnforcing();
  ASSERT_TRUE(files::WriteFile(lower_ + "/file", "lower_data"));
  ASSERT_THAT(SetLabel(lower_ + "/file", "test_u:object_r:test_overlay_read_only_file_t:s0"),
              SyscallResultIsOk());
  ASSERT_NO_FATAL_FAILURE(Mount());

  // Attempt to write to the read-only labeled file, which neither mounter nor caller can write.
  EXPECT_TRUE(RunSubprocessAs("test_u:test_r:test_overlayfs_caller_t:s0", [&] {
    EXPECT_THAT(open((overlay_ + "/file").c_str(), O_WRONLY), SyscallFailsWithErrno(EACCES));
  }));

  // Verify that the file was not promoted to the upper layer.
  EXPECT_FALSE(files::IsFile(upper_ + "/file"));
}

// Verify that opening a file to read requires both caller and mounter read access checks.
TEST_F(OverlayFsTest, OpenReadDeniedIfMounterCannotRead) {
  auto enforce = ScopedEnforcement::SetEnforcing();
  ASSERT_TRUE(files::WriteFile(lower_ + "/file", "lower_data"));
  ASSERT_THAT(SetLabel(lower_ + "/file", "test_u:object_r:test_overlay_mounter_no_read_file_t:s0"),
              SyscallResultIsOk());

  ASSERT_NO_FATAL_FAILURE(Mount());

  // Even if the caller has read access, the read should fail because the mounter does not.
  EXPECT_TRUE(RunSubprocessAs("test_u:test_r:test_overlayfs_caller_t:s0", [&] {
    EXPECT_THAT(open((overlay_ + "/file").c_str(), O_RDONLY), SyscallFailsWithErrno(EACCES));
  }));
}

// Verify that the write permission is not re-checked for the mounter if the caller switches an
// O_APPEND file to be non-append.
TEST_F(OverlayFsTest, ClearAppendNotDeniedIfMounterLacksWrite) {
  auto enforce = ScopedEnforcement::SetEnforcing();
  ASSERT_TRUE(files::WriteFile(upper_ + "/file", "upper_data"));
  ASSERT_THAT(
      SetLabel(upper_ + "/file", "test_u:object_r:test_overlay_mounter_append_only_file_t:s0"),
      SyscallResultIsOk());

  ASSERT_NO_FATAL_FAILURE(Mount());

  // Open the file with O_APPEND. This should succeed because both caller and mounter have 'append'.
  EXPECT_TRUE(RunSubprocessAs("test_u:test_r:test_overlayfs_caller_t:s0", [&] {
    fbl::unique_fd fd(open((overlay_ + "/file").c_str(), O_WRONLY | O_APPEND));
    ASSERT_THAT(fd.get(), SyscallSucceeds());

    // Attempt to clear the O_APPEND flag. This requires 'write' permission on the underlying file,
    // but once the file has been opened the lack of mounter `write` permission is not relevant.
    int flags = SAFE_SYSCALL(fcntl(fd.get(), F_GETFL));
    ASSERT_THAT(flags, SyscallSucceeds());
    EXPECT_THAT(fcntl(fd.get(), F_SETFL, flags & ~O_APPEND), SyscallSucceeds());

    EXPECT_THAT(lseek(fd.get(), 0, SEEK_SET), SyscallSucceeds());
    EXPECT_THAT(write(fd.get(), "X", 1), SyscallSucceeds());
  }));
}

// Verify that if caller and mounter have write rights then a file will be copied up.
TEST_F(OverlayFsTest, CopyUpSucceedsWithPermissions) {
  auto enforce = ScopedEnforcement::SetEnforcing();
  ASSERT_TRUE(files::WriteFile(lower_ + "/file", "lower_data"));
  ASSERT_THAT(SetLabel(lower_ + "/file", "test_u:object_r:test_overlay_file_t:s0"),
              SyscallResultIsOk());
  ASSERT_NO_FATAL_FAILURE(Mount());

  EXPECT_TRUE(RunSubprocessAs("test_u:test_r:test_overlayfs_caller_t:s0", [&] {
    EXPECT_TRUE(files::WriteFile(overlay_ + "/file", "new_data"));
  }));

  // Verify that the file was actually promoted to the upper layer.
  EXPECT_TRUE(files::IsFile(upper_ + "/file"));
}

// Verify that if the mounter only has append permission to a file then it cannot be copied-up for
// non-append writing by the caller, even if the caller can write to it.
TEST_F(OverlayFsTest, CopyUpDeniedIfMounterLacksWrite) {
  auto enforce = ScopedEnforcement::SetEnforcing();
  ASSERT_TRUE(files::WriteFile(lower_ + "/file", "upper_data"));
  ASSERT_THAT(
      SetLabel(lower_ + "/file", "test_u:object_r:test_overlay_mounter_append_only_file_t:s0"),
      SyscallResultIsOk());

  ASSERT_NO_FATAL_FAILURE(Mount());

  // Open the file for writing and without O_APPEND. This should fail because the mounter lacks
  // `write` permission to the file's domain.
  EXPECT_TRUE(RunSubprocessAs("test_u:test_r:test_overlayfs_caller_t:s0", [&] {
    EXPECT_THAT(open((overlay_ + "/file").c_str(), O_WRONLY), SyscallFailsWithErrno(EACCES));
  }));
}

// Verify that if the mounter only has append permission to a file then it cannot be copied-up even
// if the caller is only requesting append access.
TEST_F(OverlayFsTest, CopyUpDeniedForAppendIfMounterLacksWrite) {
  auto enforce = ScopedEnforcement::SetEnforcing();
  ASSERT_TRUE(files::WriteFile(lower_ + "/file", "upper_data"));
  ASSERT_THAT(
      SetLabel(lower_ + "/file", "test_u:object_r:test_overlay_mounter_append_only_file_t:s0"),
      SyscallResultIsOk());

  ASSERT_NO_FATAL_FAILURE(Mount());

  // Open the file for writing with O_APPEND. This should succeed.
  EXPECT_TRUE(RunSubprocessAs("test_u:test_r:test_overlayfs_caller_t:s0", [&] {
    EXPECT_THAT(open((overlay_ + "/file").c_str(), O_WRONLY | O_APPEND),
                SyscallFailsWithErrno(EACCES));
  }));
}

// Verify the set of checks that are made when a file is opened and written by the caller, by
// running in permissive mode and accessing a file that neither caller nor mounter can access.
TEST_F(OverlayFsTest, AuditChecksFileOpenAndWrite) {
  ASSERT_TRUE(files::WriteFile(lower_ + "/file", "upper_data"));
  ASSERT_THAT(SetLabel(lower_ + "/file", "test_u:object_r:test_overlay_no_access_file_t:s0"),
              SyscallResultIsOk());

  ASSERT_NO_FATAL_FAILURE(Mount());

  // Open the file for writing and write to it, to trigger audit logs describing the access checks.
  ASSERT_TRUE(RunSubprocessAs("test_u:test_r:test_overlayfs_caller_t:s0", [&] {
    fbl::unique_fd fd(open((overlay_ + "/file").c_str(), O_WRONLY));
    ASSERT_THAT(fd.get(), SyscallSucceeds());
    ASSERT_THAT(write(fd.get(), "test", 4), SyscallSucceeds());
  }));
}

// Verify the set of checks that are made when a file in the lower filesystem is opened and read by
// the caller, by running in permissive mode and accessing a file that neither caller nor mounter
// can access.
TEST_F(OverlayFsTest, AuditChecksFileOpenAndRead) {
  ASSERT_TRUE(files::WriteFile(lower_ + "/file", "upper_data"));
  ASSERT_THAT(SetLabel(lower_ + "/file", "test_u:object_r:test_overlay_no_access_file_t:s0"),
              SyscallResultIsOk());

  ASSERT_NO_FATAL_FAILURE(Mount());

  // Open the file for reading and read from it, to trigger audit logs describing the access checks.
  ASSERT_TRUE(RunSubprocessAs("test_u:test_r:test_overlayfs_caller_t:s0", [&] {
    fbl::unique_fd fd(open((overlay_ + "/file").c_str(), O_RDONLY));
    ASSERT_THAT(fd.get(), SyscallSucceeds());
    char buf[1];
    ASSERT_THAT(read(fd.get(), &buf, sizeof(buf)), SyscallSucceeds());
  }));
}

// Verify that the mounter must have 'capability { mknod }' for the caller to be able to create a
// device node in the overlay.
TEST_F(OverlayFsTest, MknodRequiresMounterCapability) {
  auto enforce = ScopedEnforcement::SetEnforcing();
  ASSERT_NO_FATAL_FAILURE(MountWith("test_u:test_r:test_overlayfs_mounter_no_mknod_t:s0"));

  EXPECT_TRUE(RunSubprocessAs("test_u:test_r:test_overlayfs_caller_t:s0", [&] {
    ASSERT_TRUE(test_helper::HasCapability(CAP_MKNOD));
    EXPECT_THAT(mknod((overlay_ + "/devnode").c_str(), S_IFCHR | 0666, makedev(1, 3)),
                SyscallFailsWithErrno(EPERM));
  }));
}

// Verify that the caller must have 'capability { mknod }' to create a device node in the overlay.
TEST_F(OverlayFsTest, MknodRequiresCallerCapability) {
  auto enforce = ScopedEnforcement::SetEnforcing();
  ASSERT_NO_FATAL_FAILURE(Mount());

  EXPECT_TRUE(RunSubprocessAs("test_u:test_r:test_overlayfs_caller_no_mknod_t:s0", [&] {
    ASSERT_TRUE(test_helper::HasCapability(CAP_MKNOD));
    EXPECT_THAT(mknod((overlay_ + "/devnode").c_str(), S_IFCHR | 0666, makedev(1, 3)),
                SyscallFailsWithErrno(EPERM));
  }));
}

// Verify that a device node can be created in the overlay if both mounter and caller have
// 'capability { mknod }'.
TEST_F(OverlayFsTest, MknodSucceedsWithPermissions) {
  auto enforce = ScopedEnforcement::SetEnforcing();
  ASSERT_NO_FATAL_FAILURE(Mount());

  EXPECT_TRUE(RunSubprocessAs("test_u:test_r:test_overlayfs_caller_t:s0", [&] {
    ASSERT_TRUE(test_helper::HasCapability(CAP_MKNOD));
    EXPECT_THAT(mknod((overlay_ + "/devnode").c_str(), S_IFCHR | 0666, makedev(1, 3)),
                SyscallSucceeds());
  }));
}

// Verify that the mounter does not require 'capability { mknod }' in order for "overlay" FS to
// create a whiteout node when the caller removes a file that exists in the lower layer.
TEST_F(OverlayFsTest, UnlinkLowerDoesNotRequireMounterMknodCapability) {
  auto enforce = ScopedEnforcement::SetEnforcing();
  ASSERT_TRUE(files::WriteFile(lower_ + "/file", "data"));
  ASSERT_THAT(SetLabel(lower_ + "/file", "test_u:object_r:test_overlay_file_t:s0"),
              SyscallResultIsOk());

  ASSERT_NO_FATAL_FAILURE(MountWith("test_u:test_r:test_overlayfs_mounter_no_mknod_t:s0"));

  EXPECT_TRUE(RunSubprocessAs("test_u:test_r:test_overlayfs_caller_t:s0", [&] {
    EXPECT_THAT(unlink((overlay_ + "/file").c_str()), SyscallSucceeds());
  }));
}

// Verify that retrieving a file's security label through OverlayFS is denied if the mounter lacks
// the 'getattr' permission, even if the caller has it.
TEST_F(OverlayFsTest, SecurityLabelAccessDeniedIfMounterGetattrDenied) {
  auto enforce = ScopedEnforcement::SetEnforcing();

  ASSERT_TRUE(files::WriteFile(lower_ + "/file", "lower_data"));
  ASSERT_THAT(
      SetLabel(lower_ + "/file", "test_u:object_r:test_overlay_mounter_no_getattr_file_t:s0"),
      SyscallResultIsOk());

  ASSERT_NO_FATAL_FAILURE(Mount());

  EXPECT_TRUE(RunSubprocessAs("test_u:test_r:test_overlayfs_caller_t:s0", [&] {
    // Verify that retrieving the SELinux label (getxattr) fails.
    EXPECT_THAT(GetLabel(overlay_ + "/file"), SyscallResultIsErrno(EACCES));

    // Verify that stat() also fails.
    struct stat st;
    EXPECT_THAT(stat((overlay_ + "/file").c_str(), &st), SyscallFailsWithErrno(EACCES));
  }));
}

// Verify that the file is inaccessible when the mounter cannot perform 'getattr' to allow the
// security label to be determined. Audit log expectations verify that the denial is directly the
// result of the `getattr` denial, rather than arising due to an effective "unlabeled" label.
TEST_F(OverlayFsTest, FileIsInaccessibleIfMounterGetattrDenied) {
  auto enforce = ScopedEnforcement::SetEnforcing();

  ASSERT_TRUE(files::WriteFile(lower_ + "/file", "lower_data"));
  ASSERT_THAT(
      SetLabel(lower_ + "/file", "test_u:object_r:test_overlay_mounter_no_getattr_file_t:s0"),
      SyscallResultIsOk());

  ASSERT_NO_FATAL_FAILURE(Mount());

  EXPECT_TRUE(RunSubprocessAs("test_u:test_r:test_overlayfs_caller_t:s0", [&] {
    fbl::unique_fd fd(open((overlay_ + "/file").c_str(), O_RDONLY));
    ASSERT_THAT(fd.get(), SyscallFailsWithErrno(EACCES));
  }));
}

}  // namespace
