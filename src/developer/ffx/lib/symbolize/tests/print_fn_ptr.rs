// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![warn(unsafe_op_in_unsafe_fn)]

use symbolize_test_utils::SymbolizationTestOutputs;
use symbolize_test_utils::collector::collect_modules;

unsafe extern "C" {
    // Defined in no_symbol_area.c
    fn get_ptr_after_unnamed_bytes() -> *const ();
}

fn main() {
    let heap_data: Box<str> = "This goes into the heap".into();

    let outputs = SymbolizationTestOutputs {
        // These functions return their own line number to make test assertions less fragile.
        fn_one_source_line: to_be_symbolized_one(),
        fn_two_source_line: to_be_symbolized_two(),

        // Add to the addresses to get into the functions when symbolizing, otherwise the symbolizer
        // points to code from whatever other function immediately precedes this one in the object.
        fn_one_addr: to_be_symbolized_one as *const () as u64 + 1,
        fn_two_addr: to_be_symbolized_two as *const () as u64 + 1,
        fn_sys_inc_addr: zx::sys::zx_channel_create as *const () as u64 + 1,

        // Make sure we can resolve symbols from libraries too.
        libc_addr: libc::open as *const () as u64 + 1,

        // Make sure we return an error if outside of any ELF file region.
        heap_addr: heap_data.as_ptr() as u64 + 1,

        // Make sure we return an error if in a ELF region that is not covered by any symbol.
        // SAFETY: the called function simply returns a constant pointer value.
        no_symbol_addr: unsafe { get_ptr_after_unnamed_bytes() } as u64 - 1,

        modules: collect_modules(),
    };
    println!("{}", serde_json::to_string_pretty(&outputs).unwrap());
}

// ICF shouldn't deduplicate these because they'll have different locations statically defined.
#[inline(never)]
fn to_be_symbolized_one() -> u32 {
    // Include a known-good backtrace in the system logs of the test for debugging.
    eprintln!("{}", std::backtrace::Backtrace::force_capture());
    std::panic::Location::caller().line()
}
#[inline(never)]
fn to_be_symbolized_two() -> u32 {
    eprintln!("{}", std::backtrace::Backtrace::force_capture());
    std::panic::Location::caller().line()
}
