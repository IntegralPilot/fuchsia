// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![no_std]

// This crate has tests of the Rust build support and fundamental features of
// the compiler and cross-language linkage support.  It is not a place to put
// tests for other kernel code that happens to be in Rust.

use test_macro::plus_one;

#[unsafe(no_mangle)]
pub static kConstVarDefinedInRust: i32 = 17;

#[unsafe(no_mangle)]
pub static mut gVarDefinedInRust: i32 = 42;

unsafe extern "C" {
    static kConstVarExportedToRust: i32;
    static mut gVarExportedToRust: i32;
} // extern "C"

#[unsafe(no_mangle)]
pub extern "C" fn add_one_in_rust(x: i32) -> i32 {
    plus_one!(x)
}

#[unsafe(no_mangle)]
pub extern "C" fn get_const_var_exported_to_rust() -> i32 {
    unsafe { kConstVarExportedToRust }
}

#[unsafe(no_mangle)]
pub extern "C" fn fetch_add_var_exported_to_rust(x: i32) -> i32 {
    unsafe {
        let old = gVarExportedToRust;
        gVarExportedToRust += x;
        old
    }
}
