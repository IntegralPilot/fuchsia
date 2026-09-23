// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// DO NOT EDIT.
// Generated from FIDL library `zither.aliases` by zither, a Fuchsia platform tool.

use zerocopy::{Immutable, IntoBytes, TryFromBytes};

pub type BoolAlias = bool;

pub type Int8Alias = i8;

pub type Int16Alias = i16;

pub type Int32Alias = i32;

pub type Int64Alias = i64;

pub type Uint8Alias = u8;

pub type Uint16Alias = u16;

pub type Uint32Alias = u32;

pub type Uint64Alias = u64;

/// TODO(https://fxbug.dev/42057022): The IR currently does not propagate enough
/// information for bindings to express this type as an alias.
pub const CONST_FROM_ALIAS: u8 = 0xff;

#[repr(i16)]
#[derive(Clone, Copy, Debug, Eq, Hash, Immutable, IntoBytes, PartialEq, TryFromBytes)]
pub enum Enum {
    Member = 0,
}

impl Enum {
    pub fn from_raw(raw: i16) -> Option<Self> {
        match raw {
            0 => Some(Self::Member),

            _ => None,
        }
    }
}

impl From<Enum> for i16 {
    fn from(val: Enum) -> Self {
        val as Self
    }
}

pub type EnumAlias = Enum;

#[repr(u16)]
#[derive(Clone, Copy, Debug, Eq, Hash, Immutable, IntoBytes, PartialEq, TryFromBytes)]
pub enum Bits {
    One = 1,
}

impl Bits {
    pub fn from_raw(raw: u16) -> Option<Self> {
        match raw {
            1 => Some(Self::One),

            _ => None,
        }
    }
}

impl From<Bits> for u16 {
    fn from(val: Bits) -> Self {
        val as Self
    }
}

pub type BitsAlias = Bits;

#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, Immutable, PartialEq, TryFromBytes)]
pub struct Struct {
    pub x: u64,
    pub y: u64,
    pub e: EnumAlias,
}

pub type StructAlias = Struct;

pub type ArrayAlias = [u32; 4];

pub type NestedArrayAlias = [[Struct; 8]; 4];

/// Alias with a one-line comment.
pub type AliasWithOneLineComment = bool;

/// Alias
///     with
///         a
///           many-line
///             comment.
pub type AliasWithManyLineComment = u8;
