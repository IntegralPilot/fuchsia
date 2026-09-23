// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! This crate contains utility functions used in GIDL tests and benchmarks.

use fidl::encoding::{Context, Decode, Decoder, DefaultFuchsiaResourceDialect, TypeMarker};
use fidl::{HandleDisposition, HandleInfo, HandleOp, NullableHandle, Rights};
use zx_status::Status;
use zx_types;

/// Handle subtypes that can be created via `create_handles`. Each subtype `X`
/// corresponds to a `fidl::X` type that implements `HandleBased`.
pub enum HandleSubtype {
    Event,
    Channel,
}

impl HandleSubtype {
    fn obj_type(&self) -> zx_types::zx_obj_type_t {
        match self {
            HandleSubtype::Event => zx_types::ZX_OBJ_TYPE_EVENT,
            HandleSubtype::Channel => zx_types::ZX_OBJ_TYPE_CHANNEL,
        }
    }
}

/// Specifies a handle to be created with `create_handles`. Corresponds to
/// `HandleDef` in //tools/fidl/gidl/ir/test_case.go.
pub struct HandleDef {
    pub subtype: HandleSubtype,
    pub rights: Rights,
}

/// Creates a vector of raw handles based on `defs`. Panics if creating any of
/// the handles fails. The caller is responsible for closing the handles.
pub fn create_handles(defs: &[HandleDef]) -> Vec<zx_types::zx_handle_info_t> {
    let mut factory: HandleFactory = Default::default();
    let mut handle_infos = Vec::with_capacity(defs.len());
    for def in defs {
        let default_rights_handle = match def.subtype {
            HandleSubtype::Event => factory.create_event().unwrap().into_handle(),
            HandleSubtype::Channel => factory.create_channel().unwrap().into_handle(),
        };
        handle_infos.push(zx_types::zx_handle_info_t {
            handle: match def.rights {
                Rights::SAME_RIGHTS => default_rights_handle,
                rights => default_rights_handle.replace_handle(rights).unwrap(),
            }
            .into_raw(),
            ty: def.subtype.obj_type(),
            rights: def.rights.bits(),
            unused: 0,
        });
    }
    handle_infos
}

/// HandleFactory creates handles. For handle subtypes that come in pairs, it
/// stores the second one and returns it on the next call to minimize syscalls.
#[derive(Default)]
struct HandleFactory {
    extra_channel: Option<fidl::Channel>,
}

// See src/lib/fuchsia-async/src/handle/mod.rs for handle subtypes. The ones
// marked "Everywhere" are fully emulated on non-Fuchsia, so we can define
// factory functions that work on all platforms.
impl HandleFactory {
    fn create_event(&mut self) -> Result<fidl::Event, Status> {
        Ok(fidl::Event::create())
    }

    fn create_channel(&mut self) -> Result<fidl::Channel, Status> {
        match self.extra_channel.take() {
            Some(channel) => Ok(channel),
            None => {
                let (c1, c2) = fidl::Channel::create();
                self.extra_channel = Some(c2);
                Ok(c1)
            }
        }
    }
}

/// Copies a raw handle into an owned `HandleBased` handle.
pub fn copy_handle<T: From<NullableHandle>>(handle_info: &zx_types::zx_handle_info_t) -> T {
    // Safety: The `from_raw` method is only unsafe because it can lead to
    // handles being double-closed if used incorrectly. GIDL-generated code
    // ensures that handles are only closed once.
    T::from(unsafe { NullableHandle::from_raw(handle_info.handle) })
}

/// Copies raw handles from the given indices to a new vector.
pub fn select_raw_handle_infos(
    handle_defs: &[zx_types::zx_handle_info_t],
    indices: &[usize],
) -> Vec<zx_types::zx_handle_info_t> {
    indices.iter().map(|&i| handle_defs[i]).collect()
}

/// Copies raw handles from the given indices to a new vector of owned
/// `HandleInfo`s. The caller must ensure handles are closed exactly once.
pub fn select_handle_infos(
    handle_defs: &[zx_types::zx_handle_info_t],
    indices: &[usize],
) -> Vec<HandleInfo> {
    // Safety: The `from_raw` method is only unsafe because it can lead to
    // handles being double-closed if used incorrectly. GIDL-generated code
    // ensures that handles are only closed once.
    indices.iter().map(|&i| unsafe { HandleInfo::from_raw(handle_defs[i]) }).collect()
}

