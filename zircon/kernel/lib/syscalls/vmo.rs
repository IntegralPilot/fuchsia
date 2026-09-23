// Copyright 2016 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use crate::object::{
    Dispatcher, HandleValue, InitialMutability, ProcessDispatcher, VmObjectDispatcher,
    validate_ranged_resource,
};
use crate::user_copy::{UserInOutPtr, UserInPtr, UserOutPtr};
use crate::vm::arch_vm_aspace::{
    ARCH_MMU_FLAG_CACHE_MASK, ARCH_MMU_FLAG_CACHED, ARCH_MMU_FLAG_UNCACHED,
    ARCH_MMU_FLAG_UNCACHED_DEVICE, ARCH_MMU_FLAG_WRITE_COMBINING,
};
use crate::vm::pmm::{ALLOC_FLAG_ANY, ALLOC_FLAG_CAN_WAIT};
use crate::vm::vm_object::{SupplyOptions, VmObject};
use crate::vm::vm_object_paged::VmObjectPaged;
use crate::vm::vm_page_list::VmPageSpliceList;
use debug::ltracef;
use page;
use pin_init::stack_pin_init;
use syscalls_macro::syscall;
use zx_status::Status;
use zx_types::{
    ZX_CACHE_POLICY_CACHED, ZX_CACHE_POLICY_UNCACHED, ZX_CACHE_POLICY_UNCACHED_DEVICE,
    ZX_CACHE_POLICY_WRITE_COMBINING, ZX_HANDLE_INVALID, ZX_OBJ_TYPE_VMO,
    ZX_POL_AMBIENT_MARK_VMO_EXEC, ZX_POL_NEW_VMO, ZX_RIGHT_DUPLICATE, ZX_RIGHT_EXECUTE,
    ZX_RIGHT_GET_PROPERTY, ZX_RIGHT_MAP, ZX_RIGHT_READ, ZX_RIGHT_RESIZE, ZX_RIGHT_SET_PROPERTY,
    ZX_RIGHT_WRITE, ZX_RSRC_KIND_SYSTEM, ZX_RSRC_SYSTEM_VMEX_BASE, ZX_VMO_CHILD_NO_WRITE,
    ZX_VMO_CHILD_REFERENCE, ZX_VMO_CHILD_RESIZABLE, ZX_VMO_CHILD_SNAPSHOT,
    ZX_VMO_CHILD_SNAPSHOT_AT_LEAST_ON_WRITE, ZX_VMO_CHILD_SNAPSHOT_MODIFIED,
};

const LOCAL_TRACE: u32 = 0;

const ZX_CACHE_POLICY_MASK: u32 = 3;

zr::static_assert!(ZX_CACHE_POLICY_CACHED == ARCH_MMU_FLAG_CACHED as u32);
zr::static_assert!(ZX_CACHE_POLICY_UNCACHED == ARCH_MMU_FLAG_UNCACHED as u32);
zr::static_assert!(ZX_CACHE_POLICY_UNCACHED_DEVICE == ARCH_MMU_FLAG_UNCACHED_DEVICE as u32);
zr::static_assert!(ZX_CACHE_POLICY_WRITE_COMBINING == ARCH_MMU_FLAG_WRITE_COMBINING as u32);
zr::static_assert!(ZX_CACHE_POLICY_MASK == ARCH_MMU_FLAG_CACHE_MASK as u32);

#[syscall]
pub fn sys_vmo_create(size: u64, options: u32, out: &mut HandleValue) -> Result<(), Status> {
    ltracef!("size {:#x}\n", size);

    ProcessDispatcher::with_current(|up| up.enforce_basic_policy(ZX_POL_NEW_VMO))?;

    let stats = VmObjectDispatcher::parse_create_syscall_flags(options, size)?;

    // create a vm object
    let vmo = VmObjectPaged::create(ALLOC_FLAG_ANY | ALLOC_FLAG_CAN_WAIT, stats.flags, stats.size)?;

    // create a Vm Object dispatcher
    let (kernel_handle, rights) =
        VmObjectDispatcher::create(&vmo, size, InitialMutability::Mutable)?;

    // create a handle and attach the dispatcher to it
    *out = kernel_handle.make_and_add_handle(rights)?;
    Ok(())
}

