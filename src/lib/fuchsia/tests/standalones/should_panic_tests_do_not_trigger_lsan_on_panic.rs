// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#[fuchsia::test(logging = false)]
#[should_panic]
fn should_panic_tests_do_not_trigger_lsan_on_panic() {
    src_lib_fuchsia_testing::assert_no_logger_registered!();
    let _v = vec![1, 2, 3];
    // Note that with panic=abort we will not unwind and free the vec.
    panic!()
}
