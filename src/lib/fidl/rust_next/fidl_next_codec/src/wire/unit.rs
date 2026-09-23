// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::fmt;
use core::mem::MaybeUninit;

use crate::{
    Constrained, Decode, DecodeError, Encode, EncodeError, EncodeOption, Encoder, EncoderExt,
    FromWire, FromWireRef, IntoNatural, Slot, ValidationError, Wire, wire,
};

/// The empty FIDL "unit" struct.
///
/// FIDL wire type layouts follow the same rules as C/C++ type layout rules.
/// Because every object must have a unique address, the empty "unit" type must
/// be be a single byte that is set to zero.
#[repr(u8)]
#[derive(Clone, Copy, Default, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub enum Unit {
    /// Empty structs are represented as a single 0u8.
    #[default]
    Unit = 0,
}

impl Constrained for Unit {
    type Constraint = ();

    fn validate(_: Slot<'_, Self>, _: Self::Constraint) -> Result<(), ValidationError> {
        Ok(())
    }
}

// SAFETY: `Unit` is a repr(u8) enum with size 1 and no padding.
unsafe impl Wire for Unit {
    type Narrowed<'de> = Self;

    #[inline]
    fn zero_padding(_: &mut MaybeUninit<Self>) {}
}

impl fmt::Debug for Unit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Unit").finish()
    }
}

impl fmt::Display for Unit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Unit")
    }
}

// SAFETY: `Unit` is decoded by reading a byte and validating it is 0.
unsafe impl<D: ?Sized> Decode<D> for Unit {
    fn decode(
        slot: Slot<'_, Self>,
        _: &mut D,
        _: Self::Constraint,
    ) -> Result<(), crate::DecodeError> {
        // SAFETY: `slot` is guaranteed to contain a valid `Unit` (1 byte).
        let value = unsafe { slot.as_ptr().cast::<u8>().read() };
        match value {
            0 => Ok(()),
            invalid => Err(DecodeError::InvalidUnit(invalid)),
        }
    }
}

// SAFETY: Encoding `()` to `Unit` writes `Unit::Unit` (0) to the output slot.
unsafe impl<E: ?Sized> Encode<Unit, E> for () {
    fn encode(self, _: &mut E, out: &mut MaybeUninit<Unit>, _: ()) -> Result<(), EncodeError> {
        let _ = out.write(Unit::Unit);
        Ok(())
    }
}

// SAFETY: Delegates to `()` implementation.
unsafe impl<E: ?Sized> Encode<Unit, E> for &() {
    fn encode(
        self,
        encoder: &mut E,
        out: &mut MaybeUninit<Unit>,
        constraint: (),
    ) -> Result<(), EncodeError> {
        Encode::encode((), encoder, out, constraint)
    }
}

// SAFETY: Encoding optional `()` delegates to `Box::encode_present` or `Box::encode_absent`.
unsafe impl<E> EncodeOption<wire::Box<'static, Unit>, E> for ()
where
    E: Encoder + ?Sized,
{
    #[inline]
    fn encode_option(
        this: Option<Self>,
        encoder: &mut E,
        out: &mut MaybeUninit<wire::Box<'static, Unit>>,
        constraint: (),
    ) -> Result<(), EncodeError> {
        if let Some(value) = this {
            encoder.encode_next_with_constraint(value, constraint)?;
            wire::Box::encode_present(out);
        } else {
            wire::Box::encode_absent(out);
        }

        Ok(())
    }
}

// SAFETY: Delegates to `()` implementation.
unsafe impl<E> EncodeOption<wire::Box<'static, Unit>, E> for &()
where
    E: Encoder + ?Sized,
{
    #[inline]
    fn encode_option(
        this: Option<Self>,
        encoder: &mut E,
        out: &mut MaybeUninit<wire::Box<'static, Unit>>,
        constraint: (),
    ) -> Result<(), EncodeError> {
        <()>::encode_option(this.cloned(), encoder, out, constraint)
    }
}

impl From<()> for Unit {
    fn from(_: ()) -> Self {
        Self::Unit
    }
}

impl<'a> From<&'a ()> for Unit {
    fn from(_: &'a ()) -> Self {
        Self::Unit
    }
}

impl From<Unit> for () {
    fn from(_: Unit) -> Self {}
}

impl<'a> From<&'a Unit> for () {
    fn from(_: &'a Unit) -> Self {}
}

impl FromWire<Unit> for () {
    fn from_wire(wire: Unit) -> Self {
        Self::from_wire_ref(&wire)
    }
}

impl FromWireRef<Unit> for () {
    fn from_wire_ref(_: &Unit) -> Self {}
}

impl IntoNatural for Unit {
    type Natural = ();
}

#[cfg(test)]
mod tests {
    use crate::{DecoderExt as _, EncoderExt as _, chunks, wire};

    #[test]
    fn decode_unit() {
        assert_eq!(
            chunks![0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]
                .as_mut_slice()
                .decode::<wire::Unit>()
                .unwrap(),
            wire::Unit::Unit,
        );
    }

    #[test]
    fn encode_unit() {
        assert_eq!(
            Vec::encode(()).unwrap(),
            chunks![0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
        );
    }
}
