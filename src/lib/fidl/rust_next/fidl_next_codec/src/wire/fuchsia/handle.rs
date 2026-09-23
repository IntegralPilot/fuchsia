// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::fmt;
use core::mem::{MaybeUninit, forget};

use fidl_constants::{ALLOC_ABSENT_U32, ALLOC_PRESENT_U32};
use zx::sys::{ZX_HANDLE_INVALID, zx_handle_t};

use crate::fuchsia::{HandleDecoder, HandleEncoder};
use crate::{
    Constrained, Decode, DecodeError, Encode, EncodeError, EncodeOption, FromWire, FromWireOption,
    IntoNatural, Slot, ValidationError, Wire, munge, wire,
};

/// TODO(https://fxbug.dev/465766514): remove
pub type NullableHandle = Handle;

/// A Zircon handle.
#[repr(C, align(4))]
pub union Handle {
    encoded: wire::Uint32,
    decoded: zx_handle_t,
}

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: `WireHandle` is always a valid `Handle`.
        let handle = unsafe { zx::NullableHandle::from_raw(self.as_raw_handle()) };
        drop(handle);
    }
}

// TODO: validate handle rights
impl Constrained for Handle {
    type Constraint = ();

    fn validate(_: Slot<'_, Self>, _: Self::Constraint) -> Result<(), ValidationError> {
        Ok(())
    }
}

// SAFETY: `Handle` is a union of `Uint32` and `zx_handle_t`, both of which are 4 bytes.
// It has a stable layout and no padding.
unsafe impl Wire for Handle {
    type Narrowed<'de> = Self;

    #[inline]
    fn zero_padding(_: &mut MaybeUninit<Self>) {
        // Wire handles have no padding
    }
}

impl Handle {
    /// Encodes a handle as present in an output.
    pub fn set_encoded_present(out: &mut MaybeUninit<Self>) {
        munge!(let Self { encoded } = out);
        encoded.write(wire::Uint32(ALLOC_PRESENT_U32));
    }

    /// Returns whether the underlying `zx_handle_t` is invalid.
    pub fn is_invalid(&self) -> bool {
        self.as_raw_handle() == ZX_HANDLE_INVALID
    }

    /// Returns the underlying [`zx_handle_t`].
    #[inline]
    pub fn as_raw_handle(&self) -> zx_handle_t {
        // SAFETY: `Handle` is a union of `Uint32` and `zx_handle_t`. Reading `decoded` is safe
        // because both union fields are 4-byte integers (or wrappers thereof) and do not have
        // invalid bit patterns.
        unsafe { self.decoded }
    }
}

impl fmt::Debug for Handle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.as_raw_handle().fmt(f)
    }
}

// SAFETY: If `decode` returns `Ok`, `slot` is guaranteed to contain a valid decoded `Handle`
// because it has been written with a handle taken from the decoder.
unsafe impl<D: HandleDecoder + ?Sized> Decode<D> for Handle {
    fn decode(
        mut slot: Slot<'_, Self>,
        decoder: &mut D,
        _constraint: Self::Constraint,
    ) -> Result<(), DecodeError> {
        munge!(let Self { encoded } = slot.as_mut());

        match **encoded {
            ALLOC_ABSENT_U32 => return Err(DecodeError::RequiredHandleAbsent),
            ALLOC_PRESENT_U32 => {
                let handle = decoder.take_raw_handle()?;
                munge!(let Self { mut decoded } = slot);
                decoded.write(handle);
            }
            e => return Err(DecodeError::InvalidHandlePresence(e)),
        }
        Ok(())
    }
}

/// TODO(https://fxbug.dev/465766514): remove
pub type OptionalNullableHandle = OptionalHandle;

/// An optional Zircon handle.
#[derive(Debug)]
#[repr(transparent)]
pub struct OptionalHandle {
    handle: Handle,
}

// TODO: validate handle rights
impl Constrained for OptionalHandle {
    type Constraint = ();

    fn validate(_: Slot<'_, Self>, _: Self::Constraint) -> Result<(), ValidationError> {
        Ok(())
    }
}

// SAFETY: `OptionalHandle` is a transparent wrapper around `Handle`, which is `Wire`.
unsafe impl Wire for OptionalHandle {
    type Narrowed<'de> = Self;

    #[inline]
    fn zero_padding(out: &mut MaybeUninit<Self>) {
        munge!(let Self { handle } = out);
        Handle::zero_padding(handle);
    }
}

