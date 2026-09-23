// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::ffi::c_char;

unsafe extern "C" {
    fn cpp_platform_dgetc(c: *mut c_char, wait: bool) -> i32;
    fn cpp_platform_dputs_thread(str: *const c_char, len: usize);
    fn cpp_platform_pputc(c: c_char);
    fn cpp_platform_pgetc(c: *mut c_char) -> i32;
}

/// Call the platform console input hook.
pub fn platform_dgetc(c: &mut c_char, wait: bool) -> i32 {
    // SAFETY: The mutable reference `c` guarantees that the pointer is valid, non-null, and properly aligned.
    unsafe { cpp_platform_dgetc(c as *mut c_char, wait) }
}

/// Call the platform console output hook.
pub fn platform_dputs_thread(s: &[u8]) {
    // SAFETY: The slice `s` guarantees that the pointer is valid for `s.len()` bytes.
    unsafe { cpp_platform_dputs_thread(s.as_ptr() as *const c_char, s.len()) }
}

/// Write a single character to the platform's panic print port.
pub fn platform_pputc(c: u8) {
    // SAFETY: No pointer is passed, so this is inherently safe.
    unsafe { cpp_platform_pputc(c as c_char) };
}

/// Read a single character from the platform's panic read port.
pub fn platform_pgetc(c: &mut c_char) -> i32 {
    // SAFETY: The mutable reference `c` guarantees that the pointer is valid, non-null, and properly aligned.
    unsafe { cpp_platform_pgetc(c as *mut c_char) }
}
