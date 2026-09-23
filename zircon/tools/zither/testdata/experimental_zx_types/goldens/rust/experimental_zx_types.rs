// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// DO NOT EDIT.
// Generated from FIDL library `zither.experimental.zx.types` by zither, a Fuchsia platform tool.

use zerocopy::{FromBytes, Immutable, IntoBytes, TryFromBytes};

/// 'a'
pub const CHAR_CONST: u8 = 97;

pub const SIZE_CONST: usize = 100;

pub const UINTPTR_CONST: usize = 0x1234abcd5678ffff;

#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, Immutable, PartialEq, TryFromBytes)]
pub struct StructWithPrimitives {
    pub char_field: u8,
    pub size_field: usize,
    pub uintptr_field: usize,
}

pub type Uint8Alias = u8;

#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, Immutable, PartialEq, TryFromBytes)]
pub struct StructWithPointers {
    pub u64ptr: *const u64,
    pub charptr: *const u8,
    pub usizeptr: *const usize,
    pub byteptr: *const u8,
    pub voidptr: *const u8,
    pub aliasptr: *const Uint8Alias,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, FromBytes, Immutable, IntoBytes, PartialEq)]
pub struct StructWithStringArrays {
    pub str: [u8; 10],
    pub strs: [[u8; 6]; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, FromBytes, Immutable, IntoBytes, PartialEq)]
pub struct OverlayStructVariant {
    pub value: u64,
}

#[repr(u64)]
#[derive(Clone, Copy, Immutable, IntoBytes, TryFromBytes)]
pub enum OverlayWithEquallySizedVariants {
    A(u64) = 1,
    B(i64) = 2,
    C(OverlayStructVariant) = 3,
    D(u64) = 4,
}

#[repr(u64)]
#[derive(Clone, Copy, Immutable, TryFromBytes)]
pub enum OverlayWithDifferentlySizedVariants {
    A(OverlayStructVariant) = 1,
    B(u32) = 2,
    C(bool) = 3,
}

#[repr(C)]
#[derive(Clone, Copy, Immutable, TryFromBytes)]
pub struct StructWithOverlayMembers {
    pub overlay1: OverlayWithEquallySizedVariants,
    pub overlay2: OverlayWithDifferentlySizedVariants,
}
