// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

fn main() {
    // Call library function
    assert_eq!(check_rustc_library_lib::get_env_val(), "hello_world");

    // Call proc macro
    let macro_str = check_rustc_proc_macro_lib::make_string!();
    assert_eq!(macro_str, "hello_world");
}
