// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#[fuchsia::test(add_test_attr = true)]
fn empty_test_with_add_test_attr() {
    #[cfg(target_os = "fuchsia")]
    src_lib_fuchsia_testing::assert_logger_registered!();
    #[cfg(not(target_os = "fuchsia"))]
    src_lib_fuchsia_testing::assert_no_logger_registered!();
}
