// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#[fuchsia::test(logging = false)]
fn empty_test_without_logging() {
    src_lib_fuchsia_testing::assert_no_logger_registered!();
}
