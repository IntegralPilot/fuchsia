// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use crate::object::{HandleValue, IommuDispatcher, validate_resource_kind_base};
use crate::user_copy::UserInPtr;
use debug::ltracef;
use syscalls_macro::syscall;
use zx_status::Status;
use zx_types::{ZX_IOMMU_MAX_DESC_LEN, ZX_RSRC_KIND_SYSTEM, ZX_RSRC_SYSTEM_IOMMU_BASE};

const LOCAL_TRACE: u32 = 0;

#[syscall]
pub fn sys_iommu_create(
    resource: HandleValue,
    type_param: u32,
    desc: UserInPtr<u8>,
    desc_size: usize,
    out: &mut HandleValue,
) -> Result<(), Status> {
    ltracef!(
        "resource {:#x}, type {}, desc_size {}\n",
        resource.raw_value(),
        type_param,
        desc_size
    );

    validate_resource_kind_base(resource, ZX_RSRC_KIND_SYSTEM, ZX_RSRC_SYSTEM_IOMMU_BASE)?;

    if desc_size > ZX_IOMMU_MAX_DESC_LEN {
        return Err(Status::INVALID_ARGS);
    }

    // Copy the descriptor into the kernel and try to create the dispatcher
    // using it.
    let mut uninit_buf =
        fbl::Array::<u8>::try_new_uninit_slice(desc_size).map_err(|_| Status::NO_MEMORY)?;
    let _ = desc.copy_slice_from_user(&mut uninit_buf[..])?;
    // SAFETY: `uninit_buf` was successfully initialized with bytes from user space.
    let init_buf = unsafe { uninit_buf.assume_init() };

    let (kernel_handle, rights) = IommuDispatcher::create(type_param, init_buf)?;

    *out = kernel_handle.make_and_add_handle(rights)?;
    Ok(())
}