/// Gets the koid of a handle from its raw handle info. Panics if the
/// `zx_object_get_info` syscall fails.
pub fn get_handle_koid(handle_info: &zx_types::zx_handle_info_t) -> zx_types::zx_koid_t {
    // Safety: The `from_raw` method is only unsafe because it can lead to
    // handles being double-closed if used incorrectly. We wrap it in
    // ManuallyDrop to prevent closing the handle.
    let handle =
        std::mem::ManuallyDrop::new(unsafe { NullableHandle::from_raw(handle_info.handle) });
    handle.basic_info().unwrap().koid.raw_koid()
}

/// Converts a `HandleDisposition` to a raw `zx_handle_disposition_t`.
pub fn to_zx_handle_disposition_t(
    mut hd: HandleDisposition<'_>,
) -> zx_types::zx_handle_disposition_t {
    match hd.take_op() {
        HandleOp::Move(handle) => zx_types::zx_handle_disposition_t {
            operation: zx_types::ZX_HANDLE_OP_MOVE,
            handle: handle.raw_handle(),
            type_: hd.object_type.into_raw(),
            rights: hd.rights.bits(),
            result: Status::result_into_raw(hd.result),
        },
        HandleOp::Duplicate(handle_ref) => zx_types::zx_handle_disposition_t {
            operation: zx_types::ZX_HANDLE_OP_DUPLICATE,
            handle: handle_ref.raw_handle(),
            type_: hd.object_type.into_raw(),
            rights: hd.rights.bits(),
            result: Status::result_into_raw(hd.result),
        },
    }
}

/// Returns the result of the `zx_object_get_info` syscall with topic
/// `ZX_INFO_HANDLE_VALID`. In particular, returns `Status::BAD_HANDLE` if
/// the handle is dangling because it was already closed or never assigned
/// to the process in the first place.
///
/// This should only be used in a single-threaded process immediately after
/// a handle is created/closed, since "The kernel is free to re-use the
/// integer values of closed handles for newly created objects".
/// https://fuchsia.dev/fuchsia-src/concepts/kernel/handles#invalid_handles_and_handle_reuse
#[cfg(target_os = "fuchsia")]
pub fn get_info_handle_valid(handle_info: &zx_types::zx_handle_info_t) -> Result<(), Status> {
    use zx::sys;
    Status::ok(unsafe {
        sys::zx_object_get_info(
            handle_info.handle,
            sys::ZX_INFO_HANDLE_VALID,
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    })
}

#[cfg(not(target_os = "fuchsia"))]
pub fn get_info_handle_valid(handle_info: &zx_types::zx_handle_info_t) -> Result<(), Status> {
    use fidl::EmulatedHandleRef;
    // Safety: The `from_raw` method is only unsafe because it can lead to
    // handles being double-closed if used incorrectly. We wrap it in
    // ManuallyDrop to prevent closing the handle.
    let handle =
        std::mem::ManuallyDrop::new(unsafe { NullableHandle::from_raw(handle_info.handle) });
    // Match the behavior of the syscall, returning BAD_HANDLE if the handle is
    // the special "never a valid handle" ZX_HANDLE_INVALID, or if it is invalid
    // because it was closed or never assigned in the first place (dangling).
    if handle.is_invalid() || handle.is_dangling() { Err(Status::BAD_HANDLE) } else { Ok(()) }
}

/// Returns a vector of `value` repeated `len` times.
pub fn repeat<T: Clone>(value: T, len: usize) -> Vec<T> {
    std::iter::repeat(value).take(len).collect::<Vec<_>>()
}

/// Decodes `T` from the given bytes and handles. Panics on failure.
pub fn decode_value<T: TypeMarker>(
    context: Context,
    bytes: &[u8],
    handles: &mut [HandleInfo],
) -> T::Owned
where
    T::Owned: Decode<T, DefaultFuchsiaResourceDialect>,
{
    let mut value = T::Owned::new_empty();
    Decoder::decode_with_context::<T>(context, bytes, handles, &mut value).expect(
        "failed decoding for the GIDL decode() function (not a regular decode test failure!)",
    );
    value
}
