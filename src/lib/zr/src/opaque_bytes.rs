// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::opaque::Opaque;

/// A generic, safe opaque storage container with exact const size constraints.
///
/// This integrates with `Opaque<T>` to guarantee the Rust compiler knows the underlying memory
/// is interior-mutable (via `UnsafeCell`) and potentially uninitialized (via `MaybeUninit`).
pub type OpaqueBytes<const SIZE: usize> = Opaque<[u8; SIZE]>;

/// Defines an opaque C++ storage type with explicit size, alignment, and an FFI `init` function.
#[macro_export]
macro_rules! define_opaque_storage_ffi {
    (
        $(#[$meta:meta])*
        $vis:vis struct $name:ident(
            $size:expr,
            $align_const:expr,
            $align_literal:literal,
            $ffi_fn:path
            $(, $arg:ident : $arg_ty:ty)* $(,)?
        );
    ) => {
        $(#[$meta])*
        #[repr(C, align($align_literal))]
        $vis struct $name {
            inner: $crate::OpaqueBytes<{$size}>,
            _pinned: ::core::marker::PhantomPinned,
        }

        $crate::static_assert_size_and_align!($name, $size, $align_const);

        impl $name {
            $vis unsafe fn init(
                $( $arg : $arg_ty ),*
            ) -> impl ::pin_init::PinInit<Self, ::core::convert::Infallible> {
                $crate::pin_init_ffi!($ffi_fn $(, $arg)*)
            }

            /// Returns a raw void pointer to the underlying storage.
            $vis fn as_void_ptr(&self) -> *mut ::core::ffi::c_void {
                self.inner.get() as *mut ::core::ffi::c_void
            }
        }
    };
}
