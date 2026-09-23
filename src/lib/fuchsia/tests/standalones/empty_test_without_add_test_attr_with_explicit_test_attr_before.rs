// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#[test]
#[fuchsia::test(add_test_attr = false)]
fn empty_test_without_add_test_attr_with_explicit_test_attr_before() {
    #[cfg(target_os = "fuchsia")]
    src_lib_fuchsia_testing::assert_logger_registered!();
    #[cfg(not(target_os = "fuchsia"))]
    src_lib_fuchsia_testing::assert_no_logger_registered!();
}
