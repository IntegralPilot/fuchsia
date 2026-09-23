// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#ifndef BUILD_BAZEL_BAZEL_IDK_TESTS_INCLUDE_LIB_TEST_LIB_PLATFORM_DEPENDENT_HEADER_H_
#define BUILD_BAZEL_BAZEL_IDK_TESTS_INCLUDE_LIB_TEST_LIB_PLATFORM_DEPENDENT_HEADER_H_

#include <lib/test_lib/internal/internal_header_appears_in_api_file.h>

int PlatformSpecificFunction();

int GetPlatformSpecificValue() { return internal_platform_value; }

#endif  // BUILD_BAZEL_BAZEL_IDK_TESTS_INCLUDE_LIB_TEST_LIB_PLATFORM_DEPENDENT_HEADER_H_