// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#[fuchsia::test]
fn empty_test() {
    #[cfg(target_os = "fuchsia")]
    src_lib_fuchsia_testing::assert_logger_registered!();

    #[cfg(not(target_os = "fuchsia"))]
    src_lib_fuchsia_testing::assert_no_logger_registered!();
}
