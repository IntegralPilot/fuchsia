// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#![no_std]

pub const BOOT_TEST_SUCCESS_STRING: &core::ffi::CStr = unsafe {
    core::ffi::CStr::from_bytes_with_nul_unchecked(
        core::concat!(core::env!("BOOT_TEST_SUCCESS_STRING"), "\0").as_bytes(),
    )
};
