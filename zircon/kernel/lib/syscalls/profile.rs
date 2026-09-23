// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use crate::counters::define_kcounter;
use crate::object::{
    Dispatcher, HandleValue, ProcessDispatcher, ProfileDispatcher, ThreadDispatcher,
    VmAddressRegionDispatcher, validate_resource_kind_base,
};
use crate::user_copy::UserInPtr;
use debug::ltracef;
use syscalls_macro::syscall;
use zx_status::Status;
use zx_types::{
    ZX_POL_NEW_PROFILE, ZX_RIGHT_APPLY_PROFILE, ZX_RIGHT_MANAGE_THREAD, ZX_RIGHT_OP_CHILDREN,
    ZX_RSRC_KIND_SYSTEM, ZX_RSRC_SYSTEM_PROFILE_BASE, zx_profile_info_t,
};

const LOCAL_TRACE: u32 = 0;

define_kcounter!(PROFILE_CREATE, "profile.create", Sum);
define_kcounter!(PROFILE_SET, "profile.set", Sum);

#[syscall]
pub fn sys_profile_create(
    profile_rsrc: HandleValue,
    options: u32,
    user_profile_info: UserInPtr<zx_profile_info_t>,
    out: &mut HandleValue,
) -> Result<(), Status> {
    ltracef!("profile_rsrc {:#x}, options {:#x}\n", profile_rsrc.raw_value(), options);

    ProcessDispatcher::with_current(|up| {
        up.enforce_basic_policy(ZX_POL_NEW_PROFILE)?;
        Ok::<(), Status>(())
    })?;

    if options != 0 {
        return Err(Status::INVALID_ARGS);
    }

    match validate_resource_kind_base(
        profile_rsrc,
        ZX_RSRC_KIND_SYSTEM,
        ZX_RSRC_SYSTEM_PROFILE_BASE,
    ) {
        Ok(()) => {}
        Err(Status::BAD_HANDLE) => return Err(Status::BAD_HANDLE),
        Err(_) => return Err(Status::ACCESS_DENIED),
    }

    let mut uninit_profile_info = core::mem::MaybeUninit::uninit();
    let profile_info = user_profile_info.copy_from_user(&mut uninit_profile_info)?;

    let (kernel_handle, rights) = ProfileDispatcher::create(profile_info)?;

    PROFILE_CREATE.add(1);

    let user_handle =
        ProcessDispatcher::with_current(|up| up.make_and_add_handle(kernel_handle, rights))?;

    *out = user_handle;
    Ok(())
}

#[syscall]
pub fn sys_object_set_profile(
    handle: HandleValue,
    profile_handle: HandleValue,
    options: u32,
) -> Result<(), Status> {
    ltracef!(
        "handle {:#x}, profile_handle {:#x}, options {:#x}\n",
        handle.raw_value(),
        profile_handle.raw_value(),
        options
    );

    if options != 0 {
        return Err(Status::INVALID_ARGS);
    }

    PROFILE_SET.add(1);

    let profile =
        Dispatcher::get_with_rights::<ProfileDispatcher>(profile_handle, ZX_RIGHT_APPLY_PROFILE)?;

    let (disp, rights) = Dispatcher::get_dispatcher_and_rights(handle)?;

    if let Some(thread) = disp.downcast::<ThreadDispatcher>() {
        if (rights & ZX_RIGHT_MANAGE_THREAD) == 0 {
            return Err(Status::ACCESS_DENIED);
        }
        profile.apply_profile_to_thread(thread)?;
        return Ok(());
    }

    if let Some(vmar) = disp.downcast::<VmAddressRegionDispatcher>() {
        if (rights & ZX_RIGHT_OP_CHILDREN) == 0 {
            return Err(Status::ACCESS_DENIED);
        }
        profile.apply_profile_to_vmar(vmar)?;
        return Ok(());
    }

    Err(Status::WRONG_TYPE)
}
