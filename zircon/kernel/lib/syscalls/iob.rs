// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use crate::object::{
    Dispatcher, HandleValue, IoBufferDispatcher, IoBufferSharedRegionDispatcher, ProcessDispatcher,
};
use crate::user_copy::{UserInPtr, UserOutPtr};
use debug::ltracef;
use kalloc::Box;
use syscalls_macro::syscall;
use zx_status::Status;
use zx_types::{
    ZX_IOB_MAX_REGIONS, ZX_POL_NEW_IOB, ZX_RIGHT_WRITE, zx_iob_allocate_id_options_t,
    zx_iob_region_t, zx_iob_write_options_t, zx_iovec_t,
};

const LOCAL_TRACE: u32 = 0;

#[syscall]
pub fn sys_iob_create(
    options: u64,
    regions: UserInPtr<u8>,
    num_regions: usize,
    ep0_out: &mut HandleValue,
    ep1_out: &mut HandleValue,
) -> Result<(), Status> {
    ltracef!("options {options:#x}, num_regions {num_regions}\n");

    if options != 0 {
        return Err(Status::INVALID_ARGS);
    }

    ProcessDispatcher::with_current(|up| up.enforce_basic_policy(ZX_POL_NEW_IOB))?;

    if num_regions > ZX_IOB_MAX_REGIONS {
        return Err(Status::OUT_OF_RANGE);
    }

    let mut uninit_regions = Box::<[zx_iob_region_t]>::try_new_uninit_slice(num_regions)
        .map_err(|_| Status::NO_MEMORY)?;
    let copied_regions =
        regions.reinterpret::<zx_iob_region_t>().copy_slice_from_user(&mut uninit_regions)?;

    let (handle0, handle1, rights) = IoBufferDispatcher::create(options, copied_regions)?;

    let user_handle0 = handle0.make_and_add_handle(rights)?;
    let user_handle1 = handle1.make_and_add_handle(rights)?;
    *ep0_out = user_handle0;
    *ep1_out = user_handle1;
    Ok(())
}

#[syscall]
pub fn sys_iob_allocate_id(
    handle: HandleValue,
    options: zx_iob_allocate_id_options_t,
    region_index: u32,
    blob: UserInPtr<u8>,
    blob_size: usize,
    id: UserOutPtr<u32>,
) -> Result<(), Status> {
    ltracef!(
        "handle {handle:?}, options {options:#x}, region_index {region_index}, blob_size {blob_size}\n"
    );

    if options != 0 {
        return Err(Status::INVALID_ARGS);
    }

    let iob = Dispatcher::get_with_rights::<IoBufferDispatcher>(handle, ZX_RIGHT_WRITE)?;
    let allocated_id = iob.allocate_id(region_index, blob, blob_size)?;
    id.write(allocated_id)?;
    Ok(())
}

#[syscall]
pub fn sys_iob_writev(
    handle: HandleValue,
    options: zx_iob_write_options_t,
    region_index: u32,
    vector: UserInPtr<zx_iovec_t>,
    vector_count: usize,
) -> Result<(), Status> {
    ltracef!(
        "handle {handle:?}, options {options:#x}, region_index {region_index}, vector_count {vector_count}\n"
    );

    if options != 0 {
        return Err(Status::INVALID_ARGS);
    }

    let iob = Dispatcher::get_with_rights::<IoBufferDispatcher>(handle, ZX_RIGHT_WRITE)?;
    iob.write(region_index, vector, vector_count)?;
    Ok(())
}

#[syscall]
pub fn sys_iob_create_shared_region(
    options: u64,
    size: u64,
    out: &mut HandleValue,
) -> Result<(), Status> {
    ltracef!("options {options:#x}, size {size}\n");

    if options != 0 || !page::is_aligned(size as usize) || size == 0 {
        return Err(Status::INVALID_ARGS);
    }

    ProcessDispatcher::with_current(|up| up.enforce_basic_policy(ZX_POL_NEW_IOB))?;

    let (handle, rights) = IoBufferSharedRegionDispatcher::create(size)?;
    let user_handle = handle.make_and_add_handle(rights)?;
    *out = user_handle;
    Ok(())
}
