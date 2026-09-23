// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Driver-specific extensions to FIDL.

use core::fmt;
use core::mem::{MaybeUninit, forget};
use core::num::NonZero;

use fdf_channel::channel::Channel;
use fdf_core::handle::{DriverHandle, fdf_handle_t};
use fidl_next::fuchsia::{HandleDecoder, HandleEncoder};
use fidl_next::{
    Constrained, Decode, DecodeError, Encode, EncodeError, EncodeOption, FromWire, FromWireOption,
    IntoNatural, Slot, ValidationError, Wire, munge, wire,
};

/// The FIDL wire type for [`DriverChannel`].
///
/// This type follows the FIDL wire format for handles, and is separate from the
/// Zircon handle wire type. This ensures that we never confuse the two types
/// when using FIDL.
#[repr(C, align(4))]
pub union DriverChannel {
    encoded: wire::Uint32,
    decoded: fdf_handle_t,
}

impl Drop for DriverChannel {
    fn drop(&mut self) {
        // SAFETY: `WireDriverHandle` is always non-zero.
        let raw_handle = unsafe { NonZero::new_unchecked(self.as_raw_handle()) };
        // SAFETY: `WireDriverHandle` is always a valid `DriverHandle`.
        let handle = unsafe { DriverHandle::new_unchecked(raw_handle) };
        drop(handle);
    }
}

impl Constrained for DriverChannel {
    type Constraint = ();

    fn validate(_: Slot<'_, Self>, _: Self::Constraint) -> Result<(), ValidationError> {
        Ok(())
    }
}

// SAFETY:
// - `WireDriverHandle` doesn't reference any other decoded data.
// - `WireDriverHandle` does not have any padding bytes.
unsafe impl Wire for DriverChannel {
    type Narrowed<'de> = Self;

    #[inline]
    fn zero_padding(_: &mut MaybeUninit<Self>) {
        // Wire driver handles have no padding
    }
}

impl DriverChannel {
    /// Encodes a driver handle as present in an output.
    pub fn set_encoded_present(out: &mut MaybeUninit<Self>) {
        munge!(let Self { encoded } = out);
        encoded.write(wire::Uint32(u32::MAX));
    }

    /// Returns the underlying [`fdf_handle_t`].
    #[inline]
    pub fn as_raw_handle(&self) -> fdf_handle_t {
        // SAFETY: If we have a reference to `WireDriverHandle`, then it has
        // been successfully decoded and the `decoded` field is safe to read.
        unsafe { self.decoded }
    }
}

impl fmt::Debug for DriverChannel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.as_raw_handle().fmt(f)
    }
}

// SAFETY: `decode` only returns `Ok` if it wrote to the `decoded` field of the
// handle, initializing it.
unsafe impl<D: HandleDecoder + ?Sized> Decode<D> for DriverChannel {
    fn decode(
        mut slot: Slot<'_, Self>,
        decoder: &mut D,
        _: <Self as Constrained>::Constraint,
    ) -> Result<(), DecodeError> {
        munge!(let Self { encoded } = slot.as_mut());

        match **encoded {
            u32::MAX => {
                let handle = decoder.take_raw_driver_handle()?;
                munge!(let Self { mut decoded } = slot);
                decoded.write(handle);
            }
            e => return Err(DecodeError::InvalidHandlePresence(e)),
        }
        Ok(())
    }
}

/// The FIDL wire type for optional [`DriverChannel`]s.
///
/// This type follows the FIDL wire format for handles, and is separate from the
/// Zircon handle optional wire type. This ensures that we never confuse the two
/// types when using FIDL.
#[repr(C, align(4))]
pub union OptionalDriverChannel {
    encoded: wire::Uint32,
    decoded: fdf_handle_t,
}

impl Drop for OptionalDriverChannel {
    fn drop(&mut self) {
        if let Some(handle) = self.as_raw_handle() {
            // SAFETY: If the return value from `as_raw_handle` is `Some`, then
            // it is always non-zero.
            let handle = unsafe { NonZero::new_unchecked(handle) };
            // SAFETY: `WireDriverHandle` is always a valid `DriverHandle`.
            let handle = unsafe { DriverHandle::new_unchecked(handle) };
            drop(handle);
        }
    }
}

impl Constrained for OptionalDriverChannel {
    type Constraint = ();

    fn validate(_: Slot<'_, Self>, _: Self::Constraint) -> Result<(), ValidationError> {
        Ok(())
    }
}

// SAFETY:
// - `WireOptionalDriverHandle` doesn't reference any other decoded data.
// - `WireOptionalDriverHandle` does not have any padding bytes.
unsafe impl Wire for OptionalDriverChannel {
    type Narrowed<'de> = Self;

    #[inline]
    fn zero_padding(_: &mut MaybeUninit<Self>) {
        // Wire optional driver handles have no padding
    }
}

