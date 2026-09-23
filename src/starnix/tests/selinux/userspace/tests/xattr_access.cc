// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <sys/xattr.h>
#include <unistd.h>

#include <string>

#include <fbl/unique_fd.h>
#include <gtest/gtest.h>
#include <linux/capability.h>

#include "src/starnix/tests/selinux/userspace/util.h"
#include "src/starnix/tests/syscalls/cpp/capabilities_helper.h"
#include "src/starnix/tests/syscalls/cpp/syscall_matchers.h"

namespace {

constexpr char kTestUserAttrName[] = "user.test";
constexpr char kTestSecurityAttrName[] = "security.test";
constexpr char kTestTrustedAttrName[] = "trusted.test";

constexpr char kTestAttrValue[] = "test_value";

constexpr char kTestFileLabel[] = "test_u:object_r:test_xattr_access_file_t:s0";
constexpr char kTestFileNewLabel[] = "test_u:object_r:test_xattr_access_relabeled_file_t:s0";
constexpr char kTestFileNoAssociateLabel[] =
    "test_u:object_r:test_xattr_access_noassociate_file_t:s0";

constexpr char kSelinuxAttrName[] = "security.selinux";

struct HaveTestAttrs {
  bool have_user;
  bool have_security;
  bool have_trusted;
};

HaveTestAttrs ParseListXattrs(const char* buffer, size_t buflen) {
  HaveTestAttrs result{};
  auto* bufend = buffer + buflen;
  for (auto* current = buffer; current < bufend && strlen(current) > 0;
       current += strlen(current) + 1) {
    if (strcmp(current, kTestUserAttrName) == 0) {
      result.have_user = true;
    } else if (strcmp(current, kTestSecurityAttrName) == 0) {
      result.have_security = true;
    } else if (strcmp(current, kTestTrustedAttrName) == 0) {
      result.have_trusted = true;
    }
  }
  return result;
}

int SetXattr(const std::string& path, const char* name, const std::string& value) {
  // Write the attribute value with the trailing NUL, to allow `getxattr()` checks to use
  // `sizeof(kConstant)` which will include the literal's implicit terminating NUL.
  return setxattr(path.c_str(), name, value.c_str(), value.size() + 1, 0);
}

TEST(XattrTest, SetTrustedAndSecurityXattrRequiresCapSysAdminAndSetAttr) {
  auto scoped_fscreate = ScopedTaskAttrResetter::SetTaskAttr("fscreate", kTestFileLabel);
  test_helper::ScopedTempFD file;

  auto enforcing = ScopedEnforcement::SetEnforcing();
  EXPECT_TRUE(RunSubprocessAs("test_u:test_r:test_xattr_capsysadmin_t:s0", [&]() {
    ASSERT_TRUE(test_helper::HasSysAdmin());
    EXPECT_THAT(SetXattr(file.name(), kTestSecurityAttrName, kTestAttrValue), SyscallSucceeds());
    EXPECT_THAT(SetXattr(file.name(), kTestTrustedAttrName, kTestAttrValue), SyscallSucceeds());
  }));

  EXPECT_TRUE(RunSubprocessAs("test_u:test_r:test_xattr_capsysadmin_nosetgetattr_t:s0", [&]() {
    ASSERT_TRUE(test_helper::HasSysAdmin());
    EXPECT_THAT(SetXattr(file.name(), kTestSecurityAttrName, kTestAttrValue),
                SyscallFailsWithErrno(EACCES));
    EXPECT_THAT(SetXattr(file.name(), kTestTrustedAttrName, kTestAttrValue),
                SyscallFailsWithErrno(EACCES));
  }));

  EXPECT_TRUE(RunSubprocessAs("test_u:test_r:test_xattr_nocapsysadmin_setgetattr_t:s0", [&]() {
    ASSERT_TRUE(test_helper::HasSysAdmin());
    EXPECT_THAT(SetXattr(file.name(), kTestSecurityAttrName, kTestAttrValue),
                SyscallFailsWithErrno(EPERM));
    EXPECT_THAT(SetXattr(file.name(), kTestTrustedAttrName, kTestAttrValue),
                SyscallFailsWithErrno(EPERM));
  }));
}

TEST(XattrTest, GetTrustedXattrRequiresCapSysAdminAndGetAttr) {
  auto scoped_fscreate = ScopedTaskAttrResetter::SetTaskAttr("fscreate", kTestFileLabel);
  test_helper::ScopedTempFD file;

  ASSERT_THAT(SetXattr(file.name(), kTestTrustedAttrName, kTestAttrValue), SyscallSucceeds());

  auto enforcing = ScopedEnforcement::SetEnforcing();
  EXPECT_TRUE(RunSubprocessAs("test_u:test_r:test_xattr_capsysadmin_t:s0", [&]() {
    ASSERT_TRUE(test_helper::HasSysAdmin());
    char buffer[100]{};
    EXPECT_THAT(getxattr(file.name().c_str(), kTestTrustedAttrName, buffer, sizeof(buffer)),
                SyscallSucceedsWithValue(sizeof(kTestAttrValue)));
    EXPECT_EQ(std::string_view(kTestAttrValue), buffer);
  }));

  EXPECT_TRUE(RunSubprocessAs("test_u:test_r:test_xattr_capsysadmin_nosetgetattr_t:s0", [&]() {
    ASSERT_TRUE(test_helper::HasSysAdmin());
    char buffer[100]{};
    EXPECT_THAT(getxattr(file.name().c_str(), kTestTrustedAttrName, buffer, sizeof(buffer)),
                SyscallFailsWithErrno(EACCES));
  }));

  EXPECT_TRUE(RunSubprocessAs("test_u:test_r:test_xattr_nocapsysadmin_setgetattr_t:s0", [&]() {
    ASSERT_TRUE(test_helper::HasSysAdmin());
    char buffer[100]{};
    EXPECT_THAT(getxattr(file.name().c_str(), kTestTrustedAttrName, buffer, sizeof(buffer)),
                SyscallFailsWithErrno(ENODATA));
  }));
}

TEST(XattrTest, GetSecurityXattrRequiresOnlyGetAttr) {
  auto scoped_fscreate = ScopedTaskAttrResetter::SetTaskAttr("fscreate", kTestFileLabel);
  test_helper::ScopedTempFD file;

  ASSERT_THAT(SetXattr(file.name(), kTestSecurityAttrName, kTestAttrValue), SyscallSucceeds());

  auto enforcing = ScopedEnforcement::SetEnforcing();
  EXPECT_TRUE(RunSubprocessAs("test_u:test_r:test_xattr_capsysadmin_t:s0", [&]() {
    ASSERT_TRUE(test_helper::HasSysAdmin());
    char buffer[100]{};
    EXPECT_THAT(getxattr(file.name().c_str(), kTestSecurityAttrName, buffer, sizeof(buffer)),
                SyscallSucceedsWithValue(sizeof(kTestAttrValue)));
    EXPECT_EQ(std::string_view(kTestAttrValue), buffer);
  }));

  EXPECT_TRUE(RunSubprocessAs("test_u:test_r:test_xattr_capsysadmin_nosetgetattr_t:s0", [&]() {
    ASSERT_TRUE(test_helper::HasSysAdmin());
    char buffer[100]{};
    EXPECT_THAT(getxattr(file.name().c_str(), kTestSecurityAttrName, buffer, sizeof(buffer)),
                SyscallFailsWithErrno(EACCES));
  }));

  EXPECT_TRUE(RunSubprocessAs("test_u:test_r:test_xattr_nocapsysadmin_setgetattr_t:s0", [&]() {
    ASSERT_TRUE(test_helper::HasSysAdmin());
    char buffer[100]{};
    EXPECT_THAT(getxattr(file.name().c_str(), kTestSecurityAttrName, buffer, sizeof(buffer)),
                SyscallSucceedsWithValue(sizeof(kTestAttrValue)));
    EXPECT_EQ(std::string_view(kTestAttrValue), buffer);
  }));
}

TEST(XattrTest, SetUserRequiresFileWriteAndSetAttr) {
  auto scoped_fscreate = ScopedTaskAttrResetter::SetTaskAttr("fscreate", kTestFileLabel);
  test_helper::ScopedTempFD file;

  auto enforcing = ScopedEnforcement::SetEnforcing();
  EXPECT_TRUE(RunSubprocessAs("test_u:test_r:test_xattr_user_t:s0", [&]() {
    EXPECT_THAT(SetXattr(file.name(), kTestUserAttrName, kTestAttrValue), SyscallSucceeds());
  }));
  EXPECT_TRUE(RunSubprocessAs("test_u:test_r:test_xattr_user_nowrite_t:s0", [&]() {
    EXPECT_THAT(SetXattr(file.name(), kTestUserAttrName, kTestAttrValue),
                SyscallFailsWithErrno(EACCES));
  }));
  EXPECT_TRUE(RunSubprocessAs("test_u:test_r:test_xattr_user_nosetattr_t:s0", [&]() {
    EXPECT_THAT(SetXattr(file.name(), kTestUserAttrName, kTestAttrValue),
                SyscallFailsWithErrno(EACCES));
  }));
}

TEST(XattrTest, GetUserRequiresFileReadAndGetAttr) {
  auto scoped_fscreate = ScopedTaskAttrResetter::SetTaskAttr("fscreate", kTestFileLabel);
  test_helper::ScopedTempFD file;

  ASSERT_THAT(SetXattr(file.name(), kTestUserAttrName, kTestAttrValue), SyscallSucceeds());

  auto enforcing = ScopedEnforcement::SetEnforcing();

  EXPECT_TRUE(RunSubprocessAs("test_u:test_r:test_xattr_user_t:s0", [&]() {
    char buffer[100]{};
    EXPECT_THAT(getxattr(file.name().c_str(), kTestUserAttrName, buffer, sizeof(buffer)),
                SyscallSucceedsWithValue(sizeof(kTestAttrValue)));
    EXPECT_EQ(std::string_view(kTestAttrValue), buffer);
  }));

  EXPECT_TRUE(RunSubprocessAs("test_u:test_r:test_xattr_user_noread_t:s0", [&]() {
    char buffer[100]{};
    EXPECT_THAT(getxattr(file.name().c_str(), kTestUserAttrName, buffer, sizeof(buffer)),
                SyscallFailsWithErrno(EACCES));
  }));

  EXPECT_TRUE(RunSubprocessAs("test_u:test_r:test_xattr_user_nogetattr_t:s0", [&]() {
    char buffer[100]{};
    EXPECT_THAT(getxattr(file.name().c_str(), kTestUserAttrName, buffer, sizeof(buffer)),
                SyscallFailsWithErrno(EACCES));
  }));
}

TEST(XattrTest, ListOmitsInaccessiblePrefixes) {
  auto scoped_fscreate = ScopedTaskAttrResetter::SetTaskAttr("fscreate", kTestFileLabel);
  test_helper::ScopedTempFD file;

  ASSERT_THAT(SetXattr(file.name(), kTestUserAttrName, kTestAttrValue), SyscallSucceeds());
  ASSERT_THAT(SetXattr(file.name(), kTestSecurityAttrName, kTestAttrValue), SyscallSucceeds());
  ASSERT_THAT(SetXattr(file.name(), kTestTrustedAttrName, kTestAttrValue), SyscallSucceeds());

  auto enforcing = ScopedEnforcement::SetEnforcing();

  // `listxattr()` includes the "user.*" xattrs even though "read" permission is lacking.
  EXPECT_TRUE(RunSubprocessAs("test_u:test_r:test_xattr_capsysadmin_t:s0", [&]() {
    char buffer[1024]{};
    EXPECT_THAT(listxattr(file.name().c_str(), buffer, sizeof(buffer)), SyscallSucceeds());
    auto attrs = ParseListXattrs(buffer, sizeof(buffer));
    EXPECT_TRUE(attrs.have_user);
    EXPECT_TRUE(attrs.have_security);
    EXPECT_TRUE(attrs.have_trusted);
  }));

  // `listxattr()` omits the "trusted.*" xattr because CAP_SYS_ADMIN permission is lacking.
  EXPECT_TRUE(RunSubprocessAs("test_u:test_r:test_xattr_user_t:s0", [&]() {
    char buffer[1024]{};
    EXPECT_THAT(listxattr(file.name().c_str(), buffer, sizeof(buffer)), SyscallSucceeds());
    auto attrs = ParseListXattrs(buffer, sizeof(buffer));
    EXPECT_TRUE(attrs.have_user);
    EXPECT_TRUE(attrs.have_security);
    EXPECT_FALSE(attrs.have_trusted);
  }));

  // `listxattr()` includes the "user.*" namespace even if the user is not the owner.
  EXPECT_TRUE(RunSubprocessAs("test_u:test_r:test_xattr_user_t:s0", [&]() {
    SAFE_SYSCALL(setuid(1));
    ASSERT_FALSE(test_helper::HasSysAdmin());
    ASSERT_FALSE(test_helper::HasCapability(CAP_FOWNER));

    char buffer[1024]{};
    EXPECT_THAT(listxattr(file.name().c_str(), buffer, sizeof(buffer)), SyscallSucceeds());
    auto attrs = ParseListXattrs(buffer, sizeof(buffer));
    EXPECT_TRUE(attrs.have_user);
  }));

  // `listxattr()` omits the "trusted.*" xattr because CAP_SYS_ADMIN permission is lacking,
  // but includes the "user.*" attributes even though the "read" permission is lacking.
  EXPECT_TRUE(RunSubprocessAs("test_u:test_r:test_xattr_user_noread_t:s0", [&]() {
    char buffer[1024]{};
    EXPECT_THAT(listxattr(file.name().c_str(), buffer, sizeof(buffer)), SyscallSucceeds());
    auto attrs = ParseListXattrs(buffer, sizeof(buffer));
    EXPECT_TRUE(attrs.have_user);
    EXPECT_TRUE(attrs.have_security);
    EXPECT_FALSE(attrs.have_trusted);
  }));

  // `listxattr()` requires the "getattr" permission to the file.
  EXPECT_TRUE(RunSubprocessAs("test_u:test_r:test_xattr_user_nogetattr_t:s0", [&]() {
    char buffer[1024]{};
    EXPECT_THAT(listxattr(file.name().c_str(), buffer, sizeof(buffer)),
                SyscallFailsWithErrno(EACCES));
  }));
}

TEST(XattrTest, RemoveRequiresSameChecksAsSet) {
  auto scoped_fscreate = ScopedTaskAttrResetter::SetTaskAttr("fscreate", kTestFileLabel);
  test_helper::ScopedTempFD file;

  ASSERT_THAT(SetXattr(file.name(), kTestUserAttrName, kTestAttrValue), SyscallSucceeds());
  ASSERT_THAT(SetXattr(file.name(), kTestSecurityAttrName, kTestAttrValue), SyscallSucceeds());
  ASSERT_THAT(SetXattr(file.name(), kTestTrustedAttrName, kTestAttrValue), SyscallSucceeds());

  auto enforcing = ScopedEnforcement::SetEnforcing();

  // trusted.* and security.* attributes can be removed with CAP_SYS_ADMIN, but user.* cannot
  // without "write" & "setattr".
  EXPECT_TRUE(RunSubprocessAs("test_u:test_r:test_xattr_capsysadmin_t:s0", [&]() {
    EXPECT_THAT(removexattr(file.name().c_str(), kTestUserAttrName), SyscallFailsWithErrno(EACCES));
    EXPECT_THAT(removexattr(file.name().c_str(), kTestSecurityAttrName), SyscallSucceeds());
    EXPECT_THAT(removexattr(file.name().c_str(), kTestTrustedAttrName), SyscallSucceeds());
  }));

  ASSERT_THAT(SetXattr(file.name(), kTestUserAttrName, kTestAttrValue), SyscallSucceeds());
  ASSERT_THAT(SetXattr(file.name(), kTestSecurityAttrName, kTestAttrValue), SyscallSucceeds());
  ASSERT_THAT(SetXattr(file.name(), kTestTrustedAttrName, kTestAttrValue), SyscallSucceeds());

  // trusted.* and security.* cannot be removed without CAP_SYS_ADMIN, but user.* can
  // be removed, by domain with "write" and "setattr".
  EXPECT_TRUE(RunSubprocessAs("test_u:test_r:test_xattr_user_t:s0", [&]() {
    EXPECT_THAT(removexattr(file.name().c_str(), kTestUserAttrName), SyscallSucceeds());
    EXPECT_THAT(removexattr(file.name().c_str(), kTestSecurityAttrName),
                SyscallFailsWithErrno(EPERM));
    EXPECT_THAT(removexattr(file.name().c_str(), kTestTrustedAttrName),
                SyscallFailsWithErrno(EPERM));
  }));

  ASSERT_THAT(SetXattr(file.name(), kTestUserAttrName, kTestAttrValue), SyscallSucceeds());
  ASSERT_THAT(SetXattr(file.name(), kTestSecurityAttrName, kTestAttrValue), SyscallSucceeds());
  ASSERT_THAT(SetXattr(file.name(), kTestTrustedAttrName, kTestAttrValue), SyscallSucceeds());

  // trusted.* and security.* cannot be removed without CAP_SYS_ADMIN.
  // user.* cannot be removed without "write".
  EXPECT_TRUE(RunSubprocessAs("test_u:test_r:test_xattr_user_nowrite_t:s0", [&]() {
    EXPECT_THAT(removexattr(file.name().c_str(), kTestUserAttrName), SyscallFailsWithErrno(EACCES));
    EXPECT_THAT(removexattr(file.name().c_str(), kTestSecurityAttrName),
                SyscallFailsWithErrno(EPERM));
    EXPECT_THAT(removexattr(file.name().c_str(), kTestTrustedAttrName),
                SyscallFailsWithErrno(EPERM));
  }));

  ASSERT_THAT(SetXattr(file.name(), kTestUserAttrName, kTestAttrValue), SyscallSucceeds());
  ASSERT_THAT(SetXattr(file.name(), kTestSecurityAttrName, kTestAttrValue), SyscallSucceeds());
  ASSERT_THAT(SetXattr(file.name(), kTestTrustedAttrName, kTestAttrValue), SyscallSucceeds());

  // trusted.* and security.* cannot be removed without CAP_SYS_ADMIN.
  // user.* cannot be removed without "setattr".
  EXPECT_TRUE(RunSubprocessAs("test_u:test_r:test_xattr_user_nosetattr_t:s0", [&]() {
    EXPECT_THAT(removexattr(file.name().c_str(), kTestUserAttrName), SyscallFailsWithErrno(EACCES));
    EXPECT_THAT(removexattr(file.name().c_str(), kTestSecurityAttrName),
                SyscallFailsWithErrno(EPERM));
    EXPECT_THAT(removexattr(file.name().c_str(), kTestTrustedAttrName),
                SyscallFailsWithErrno(EPERM));
  }));
}

TEST(XattrTest, SetSelinuxWithRelabelFromAndTo) {
  auto scoped_fscreate = ScopedTaskAttrResetter::SetTaskAttr("fscreate", kTestFileLabel);
  test_helper::ScopedTempFD file;

  auto enforcing = ScopedEnforcement::SetEnforcing();

  EXPECT_TRUE(RunSubprocessAs("test_u:test_r:test_xattr_relabel_t:s0", [&]() {
    EXPECT_THAT(SetXattr(file.name(), kSelinuxAttrName, kTestFileNewLabel), SyscallSucceeds());
  }));
}

TEST(XattrTest, CannotSetSelinuxToSameWithoutRelabelFrom) {
  auto scoped_fscreate = ScopedTaskAttrResetter::SetTaskAttr("fscreate", kTestFileLabel);
  test_helper::ScopedTempFD file;

  auto enforcing = ScopedEnforcement::SetEnforcing();

  EXPECT_TRUE(RunSubprocessAs("test_u:test_r:test_xattr_relabel_t:s0", [&]() {
    EXPECT_THAT(SetXattr(file.name(), kSelinuxAttrName, kTestFileNewLabel), SyscallSucceeds());
    EXPECT_THAT(SetXattr(file.name(), kSelinuxAttrName, kTestFileNewLabel),
                SyscallFailsWithErrno(EACCES));
  }));
}

TEST(XattrTest, SetSelinuxNonOwnerFailsWithoutCapFOwner) {
  auto scoped_fscreate = ScopedTaskAttrResetter::SetTaskAttr("fscreate", kTestFileLabel);
  test_helper::ScopedTempFD file;

  auto enforcing = ScopedEnforcement::SetEnforcing();

  EXPECT_TRUE(RunSubprocessAs("test_u:test_r:test_xattr_relabel_t:s0", [&]() {
    SAFE_SYSCALL(setuid(1));

    EXPECT_THAT(SetXattr(file.name(), kSelinuxAttrName, kTestFileNewLabel),
                SyscallFailsWithErrno(EPERM));
  }));
}

TEST(XattrTest, GetSelinuxRequiresNoPerms) {
  auto scoped_fscreate = ScopedTaskAttrResetter::SetTaskAttr("fscreate", kTestFileLabel);
  test_helper::ScopedTempFD file;

  auto enforcing = ScopedEnforcement::SetEnforcing();

  EXPECT_TRUE(RunSubprocessAs("test_u:test_r:test_xattr_getlabel_t:s0", [&]() {
    char buffer[256]{};
    EXPECT_THAT(getxattr(file.name().c_str(), kSelinuxAttrName, buffer, sizeof(buffer)),
                SyscallSucceedsWithValue(sizeof(kTestFileLabel)));
    EXPECT_EQ(std::string_view(kTestFileLabel), buffer);
  }));
}

TEST(XattrTest, SetSelinuxMissingPermissions) {
  auto scoped_fscreate = ScopedTaskAttrResetter::SetTaskAttr("fscreate", kTestFileLabel);
  test_helper::ScopedTempFD file;

  auto enforcing = ScopedEnforcement::SetEnforcing();

  EXPECT_TRUE(RunSubprocessAs("test_u:test_r:test_xattr_relabel_norelabelto_t:s0", [&]() {
    EXPECT_THAT(SetXattr(file.name(), kSelinuxAttrName, kTestFileNewLabel),
                SyscallFailsWithErrno(EACCES));
  }));
  EXPECT_TRUE(RunSubprocessAs("test_u:test_r:test_xattr_relabel_norelabelfrom_t:s0", [&]() {
    EXPECT_THAT(SetXattr(file.name(), kSelinuxAttrName, kTestFileNewLabel),
                SyscallFailsWithErrno(EACCES));
  }));
  EXPECT_TRUE(RunSubprocessAs("test_u:test_r:test_xattr_relabel_t:s0", [&]() {
    EXPECT_THAT(SetXattr(file.name(), kSelinuxAttrName, kTestFileNoAssociateLabel),
                SyscallFailsWithErrno(EACCES));
  }));
}

TEST(XattrTest, RemoveSelinuxNotPermitted) {
  auto scoped_fscreate = ScopedTaskAttrResetter::SetTaskAttr("fscreate", kTestFileLabel);
  test_helper::ScopedTempFD file;

  auto enforcing = ScopedEnforcement::SetEnforcing();

  EXPECT_TRUE(RunSubprocessAs("test_u:test_r:test_xattr_relabel_t:s0", [&]() {
    EXPECT_THAT(removexattr(file.name().c_str(), kSelinuxAttrName), SyscallFailsWithErrno(EACCES));
  }));
}

}  // namespace

extern std::string DoPrePolicyLoadWork() { return "xattr_access_policy"; }