#[syscall]
pub fn sys_vmo_read(
    handle: HandleValue,
    data: UserOutPtr<u8>,
    offset: u64,
    len: usize,
) -> Result<(), Status> {
    ltracef!(
        "handle {:x}, data {:p}, offset {:#x}, len {:#x}\n",
        handle.raw_value(),
        data.as_ptr(),
        offset,
        len
    );

    // lookup the dispatcher from handle
    let vmo = Dispatcher::get_with_rights::<VmObjectDispatcher>(handle, ZX_RIGHT_READ)?;

    vmo.read(data, offset, len)?;
    Ok(())
}

#[syscall]
pub fn sys_vmo_write(
    handle: HandleValue,
    data: UserInPtr<u8>,
    offset: u64,
    len: usize,
) -> Result<(), Status> {
    ltracef!(
        "handle {:x}, data {:p}, offset {:#x}, len {:#x}\n",
        handle.raw_value(),
        data.as_ptr(),
        offset,
        len
    );

    // lookup the dispatcher from handle
    let vmo = Dispatcher::get_with_rights::<VmObjectDispatcher>(handle, ZX_RIGHT_WRITE)?;

    vmo.write(data, offset, len)?;
    Ok(())
}

#[syscall]
pub fn sys_vmo_transfer_data(
    dst_vmo_handle: HandleValue,
    options: u32,
    offset: u64,
    length: u64,
    src_vmo_handle: HandleValue,
    src_offset: u64,
) -> Result<(), Status> {
    // Currently, there are no supported options. This may change in the future.
    if options != 0 {
        return Err(Status::INVALID_ARGS);
    }

    if !page::is_aligned(offset as usize)
        || !page::is_aligned(length as usize)
        || !page::is_aligned(src_offset as usize)
    {
        return Err(Status::INVALID_ARGS);
    }

    let dst_vmo_dispatcher =
        Dispatcher::get_with_rights::<VmObjectDispatcher>(dst_vmo_handle, ZX_RIGHT_WRITE)?;
    let src_vmo_dispatcher = Dispatcher::get_with_rights::<VmObjectDispatcher>(
        src_vmo_handle,
        ZX_RIGHT_READ | ZX_RIGHT_WRITE,
    )?;

    // Short circuit out if src_vmo and dst_vmo are identical and the src_offset is the same as
    // the destination offset.
    if src_vmo_dispatcher.get_koid() == dst_vmo_dispatcher.get_koid() && src_offset == offset {
        return Ok(());
    }

    stack_pin_init!(let pages = VmPageSpliceList::new());
    src_vmo_dispatcher.vmo().take_pages(src_offset, length, pages.as_mut())?;

    dst_vmo_dispatcher.vmo().supply_pages(
        offset,
        length,
        pages.as_mut(),
        SupplyOptions::TransferData,
    )?;
    Ok(())
}

#[syscall]
pub fn sys_vmo_get_size(handle: HandleValue, size: UserOutPtr<u64>) -> Result<(), Status> {
    ltracef!("handle {:x}, sizep {:p}\n", handle.raw_value(), size.as_ptr());

    // lookup the dispatcher from handle
    let vmo = Dispatcher::get::<VmObjectDispatcher>(handle)?;

    // no rights check, anyone should be able to get the size

    // do the operation
    let vmo_size = vmo.get_size()?;

    size.copy_to_user(&vmo_size)?;
    Ok(())
}

#[syscall]
pub fn sys_vmo_get_stream_size(handle: HandleValue, size: UserOutPtr<u64>) -> Result<(), Status> {
    // lookup the dispatcher from handle (no rights required to get stream size).
    let vmo = Dispatcher::get::<VmObjectDispatcher>(handle)?;

    let stream_size = vmo.get_stream_size();
    size.copy_to_user(&stream_size)?;
    Ok(())
}

#[syscall]
pub fn sys_vmo_set_size(handle: HandleValue, size: u64) -> Result<(), Status> {
    ltracef!("handle {:x}, size {:#x}\n", handle.raw_value(), size);

    // lookup the dispatcher from handle
    let (vmo, rights) =
        Dispatcher::get_with_rights_and_actual::<VmObjectDispatcher>(handle, ZX_RIGHT_WRITE)?;

    // VMOs that are not resizable should fail with ZX_ERR_UNAVAILABLE for backwards compatibility,
    // which will be handled by the SetSize call below. Only validate the RESIZE right if the VMO is
    // resizable.
    if vmo.vmo().is_resizable() && (rights & ZX_RIGHT_RESIZE) == 0 {
        return Err(Status::ACCESS_DENIED);
    }

    // do the operation
    vmo.set_size(size)?;
    Ok(())
}

