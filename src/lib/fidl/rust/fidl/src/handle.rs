// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A portable representation of handle-like objects for fidl.

#[cfg(target_os = "fuchsia")]
pub use fuchsia_handles::*;

#[cfg(not(target_os = "fuchsia"))]
pub use non_fuchsia_handles::*;

pub use fuchsia_async::{Channel as AsyncChannel, OnSignalsRef, Socket as AsyncSocket};

/// Fuchsia implementation of handles just aliases the zircon library
#[cfg(target_os = "fuchsia")]
pub mod fuchsia_handles {

    pub use zx::{
        AsHandleRef, Handle, HandleDisposition, HandleInfo, HandleOp, HandleRef, Koid,
        MessageBufEtc, NullableHandle, ObjectType, Peered, Rights, Signals, Status,
    };

    pub use fuchsia_async::invoke_for_handle_types;

    macro_rules! fuchsia_handle {

        ($x:tt, $docname:expr, $name:ident, $value:expr, $availability:tt) => {
            pub use zx::$x;
        };
    }

    invoke_for_handle_types!(fuchsia_handle);

    pub use zx::SocketOpts;
}

/// Non-Fuchsia implementation of handles
#[cfg(not(target_os = "fuchsia"))]
pub mod non_fuchsia_handles {
    pub use fuchsia_async::emulated_handle::{
        AsHandleRef, EmulatedHandleRef, Handle, Handle as NullableHandle, HandleDisposition,
        HandleInfo, HandleOp, HandleRef, Koid, MessageBufEtc, ObjectType, Peered, Rights, Signals,
        SocketOpts,
    };
    pub use zx_status::Status;

    pub use fuchsia_async::invoke_for_handle_types;

    macro_rules! declare_unsupported_fidl_handle {
        ($name:ident) => {
            /// An unimplemented Zircon-like $name
            #[derive(PartialEq, Eq, Debug, PartialOrd, Ord, Hash)]
            pub struct $name;

            impl From<$crate::handle::NullableHandle> for $name {
                fn from(_: $crate::handle::NullableHandle) -> $name {
                    $name
                }
            }
            impl From<$name> for NullableHandle {
                fn from(_: $name) -> $crate::handle::NullableHandle {
                    $crate::handle::NullableHandle::invalid()
                }
            }
            impl AsHandleRef for $name {
                fn as_handle_ref(&self) -> HandleRef<'_> {
                    HandleRef::invalid()
                }
            }
        };
    }

    macro_rules! declare_fidl_handle {
        ($name:ident) => {
            pub use fuchsia_async::emulated_handle::$name;
        };
    }

    macro_rules! host_handle {
        ($x:tt, $docname:expr, $name:ident, $zx_name:ident, Everywhere) => {
            declare_fidl_handle! {$x}
        };
        ($x:tt, $docname:expr, $name:ident, $zx_name:ident, $availability:ident) => {
            declare_unsupported_fidl_handle! {$x}
        };
    }

    invoke_for_handle_types!(host_handle);
}

#[allow(clippy::too_long_first_doc_paragraph)]
/// Converts a vector of `HandleDisposition` (handles bundled with their
/// intended object type and rights) to a vector of `HandleInfo` (handles
/// bundled with their actual type and rights, guaranteed by the kernel).
///
/// This makes a `zx_handle_replace` syscall for each handle unless the rights
/// are `Rights::SAME_RIGHTS`.
///
/// # Panics
///
/// Panics if any of the handle dispositions uses `HandleOp::Duplicate`. This is
/// never the case for handle dispositions return by `standalone_encode`.
pub fn convert_handle_dispositions_to_infos(
    handle_dispositions: Vec<HandleDisposition<'_>>,
) -> crate::Result<Vec<HandleInfo>> {
    handle_dispositions
        .into_iter()
        .map(|mut hd| {
            Ok(HandleInfo::new(
                match hd.take_op() {
                    HandleOp::Move(h) if hd.rights == Rights::SAME_RIGHTS => h,
                    HandleOp::Move(h) => {
                        h.replace_handle(hd.rights).map_err(crate::Error::HandleReplace)?
                    }
                    HandleOp::Duplicate(_) => panic!("unexpected HandleOp::Duplicate"),
                },
                hd.object_type,
                hd.rights,
            ))
        })
        .collect()
}
