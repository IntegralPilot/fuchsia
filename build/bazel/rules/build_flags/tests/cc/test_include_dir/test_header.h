// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef BUILD_BAZEL_RULES_BUILD_FLAGS_TESTS_CC_TEST_INCLUDE_DIR_TEST_HEADER_H_
#define BUILD_BAZEL_RULES_BUILD_FLAGS_TESTS_CC_TEST_INCLUDE_DIR_TEST_HEADER_H_

// This header should be included simply as "test_header.h" in the
// test source file to verify that the 'include_dirs' build_flags()
// attribute is processed properly.
#define INCLUDE_DIRS_WORKS 1

#endif  // BUILD_BAZEL_RULES_BUILD_FLAGS_TESTS_CC_TEST_INCLUDE_DIR_TEST_HEADER_H_
