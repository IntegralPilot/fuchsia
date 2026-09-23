// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use test_macro::plus_one;

pub fn test_fn() -> i32 {
    plus_one!(crate::test_mod::test_dep_pub_mod::TEST_DEP_CONST)
}
