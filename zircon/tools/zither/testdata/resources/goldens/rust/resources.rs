// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// DO NOT EDIT.
// Generated from FIDL library `zither.resources` by zither, a Fuchsia platform tool.

use zerocopy::{FromBytes, Immutable, IntoBytes, TryFromBytes};

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, Hash, Immutable, IntoBytes, PartialEq, TryFromBytes)]
pub enum Subtype {
    A = 0,
    B = 1,
}

impl Subtype {
    pub fn from_raw(raw: u32) -> Option<Self> {
        match raw {
            0 => Some(Self::A),

            1 => Some(Self::B),

            _ => None,
        }
    }
}

impl From<Subtype> for u32 {
    fn from(val: Subtype) -> Self {
        val as Self
    }
}

/// This is a handle.
pub type Handle = u32;

#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, FromBytes, Immutable, IntoBytes, PartialEq)]
pub struct StructWithHandleMembers {
    pub untyped_handle: Handle,
    pub handle_a: Handle,
    pub handle_b: Handle,
}
