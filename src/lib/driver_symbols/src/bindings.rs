// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![allow(dead_code)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]

pub use zx_types::*;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct restricted_symbols {
    _unused: [u8; 0],
}
pub type restricted_symbols_t = restricted_symbols;
unsafe extern "C" {
    pub fn restricted_symbols_find(
        driver_vmo: zx_handle_t,
        driver_url: *const ::core::ffi::c_char,
        out_symbols: *mut *mut restricted_symbols_t,
        out_symbols_found: *mut usize,
    ) -> zx_status_t;
}
unsafe extern "C" {
    pub fn restricted_symbols_get(
        symbols: *mut restricted_symbols_t,
        index: usize,
    ) -> *const ::core::ffi::c_char;
}
unsafe extern "C" {
    pub fn restricted_symbols_free(symbols: *mut restricted_symbols_t);
}