impl OptionalDriverChannel {
    /// Encodes a driver handle as present in a slot.
    pub fn set_encoded_present(out: &mut MaybeUninit<Self>) {
        munge!(let Self { encoded } = out);
        encoded.write(wire::Uint32(u32::MAX));
    }

    /// Encodes a driver handle as absent in an output.
    pub fn set_encoded_absent(out: &mut MaybeUninit<Self>) {
        munge!(let Self { encoded } = out);
        encoded.write(wire::Uint32(0));
    }

    /// Returns whether a handle is present.
    pub fn is_some(&self) -> bool {
        self.as_raw_handle().is_some()
    }

    /// Returns whether a handle is absent.
    pub fn is_none(&self) -> bool {
        self.as_raw_handle().is_none()
    }

    /// Returns the underlying [`fdf_handle_t`], if any.
    #[inline]
    pub fn as_raw_handle(&self) -> Option<fdf_handle_t> {
        // SAFETY: If we have a reference to `WireDriverHandle`, then it has
        // been successfully decoded and the `decoded` field is safe to read.
        let decoded = unsafe { self.decoded };
        if decoded == 0 { None } else { Some(decoded) }
    }
}

// SAFETY: `decode` only returns `Ok` if either:
// - It wrote to the `decoded` field of the handle, initializing it.
// - The handle's encoded (and decoded) value was zero, indicating `None`.
unsafe impl<D: HandleDecoder + ?Sized> Decode<D> for OptionalDriverChannel {
    fn decode(
        mut slot: Slot<'_, Self>,
        decoder: &mut D,
        _: <Self as Constrained>::Constraint,
    ) -> Result<(), DecodeError> {
        munge!(let Self { encoded } = slot.as_mut());

        match **encoded {
            0 => (),
            u32::MAX => {
                let handle = decoder.take_raw_driver_handle()?;
                munge!(let Self { mut decoded } = slot);
                decoded.write(handle);
            }
            e => return Err(DecodeError::InvalidHandlePresence(e)),
        }
        Ok(())
    }
}

// SAFETY: `encode` calls `set_encoded_present`, which initializes all of the
// bytes of `out`.
unsafe impl<E: HandleEncoder + ?Sized> Encode<DriverChannel, E> for crate::DriverChannel {
    fn encode(
        self,
        encoder: &mut E,
        out: &mut MaybeUninit<DriverChannel>,
        _: (),
    ) -> Result<(), EncodeError> {
        let handle = self.channel.into_driver_handle();
        // SAFETY: `self.into_raw()` returns a valid driver handle.
        unsafe {
            encoder.push_raw_driver_handle(handle.into_raw().get())?;
        }
        DriverChannel::set_encoded_present(out);
        Ok(())
    }
}

impl FromWire<DriverChannel> for crate::DriverChannel {
    fn from_wire(wire: DriverChannel) -> Self {
        // SAFETY: `WireDriverHandle` is always non-zero.
        let raw_handle = unsafe { NonZero::new_unchecked(wire.as_raw_handle()) };
        // SAFETY: `WireDriverHandle` is always a valid `Handle`.
        let handle = unsafe { DriverHandle::new_unchecked(raw_handle) };
        // SAFETY: `WireDriverHandle` is always a valid `Channel`.
        let channel = unsafe { Channel::from_driver_handle(handle) };
        forget(wire);
        crate::DriverChannel::new(channel)
    }
}

impl IntoNatural for DriverChannel {
    type Natural = crate::DriverChannel;
}

// SAFETY: `encode_option` calls either `set_encoded_present` or
// `set_encoded_absent`, both of which initializes all of the bytes of `out`.
unsafe impl<E: HandleEncoder + ?Sized> EncodeOption<OptionalDriverChannel, E>
    for crate::DriverChannel
{
    fn encode_option(
        this: Option<Self>,
        encoder: &mut E,
        out: &mut MaybeUninit<OptionalDriverChannel>,
        _: (),
    ) -> Result<(), EncodeError> {
        if let Some(driver_channel) = this {
            let handle = driver_channel.channel.into_driver_handle();
            // SAFETY: `self.into_raw()` returns a valid driver handle.
            unsafe {
                encoder.push_raw_driver_handle(handle.into_raw().get())?;
            }
            OptionalDriverChannel::set_encoded_present(out);
        } else {
            OptionalDriverChannel::set_encoded_absent(out);
        }
        Ok(())
    }
}

