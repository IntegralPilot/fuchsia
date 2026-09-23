// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#ifndef BUILD_BAZEL_BAZEL_IDK_TESTS_INCLUDE_LIB_TEST_LIB_FUCHSIA_ONLY_HEADER_H_
#define BUILD_BAZEL_BAZEL_IDK_TESTS_INCLUDE_LIB_TEST_LIB_FUCHSIA_ONLY_HEADER_H_

#ifndef __Fuchsia__
#error Fuchsia-only header should not be included in non-Fuchsia code.
#endif

int FuchsiaSpecificFunction();

#endif  // BUILD_BAZEL_BAZEL_IDK_TESTS_INCLUDE_LIB_TEST_LIB_FUCHSIA_ONLY_HEADER_H_
