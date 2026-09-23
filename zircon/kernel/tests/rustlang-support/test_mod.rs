// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub mod test_dep_pub_mod;
#[path = "test_dep_was_renamed_mod.rs"]
mod test_dep_renamed_mod;
mod test_submod;

use test_submod::*;

#[unsafe(no_mangle)]
pub extern "C" fn rust_mod_fn() -> i32 {
    test_fn()
}
