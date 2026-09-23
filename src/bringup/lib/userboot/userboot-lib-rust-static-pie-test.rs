// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::fmt::Write as _;
use userboot::{Log, take_system_handles};

fn main() {
    let handles = take_system_handles().expect("Cannot get system handles");
    let count = handles.len();
    let success_str = env!("BOOT_TEST_SUCCESS_STRING");
    let _ = write!(
        Log::new(),
        "Hello from Rust userland! Got {count} system capability handles! {success_str}"
    );
}
