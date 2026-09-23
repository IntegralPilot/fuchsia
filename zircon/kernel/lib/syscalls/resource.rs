// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use crate::object::{
    Dispatcher, HandleValue, ResourceDispatcher, StrictValidation, ZX_RSRC_FLAGS_MASK,
    is_valid_kind, validate_ranged_resource_dispatcher,
};
use crate::user_copy::UserInPtr;
use core::mem::MaybeUninit;
use syscalls_macro::syscall;
use zx_status::Status;
use zx_types::{ZX_MAX_NAME_LEN, ZX_RIGHT_WRITE, ZX_RSRC_FLAG_EXCLUSIVE};

const ZX_RSRC_KIND_MASK: u32 = 0x0000_FFFF;

#[inline]
const fn zx_rsrc_extract_kind(options: u32) -> u32 {
    options & ZX_RSRC_KIND_MASK
}

#[inline]
const fn zx_rsrc_extract_flags(options: u32) -> u32 {
    options & !ZX_RSRC_KIND_MASK
}

/// Creates a new resource, child of the provided resource. On success, a new resource is created
/// and a handle is returned in `resource_out`.
///
/// For more information on resources, see `//docs/reference/kernel_objects/resource.md`.
///
/// The range low:high is inclusive on both ends, high must be greater than or equal to low.
///
/// `parent_rsrc` must be a resource of the same kind as `kind`, or a root resource. `base` and
/// `size` represent an inclusive range from `base` to `base + size` for the child resource.
#[syscall]
pub fn sys_resource_create(
    parent_rsrc: HandleValue,
    options: u32,
    base: u64,
    size: usize,
    user_name: UserInPtr<u8>,
    name_size: usize,
    resource_out: &mut HandleValue,
) -> Result<(), Status> {
    // Verify that the given base and size don't lead to an integer overflow.
    if base.checked_add(size as u64).is_none() {
        return Err(Status::INVALID_ARGS);
    }

    // Obtain the parent Resource. WRITE access is required to create a child resource.
    let parent = Dispatcher::get_with_rights::<ResourceDispatcher>(parent_rsrc, ZX_RIGHT_WRITE)?;

    let kind = zx_rsrc_extract_kind(options);
    let flags = zx_rsrc_extract_flags(options);
    if !is_valid_kind(kind) || (flags & !ZX_RSRC_FLAGS_MASK) != 0 {
        return Err(Status::INVALID_ARGS);
    }

    // Validate the parent resource the same way we would validate any resource usage in another
    // syscall.
    if validate_ranged_resource_dispatcher(&parent, kind, base, size, StrictValidation::Yes)
        .is_err()
    {
        return Err(Status::ACCESS_DENIED);
    }

    // If the resource is a slice of a larger resource then neither the new resource nor its
    // parent are permitted to be exclusive resources. In this case, `parent_rsrc` will not be the
    // root resource for `kind`.
    if !parent.is_ranged_root(kind)
        && ((parent.get_flags() & ZX_RSRC_FLAG_EXCLUSIVE != 0)
            || (flags & ZX_RSRC_FLAG_EXCLUSIVE != 0))
    {
        return Err(Status::INVALID_ARGS);
    }

    // Extract the name from userspace if one was provided.
    let mut buf = [MaybeUninit::<u8>::uninit(); ZX_MAX_NAME_LEN];
    let name = user_name.copy_user_string(name_size, &mut buf)?;

    // Create a new Resource
    let (kernel_handle, rights) = ResourceDispatcher::create(kind, base, size, flags, name)?;

    // Create a handle for the child
    *resource_out = kernel_handle.make_and_add_handle(rights)?;
    Ok(())
}
