// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use super::handle::KernelHandle;
use super::vm_object_dispatcher::{InitialMutability, VmObjectDispatcher};
use crate::vm::vm_object::VmObject;
use core::mem::MaybeUninit;
use fbl::RefPtr;
use zx_types::{zx_rights_t, zx_status_t};

unsafe extern "C" {
    pub(crate) fn cpp_vm_object_dispatcher_get_vmo(
        disp: *const VmObjectDispatcher,
    ) -> *const RefPtr<VmObject>;
    pub(crate) fn cpp_vm_object_dispatcher_create(
        raw_vmo: *mut VmObject,
        stream_size: u64,
        initial_mutability: InitialMutability,
        out_handle: *mut MaybeUninit<KernelHandle<VmObjectDispatcher>>,
        out_rights: *mut MaybeUninit<zx_rights_t>,
    ) -> zx_status_t;
    pub(crate) fn cpp_vm_object_dispatcher_get_vmo_info(
        vmo: *mut VmObjectDispatcher,
        rights: zx_types::zx_rights_t,
    ) -> zx_types::zx_info_vmo_t;
    pub(crate) fn cpp_vm_object_dispatcher_read(
        disp: *mut VmObjectDispatcher,
        user_data: *mut u8,
        offset: u64,
        length: usize,
    ) -> zx_status_t;
    pub(crate) fn cpp_vm_object_dispatcher_write(
        disp: *mut VmObjectDispatcher,
        user_data: *const u8,
        offset: u64,
        length: usize,
    ) -> zx_status_t;
    pub(crate) fn cpp_vm_object_dispatcher_get_size(
        disp: *mut VmObjectDispatcher,
        size: *mut u64,
    ) -> zx_status_t;
    pub(crate) fn cpp_vm_object_dispatcher_get_stream_size(disp: *const VmObjectDispatcher) -> u64;
    pub(crate) fn cpp_vm_object_dispatcher_set_size(
        disp: *mut VmObjectDispatcher,
        size: u64,
    ) -> zx_status_t;
    pub(crate) fn cpp_vm_object_dispatcher_set_stream_size(
        disp: *mut VmObjectDispatcher,
        size: u64,
    ) -> zx_status_t;
    pub(crate) fn cpp_vm_object_dispatcher_range_op(
        disp: *mut VmObjectDispatcher,
        op: u32,
        offset: u64,
        size: u64,
        buffer: *mut u8,
        buffer_size: usize,
        rights: zx_rights_t,
    ) -> zx_status_t;
    pub(crate) fn cpp_vm_object_dispatcher_set_mapping_cache_policy(
        disp: *mut VmObjectDispatcher,
        cache_policy: u32,
    ) -> zx_status_t;
    pub(crate) fn cpp_vm_object_dispatcher_create_child(
        disp: *mut VmObjectDispatcher,
        options: u32,
        offset: u64,
        size: u64,
        copy_name: bool,
        out_child_vmo: *mut MaybeUninit<RefPtr<VmObject>>,
    ) -> zx_status_t;
    pub(crate) fn cpp_vm_object_dispatcher_create_with_parent_stream_size(
        parent_disp: *mut VmObjectDispatcher,
        raw_child_vmo: *mut VmObject,
        initial_mutability: InitialMutability,
        out_handle: *mut MaybeUninit<KernelHandle<VmObjectDispatcher>>,
        out_rights: *mut MaybeUninit<zx_rights_t>,
    ) -> zx_status_t;
}

use crate::vm::stream_size_manager::StreamSizeManager;

/// Sets the size of a `VmObjectDispatcher` from C++.
///
/// # Safety
///
/// `disp` and `stream_size_manager` must be non-null and point to valid objects.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rust_vm_object_dispatcher_set_size(
    disp: &VmObjectDispatcher,
    stream_size_manager: &StreamSizeManager,
    size: u64,
) -> zx_types::zx_status_t {
    match disp.set_size_with_ssm(stream_size_manager, size) {
        Ok(()) => zx_types::ZX_OK,
        Err(status) => status.into_raw(),
    }
}

/// Sets the stream size of a `VmObjectDispatcher` from C++.
///
/// # Safety
///
/// `disp` and `stream_size_manager` must be non-null and point to valid objects.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rust_vm_object_dispatcher_set_stream_size(
    disp: &VmObjectDispatcher,
    stream_size_manager: &StreamSizeManager,
    stream_size: u64,
) -> zx_types::zx_status_t {
    match disp.set_stream_size_with_ssm(stream_size_manager, stream_size) {
        Ok(()) => zx_types::ZX_OK,
        Err(status) => status.into_raw(),
    }
}