impl OptionalHandle {
    /// Encodes a handle as present in a slot.
    pub fn set_encoded_present(out: &mut MaybeUninit<Self>) {
        munge!(let Self { handle } = out);
        Handle::set_encoded_present(handle);
    }

    /// Encodes a handle as absent in an output.
    pub fn set_encoded_absent(out: &mut MaybeUninit<Self>) {
        munge!(let Self { handle: Handle { encoded } } = out);
        encoded.write(wire::Uint32(ZX_HANDLE_INVALID));
    }

    /// Returns whether a handle is present.
    pub fn is_some(&self) -> bool {
        !self.handle.is_invalid()
    }

    /// Returns whether a handle is absent.
    pub fn is_none(&self) -> bool {
        self.handle.is_invalid()
    }

    /// Returns the underlying [`zx_handle_t`], if any.
    #[inline]
    pub fn as_raw_handle(&self) -> Option<zx_handle_t> {
        self.is_some().then(|| self.handle.as_raw_handle())
    }
}

// SAFETY: If `decode` returns `Ok`, `slot` is guaranteed to contain a valid decoded
// `OptionalHandle` because it is either left as `ALLOC_ABSENT_U32` (representing `None`) or
// written with a handle taken from the decoder.
unsafe impl<D: HandleDecoder + ?Sized> Decode<D> for OptionalHandle {
    fn decode(mut slot: Slot<'_, Self>, decoder: &mut D, _: ()) -> Result<(), DecodeError> {
        munge!(let Self { handle: mut wire_handle } = slot.as_mut());
        munge!(let Handle { encoded } = wire_handle.as_mut());

        match **encoded {
            ALLOC_ABSENT_U32 => (),
            ALLOC_PRESENT_U32 => {
                let handle = decoder.take_raw_handle()?;
                munge!(let Handle { mut decoded } = wire_handle);
                decoded.write(handle);
            }
            e => return Err(DecodeError::InvalidHandlePresence(e)),
        }
        Ok(())
    }
}

// SAFETY: `Handle` has no padding, and `encode` initializes the entire 4 bytes of `out`
// by calling `Handle::set_encoded_present`.
unsafe impl<E: HandleEncoder + ?Sized> Encode<Handle, E> for zx::NullableHandle {
    fn encode(
        self,
        encoder: &mut E,
        out: &mut MaybeUninit<Handle>,
        _constraint: (),
    ) -> Result<(), EncodeError> {
        if self.is_invalid() {
            Err(EncodeError::InvalidRequiredHandle)
        } else {
            encoder.push_handle(self)?;
            Handle::set_encoded_present(out);
            Ok(())
        }
    }
}

impl FromWire<Handle> for zx::NullableHandle {
    fn from_wire(wire: Handle) -> Self {
        // SAFETY: `WireHandle` is always a valid `NullableHandle`.
        let handle = unsafe { zx::NullableHandle::from_raw(wire.as_raw_handle()) };
        forget(wire);
        handle
    }
}

impl IntoNatural for Handle {
    type Natural = zx::NullableHandle;
}

// SAFETY: `OptionalHandle` has no padding, and `encode_option` initializes the entire 4 bytes
// of `out` by calling either `set_encoded_present` or `set_encoded_absent`.
unsafe impl<E: HandleEncoder + ?Sized> EncodeOption<OptionalHandle, E> for zx::NullableHandle {
    fn encode_option(
        this: Option<Self>,
        encoder: &mut E,
        out: &mut MaybeUninit<OptionalHandle>,
        _constraint: (),
    ) -> Result<(), EncodeError> {
        if let Some(handle) = this {
            encoder.push_handle(handle)?;
            OptionalHandle::set_encoded_present(out);
        } else {
            OptionalHandle::set_encoded_absent(out);
        }
        Ok(())
    }
}

impl FromWireOption<OptionalHandle> for zx::NullableHandle {
    fn from_wire_option(wire: OptionalHandle) -> Option<Self> {
        let raw_handle = wire.as_raw_handle();
        forget(wire);
        // SAFETY: `raw` is a valid handle value from a decoded `OptionalHandle`.
        // We `forget(wire)` above to prevent double-closing the handle.
        raw_handle.map(|raw| unsafe { zx::NullableHandle::from_raw(raw) })
    }
}

impl IntoNatural for OptionalHandle {
    type Natural = Option<zx::NullableHandle>;
}
