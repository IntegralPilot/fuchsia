// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use crate::object::{Dispatcher, HandleValue, IoBufferDispatcher, VmAddressRegionDispatcher};
use crate::user_copy::UserOutPtr;
use crate::vm::vm_object::VmObject;
use fbl::RefPtr;
use syscalls_macro::syscall;
use zx_status::Status;
use zx_types::{zx_rights_t, zx_status_t, zx_vaddr_t, zx_vm_option_t};

unsafe extern "C" {
    fn cpp_vmar_map_common(
        options: zx_vm_option_t,
        vmar: *mut VmAddressRegionDispatcher,
        vmar_offset: u64,
        vmar_rights: zx_rights_t,
        vmo: *mut VmObject,
        vmo_offset: u64,
        vmo_rights: zx_rights_t,
        len: u64,
        mapped_addr: *mut zx_vaddr_t,
    ) -> zx_status_t;
}

#[syscall]
pub fn sys_vmar_map_iob(
    handle: HandleValue,
    options: zx_vm_option_t,
    vmar_offset: usize,
    ep: HandleValue,
    region_index: u32,
    region_offset: u64,
    region_length: usize,
    mapped_addr: UserOutPtr<zx_vaddr_t>,
) -> Result<(), Status> {
    let (vmar, vmar_rights) = Dispatcher::get_and_rights::<VmAddressRegionDispatcher>(handle)?;
    let (iob, iob_rights) = Dispatcher::get_and_rights::<IoBufferDispatcher>(ep)?;

    if region_index as usize >= iob.region_count() {
        return Err(Status::OUT_OF_RANGE);
    }

    let vmo = iob.create_mappable_vmo_for_region(region_index as usize)?;
    let region_rights = iob.get_map_rights(iob_rights, region_index as usize);

    // SAFETY: We transfer ownership of the `RefPtr` references into `cpp_vmar_map_common`
    // via `RefPtr::into_raw`, which `ImportFromRawPtr` reclaims on the C++ side.
    let status = unsafe {
        cpp_vmar_map_common(
            options,
            RefPtr::into_raw(vmar).cast_mut(),
            vmar_offset as u64,
            vmar_rights,
            RefPtr::into_raw(vmo).cast_mut(),
            region_offset,
            region_rights,
            region_length as u64,
            mapped_addr.as_ptr(),
        )
    };
    Status::ok(status)?;
    Ok(())
}
