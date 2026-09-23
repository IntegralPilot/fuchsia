// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// This include is intentionally relative to
// test_include_dir/, which should be handled by the build_flags()
// `include_dirs` attribute value in BUILD.bazel.
#include "test_header.h"

#if !defined(INCLUDE_DIRS_WORKS) || INCLUDE_DIRS_WORKS != 1
#error "include_dirs attribute not correctly passed!"
#endif

// Declare the external function from the test static library.
extern "C" int prebuilt_test_func(void);

int main() {
  // Call dummy_func to verify that libtest_dummy was found and linked correctly.
  if (prebuilt_test_func() != 42) {
    return 1;
  }
  return 0;
}
