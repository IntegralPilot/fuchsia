// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::constants;
use std::fmt;
use std::ops::{Add, AddAssign, BitXor, Deref, Sub, SubAssign};

#[derive(Debug, Copy, Clone, Ord, PartialOrd, PartialEq, Eq, Hash)]
pub struct BlockIndex(u32);

impl BlockIndex {
    pub const ROOT: Self = Self::new(0);
    pub const HEADER: Self = Self::new(0);
    pub const EMPTY: Self = Self::new(0);

    pub const fn new(idx: u32) -> Self {
        Self(idx)
    }

    /// Get index in the VMO for a given |offset|.
    pub fn from_offset(offset: usize) -> BlockIndex {
        // Safety: `offset` can't be larger than the largest `BlockIndex`
        Self::new(u32::try_from(offset / constants::MIN_ORDER_SIZE).unwrap())
    }

    /// Get offset in the VMO for |self|.
    pub fn offset(&self) -> usize {
        usize::from(self) * constants::MIN_ORDER_SIZE
    }
}

impl From<u32> for BlockIndex {
    fn from(idx: u32) -> BlockIndex {
        BlockIndex::new(idx)
    }
}

impl From<&u32> for BlockIndex {
    fn from(idx: &u32) -> BlockIndex {
        BlockIndex::new(*idx)
    }
}

impl Deref for BlockIndex {
    type Target = u32;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

// Panics if a u32 is larger than a usize
impl From<BlockIndex> for usize {
    fn from(idx: BlockIndex) -> usize {
        // usize on Fuchsia is 64 bits
        *idx as usize
    }
}

// Panics if a u32 is larger than a usize
impl From<&BlockIndex> for usize {
    fn from(idx: &BlockIndex) -> usize {
        // usize on Fuchsia is 64 bits
        **idx as usize
    }
}

impl fmt::Display for BlockIndex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

// Basically pointer arithmetic for BlockIndex, which is somewhat sad
// but pretty useful conceptually
impl Add<u32> for BlockIndex {
    type Output = Self;

    fn add(self, other: u32) -> Self {
        BlockIndex::new(self.0 + other)
    }
}

impl AddAssign<u32> for BlockIndex {
    fn add_assign(&mut self, other: u32) {
        self.0 += other
    }
}

impl Sub<u32> for BlockIndex {
    type Output = Self;

    fn sub(self, other: u32) -> Self {
        BlockIndex::new(self.0 - other)
    }
}

impl SubAssign<u32> for BlockIndex {
    fn sub_assign(&mut self, other: u32) {
        self.0 -= other
    }
}

impl Add for BlockIndex {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        BlockIndex::new(self.0 + other.0)
    }
}

impl AddAssign for BlockIndex {
    fn add_assign(&mut self, other: Self) {
        self.0 += other.0
    }
}

impl Sub for BlockIndex {
    type Output = Self;

    fn sub(self, other: Self) -> Self {
        BlockIndex::new(self.0 - other.0)
    }
}

impl SubAssign for BlockIndex {
    fn sub_assign(&mut self, other: Self) {
        self.0 -= other.0
    }
}

impl BitXor for BlockIndex {
    type Output = Self;

    fn bitxor(self, rhs: Self) -> Self::Output {
        Self(self.0 ^ rhs.0)
    }
}