impl FromWireOption<OptionalDriverChannel> for crate::DriverChannel {
    fn from_wire_option(wire: OptionalDriverChannel) -> Option<Self> {
        let raw_handle = wire.as_raw_handle();
        forget(wire);
        raw_handle.map(|raw| {
            // SAFETY: `WireDriverHandle::as_raw_handle()` only returns `Some`
            // with a non-zero raw handle.
            let raw_handle = unsafe { NonZero::new_unchecked(raw) };
            // SAFETY: `wire` previously owned the valid driver handle. It has
            // been forgotten, passing ownership to the returned `DriverHandle`.
            let handle = unsafe { DriverHandle::new_unchecked(raw_handle) };
            // SAFETY: `WireOptionalDriverChannel` is always a valid `Channel`.
            let channel = unsafe { Channel::from_driver_handle(handle) };
            crate::DriverChannel::new(channel)
        })
    }
}

impl IntoNatural for OptionalDriverChannel {
    type Natural = Option<crate::DriverChannel>;
}

#[cfg(test)]
mod tests {
    use fdf_channel::arena::Arena;
    use fdf_channel::message::Message;
    use fdf_core::handle::MixedHandleType;
    use fidl_next::{AsDecoderExt as _, Chunk, EncoderExt as _, chunks};

    use crate::{RecvBuffer, SendBuffer};

    use super::*;

    #[test]
    fn roundtrip() {
        let (channel, _) = Channel::<[Chunk]>::create();
        // SAFETY: this handle won't be used as a driver handle.
        let handle_raw = unsafe { channel.driver_handle().get_raw() };
        let driver_channel = crate::DriverChannel::new(channel);

        let encoder = SendBuffer::encode(driver_channel).unwrap();

        assert_eq!(encoder.handles.len(), 1);
        let driver_ref = encoder.handles[0].as_ref().unwrap().resolve_ref();
        let MixedHandleType::Driver(handle) = &driver_ref else {
            panic!("expected a driver handle");
        };
        assert_eq!(unsafe { handle.get_raw() }, handle_raw);
        assert_eq!(encoder.data, chunks![0xff, 0xff, 0xff, 0xff, 0x00, 0x00, 0x00, 0x00],);

        let arena = Arena::new();
        let data = arena.insert_boxed_slice(encoder.data.into_boxed_slice());
        let handles = arena.insert_boxed_slice(encoder.handles.into_boxed_slice());
        let buffer = Some(Message::new(&arena, Some(data), Some(handles)));
        let decoder = RecvBuffer { message: buffer };

        let decoded = decoder.into_decoded::<DriverChannel>().unwrap();
        assert_eq!(decoded.as_raw_handle(), handle_raw.get());

        let handle = decoded.take();
        let roundtripped_raw = unsafe { handle.channel.driver_handle().get_raw() };
        assert_eq!(roundtripped_raw, handle_raw);
    }

    #[test]
    fn roundtrip_some() {
        let (channel, _) = Channel::<[Chunk]>::create();
        // SAFETY: this handle won't be used as a driver handle.
        let handle_raw = unsafe { channel.driver_handle().get_raw() };
        let driver_channel = crate::DriverChannel::new(channel);

        let encoder = SendBuffer::encode(Some(driver_channel)).unwrap();

        assert_eq!(encoder.handles.len(), 1);
        let driver_ref = encoder.handles[0].as_ref().unwrap().resolve_ref();
        let MixedHandleType::Driver(handle) = &driver_ref else {
            panic!("expected a driver handle");
        };
        assert_eq!(unsafe { handle.get_raw() }, handle_raw);
        assert_eq!(encoder.data, chunks![0xff, 0xff, 0xff, 0xff, 0x00, 0x00, 0x00, 0x00],);

        let arena = Arena::new();
        let data = arena.insert_boxed_slice(encoder.data.into_boxed_slice());
        let handles = arena.insert_boxed_slice(encoder.handles.into_boxed_slice());
        let buffer = Some(Message::new(&arena, Some(data), Some(handles)));
        let decoder = RecvBuffer { message: buffer };

        let decoded = decoder.into_decoded::<OptionalDriverChannel>().unwrap();
        assert_eq!(decoded.as_raw_handle(), Some(handle_raw.get()));

        let handle = decoded.take();
        let roundtripped_raw = unsafe { handle.unwrap().channel.driver_handle().get_raw() };
        assert_eq!(roundtripped_raw, handle_raw);
    }

    #[test]
    fn roundtrip_none() {
        let encoder = SendBuffer::encode(None::<crate::DriverChannel>).unwrap();

        assert_eq!(encoder.handles.len(), 0);
        assert_eq!(encoder.data, chunks![0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],);

        let arena = Arena::new();
        let data = arena.insert_boxed_slice(encoder.data.into_boxed_slice());
        let handles = arena.insert_boxed_slice(encoder.handles.into_boxed_slice());
        let buffer = Some(Message::new(&arena, Some(data), Some(handles)));
        let decoder = RecvBuffer { message: buffer };

        let decoded = decoder.into_decoded::<OptionalDriverChannel>().unwrap();
        assert_eq!(decoded.as_raw_handle(), None);

        let handle = decoded.take();
        assert!(handle.is_none());
    }
}
