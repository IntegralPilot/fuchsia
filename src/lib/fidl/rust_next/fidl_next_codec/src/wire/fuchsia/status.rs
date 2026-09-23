// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::fmt;
use core::mem::MaybeUninit;

use munge::munge;

use crate::{
    Constrained, Decode, DecodeError, Encode, EncodeError, FromWire, FromWireRef, IntoNatural,
    Slot, ValidationError, Wire, wire,
};

/// The wire type for [`Result<(), zx::Status>`].
#[derive(Clone, Copy)]
#[repr(transparent)]
pub struct StatusResult {
    inner: wire::Int32,
}

impl Constrained for StatusResult {
    type Constraint = ();

    fn validate(_: Slot<'_, Self>, _: Self::Constraint) -> Result<(), ValidationError> {
        Ok(())
    }
}

// SAFETY:
// - Lifetime erasure: `StatusResult` has no lifetimes, so `Narrowed` is `Self`.
// - Padding: `StatusResult` is transparent over `Int32`, which has no padding.
unsafe impl Wire for StatusResult {
    type Narrowed<'de> = Self;

    #[inline]
    fn zero_padding(out: &mut MaybeUninit<Self>) {
        munge!(let Self { inner } = out);
        wire::Int32::zero_padding(inner);
    }
}

impl StatusResult {
    /// Returns the raw status code.
    pub fn into_raw(self) -> i32 {
        *self.inner
    }

    /// Returns a `Result<(), zx::Status>` with the same value as this wire type.
    pub fn to_result(self) -> Result<(), zx::Status> {
        zx::Status::ok(*self.inner)
    }
}

impl From<zx::Status> for StatusResult {
    fn from(value: zx::Status) -> Self {
        Self { inner: wire::Int32(value.into_raw()) }
    }
}

impl From<Result<(), zx::Status>> for StatusResult {
    fn from(value: Result<(), zx::Status>) -> Self {
        Self { inner: wire::Int32(zx::Status::result_into_raw(value)) }
    }
}

impl fmt::Debug for StatusResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.to_result().fmt(f)
    }
}

// SAFETY: `decode` delegates to `Int32::decode`, which initializes the underlying `Int32`
// and ensures `slot` contains a valid decoded `StatusResult`.
unsafe impl<D: ?Sized> Decode<D> for StatusResult {
    fn decode(
        slot: Slot<'_, Self>,
        decoder: &mut D,
        _: Self::Constraint,
    ) -> Result<(), DecodeError> {
        munge!(let Self { inner } = slot);
        wire::Int32::decode(inner, decoder, ())
    }
}

// SAFETY: `encode` delegates to the `Encode` implementation of the raw `i32` status value,
// which initializes all non-padding bytes of `out`.
unsafe impl<E: ?Sized> Encode<StatusResult, E> for zx::Status {
    fn encode(
        self,
        encoder: &mut E,
        out: &mut MaybeUninit<StatusResult>,
        constraint: (),
    ) -> Result<(), EncodeError> {
        munge!(let StatusResult { inner } = out);
        self.into_raw().encode(encoder, inner, constraint)
    }
}

// SAFETY: `encode` delegates to `zx::Status`'s `Encode` implementation, which initializes
// all non-padding bytes of `out`.
unsafe impl<E: ?Sized> Encode<StatusResult, E> for &zx::Status {
    fn encode(
        self,
        encoder: &mut E,
        out: &mut MaybeUninit<StatusResult>,
        constraint: (),
    ) -> Result<(), EncodeError> {
        Encode::encode(*self, encoder, out, constraint)
    }
}

// SAFETY: `encode` delegates to the raw integer status encoding.
unsafe impl<E: ?Sized> Encode<StatusResult, E> for Result<(), zx::Status> {
    fn encode(
        self,
        encoder: &mut E,
        out: &mut MaybeUninit<StatusResult>,
        constraint: (),
    ) -> Result<(), EncodeError> {
        munge!(let StatusResult { inner } = out);
        zx::Status::result_into_raw(self).encode(encoder, inner, constraint)
    }
}

