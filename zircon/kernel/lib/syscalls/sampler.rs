// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use boot_options::BootOptions;
use debug::ltracef;
use syscalls_macro::syscall;
use zx_status::Status;
use zx_types::{
    ZX_POL_NEW_SAMPLER, ZX_RSRC_KIND_SYSTEM, ZX_RSRC_SYSTEM_SAMPLING_BASE,
    ZX_SAMPLER_MAX_BUFFER_SIZE, ZX_SAMPLER_MIN_PERIOD, zx_sampler_config_t,
};

use crate::object::{
    Dispatcher, HandleValue, ProcessDispatcher, SamplerDispatcher, validate_ranged_resource,
};
use crate::user_copy::{UserInPtr, UserOutPtr};

const LOCAL_TRACE: u32 = 0;

fn check_sampler_supported() -> Result<(), Status> {
    if !BootOptions::get().enable_debugging_syscalls {
        return Err(Status::NOT_SUPPORTED);
    }
    Ok(())
}

#[syscall]
pub fn sys_sampler_create(
    rsrc: HandleValue,
    options: u64,
    config_ptr: UserInPtr<zx_sampler_config_t>,
    out_handle: &mut HandleValue,
) -> Result<(), Status> {
    ltracef!("options {:#x}\n", options);

    if options != 0 {
        return Err(Status::INVALID_ARGS);
    }

    check_sampler_supported()?;

    validate_ranged_resource(rsrc, ZX_RSRC_KIND_SYSTEM, ZX_RSRC_SYSTEM_SAMPLING_BASE, 1)?;

    ProcessDispatcher::with_current(|up| {
        up.enforce_basic_policy(ZX_POL_NEW_SAMPLER)?;
        Ok::<(), Status>(())
    })?;

    let mut uninit_config = core::mem::MaybeUninit::uninit();
    let config = config_ptr.copy_from_user(&mut uninit_config)?;

    // We'll pick a arbitrary unreasonably large max size for the per cpu buffers.
    //
    // When we implement streamed reads, we can reduce this to something more
    // reasonable.
    if config.buffer_size > ZX_SAMPLER_MAX_BUFFER_SIZE {
        return Err(Status::INVALID_ARGS);
    }

    // The act of taking a sample takes on the order of single digit microseconds. A period close to
    // or shorter than that doesn't make sense.
    if config.period < ZX_SAMPLER_MIN_PERIOD {
        return Err(Status::INVALID_ARGS);
    }

    let (kernel_handle, rights) = SamplerDispatcher::create(config)?;
    *out_handle = kernel_handle.make_and_add_handle(rights)?;
    Ok(())
}

#[syscall]
pub fn sys_sampler_start(sampler_handle: HandleValue) -> Result<(), Status> {
    ltracef!("handle {:#x}\n", sampler_handle.raw_value());

    check_sampler_supported()?;

    let sampler = Dispatcher::get_with_rights::<SamplerDispatcher>(sampler_handle, 0)?;
    sampler.start()?;
    Ok(())
}

#[syscall]
pub fn sys_sampler_stop(sampler_handle: HandleValue) -> Result<(), Status> {
    ltracef!("handle {:#x}\n", sampler_handle.raw_value());

    check_sampler_supported()?;

    let sampler = Dispatcher::get_with_rights::<SamplerDispatcher>(sampler_handle, 0)?;
    sampler.stop()?;
    Ok(())
}

#[syscall]
pub fn sys_sampler_read(
    sampler_handle: HandleValue,
    data: UserOutPtr<u8>,
    len: usize,
    actual: UserOutPtr<usize>,
) -> Result<(), Status> {
    ltracef!("handle {:#x}, len {}\n", sampler_handle.raw_value(), len);

    check_sampler_supported()?;

    let sampler = Dispatcher::get_with_rights::<SamplerDispatcher>(sampler_handle, 0)?;
    let (res, bytes_copied) = sampler.read_user(data, len);

    // We may have a partial read: some bytes were copied, but we received an error later on.
    // We provide the caller with how many bytes we copied, but also the error we ran into.
    actual.copy_to_user(&bytes_copied)?;
    res
}
