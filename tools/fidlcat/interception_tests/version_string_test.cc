// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <gtest/gtest.h>

#include "tools/fidlcat/interception_tests/interception_workflow_test.h"

namespace fidlcat {

// zx_system_get_version_string tests.

std::unique_ptr<SystemCallTest> ZxGetVersionString(const char* result,
                                                   std::string_view result_name) {
  result128_t res = {.first_word = reinterpret_cast<int64_t>(result),
                     .second_word = strlen(result)};
  return std::make_unique<SystemCallTest>("zx_system_get_version_string", res, result_name);
}

#define SYSTEM_GET_VERSION_STRING_DISPLAY_TEST_CONTENT(result, expected)                        \
  PerformDisplayTest("$plt(zx_system_get_version_string)", ZxGetVersionString(result, #result), \
                     expected)

// The characters at the end have different UTF-8 encoding lengths.
#define TEST_STRING "abcdefghijklmnopqrstuvwxyzéū娉"

#define SYSTEM_GET_VERSION_STRING_DISPLAY_TEST(name, errno, expected)      \
  TEST_F(InterceptionWorkflowTestX64, name) {                              \
    SYSTEM_GET_VERSION_STRING_DISPLAY_TEST_CONTENT(TEST_STRING, expected); \
  }                                                                        \
  TEST_F(InterceptionWorkflowTestArm, name) {                              \
    SYSTEM_GET_VERSION_STRING_DISPLAY_TEST_CONTENT(TEST_STRING, expected); \
  }

SYSTEM_GET_VERSION_STRING_DISPLAY_TEST(
    ZxSystemGetVersionString, ZX_OK,
    "\n"
    "\x1B[32m0.000000\x1B[0m "
    "test_3141 \x1B[31m3141\x1B[0m:\x1B[31m8764\x1B[0m zx_system_get_version_string()\n"
    "\x1B[32m0.000000\x1B[0m "
    "  -> \x1B[34m" TEST_STRING "\x1B[0m\n")

}  // namespace fidlcat
