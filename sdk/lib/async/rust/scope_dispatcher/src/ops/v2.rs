// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use libasync_sys::{async_dispatcher_t, async_irq_t, async_paged_vmo_t};
use zx::sys::{ZX_ERR_NOT_SUPPORTED, zx_handle_t, zx_status_t};

// ops_v2 dispatch functions
pub unsafe extern "C" fn bind_irq(
    _dispatcher_ptr: *mut async_dispatcher_t,
    _irq_ptr: *mut async_irq_t,
) -> zx_status_t {
    ZX_ERR_NOT_SUPPORTED
}
pub unsafe extern "C" fn unbind_irq(
    _dispatcher_ptr: *mut async_dispatcher_t,
    _irq_ptr: *mut async_irq_t,
) -> zx_status_t {
    ZX_ERR_NOT_SUPPORTED
}
pub unsafe extern "C" fn create_paged_vmo(
    _dispatcher_ptr: *mut async_dispatcher_t,
    _vmo_ptr: *mut async_paged_vmo_t,
    _opts: u32,
    _pager: zx_handle_t,
    _vmo_size: u64,
    _vmo_out: *mut zx_handle_t,
) -> zx_status_t {
    ZX_ERR_NOT_SUPPORTED
}
pub unsafe extern "C" fn detach_paged_vmo(
    _dispatcher_ptr: *mut async_dispatcher_t,
    _vmo_ptr: *mut async_paged_vmo_t,
) -> zx_status_t {
    ZX_ERR_NOT_SUPPORTED
}
