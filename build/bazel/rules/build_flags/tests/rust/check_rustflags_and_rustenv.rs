// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Verify that the '--cfg=rustflags_work' rustflag is passed correctly.
#[cfg(not(rustflags_work))]
compile_error!("rustflags attribute not correctly passed!");

// Verify that the environment variable RUSTENV_TEST_VAR is set correctly.
const RUSTENV_VAL: &str = env!("RUSTENV_TEST_VAR");

fn main() {
    // Assert the expected value at runtime to be absolutely certain.
    assert_eq!(RUSTENV_VAL, "hello_world");
}
