// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// DO NOT EDIT.
// Generated from FIDL library `zither.enums` by zither, a Fuchsia platform tool.

use zerocopy::{Immutable, IntoBytes, TryFromBytes};

#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, Hash, Immutable, IntoBytes, PartialEq, TryFromBytes)]
pub enum Color {
    Red = 0,
    Orange = 1,
    Yellow = 2,
    Green = 3,
    Blue = 4,
    Indigo = 5,
    Violet = 6,
}

impl Color {
    pub fn from_raw(raw: u8) -> Option<Self> {
        match raw {
            0 => Some(Self::Red),

            1 => Some(Self::Orange),

            2 => Some(Self::Yellow),

            3 => Some(Self::Green),

            4 => Some(Self::Blue),

            5 => Some(Self::Indigo),

            6 => Some(Self::Violet),

            _ => None,
        }
    }
}

impl From<Color> for u8 {
    fn from(val: Color) -> Self {
        val as Self
    }
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, Hash, Immutable, IntoBytes, PartialEq, TryFromBytes)]
pub enum Uint8Limits {
    Min = 0,
    Max = 0b11111111,
}

impl Uint8Limits {
    pub fn from_raw(raw: u8) -> Option<Self> {
        match raw {
            0 => Some(Self::Min),

            0b11111111 => Some(Self::Max),

            _ => None,
        }
    }
}

impl From<Uint8Limits> for u8 {
    fn from(val: Uint8Limits) -> Self {
        val as Self
    }
}

#[repr(u16)]
#[derive(Clone, Copy, Debug, Eq, Hash, Immutable, IntoBytes, PartialEq, TryFromBytes)]
pub enum Uint16Limits {
    Min = 0,
    Max = 0xffff,
}

impl Uint16Limits {
    pub fn from_raw(raw: u16) -> Option<Self> {
        match raw {
            0 => Some(Self::Min),

            0xffff => Some(Self::Max),

            _ => None,
        }
    }
}

impl From<Uint16Limits> for u16 {
    fn from(val: Uint16Limits) -> Self {
        val as Self
    }
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, Hash, Immutable, IntoBytes, PartialEq, TryFromBytes)]
pub enum Uint32Limits {
    Min = 0,
    Max = 0xffffffff,
}

impl Uint32Limits {
    pub fn from_raw(raw: u32) -> Option<Self> {
        match raw {
            0 => Some(Self::Min),

            0xffffffff => Some(Self::Max),

            _ => None,
        }
    }
}

impl From<Uint32Limits> for u32 {
    fn from(val: Uint32Limits) -> Self {
        val as Self
    }
}

#[repr(u64)]
#[derive(Clone, Copy, Debug, Eq, Hash, Immutable, IntoBytes, PartialEq, TryFromBytes)]
pub enum Uint64Limits {
    Min = 0,
    Max = 0xffffffffffffffff,
}

impl Uint64Limits {
    pub fn from_raw(raw: u64) -> Option<Self> {
        match raw {
            0 => Some(Self::Min),

            0xffffffffffffffff => Some(Self::Max),

            _ => None,
        }
    }
}

impl From<Uint64Limits> for u64 {
    fn from(val: Uint64Limits) -> Self {
        val as Self
    }
}

#[repr(i8)]
#[derive(Clone, Copy, Debug, Eq, Hash, Immutable, IntoBytes, PartialEq, TryFromBytes)]
pub enum Int8Limits {
    Min = -0x80,
    Max = 0x7f,
}

impl Int8Limits {
    pub fn from_raw(raw: i8) -> Option<Self> {
        match raw {
            -0x80 => Some(Self::Min),

            0x7f => Some(Self::Max),

            _ => None,
        }
    }
}

impl From<Int8Limits> for i8 {
    fn from(val: Int8Limits) -> Self {
        val as Self
    }
}

#[repr(i16)]
#[derive(Clone, Copy, Debug, Eq, Hash, Immutable, IntoBytes, PartialEq, TryFromBytes)]
pub enum Int16Limits {
    Min = -0x8000,
    Max = 0x7fff,
}

impl Int16Limits {
    pub fn from_raw(raw: i16) -> Option<Self> {
        match raw {
            -0x8000 => Some(Self::Min),

            0x7fff => Some(Self::Max),

            _ => None,
        }
    }
}

impl From<Int16Limits> for i16 {
    fn from(val: Int16Limits) -> Self {
        val as Self
    }
}

#[repr(i32)]
#[derive(Clone, Copy, Debug, Eq, Hash, Immutable, IntoBytes, PartialEq, TryFromBytes)]
pub enum Int32Limits {
    Min = -0x80000000,
    Max = 0x7fffffff,
}

