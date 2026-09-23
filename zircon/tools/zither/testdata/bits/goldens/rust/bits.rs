// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// DO NOT EDIT.
// Generated from FIDL library `zither.bits` by zither, a Fuchsia platform tool.

use bitflags::bitflags;
use zerocopy::{FromBytes, Immutable, IntoBytes};

#[repr(C)]
#[derive(
    Immutable, IntoBytes, FromBytes, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
pub struct Uint8Bits(u8);

bitflags! {
    impl Uint8Bits : u8 {
        const ONE = 1 << 0;
        const TWO = 1 << 1;
        const FOUR = 1 << 2;
        const EIGHT = 1 << 3;
        const SIXTEEN = 1 << 4;
        const THIRTY_TWO = 1 << 5;
        const SIXTY_FOUR = 1 << 6;
        const ONE_HUNDRED_TWENTY_EIGHT = 1 << 7;
  }
}

#[repr(C)]
#[derive(
    Immutable, IntoBytes, FromBytes, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
pub struct Uint16Bits(u16);

bitflags! {
    impl Uint16Bits : u16 {
        const ZEROTH = 1 << 0;
        const FIRST = 1 << 1;
        const SECOND = 1 << 2;
        const THIRD = 1 << 3;
        const FOURTH = 1 << 4;
        const FIFTH = 1 << 5;
        const SIXTH = 1 << 6;
        const SEVENTH = 1 << 7;
        const EIGHT = 1 << 8;
        const NINTH = 1 << 9;
        const TENTH = 1 << 10;
        const ELEVENTH = 1 << 11;
        const TWELFTH = 1 << 12;
        const THIRTEENTH = 1 << 13;
        const FOURTEENTH = 1 << 14;
        const FIFTHTEENTH = 1 << 15;
  }
}

#[repr(C)]
#[derive(
    Immutable, IntoBytes, FromBytes, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
pub struct Uint32Bits(u32);

bitflags! {
    impl Uint32Bits : u32 {
        const POW_0 = 1 << 0;
        const POW_31 = 1 << 31;
  }
}

#[repr(C)]
#[derive(
    Immutable, IntoBytes, FromBytes, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
pub struct Uint64Bits(u64);

bitflags! {
    impl Uint64Bits : u64 {
        const POW_0 = 1 << 0;
        const POW_63 = 1 << 63;
  }
}

/// Bits with a one-line comment.
#[repr(C)]
#[derive(
    Immutable, IntoBytes, FromBytes, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
pub struct BitsWithOneLineComment(u8);

bitflags! {
    impl BitsWithOneLineComment : u8 {

        /// Bits member with one-line comment.
        const MEMBER_WITH_ONE_LINE_COMMENT = 1 << 0;

        /// Bits member
        ///     with a
        ///         many-line
        ///           comment.
        const MEMBER_WITH_MANY_LINE_COMMENT = 1 << 6;
  }
}

/// Bits
///
///     with a
///         many-line
///           comment.
#[repr(C)]
#[derive(
    Immutable, IntoBytes, FromBytes, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
pub struct BitsWithManyLineComment(u16);

bitflags! {
    impl BitsWithManyLineComment : u16 {
        const MEMBER = 1 << 0;
  }
}

pub const SEVENTY_TWO: Uint8Bits = Uint8Bits::from_bits_truncate(0b1001000); // Uint8Bits.SIXTY_FOUR | Uint8Bits.EIGHT

pub const SOME_BITS: Uint16Bits = Uint16Bits::from_bits_truncate(0b1001000000010); // Uint16Bits.FIRST | Uint16Bits.NINTH | Uint16Bits.TWELFTH

pub const U32_POW_0: Uint32Bits = Uint32Bits::POW_0;

pub const U64_POW_63: Uint64Bits = Uint64Bits::POW_63;
