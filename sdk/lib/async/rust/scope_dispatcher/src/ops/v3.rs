// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::ffi::c_char;

use libasync_sys::{async_dispatcher_t, async_sequence_id};
use zx::sys::{ZX_ERR_NOT_SUPPORTED, zx_status_t};

// ops_v3 dispatch functions
pub unsafe extern "C" fn get_sequence_id(
    _dispatcher_ptr: *mut async_dispatcher_t,
    _sequence_id: *mut async_sequence_id,
    err_out: *mut *const c_char,
) -> zx_status_t {
    if !err_out.is_null() {
        // SAFETY: Caller provided a pointer which means they expect us to fill it in so it will
        // point to valid memory.
        unsafe { *err_out = b"not supported".as_ptr() as *const c_char };
    }
    ZX_ERR_NOT_SUPPORTED
}
pub unsafe extern "C" fn check_sequence_id(
    _dispatcher_ptr: *mut async_dispatcher_t,
    _sequence_id: async_sequence_id,
    err_out: *mut *const c_char,
) -> zx_status_t {
    if !err_out.is_null() {
        // SAFETY: Caller provided a pointer which means they expect us to fill it in so it will
        // point to valid memory.
        unsafe { *err_out = b"not supported".as_ptr() as *const c_char };
    }
    ZX_ERR_NOT_SUPPORTED
}
