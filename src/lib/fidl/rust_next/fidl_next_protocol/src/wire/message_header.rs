// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::mem::MaybeUninit;

use fidl_constants::MAGIC_NUMBER_INITIAL;
use fidl_next_codec::{
    Constrained, Decode, DecodeError, Encode, EncodeError, Slot, ValidationError, Wire, bitflags,
    wire,
};
use zerocopy::IntoBytes;

use crate::Flexibility;

/// The transactional message header flags in byte 0.
#[derive(Clone, Copy, Debug, IntoBytes)]
#[repr(transparent)]
pub struct MessageHeaderFlags0(u8);

/// The transactional message header flags in byte 1.
#[derive(Clone, Copy, Debug, IntoBytes)]
#[repr(transparent)]
pub struct MessageHeaderFlags1(u8);

/// The transactional message header flags in byte 2.
#[derive(Clone, Copy, Debug, IntoBytes)]
#[repr(transparent)]
pub struct MessageHeaderFlags2(u8);

bitflags::bitflags! {
    impl MessageHeaderFlags0: u8 {
        /// The bit set to indicate that the FIDL wire format is version 2.
        const WIRE_FORMAT_V2 = 1 << 1;
    }

    impl MessageHeaderFlags1: u8 {
    }

    impl MessageHeaderFlags2: u8 {
        /// The bit set to indicate that the FIDL method is flexible.
        const FLEXIBLE_METHOD = 1 << 7;
    }
}

/// A FIDL protocol message header
#[derive(Clone, Copy, Debug, IntoBytes)]
#[repr(C)]
pub struct MessageHeader {
    /// The transaction ID of the message header
    pub txid: wire::Uint32,
    /// Flags byte 0
    pub flags_0: MessageHeaderFlags0,
    /// Flags byte 1
    pub flags_1: MessageHeaderFlags1,
    /// Flags byte 2
    pub flags_2: MessageHeaderFlags2,
    /// Magic number
    pub magic_number: u8,
    /// The ordinal of the message following this header
    pub ordinal: wire::Uint64,
}

impl MessageHeader {
    /// Returns a new message header with the given transaction ID, ordinal, and
    /// flexibility.
    pub fn new(txid: u32, ordinal: u64, flexibility: Flexibility) -> Self {
        Self {
            txid: wire::Uint32(txid),
            flags_0: MessageHeaderFlags0::WIRE_FORMAT_V2,
            flags_1: MessageHeaderFlags1::empty(),
            flags_2: match flexibility {
                Flexibility::Strict => MessageHeaderFlags2::empty(),
                Flexibility::Flexible => MessageHeaderFlags2::FLEXIBLE_METHOD,
            },
            magic_number: MAGIC_NUMBER_INITIAL,
            ordinal: wire::Uint64(ordinal),
        }
    }

    /// Returns the flexibility of the message header.
    pub fn flexibility(&self) -> Flexibility {
        if self.flags_2.contains(MessageHeaderFlags2::FLEXIBLE_METHOD) {
            Flexibility::Flexible
        } else {
            Flexibility::Strict
        }
    }
}

impl Constrained for MessageHeader {
    type Constraint = ();

    fn validate(_: Slot<'_, Self>, _: Self::Constraint) -> Result<(), ValidationError> {
        Ok(())
    }
}

unsafe impl Wire for MessageHeader {
    type Narrowed<'de> = Self;

    #[inline]
    fn zero_padding(_: &mut MaybeUninit<Self>) {
        // Wire message headers have no padding
    }
}

unsafe impl<E: ?Sized> Encode<MessageHeader, E> for MessageHeader {
    #[inline]
    fn encode(
        self,
        _: &mut E,
        out: &mut MaybeUninit<MessageHeader>,
        _: (),
    ) -> Result<(), EncodeError> {
        out.write(self);
        Ok(())
    }
}

unsafe impl<E: ?Sized> Encode<MessageHeader, E> for &MessageHeader {
    #[inline]
    fn encode(
        self,
        encoder: &mut E,
        out: &mut MaybeUninit<MessageHeader>,
        constraint: (),
    ) -> Result<(), EncodeError> {
        Encode::encode(*self, encoder, out, constraint)
    }
}

unsafe impl<D: ?Sized> Decode<D> for MessageHeader {
    #[inline]
    fn decode(_: Slot<'_, Self>, _: &mut D, _: ()) -> Result<(), DecodeError> {
        Ok(())
    }
}
