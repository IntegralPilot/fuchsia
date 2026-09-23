// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <lib/fit/result.h>
#include <unistd.h>

#include <string>

#include <fbl/unique_fd.h>
#include <gtest/gtest.h>
#include <linux/fs.h>

#include "src/lib/files/file.h"
#include "src/lib/fxl/strings/string_number_conversions.h"
#include "src/lib/fxl/strings/string_printf.h"
#include "src/starnix/tests/selinux/userspace/util.h"
#include "src/starnix/tests/syscalls/cpp/syscall_matchers.h"

extern std::string DoPrePolicyLoadWork() { return "selinuxfs_policy"; }

namespace {

using SeLinuxApiResult = fit::result<int, std::string>;

/// Calls the specified `api` with the supplied `request` and returns the resulting output.
SeLinuxApiResult CallSeLinuxApi(std::string_view api, std::string_view request) {
  constexpr char kSeLinuxFsApiFmt[] = "/sys/fs/selinux/%s";

  auto api_path = fxl::StringPrintf(kSeLinuxFsApiFmt, std::string(api).data());
  fbl::unique_fd api_fd(open(api_path.data(), O_RDWR));
  if (!api_fd.is_valid()) {
    return fit::error(errno);
  }

  if (!request.empty() && write(api_fd.get(), request.data(), request.length()) < 0) {
    return fit::error(errno);
  }

  std::string result;
  char read_buf[10];
  while (true) {
    ssize_t read_result = read(api_fd.get(), read_buf, sizeof(read_buf));
    if (read_result == 0) {
      return fit::ok(result);
    }
    if (read_result < 0) {
      return fit::error(errno);
    }
    result.append(read_buf, read_result);
  }
}

/// Calls CallSeLinuxApi() and expects the result to be empty, or to have a trailing NUL, which is
/// removed.
SeLinuxApiResult CallSeLinuxApiAndRemoveNul(std::string_view api, std::string_view request) {
  auto result = CallSeLinuxApi(api, request);
  if (result.is_error()) {
    return result;
  }
  return RemoveTrailingNul(result.value());
}

/// Reads a `uint32_t` value from the SELinuxFS `path` and returns it, if it is well-formed.
uint32_t ReadSeLinuxFsU32(std::string path) {
  auto result = CallSeLinuxApi(path, std::string_view());
  uint32_t class_id{};
  if (result.is_error() || !fxl::StringToNumberWithError(result.value(), &class_id)) {
    ADD_FAILURE() << "Failed to read SELinuxFS U32: " << path;
    return 0u;
  }
  return class_id;
}

uint32_t GetClassId(std::string_view class_name) {
  constexpr char kSeLinuxFsClassIndexApiNameFmt[] = "class/%s/index";

  auto api_name = fxl::StringPrintf(kSeLinuxFsClassIndexApiNameFmt, std::string(class_name).data());
  return ReadSeLinuxFsU32(api_name);
}

uint32_t GetPermissionId(std::string_view class_name, std::string_view perm_name) {
  constexpr char kSeLinuxFsPermissionIndexApiNameFmt[] = "class/%s/perms/%s";

  auto api_name = fxl::StringPrintf(kSeLinuxFsPermissionIndexApiNameFmt,
                                    std::string(class_name).data(), std::string(perm_name).data());
  return ReadSeLinuxFsU32(api_name);
}

uint32_t GetAccessVector(std::string_view class_name, std::string_view perm_name) {
  auto perm_index = GetPermissionId(class_name, perm_name);
  if (perm_index < 1 || perm_index > 32) {
    ADD_FAILURE() << "Read invalid permission Id: " << perm_index;
    return 0u;
  }
  uint32_t perm_mask = 1 << (perm_index - 1);
  return perm_mask;
}

SeLinuxApiResult ValidateContext(std::string_view context) {
  return CallSeLinuxApiAndRemoveNul("context", context);
}

SeLinuxApiResult ComputeCreateContext(std::string_view source_context,
                                      std::string_view target_context,
                                      std::string_view target_class) {
  auto class_id = GetClassId(target_class);
  constexpr char kSeLinuxCreateRequestFmt[] = "%s %s %hu";
  auto request = fxl::StringPrintf(kSeLinuxCreateRequestFmt, std::string(source_context).data(),
                                   std::string(target_context).data(), class_id);
  return CallSeLinuxApiAndRemoveNul("create", request);
}

SeLinuxApiResult ComputeAccess(std::string_view source_context, std::string_view target_context,
                               std::string_view target_class, uint32_t requested) {
  auto class_id = GetClassId(target_class);
  constexpr char kSeLinuxCreateRequestFmt[] = "%s %s %hu %x";
  auto request = fxl::StringPrintf(kSeLinuxCreateRequestFmt, std::string(source_context).data(),
                                   std::string(target_context).data(), class_id, requested);
  return CallSeLinuxApi("access", request);
}

TEST(SeLinuxFsNull, HasPolicyDevnullContext) {
  constexpr char kSeLinuxFsNull[] = "/sys/fs/selinux/null";

  EXPECT_THAT(GetLabel(kSeLinuxFsNull), SyscallResultIsOk("system_u:object_r:devnull_t:s0"));
}

TEST(SeLinuxFs, RootNodeLabel) {
  EXPECT_THAT(GetLabel("/sys/fs/selinux"), SyscallResultIsOk("system_u:object_r:selinuxfs_t:s0"));
}

TEST(SeLinuxFsContext, OneRequestPerInstance) {
  fbl::unique_fd api_fd(open("/sys/fs/selinux/context", O_RDWR));
  ASSERT_TRUE(api_fd.is_valid()) << strerror(errno);

  constexpr char kMinimumValidContext[] = "test_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0";
  ASSERT_EQ(write(api_fd.get(), kMinimumValidContext, sizeof(kMinimumValidContext)),
            (ssize_t)sizeof(kMinimumValidContext));

  EXPECT_EQ(write(api_fd.get(), kMinimumValidContext, sizeof(kMinimumValidContext)), -1);
  EXPECT_EQ(errno, EBUSY);
}

TEST(SeLinuxFsContext, ReadUpdatesSeekPosition) {
  fbl::unique_fd api_fd(open("/sys/fs/selinux/context", O_RDWR));
  ASSERT_TRUE(api_fd.is_valid()) << strerror(errno);

  constexpr char kMinimumValidContext[] = "test_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0";
  ASSERT_EQ(write(api_fd.get(), kMinimumValidContext, sizeof(kMinimumValidContext)),
            (ssize_t)sizeof(kMinimumValidContext));

  // Read the result back one byte at a time.
  std::string result;
  for (;;) {
    char buf;
    auto read_result = read(api_fd.get(), &buf, sizeof(buf));
    if (read_result == 0) {
      break;
    } else if (read_result < 0) {
      ADD_FAILURE() << "read() failed: " << strerror(errno);
    }
    result.push_back(buf);
  }

  EXPECT_THAT(RemoveTrailingNul(result), SyscallResultIsOk(kMinimumValidContext));
}

TEST(SeLinuxFsContext, ValidatesRequiredFieldsPresent) {
  // Contexts that have too few colons to provide user, role, type & sensitivity are rejected.
  EXPECT_THAT(ValidateContext("test_selinuxfs_u"), SyscallResultIsErrno(EINVAL));
  EXPECT_THAT(ValidateContext("test_selinuxfs_u:test_selinuxfs_r"), SyscallResultIsErrno(EINVAL));
  EXPECT_THAT(ValidateContext("test_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t"),
              SyscallResultIsErrno(EINVAL));

  // The minimum valid context has at least user, role, type and low/default sensitivity.
  constexpr std::string_view kMinimumValidContext =
      "test_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0";
  EXPECT_THAT(ValidateContext(kMinimumValidContext), SyscallResultIsOk(kMinimumValidContext));
}

TEST(SeLinuxFsContext, ValidatesFieldValues) {
  // Valid contexts are successfully written, and can be read-back.
  constexpr std::string_view kValidContext =
      "test_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0:c0-s2:c0.c2";
  EXPECT_THAT(ValidateContext(kValidContext), SyscallResultIsOk(kValidContext));

  // Context user must be defined by the policy.
  EXPECT_THAT(ValidateContext("bad_value:test_selinuxfs_r:test_selinuxfs_t:s0:c0-s2:c0.c2"),
              SyscallResultIsErrno(EINVAL));

  // Context role must be defined by the policy.
  EXPECT_THAT(ValidateContext("test_selinuxfs_u:bad_value:test_selinuxfs_t:s0:c0-s2:c0.c2"),
              SyscallResultIsErrno(EINVAL));

  // Context type/domain must be defined by the policy.
  EXPECT_THAT(ValidateContext("test_selinuxfs_u:test_selinuxfs_r:bad_value:s0:c0-s2:c0.c2"),
              SyscallResultIsErrno(EINVAL));

  // Context low & high sensitivities must be defined by the policy.
  EXPECT_THAT(
      ValidateContext("test_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:bad_value:c0-s2:c0.c2"),
      SyscallResultIsErrno(EINVAL));
  EXPECT_THAT(
      ValidateContext("test_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0:c0-bad_value:c0.c2"),
      SyscallResultIsErrno(EINVAL));

  // Context low & high categories must be defined by the policy.
  EXPECT_THAT(
      ValidateContext("test_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0:bad_value-s2:c0.c2"),
      SyscallResultIsErrno(EINVAL));
  EXPECT_THAT(
      ValidateContext("test_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0:c0-s2:c0.bad_value"),
      SyscallResultIsErrno(EINVAL));
}

TEST(SeLinuxFsContext, ValidatesAllowedUserFieldValues) {
  // The "test_selinuxfs_u" user is granted the full range of categories.
  constexpr std::string_view kValidContext =
      "test_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0:c0-s2:c0.c2";
  EXPECT_THAT(ValidateContext(kValidContext), SyscallResultIsOk(kValidContext));

  // The "test_selinuxfs_limited_u" user is granted only "s0" sensitivity, must have "c0" category
  // and may have "c1" category.
  constexpr std::string_view kLimitedContext_Valid =
      "test_selinuxfs_limited_level_u:test_selinuxfs_r:test_selinuxfs_t:s0:c0";
  EXPECT_THAT(ValidateContext(kLimitedContext_Valid), SyscallResultIsOk(kLimitedContext_Valid));

  constexpr std::string_view kLimitedContext_MissingCategory =
      "test_selinuxfs_limited_level_u:test_selinuxfs_r:test_selinuxfs_t:s0";
  EXPECT_THAT(ValidateContext(kLimitedContext_MissingCategory), SyscallResultIsErrno(EINVAL));

  constexpr std::string_view kLimitedContext_BadSensitivity =
      "test_selinuxfs_limited_level_u:test_selinuxfs_r:test_selinuxfs_t:s1:c0";
  EXPECT_THAT(ValidateContext(kLimitedContext_BadSensitivity), SyscallResultIsErrno(EINVAL));
}

TEST(SeLinuxFsContext, NormalizeCategories) {
  // Expansion of a three-element category span results in the same Security Context as using the
  // range syntax directly.
  constexpr std::string_view kThreeCategoryContextFormA =
      "test_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s2:c0.c2";
  constexpr std::string_view kThreeCategoryContextFormB =
      "test_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s2:c0,c1,c2";
  EXPECT_EQ(ValidateContext(kThreeCategoryContextFormA),
            ValidateContext(kThreeCategoryContextFormB));

  // Using a pair of categories results in the same Security Context as a two-element range.
  constexpr std::string_view kTwoCategoryContextFormA =
      "test_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s2:c0.c1";
  constexpr std::string_view kTwoCategoryContextFormB =
      "test_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s2:c0,c1";
  EXPECT_EQ(ValidateContext(kTwoCategoryContextFormA), ValidateContext(kTwoCategoryContextFormB));
}

// Validate that Security Contexts for new "process" class instances inherit the source role & type.
TEST(SeLinuxFsCreate, DefaultComputeCreateForProcess) {
  constexpr std::string_view kSourceContext =
      "test_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0";
  constexpr std::string_view kTargetContext =
      "test_selinuxfs_create_target_u:test_selinuxfs_create_target_r:test_selinuxfs_create_target_t:s0";

  EXPECT_THAT(ComputeCreateContext(kSourceContext, kTargetContext, "process"),
              SyscallResultIsOk("test_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0"));
}

TEST(SeLinuxFsCreate, OneRequestPerInstance) {
  fbl::unique_fd api_fd(open("/sys/fs/selinux/create", O_RDWR));
  ASSERT_TRUE(api_fd.is_valid()) << strerror(errno);

  const uint32_t kTargetClassId = GetClassId("test_selinuxfs_target_class");
  const auto kValidRequest = fxl::StringPrintf(
      "test_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0 test_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0 %u",
      kTargetClassId);
  ASSERT_EQ(write(api_fd.get(), kValidRequest.data(), kValidRequest.size()),
            (ssize_t)kValidRequest.size());

  EXPECT_EQ(write(api_fd.get(), kValidRequest.data(), kValidRequest.size()), -1);
  EXPECT_EQ(errno, EBUSY);
}

TEST(SeLinuxFsCreate, ReadUpdatesSeekPosition) {
  fbl::unique_fd api_fd(open("/sys/fs/selinux/create", O_RDWR));
  ASSERT_TRUE(api_fd.is_valid()) << strerror(errno);

  const uint32_t kTargetClassId = GetClassId("process");
  const auto kValidRequest = fxl::StringPrintf(
      "test_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0 test_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0 %u",
      kTargetClassId);
  ASSERT_EQ(write(api_fd.get(), kValidRequest.data(), kValidRequest.size()),
            (ssize_t)kValidRequest.size());

  // Read the result back one byte at a time.
  std::string result;
  for (;;) {
    char buf;
    auto read_result = read(api_fd.get(), &buf, sizeof(buf));
    if (read_result == 0) {
      break;
    } else if (read_result < 0) {
      ADD_FAILURE() << "read() failed: " << strerror(errno);
    }
    result.push_back(buf);
  }

  constexpr char kExpected[] = "test_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0";
  EXPECT_THAT(RemoveTrailingNul(result), SyscallResultIsOk(kExpected));
}

// Validate that Security Contexts for new socket-like class instances behave the same as "process".
TEST(SeLinuxFsCreate, DefaultComputeCreateForSocket) {
  constexpr std::string_view kSourceContext =
      "test_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0";
  constexpr std::string_view kTargetContext =
      "test_selinuxfs_create_target_u:test_selinuxfs_create_target_r:test_selinuxfs_create_target_t:s0";

  EXPECT_THAT(ComputeCreateContext(kSourceContext, kTargetContext, "tcp_socket"),
              SyscallResultIsOk("test_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0"));
}

// Validate that Security Contexts for non-process/socket-like class instances receive "object_r"
// role, and the target/parent type.
TEST(SeLinuxFsCreate, DefaultComputeCreateForFile) {
  constexpr std::string_view kSourceContext =
      "test_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0";
  constexpr std::string_view kTargetContext =
      "test_selinuxfs_create_target_u:test_selinuxfs_create_target_r:test_selinuxfs_create_target_t:s0";

  EXPECT_THAT(ComputeCreateContext(kSourceContext, kTargetContext, "file"),
              SyscallResultIsOk("test_selinuxfs_u:object_r:test_selinuxfs_create_target_t:s0"));
}

TEST(SeLinuxFsCreate, ComputeCreateForFifoFile) {
  constexpr std::string_view kSourceContext =
      "test_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0";
  constexpr std::string_view kTargetContext =
      "test_selinuxfs_create_target_u:test_selinuxfs_create_target_r:test_selinuxfs_create_target_t:s0";

  EXPECT_THAT(ComputeCreateContext(kSourceContext, kTargetContext, "fifo_file"),
              SyscallResultIsOk("test_selinuxfs_u:object_r:test_selinuxfs_create_target_t:s0"));
}

// Validate handling of whitespace between request elements.
TEST(SeLinuxFsCreate, ExtraWhitespace) {
  constexpr std::string_view kExpectedContext =
      "test_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0";

  // Additional whitespace between elements is ignored.
  EXPECT_THAT(
      ComputeCreateContext(" test_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0",
                           "test_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0", "process"),
      SyscallResultIsOk(kExpectedContext));
  EXPECT_THAT(
      ComputeCreateContext("test_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0 ",
                           "test_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0", "process"),
      SyscallResultIsOk(kExpectedContext));
  EXPECT_THAT(
      ComputeCreateContext("test_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0",
                           "test_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0 ", "process"),
      SyscallResultIsOk(kExpectedContext));

  // Fields are separated by spaces, with leading & trailing whitespace around each element ignored.
  EXPECT_THAT(
      ComputeCreateContext("\ttest_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0",
                           "test_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0", "process"),
      SyscallResultIsOk(kExpectedContext));
  EXPECT_THAT(
      ComputeCreateContext("test_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0\t",
                           "test_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0", "process"),
      SyscallResultIsOk(kExpectedContext));
  EXPECT_THAT(
      ComputeCreateContext("test_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0",
                           "test_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0\t", "process"),
      SyscallResultIsOk(kExpectedContext));
  EXPECT_THAT(
      ComputeCreateContext("\ntest_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0",
                           "test_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0\n", "process"),
      SyscallResultIsOk(kExpectedContext));

  // Although whitespace around elements is ignored, only spaces are valid separators.
  EXPECT_THAT(
      CallSeLinuxApi(
          "create",
          "test_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0\ttest_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0\tprocess"),
      SyscallResultIsErrno(EINVAL));
  EXPECT_THAT(
      CallSeLinuxApi(
          "create",
          "test_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0\ntest_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0\nprocess"),
      SyscallResultIsErrno(EINVAL));
}

// "create" API requests are appropriately validated.
TEST(SeLinuxFsCreate, ComputeCreateRequestValidation) {
  // Fewer than three arguments is an invalid request.
  EXPECT_THAT(CallSeLinuxApi("create", "test_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0"),
              SyscallResultIsErrno(EINVAL));
  EXPECT_THAT(
      CallSeLinuxApi(
          "create",
          "test_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0 test_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0"),
      SyscallResultIsErrno(EINVAL));

  // More than four arguments is an invalid request.
  EXPECT_THAT(
      CallSeLinuxApi(
          "create",
          "test_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0 test_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0 process test bad"),
      SyscallResultIsErrno(EINVAL));

  // If either security context is malformed, it is an invalid request.
  EXPECT_THAT(
      ComputeCreateContext("test_selinuxfs_non_existent_u:test_selinuxfs_r:test_selinuxfs_t:s0",
                           "test_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0", "process"),
      SyscallResultIsErrno(EINVAL));
  EXPECT_THAT(ComputeCreateContext(
                  "test_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0",
                  "test_selinuxfs_non_existent_u:test_selinuxfs_r:test_selinuxfs_t:s0", "process"),
              SyscallResultIsErrno(EINVAL));
}

// Validate "create" handling of class Id values not defined in the loaded policy.
// It appears that invalid class Ids may be "clamped" to the class Id range, resulting in their
// being labeled according to whether the first/last class in the policy is process/socket-like, or
// not.
TEST(SeLinuxFsCreate, InvalidComputeCreateClassId) {
  // Zero is not a valid class Id, but is apparently treated as process/socket-like.
  EXPECT_THAT(
      CallSeLinuxApiAndRemoveNul(
          "create",
          "test_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0 test_selinuxfs_create_target_u:test_selinuxfs_create_target_r:test_selinuxfs_create_target_t:s0 0"),
      SyscallResultIsOk("test_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0"));

  // 65535 is not a valid class Id in the test policy, but is apparently treated as
  // non-process/socket-like.
  EXPECT_THAT(
      CallSeLinuxApiAndRemoveNul(
          "create",
          "test_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0 test_selinuxfs_create_target_u:test_selinuxfs_create_target_r:test_selinuxfs_create_target_t:s0 65535"),
      SyscallResultIsOk("test_selinuxfs_u:object_r:test_selinuxfs_create_target_t:s0"));
}

TEST(SeLinuxFsAccess, OneRequestPerInstance) {
  fbl::unique_fd api_fd(open("/sys/fs/selinux/access", O_RDWR));
  ASSERT_TRUE(api_fd.is_valid()) << strerror(errno);

  const uint32_t kTargetClassId = GetClassId("test_selinuxfs_target_class");
  const auto kValidRequest = fxl::StringPrintf(
      "test_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0 test_selinuxfs_u:object_r:test_selinuxfs_access_myperm1234_target_t:s0 %u",
      kTargetClassId);
  ASSERT_EQ(write(api_fd.get(), kValidRequest.data(), kValidRequest.size()),
            (ssize_t)kValidRequest.size());

  EXPECT_EQ(write(api_fd.get(), kValidRequest.data(), kValidRequest.size()), -1);
  EXPECT_EQ(errno, EBUSY);
}

TEST(SeLinuxFsAccess, ReadUpdatesSeekPosition) {
  fbl::unique_fd api_fd(open("/sys/fs/selinux/access", O_RDWR));
  ASSERT_TRUE(api_fd.is_valid()) << strerror(errno);

  const uint32_t kTargetClassId = GetClassId("test_selinuxfs_target_class");
  const auto kValidRequest = fxl::StringPrintf(
      "test_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0 test_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0 %u",
      kTargetClassId);
  ASSERT_EQ(write(api_fd.get(), kValidRequest.data(), kValidRequest.size()),
            (ssize_t)kValidRequest.size());

  // Read the result back one byte at a time.
  std::string result;
  for (;;) {
    char buf;
    auto read_result = read(api_fd.get(), &buf, sizeof(buf));
    if (read_result <= 0) {
      if (read_result < 0) {
        ADD_FAILURE() << "read() failed: " << strerror(errno);
      }
      break;
    }
    result.push_back(buf);
  }

  constexpr char kExpected[] = "0 ffffffff 0 ffffffff 1 0";
  EXPECT_EQ(result, kExpected);
}

// Validate handling of whitespace between request elements.
TEST(SeLinuxFsAccess, ExtraWhitespace) {
  const auto kRequested = GetAccessVector("process", "fork");
  const auto kExpectedResult =
      ComputeAccess("test_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0",
                    "test_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0", "process", kRequested)
          .value();

  // Additional whitespace between elements is ignored.
  EXPECT_THAT(
      ComputeAccess(" test_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0",
                    "test_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0", "process", kRequested),
      SyscallResultIsOk(kExpectedResult));
  EXPECT_THAT(
      ComputeAccess("test_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0 ",
                    "test_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0", "process", kRequested),
      SyscallResultIsOk(kExpectedResult));
  EXPECT_THAT(ComputeAccess("test_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0",
                            "test_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0 ", "process",
                            kRequested),
              SyscallResultIsOk(kExpectedResult));

  // Fields are separated by spaces, with leading & trailing whitespace around each element ignored.
  EXPECT_THAT(
      ComputeAccess("\ttest_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0",
                    "test_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0", "process", kRequested),
      SyscallResultIsOk(kExpectedResult));
  EXPECT_THAT(
      ComputeAccess("test_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0\t",
                    "test_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0", "process", kRequested),
      SyscallResultIsOk(kExpectedResult));
  EXPECT_THAT(ComputeAccess("test_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0",
                            "test_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0\t", "process",
                            kRequested),
              SyscallResultIsOk(kExpectedResult));
  EXPECT_THAT(ComputeAccess("\ntest_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0",
                            "test_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0\n", "process",
                            kRequested),
              SyscallResultIsOk(kExpectedResult));

  // Although whitespace around elements is ignored, only spaces are valid separators.
  EXPECT_THAT(
      CallSeLinuxApi(
          "access",
          "test_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0\ttest_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0\tprocess\t1"),
      SyscallResultIsErrno(EINVAL));
  EXPECT_THAT(
      CallSeLinuxApi(
          "access",
          "test_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0\ntest_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0\nprocess\t1"),
      SyscallResultIsErrno(EINVAL));
}

// "access" API requests with too few arguments are invalid.
TEST(SeLinuxFsAccess, TooFewArgumentsIsInvalid) {
  // Fewer than three arguments is an invalid request.
  EXPECT_THAT(CallSeLinuxApi("access", "test_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0"),
              SyscallResultIsErrno(EINVAL));
  EXPECT_THAT(
      CallSeLinuxApi(
          "access",
          "test_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0 test_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0"),
      SyscallResultIsErrno(EINVAL));
}

// "access" API requests with three arguments are valid, even with trailing data.
TEST(SeLinuxFsAccess, ClassIdMayHaveTrailingNonNumericData) {
  const uint32_t kTargetClassId = GetClassId("test_selinuxfs_target_class");
  const auto kRequestBase = fxl::StringPrintf(
      "test_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0 test_selinuxfs_u:object_r:test_selinuxfs_access_myperm1234_target_t:s0 %u",
      kTargetClassId);
  const std::string_view kExpectedAccess = "f ffffffff 0 ffffffff 1 0";

  // Three arguments (just the `kRequestBase` with no "requested" set) is valid.
  ASSERT_THAT(CallSeLinuxApi("access", kRequestBase), SyscallResultIsOk(kExpectedAccess));

  // Trailing non-numeric characters in the target class argument appear to be ignored. This is
  // consistent with the way that `atoi()` handles trailing non-numeric characters.
  EXPECT_THAT(CallSeLinuxApi("access", kRequestBase + "third_arg_trailing_junk"),
              SyscallResultIsOk(kExpectedAccess));
}

// The class Id argument must be a valid decimal.
TEST(SeLinuxFsAccess, ClassIdMustBeDecimal) {
  // Non-numeric class Ids are rejected.
  EXPECT_THAT(
      CallSeLinuxApi(
          "access",
          "test_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0 test_selinuxfs_create_target_u:test_selinuxfs_create_target_r:test_selinuxfs_create_target_t:s0 bad"),
      SyscallResultIsErrno(EINVAL));
}

// "access" API requests with a fourth argument containing arbitrary data is accepted.
TEST(SeLinuxFsAccess, FourthArgumentIsNotValidated) {
  const uint32_t kTargetClassId = GetClassId("test_selinuxfs_target_class");
  const auto kRequestBase = fxl::StringPrintf(
      "test_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0 test_selinuxfs_u:object_r:test_selinuxfs_access_myperm1234_target_t:s0 %u",
      kTargetClassId);
  const std::string_view kExpectedAccess = "f ffffffff 0 ffffffff 1 0";

  // Four arguments (i.e. including the "requested" set) is valid, though the requested set has no
  // effect on the result.
  EXPECT_THAT(CallSeLinuxApi("access", kRequestBase + " 0"), SyscallResultIsOk(kExpectedAccess));
  EXPECT_THAT(CallSeLinuxApi("access", kRequestBase + " ffffffff"),
              SyscallResultIsOk(kExpectedAccess));
  EXPECT_THAT(CallSeLinuxApi("access", kRequestBase + " -1"), SyscallResultIsOk(kExpectedAccess));

  // There is no validation that the requested set argument is a valid hex value.
  EXPECT_THAT(CallSeLinuxApi("access", kRequestBase + " non_numeric"),
              SyscallResultIsOk(kExpectedAccess));
}

// If either security context is malformed, it is an invalid request.
TEST(SeLinuxFsAccess, SecurityContextsMustBeValid) {
  const auto kRequested = GetAccessVector("process", "fork");
  EXPECT_THAT(
      ComputeAccess("test_selinuxfs_non_existent_u:test_selinuxfs_r:test_selinuxfs_t:s0",
                    "test_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0", "process", kRequested),
      SyscallResultIsErrno(EINVAL));
  EXPECT_THAT(ComputeAccess("test_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0",
                            "test_selinuxfs_non_existent_u:test_selinuxfs_r:test_selinuxfs_t:s0",
                            "process", kRequested),
              SyscallResultIsErrno(EINVAL));
}

// Validate "access" handling of class Id values not defined in the loaded policy.
// These queries appear to fall-back on the "handle unknown" behaviour, which is set to "deny" in
// our test policy.
TEST(SeLinuxFsAccess, UnknownClassIdIsAccepted) {
  constexpr char kAllAccessDenied[] = "0 ffffffff 0 ffffffff 1 0";

  // Zero is not a valid class Id.
  EXPECT_THAT(
      CallSeLinuxApi(
          "access",
          "test_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0 test_selinuxfs_create_target_u:test_selinuxfs_create_target_r:test_selinuxfs_create_target_t:s0 0 1"),
      SyscallResultIsOk(kAllAccessDenied));

  // 65535 is not a valid class Id in the test policy.
  EXPECT_THAT(
      CallSeLinuxApi(
          "access",
          "test_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0 test_selinuxfs_create_target_u:test_selinuxfs_create_target_r:test_selinuxfs_create_target_t:s0 65535 1"),
      SyscallResultIsOk(kAllAccessDenied));
}

// Validate that a subject domain that is marked `permissive` in the policy is reported as having
// no permissions allowed, but the permissive bit set in the "flags" argument of the result.
TEST(SeLinuxFsAccess, ComputeAccessPermissiveSubject) {
  constexpr uint32_t kAllPerms = UINT32_MAX;

  EXPECT_THAT(
      ComputeAccess("test_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_access_permissive_t:s0",
                    "test_selinuxfs_u:object_r:test_selinuxfs_access_no_perms_target_t:s0",
                    "test_selinuxfs_target_class", kAllPerms),
      SyscallResultIsOk("0 ffffffff 0 ffffffff 1 1"));
}

// Validate that the allowed permissions are reported correctly in the result.
TEST(SeLinuxFsAccess, ComputeAccessPermissions) {
  constexpr uint32_t kAllPerms = UINT32_MAX;

  EXPECT_THAT(ComputeAccess("test_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0",
                            "test_selinuxfs_u:object_r:test_selinuxfs_access_no_perms_target_t:s0",
                            "test_selinuxfs_target_class", kAllPerms),
              SyscallResultIsOk("0 ffffffff 0 ffffffff 1 0"));
  EXPECT_THAT(ComputeAccess("test_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0",
                            "test_selinuxfs_u:object_r:test_selinuxfs_access_myperm1_target_t:s0",
                            "test_selinuxfs_target_class", kAllPerms),
              SyscallResultIsOk("1 ffffffff 0 ffffffff 1 0"));
  EXPECT_THAT(
      ComputeAccess("test_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0",
                    "test_selinuxfs_u:object_r:test_selinuxfs_access_myperm1234_target_t:s0",
                    "test_selinuxfs_target_class", kAllPerms),
      SyscallResultIsOk("f ffffffff 0 ffffffff 1 0"));
  EXPECT_THAT(ComputeAccess("test_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0",
                            "test_selinuxfs_u:object_r:test_selinuxfs_access_all_target_t:s0",
                            "test_selinuxfs_target_class", kAllPerms),
              SyscallResultIsOk("f ffffffff 0 ffffffff 1 0"));
}

// Validate that the `auditallow` and `dontaudit` statements are taken into account in the reported
// "audit allow" and "audit deny" sets.
TEST(SeLinuxFsAccess, ComputeAccessAudit) {
  constexpr uint32_t kAllPerms = UINT32_MAX;

  EXPECT_THAT(ComputeAccess("test_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0",
                            "test_selinuxfs_u:object_r:test_selinuxfs_access_audit_all_target_t:s0",
                            "test_selinuxfs_target_class", kAllPerms),
              SyscallResultIsOk("0 ffffffff f ffffffff 1 0"));
  EXPECT_THAT(
      ComputeAccess("test_selinuxfs_u:test_selinuxfs_r:test_selinuxfs_t:s0",
                    "test_selinuxfs_u:object_r:test_selinuxfs_access_audit_none_target_t:s0",
                    "test_selinuxfs_target_class", kAllPerms),
      SyscallResultIsOk("0 ffffffff 0 fffffff0 1 0"));
}

TEST(SeLinuxFsPolicy, ReadPolicyPermission) {
  auto enforcement = ScopedEnforcement::SetEnforcing();

  // Verify that a domain with read_policy permission (like unconfined_t) can read the policy.
  ASSERT_TRUE(RunSubprocessAs("system_u:unconfined_r:unconfined_t:s0", []() {
    fbl::unique_fd fd(open("/sys/fs/selinux/policy", O_RDONLY));
    ASSERT_THAT(fd.get(), SyscallSucceeds());

    char buf[1];
    EXPECT_THAT(read(fd.get(), buf, sizeof(buf)), SyscallSucceedsWithValue(1));
  }));

  // Verify that a domain without read_policy permission cannot read the policy.
  ASSERT_TRUE(RunSubprocessAs("system_u:unconfined_r:test_policy_read_denied_t:s0", []() {
    fbl::unique_fd fd(open("/sys/fs/selinux/policy", O_RDONLY));
    EXPECT_THAT(fd.get(), SyscallFailsWithErrno(EACCES));
  }));
}

}  // namespace
