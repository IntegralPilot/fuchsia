// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use core::hash::{BuildHasher, Hasher};

/// A 64-bit FNV-1a hasher.
#[derive(Debug, Clone, Default)]
pub struct FnvHasher64(u64);

impl FnvHasher64 {
    /// The initial offset basis constant for 64-bit FNV-1a hashing.
    pub const OFFSET_BASIS: u64 = 0xcbf29ce484222325;

    /// The 64-bit FNV prime multiplier constant for FNV-1a hashing.
    pub const PRIME: u64 = 0x100000001b3;

    /// Creates a new FNV-1a hasher initialized with the standard offset basis.
    pub const fn new() -> Self {
        Self(Self::OFFSET_BASIS)
    }
}

impl Hasher for FnvHasher64 {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.0 ^= u64::from(byte);
            self.0 = self.0.wrapping_mul(Self::PRIME);
        }
    }
}

/// A 32-bit FNV-1a hasher.
#[derive(Debug, Clone, Default)]
pub struct FnvHasher32(u32);

impl FnvHasher32 {
    /// The initial offset basis constant for 32-bit FNV-1a hashing.
    pub const OFFSET_BASIS: u32 = 0x811c9dc5;

    /// The 32-bit FNV prime multiplier constant for FNV-1a hashing.
    pub const PRIME: u32 = 0x01000193;

    /// Creates a new FNV-1a hasher initialized with the standard offset basis.
    pub const fn new() -> Self {
        Self(Self::OFFSET_BASIS)
    }
}

impl Hasher for FnvHasher32 {
    fn finish(&self) -> u64 {
        self.0.into()
    }

    fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.0 ^= u32::from(byte);
            self.0 = self.0.wrapping_mul(Self::PRIME);
        }
    }
}
impl BuildHasher for FnvHasher32 {
    type Hasher = Self;

    fn build_hasher(&self) -> Self {
        Self::default()
    }
}

impl BuildHasher for FnvHasher64 {
    type Hasher = Self;

    fn build_hasher(&self) -> Self {
        Self::default()
    }
}

#[cfg(target_pointer_width = "32")]
pub type DefaultHasher = FnvHasher32;
#[cfg(target_pointer_width = "64")]
pub type DefaultHasher = FnvHasher64;