// SAFETY: delegates to value encoding.
unsafe impl<E: ?Sized> Encode<StatusResult, E> for &Result<(), zx::Status> {
    fn encode(
        self,
        encoder: &mut E,
        out: &mut MaybeUninit<StatusResult>,
        constraint: (),
    ) -> Result<(), EncodeError> {
        Encode::encode(*self, encoder, out, constraint)
    }
}

impl FromWire<StatusResult> for Result<(), zx::Status> {
    fn from_wire(wire: StatusResult) -> Self {
        Self::from_wire_ref(&wire)
    }
}

impl FromWireRef<StatusResult> for Result<(), zx::Status> {
    fn from_wire_ref(wire: &StatusResult) -> Self {
        zx::Status::ok(*wire.inner)
    }
}

impl IntoNatural for StatusResult {
    type Natural = Result<(), zx::Status>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CHUNK_SIZE;

    #[test]
    fn test_status_result_decode_zero() {
        let mut buffer = [0u8; CHUNK_SIZE];
        let mut decoder = ();
        // SAFETY: `buffer` is sufficiently sized and aligned for `StatusResult`.
        let mut slot = unsafe { Slot::<StatusResult>::new_unchecked(buffer.as_mut_ptr().cast()) };
        StatusResult::decode(slot.as_mut(), &mut decoder, ())
            .expect("failed to decode 0 as StatusResult");
        // SAFETY: `slot` was successfully decoded and initialized.
        let wire_result = unsafe { slot.as_ptr().cast::<StatusResult>().read() };
        assert_eq!(wire_result.to_result(), Ok(()));
        assert_eq!(
            <Result<(), zx::Status> as FromWire<StatusResult>>::from_wire(wire_result),
            Ok(())
        );
    }

    #[test]
    fn test_status_result_decode_error() {
        let mut buffer = [0u8; CHUNK_SIZE];
        let status_raw = zx::Status::NOT_SUPPORTED.into_raw();
        buffer[..4].copy_from_slice(&status_raw.to_le_bytes());

        let mut decoder = ();
        // SAFETY: `buffer` is sufficiently sized and aligned for `StatusResult`.
        let mut slot = unsafe { Slot::<StatusResult>::new_unchecked(buffer.as_mut_ptr().cast()) };
        StatusResult::decode(slot.as_mut(), &mut decoder, ())
            .expect("failed to decode error as StatusResult");
        // SAFETY: `slot` was successfully decoded and initialized.
        let wire_result = unsafe { slot.as_ptr().cast::<StatusResult>().read() };
        assert_eq!(wire_result.to_result(), Err(zx::Status::NOT_SUPPORTED));
        assert_eq!(
            <Result<(), zx::Status> as FromWire<StatusResult>>::from_wire(wire_result),
            Err(zx::Status::NOT_SUPPORTED)
        );
    }

    #[test]
    fn test_status_result_encode() {
        let mut out = MaybeUninit::<StatusResult>::uninit();
        let mut encoder = ();
        Ok::<(), zx::Status>(()).encode(&mut encoder, &mut out, ()).unwrap();
        // SAFETY: `encode` succeeded, so `out` is initialized.
        let encoded = unsafe { out.assume_init() };
        assert_eq!(encoded.into_raw(), 0);

        let mut out = MaybeUninit::<StatusResult>::uninit();
        let result: Result<(), zx::Status> = Err(zx::Status::NOT_FOUND);
        result.encode(&mut encoder, &mut out, ()).unwrap();
        // SAFETY: `encode` succeeded, so `out` is initialized.
        let encoded = unsafe { out.assume_init() };
        assert_eq!(encoded.into_raw(), zx::Status::NOT_FOUND.into_raw());
    }
}
