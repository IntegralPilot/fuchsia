// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#ifndef BUILD_BAZEL_BAZEL_IDK_TESTS_INCLUDE_LIB_TEST_LIB_INTERNAL_INTERNAL_HEADER_APPEARS_IN_API_FILE_H_
#define BUILD_BAZEL_BAZEL_IDK_TESTS_INCLUDE_LIB_TEST_LIB_INTERNAL_INTERNAL_HEADER_APPEARS_IN_API_FILE_H_

#ifdef __Fuchsia__
#include <lib/test_lib/internal/fuchsia_only_internal_header_appears_in_api_file.h>
#else
#include <lib/test_lib/internal/non_fuchsia_internal_header_does_not_appear_in_api_file.h>
#endif

#endif  // BUILD_BAZEL_BAZEL_IDK_TESTS_INCLUDE_LIB_TEST_LIB_INTERNAL_INTERNAL_HEADER_APPEARS_IN_API_FILE_H_
