// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use super::handle::KernelHandle;
use super::vm_address_region_dispatcher::VmAddressRegionDispatcher;
use crate::vm::vm_object::VmObject;
use core::mem::MaybeUninit;
use zx_types::{zx_rights_t, zx_status_t, zx_vaddr_t};

unsafe extern "C" {
    /// Calls into C++ implementation to set the memory priority of a VMAR.
    ///
    /// # Safety
    ///
    /// `vmar` must point to a valid `VmAddressRegionDispatcher`.
    /// `priority` is the memory priority to set.
    pub fn cpp_vmar_dispatcher_set_memory_priority(
        vmar: &VmAddressRegionDispatcher,
        priority: u32,
    ) -> zx_status_t;

    /// Calls into C++ implementation to allocate a sub-VMAR.
    ///
    /// # Safety
    ///
    /// `vmar` must point to a valid `VmAddressRegionDispatcher`.
    /// `handle_out` and `rights_out` must point to valid writable stack memory.
    pub fn cpp_vmar_dispatcher_allocate(
        vmar: &VmAddressRegionDispatcher,
        offset: usize,
        size: usize,
        flags: u32,
        handle_out: *mut MaybeUninit<KernelHandle<VmAddressRegionDispatcher>>,
        rights_out: *mut MaybeUninit<zx_rights_t>,
    ) -> zx_status_t;

    /// Calls into C++ implementation to map a VMO into a VMAR.
    ///
    /// # Safety
    ///
    /// `vmar` must point to a valid `VmAddressRegionDispatcher`.
    /// `vmo` must point to a valid `VmObject`.
    /// `out_base` must point to a valid writable `zx_vaddr_t`.
    pub fn cpp_vmar_dispatcher_map(
        vmar: &VmAddressRegionDispatcher,
        vmar_offset: usize,
        vmo: &VmObject,
        vmo_offset: u64,
        len: usize,
        flags: u32,
        out_base: *mut zx_vaddr_t,
    ) -> zx_status_t;

    /// Calls into C++ implementation to get VMAR info.
    ///
    /// # Safety
    ///
    /// `vmar` must point to a valid `VmAddressRegionDispatcher`.
    pub fn cpp_vmar_dispatcher_get_vmar_info(
        vmar: *const VmAddressRegionDispatcher,
    ) -> zx_types::zx_info_vmar_t;
}
