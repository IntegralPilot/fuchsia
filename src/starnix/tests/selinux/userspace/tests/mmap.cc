// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <sys/mman.h>
#include <unistd.h>

#include <gtest/gtest.h>

#include "src/starnix/tests/selinux/userspace/util.h"
#include "src/starnix/tests/syscalls/cpp/syscall_matchers.h"
#include "src/starnix/tests/syscalls/cpp/test_helper.h"

extern std::string DoPrePolicyLoadWork() { return "mmap_policy"; }

namespace {

constexpr char kFileLabel[] = "test_u:object_r:test_mmap_file_t:s0";
constexpr char kTestPayload[] = "mmap_selinux_test_data";

// Verifies that mmap() succeeds for both shared and private mappings when all required permissions
// are granted.
TEST(MMapTest, MMapAllowed) {
  auto enforce = ScopedEnforcement::SetEnforcing();

  ASSERT_TRUE(RunSubprocessAs(MakeTestSecurityContext("test_mmap_parent_t"), [&] {
    auto test_file = ScopedTempFDWithLabel(kFileLabel);
    ASSERT_THAT(test_file.fd(), SyscallSucceeds());

    ASSERT_THAT(write(test_file.fd(), kTestPayload, sizeof(kTestPayload)),
                SyscallSucceedsWithValue(sizeof(kTestPayload)));

    auto shared_mapping = test_helper::ScopedMMap::MMap(nullptr, sizeof(kTestPayload), PROT_READ,
                                                        MAP_SHARED, test_file.fd(), 0);
    EXPECT_THAT(shared_mapping, SyscallResultIsOk());
    if (shared_mapping.is_ok()) {
      EXPECT_STREQ(static_cast<const char*>(shared_mapping->mapping()), kTestPayload);
    }

    auto private_mapping = test_helper::ScopedMMap::MMap(nullptr, sizeof(kTestPayload), PROT_READ,
                                                         MAP_PRIVATE, test_file.fd(), 0);
    EXPECT_THAT(private_mapping, SyscallResultIsOk());
    if (private_mapping.is_ok()) {
      EXPECT_STREQ(static_cast<const char*>(private_mapping->mapping()), kTestPayload);
    }
  }));
}

class MMapDeniedTest : public testing::TestWithParam<const char*> {};

// Verifies that mmap() with PROT_READ | PROT_WRITE fails with EACCES when dynamically transitioning
// to a domain lacking required permissions (`fd { use }`, `file { map }`, `file { read }`, or
// `file { write }`), except that MAP_PRIVATE succeeds without `file { write }` (since private
// mappings do not write back to the underlying file).
TEST_P(MMapDeniedTest, MMapDenied) {
  auto enforce = ScopedEnforcement::SetEnforcing();
  const std::string child_domain =
      MakeTestSecurityContext(std::string("test_mmap_child_") + GetParam() + "_t");

  ASSERT_TRUE(RunSubprocessAs(MakeTestSecurityContext("test_mmap_parent_t"), [&] {
    auto test_file = ScopedTempFDWithLabel(kFileLabel);
    ASSERT_THAT(test_file.fd(), SyscallSucceeds());

    ASSERT_THAT(WriteTaskAttr("current", child_domain), SyscallResultIsOk());

    EXPECT_THAT(test_helper::ScopedMMap::MMap(nullptr, sizeof(kTestPayload), PROT_READ | PROT_WRITE,
                                              MAP_SHARED, test_file.fd(), 0),
                SyscallResultIsErrno(EACCES));

    auto private_mapping = test_helper::ScopedMMap::MMap(
        nullptr, sizeof(kTestPayload), PROT_READ | PROT_WRITE, MAP_PRIVATE, test_file.fd(), 0);
    if (std::string_view(GetParam()) == "no_write") {
      EXPECT_THAT(private_mapping, SyscallResultIsOk());
    } else {
      EXPECT_THAT(private_mapping, SyscallResultIsErrno(EACCES));
    }
  }));
}

INSTANTIATE_TEST_SUITE_P(MMapTest, MMapDeniedTest,
                         testing::Values("no_use_fd", "no_map", "no_read", "no_write"),
                         [](const testing::TestParamInfo<const char*>& info) {
                           return info.param;
                         });

}  // namespace