#[syscall]
pub fn sys_vmo_set_stream_size(handle: HandleValue, size: u64) -> Result<(), Status> {
    ltracef!("handle {:x}, size {:#x}\n", handle.raw_value(), size);

    // lookup the dispatcher from handle
    let vmo = Dispatcher::get_with_rights::<VmObjectDispatcher>(handle, ZX_RIGHT_WRITE)?;

    // do the operation
    vmo.set_stream_size(size)?;
    Ok(())
}

#[syscall]
pub fn sys_vmo_op_range(
    handle: HandleValue,
    op: u32,
    offset: u64,
    size: u64,
    buffer: UserInOutPtr<u8>,
    buffer_size: usize,
) -> Result<(), Status> {
    ltracef!(
        "handle {:x} op {} offset {:#x} size {:#x} buffer {:p} buffer_size {}\n",
        handle.raw_value(),
        op,
        offset,
        size,
        buffer.as_ptr(),
        buffer_size
    );

    // lookup the dispatcher from handle
    // save the rights and pass down into the dispatcher for further testing
    let (vmo, rights) = Dispatcher::get_and_rights::<VmObjectDispatcher>(handle)?;

    vmo.range_op(op, offset, size, buffer, buffer_size, rights)?;
    Ok(())
}

#[syscall]
pub fn sys_vmo_set_cache_policy(handle: HandleValue, cache_policy: u32) -> Result<(), Status> {
    // Sanity check the cache policy.
    if (cache_policy & !ZX_CACHE_POLICY_MASK) != 0 {
        return Err(Status::INVALID_ARGS);
    }

    // lookup the dispatcher from handle.
    let vmo = Dispatcher::get_with_rights::<VmObjectDispatcher>(handle, ZX_RIGHT_MAP)?;

    vmo.set_mapping_cache_policy(cache_policy)?;
    Ok(())
}

