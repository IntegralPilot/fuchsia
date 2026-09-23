// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef BUILD_BAZEL_RULES_HOST_TESTS_TESTS_CC_FOO_H_
#define BUILD_BAZEL_RULES_HOST_TESTS_TESTS_CC_FOO_H_

// The attribute is required to ensure the foo() symbol is
// visibly when the library is linked into a shared library.
extern int foo();
__attribute__((visibility("default")))

#endif  // BUILD_BAZEL_RULES_HOST_TESTS_TESTS_CC_FOO_H_
