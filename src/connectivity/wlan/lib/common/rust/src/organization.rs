// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout, Unaligned};

// IEEE Std 802.11-2016, 9.4.1.32
// 24-bit organization unique identifier
#[repr(C, packed)]
#[derive(
    Eq,
    PartialEq,
    Hash,
    IntoBytes,
    KnownLayout,
    FromBytes,
    Immutable,
    Unaligned,
    Copy,
    Clone,
    Debug,
    PartialOrd,
    Default,
)]
pub struct Oui([u8; 3]);

impl Oui {
    pub const DOT11: Self = Self([0x00, 0x0F, 0xAC]);
    pub const MSFT: Self = Self([0x00, 0x50, 0xF2]);
    pub const WFA: Self = Self([0x50, 0x6F, 0x9A]);

    pub fn new(oui: [u8; 3]) -> Self {
        Self(oui)
    }
}

impl std::ops::Deref for Oui {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.0[..]
    }
}

impl From<Oui> for [u8; 3] {
    fn from(src: Oui) -> Self {
        src.0
    }
}