impl Int32Limits {
    pub fn from_raw(raw: i32) -> Option<Self> {
        match raw {
            -0x80000000 => Some(Self::Min),

            0x7fffffff => Some(Self::Max),

            _ => None,
        }
    }
}

impl From<Int32Limits> for i32 {
    fn from(val: Int32Limits) -> Self {
        val as Self
    }
}

#[repr(i64)]
#[derive(Clone, Copy, Debug, Eq, Hash, Immutable, IntoBytes, PartialEq, TryFromBytes)]
pub enum Int64Limits {
    Min = -0x8000000000000000,
    Max = 0x7fffffffffffffff,
}

impl Int64Limits {
    pub fn from_raw(raw: i64) -> Option<Self> {
        match raw {
            -0x8000000000000000 => Some(Self::Min),

            0x7fffffffffffffff => Some(Self::Max),

            _ => None,
        }
    }
}

impl From<Int64Limits> for i64 {
    fn from(val: Int64Limits) -> Self {
        val as Self
    }
}

pub const FOUR: u16 = 0b100;

#[repr(u16)]
#[derive(Clone, Copy, Debug, Eq, Hash, Immutable, IntoBytes, PartialEq, TryFromBytes)]
pub enum EnumWithExpressions {
    OrWithLiteral = 3,  // 0b01 | 0b10
    OrWithConstant = 5, // 0b001 | FOUR
}

impl EnumWithExpressions {
    pub fn from_raw(raw: u16) -> Option<Self> {
        match raw {
            3 => Some(Self::OrWithLiteral),

            5 => Some(Self::OrWithConstant),

            _ => None,
        }
    }
}

impl From<EnumWithExpressions> for u16 {
    fn from(val: EnumWithExpressions) -> Self {
        val as Self
    }
}

/// Enum with a one-line comment.
#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, Hash, Immutable, IntoBytes, PartialEq, TryFromBytes)]
pub enum EnumWithOneLineComment {
    /// Enum member with one-line comment.
    MemberWithOneLineComment = 0,

    /// Enum member
    ///     with a
    ///         many-line
    ///           comment.
    MemberWithManyLineComment = 1,
}

impl EnumWithOneLineComment {
    pub fn from_raw(raw: u8) -> Option<Self> {
        match raw {
            0 => Some(Self::MemberWithOneLineComment),

            1 => Some(Self::MemberWithManyLineComment),

            _ => None,
        }
    }
}

impl From<EnumWithOneLineComment> for u8 {
    fn from(val: EnumWithOneLineComment) -> Self {
        val as Self
    }
}

/// Enum
///
///     with a
///         many-line
///           comment.
#[repr(u16)]
#[derive(Clone, Copy, Debug, Eq, Hash, Immutable, IntoBytes, PartialEq, TryFromBytes)]
pub enum EnumWithManyLineComment {
    Member = 0,
}

impl EnumWithManyLineComment {
    pub fn from_raw(raw: u16) -> Option<Self> {
        match raw {
            0 => Some(Self::Member),

            _ => None,
        }
    }
}

impl From<EnumWithManyLineComment> for u16 {
    fn from(val: EnumWithManyLineComment) -> Self {
        val as Self
    }
}

pub const RED: Color = Color::Red;

pub const UINT8_MIN: Uint8Limits = Uint8Limits::Min;

pub const UINT8_MAX: Uint8Limits = Uint8Limits::Max;

pub const UINT16_MIN: Uint16Limits = Uint16Limits::Min;

pub const UINT16_MAX: Uint16Limits = Uint16Limits::Max;

pub const UINT32_MIN: Uint32Limits = Uint32Limits::Min;

pub const UINT32_MAX: Uint32Limits = Uint32Limits::Max;

pub const UINT64_MIN: Uint64Limits = Uint64Limits::Min;

pub const UINT64_MAX: Uint64Limits = Uint64Limits::Max;

pub const INT8_MIN: Int8Limits = Int8Limits::Min;

pub const INT8_MAX: Int8Limits = Int8Limits::Max;

pub const INT16_MIN: Int16Limits = Int16Limits::Min;

pub const INT16_MAX: Int16Limits = Int16Limits::Max;

pub const INT32_MIN: Int32Limits = Int32Limits::Min;

pub const INT32_MAX: Int32Limits = Int32Limits::Max;

pub const INT64_MIN: Int64Limits = Int64Limits::Min;

pub const INT64_MAX: Int64Limits = Int64Limits::Max;
