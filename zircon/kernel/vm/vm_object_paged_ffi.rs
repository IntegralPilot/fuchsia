// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use crate::user_copy::{UserInPtr, UserOutPtr};
use vm_object_paged_bindings as bindings;
use zx_types::zx_iovec_t;

// C++ FFI declarations
#[allow(improper_ctypes)]
unsafe extern "C" {
    pub(crate) fn cpp_vm_object_paged_read_user_vector(
        vmo: *mut bindings::VmObjectPaged,
        vector: UserOutPtr<zx_iovec_t>,
        count: usize,
        offset: u64,
        length: usize,
        out_actual: *mut usize,
    ) -> zx_types::zx_status_t;
    pub(crate) fn cpp_vm_object_paged_write_user_vector(
        vmo: *mut bindings::VmObjectPaged,
        vector: UserInPtr<zx_iovec_t>,
        count: usize,
        offset: u64,
        length: usize,
        out_actual: *mut usize,
    ) -> zx_types::zx_status_t;
    pub(crate) fn cpp_vm_object_paged_write_user_vector_progress(
        vmo: *mut bindings::VmObjectPaged,
        vector: UserInPtr<zx_iovec_t>,
        count: usize,
        offset: u64,
        length: usize,
        prev_stream_size: u64,
        out_actual: *mut usize,
        cb: extern "C" fn(*mut core::ffi::c_void, u64, usize),
        cookie: *mut core::ffi::c_void,
    ) -> zx_types::zx_status_t;
    pub(crate) fn cpp_vm_object_paged_zero_range(
        vmo: *mut bindings::VmObjectPaged,
        offset: u64,
        length: u64,
    ) -> zx_types::zx_status_t;
    pub(crate) fn cpp_vm_object_paged_zero_range_untracked(
        vmo: *mut bindings::VmObjectPaged,
        offset: u64,
        length: u64,
    ) -> zx_types::zx_status_t;
    pub(crate) fn cpp_vm_object_paged_resize(
        vmo: *mut bindings::VmObjectPaged,
        size: u64,
    ) -> zx_types::zx_status_t;
    pub(crate) fn cpp_vm_object_paged_unmap_and_call(
        vmo: *mut bindings::VmObjectPaged,
        offset: u64,
        length: u64,
        call_fn: Option<unsafe extern "C" fn(*mut core::ffi::c_void)>,
        call_ctx: *mut core::ffi::c_void,
    );
    pub(crate) fn cpp_vm_object_paged_set_user_stream_size(
        vmo: *mut bindings::VmObjectPaged,
        ssm: *mut core::ffi::c_void,
    );
    pub(crate) fn cpp_vm_object_paged_user_stream_size(
        vmo: *mut bindings::VmObjectPaged,
        out_stream_size: *mut u64,
    ) -> bool;
}
