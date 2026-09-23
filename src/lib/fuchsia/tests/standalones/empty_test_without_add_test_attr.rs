// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#[allow(unused)]
use src_lib_fuchsia_testing as _src_lib_fuchsia_testing;

#[allow(dead_code)]
#[fuchsia::test(add_test_attr = false)]
fn empty_test_without_add_test_attr() {
    // With `add_test_attr = false`, this function won't get a #[test] annotation, and therefore
    // is expected to _not_ run during tests.
    assert!(false)
}
