// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#[fuchsia::test(logging_minimum_severity = "warn")]
async fn empty_test_with_minimum_severity() {
    src_lib_fuchsia_testing::assert_logger_registered!();
}