#[syscall]
pub fn sys_vmo_create_child(
    handle: HandleValue,
    mut options: u32,
    offset: u64,
    size: u64,
    out_handle: &mut HandleValue,
) -> Result<(), Status> {
    ltracef!(
        "handle {:x} options {:#x} offset {:#x} size {:#x}\n",
        handle.raw_value(),
        options,
        offset,
        size
    );

    let mut no_write = false;

    // VMO size is rounded up to the nearest page boundary.
    let vmo_size = VmObject::round_size(size)?;

    // Resizing a VMO requires the WRITE permissions, but NO_WRITE forbids the WRITE permissions, as
    // such it does not make sense to create a VMO with both of these.
    if (options & ZX_VMO_CHILD_NO_WRITE) != 0 && (options & ZX_VMO_CHILD_RESIZABLE) != 0 {
        return Err(Status::INVALID_ARGS);
    }

    // Writable is a property of the handle, not the object, so we consume this option here before
    // calling create_child.
    if (options & ZX_VMO_CHILD_NO_WRITE) != 0 {
        no_write = true;
        options &= !ZX_VMO_CHILD_NO_WRITE;
    }

    // Reference children share their size with the parent, so attempts to resize them
    // "pass through" and resize the parent. Require ZX_RIGHT_RESIZE on the parent handle to create
    // resizable reference children, as they functionally allow the caller to resize parent VMOs.
    let would_resize_pass_through =
        (options & ZX_VMO_CHILD_RESIZABLE) != 0 && (options & ZX_VMO_CHILD_REFERENCE) != 0;
    let desired_rights = ZX_RIGHT_DUPLICATE
        | ZX_RIGHT_READ
        | if would_resize_pass_through { ZX_RIGHT_RESIZE } else { 0 };

    // lookup the dispatcher from handle, save a copy of the rights for later. We must hold onto
    // the refptr of this VMO up until we create the dispatcher. The reason for this is that
    // VmObjectDispatcher::Create sets the user_id and page_attribution_id in the created child
    // vmo. Should the vmo destroyed between creating the child and setting the id in the dispatcher
    // the currently unset user_id may be used to re-attribute a parent. Holding the refptr prevents
    // any destruction from occurring.
    let (vmo, actual_rights) =
        Dispatcher::get_with_rights_and_actual::<VmObjectDispatcher>(handle, desired_rights)?;

    // clone the vmo into a new one
    let child_vmo =
        vmo.create_child(options, offset, vmo_size, (actual_rights & ZX_RIGHT_GET_PROPERTY) != 0)?;

    // This checks that the child VMO is explicitly created with ZX_VMO_CHILD_SNAPSHOT.
    // There are other ways that VMOs can be effectively immutable, for instance if the VMO is
    // created with ZX_VMO_CHILD_SNAPSHOT_AT_LEAST_ON_WRITE and meets certain criteria it will be
    // "upgraded" to a snapshot. However this behavior is not guaranteed at the API level.
    // A choice was made to conservatively only mark VMOs as immutable when the user explicitly
    // creates a VMO in a way that is guaranteed at the API level to always output an immutable VMO.
    let mut initial_mutability = InitialMutability::Mutable;
    if no_write && (options & ZX_VMO_CHILD_SNAPSHOT) != 0 {
        initial_mutability = InitialMutability::Immutable;
    }

    // create a Vm Object dispatcher
    let (kernel_handle, default_rights) =
        vmo.create_child_dispatcher(&child_vmo, size, options, initial_mutability)?;

    // Set the rights to the new handle to no greater than the input (parent) handle minus the
    // RESIZE right, which is added independently based on ZX_VMO_CHILD_RESIZABLE; it is possible
    // for a non-resizable parent to have a resizable child and vice versa. Always allow
    // GET/SET_PROPERTY so the user can set ZX_PROP_NAME on the new clone.
    let mut rights = (actual_rights & !ZX_RIGHT_RESIZE)
        | if (options & ZX_VMO_CHILD_RESIZABLE) != 0 { ZX_RIGHT_RESIZE } else { 0 }
        | ZX_RIGHT_GET_PROPERTY
        | ZX_RIGHT_SET_PROPERTY;

    // Unless it was explicitly requested to be removed, WRITE can be added to CoW clones at the
    // expense of executability.
    if no_write {
        rights &= !ZX_RIGHT_WRITE;
        // NO_WRITE and RESIZABLE cannot be specified together, so we should not have the RESIZE
        // right.
        debug_assert!((rights & ZX_RIGHT_RESIZE) == 0);
    } else if (options
        & (ZX_VMO_CHILD_SNAPSHOT
            | ZX_VMO_CHILD_SNAPSHOT_AT_LEAST_ON_WRITE
            | ZX_VMO_CHILD_SNAPSHOT_MODIFIED))
        != 0
    {
        rights &= !ZX_RIGHT_EXECUTE;
        rights |= ZX_RIGHT_WRITE;
    }

    // make sure we're somehow not elevating rights beyond what a new vmo should have
    debug_assert!(((default_rights | ZX_RIGHT_EXECUTE) & rights) == rights);

    // create a handle and attach the dispatcher to it
    *out_handle = kernel_handle.make_and_add_handle(rights)?;
    Ok(())
}

#[syscall]
pub fn sys_vmo_replace_as_executable(
    handle: HandleValue,
    vmex: HandleValue,
    out: &mut HandleValue,
) -> Result<(), Status> {
    ltracef!("repexec {:x} {:x}\n", handle.raw_value(), vmex.raw_value());

    let vmex_status = if vmex.raw_value() != ZX_HANDLE_INVALID {
        validate_ranged_resource(vmex, ZX_RSRC_KIND_SYSTEM, ZX_RSRC_SYSTEM_VMEX_BASE, 1)
    } else {
        ProcessDispatcher::with_current(|up| up.enforce_basic_policy(ZX_POL_AMBIENT_MARK_VMO_EXEC))
    };

    let orig_handle =
        ProcessDispatcher::with_current(|up| up.remove_handle(handle)).ok_or(Status::BAD_HANDLE)?;
    if orig_handle.dispatcher().get_type() != ZX_OBJ_TYPE_VMO {
        return Err(Status::BAD_HANDLE);
    }

    vmex_status?;

    *out = ProcessDispatcher::with_current(|up| {
        up.make_and_add_handle_from_ref(
            orig_handle.dispatcher(),
            orig_handle.rights() | ZX_RIGHT_EXECUTE,
        )
    })?;
    Ok(())
}
