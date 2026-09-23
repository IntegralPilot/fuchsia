// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Fuchsia-specific extensions to FIDL.

use zx::NullableHandle;
use zx::sys::zx_handle_t;

use crate::decoder::InternalHandleDecoder;
use crate::encoder::InternalHandleEncoder;
use crate::{DecodeError, EncodeError};

pub use zx;

/// A decoder which support Zircon handles.
pub trait HandleDecoder: InternalHandleDecoder {
    /// Takes the next raw handle from the decoder.
    ///
    /// The returned raw handle must not be considered owned until the decoder is committed.
    fn take_raw_handle(&mut self) -> Result<zx_handle_t, DecodeError>;

    /// Returns the number of handles remaining in the decoder.
    fn handles_remaining(&mut self) -> usize;

    /// Takes the next raw driver handle from the decoder.
    #[doc(hidden)]
    fn take_raw_driver_handle(&mut self) -> Result<u32, DecodeError> {
        Err(DecodeError::DriverHandlesUnsupported)
    }
}

/// An encoder which supports Zircon handles.
pub trait HandleEncoder: InternalHandleEncoder {
    /// Pushes a handle into the encoder.
    fn push_handle(&mut self, handle: NullableHandle) -> Result<(), EncodeError>;

    /// Returns the number of handles added to the encoder.
    fn handles_pushed(&self) -> usize;

    /// Pushes a raw driver handle into the encoder.
    ///
    /// # Safety
    ///
    /// `raw_driver_handle` must be a valid `DriverHandle`. Calling
    /// `push_raw_driver_Handle` moves ownership of the handle into the encoder.
    #[doc(hidden)]
    unsafe fn push_raw_driver_handle(
        &mut self,
        #[allow(unused)] raw_driver_handle: u32,
    ) -> Result<(), EncodeError> {
        Err(EncodeError::DriverHandlesUnsupported)
    }
}

// TODO: `HandleDecoder` and `HandleEncoder` have terrible little methods:
// `take_raw_driver_handle` and `push_raw_driver_handle`. These exist because of
// two intersecting problems:
//
// 1. When writing `Encode` and `Decode` impls for a type, it can't just add
//    `where` clauses bounding `<field_ty>: Encode<___E>`. This is because some
//    FIDL type definitions are recursive, and Rust impls can't be coinductive.
//    So instead, we have to check whether the type is a `resource` and emit
//    `E: HandleEncoder` if it is.
//
// 2. The FIDL IR only tracks whether or not a type is a resource. It doesn't
//    track which resource types it contains. That means that if a type is a
//    resource, we don't know whether that's because it contains a handle, or
//    because it contains a driver handle.
//
// So the unfortunate result is that we have to combine all of the resource type
// encoding and decoding traits into a single one. If we fix this someday, then
// we can get rid of those methods and make separate `DriverHandleDecoder` and
// `DriverHandleEncoder` types.
//
// /// A decoder which support driver handles.
// pub trait DriverHandleDecoder: InternalHandleDecoder {
//     /// Takes the next raw driver handle from the decoder.
//     ///
//     /// The returned raw driver handle must not be considered owned until the decoder is committed.
//     fn take_raw_driver_handle(&mut self) -> Result<fdf_handle_t, DecodeError>;
//
//     /// Returns the number of driver handles remaining in the decoder.
//     fn driver_handles_remaining(&mut self) -> usize;
// }
//
// /// An encoder which supports Zircon handles.
// pub trait DriverHandleEncoder: InternalHandleEncoder {
//     /// Pushes a driver handle into the encoder.
//     fn push_driver_handle(&mut self, handle: DriverHandle) -> Result<(), EncodeError>;
//
//     /// Returns the number of driver handles added to the encoder.
//     fn driver_handles_pushed(&self) -> usize;
// }
