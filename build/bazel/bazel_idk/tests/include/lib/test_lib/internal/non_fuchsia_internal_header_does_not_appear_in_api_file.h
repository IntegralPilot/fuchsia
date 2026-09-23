// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#ifndef BUILD_BAZEL_BAZEL_IDK_TESTS_INCLUDE_LIB_TEST_LIB_INTERNAL_NON_FUCHSIA_INTERNAL_HEADER_DOES_NOT_APPEAR_IN_API_FILE_H_
#define BUILD_BAZEL_BAZEL_IDK_TESTS_INCLUDE_LIB_TEST_LIB_INTERNAL_NON_FUCHSIA_INTERNAL_HEADER_DOES_NOT_APPEAR_IN_API_FILE_H_

#ifdef __Fuchsia__
#error Non-Fuchsia header should not be included in Fuchsia code.
#endif

const int internal_platform_value = 0;

#endif  // BUILD_BAZEL_BAZEL_IDK_TESTS_INCLUDE_LIB_TEST_LIB_INTERNAL_NON_FUCHSIA_INTERNAL_HEADER_DOES_NOT_APPEAR_IN_API_FILE_H_
