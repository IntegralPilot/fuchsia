// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#ifndef BUILD_BAZEL_BAZEL_IDK_TESTS_INCLUDE_LIB_TEST_LIB_INTERNAL_FUCHSIA_ONLY_INTERNAL_HEADER_APPEARS_IN_API_FILE_H_
#define BUILD_BAZEL_BAZEL_IDK_TESTS_INCLUDE_LIB_TEST_LIB_INTERNAL_FUCHSIA_ONLY_INTERNAL_HEADER_APPEARS_IN_API_FILE_H_

#ifndef __Fuchsia__
#error Fuchsia-only header should not be included in non-Fuchsia code.
#endif

const int internal_platform_value = 0xFF00FF;

#endif  // BUILD_BAZEL_BAZEL_IDK_TESTS_INCLUDE_LIB_TEST_LIB_INTERNAL_FUCHSIA_ONLY_INTERNAL_HEADER_APPEARS_IN_API_FILE_H_
