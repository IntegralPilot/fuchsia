// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use bitflags::bitflags;

mod fifo;

pub use fifo::*;

bitflags! {
    /// Options that may be used for writes.
    #[repr(transparent)]
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct WriteFlags: u32 {
        const FORCE_ACCESS = 1;
        const PRE_BARRIER = 2;
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WriteOptions {
    pub inline_crypto: InlineCryptoOptions,
    pub flags: WriteFlags,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReadOptions {
    pub inline_crypto: InlineCryptoOptions,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InlineCryptoOptions {
    pub is_enabled: bool,
    pub slot: u8,
    pub dun: u64,
}

impl InlineCryptoOptions {
    pub fn enabled(slot: u8, dun: u64) -> Self {
        Self { is_enabled: true, slot, dun }
    }
}
