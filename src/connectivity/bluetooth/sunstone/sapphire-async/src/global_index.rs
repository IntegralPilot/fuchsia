// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

/// A monotonically increasing logical offset index identifying a unique enqueued item or request.
///
/// Implements wrapping modular arithmetic to cleanly manage integer overflows on `usize::MAX`.
/// Comparisons (`<`, `>`, etc.) are defined using signed differences, assuming the absolute
/// logical distance between any two active indices never exceeds `isize::MAX`.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct GlobalIndex(usize);

impl GlobalIndex {
    /// Creates a new `GlobalIndex` with the given raw value.
    pub const fn new(val: usize) -> Self {
        Self(val)
    }
}

impl core::ops::Sub for GlobalIndex {
    type Output = isize;

    /// Subtracts `rhs` from `self`, returning the signed offset distance.
    ///
    /// Uses wrapping subtraction casted to `isize`. Under signed modular arithmetic,
    /// a negative difference indicates that `rhs` is logically ahead of `self` (in the future)
    /// or that the index was already reclaimed in the past.
    fn sub(self, rhs: Self) -> Self::Output {
        self.0.wrapping_sub(rhs.0) as isize
    }
}

impl core::ops::Add<usize> for GlobalIndex {
    type Output = Self;
    fn add(self, rhs: usize) -> Self::Output {
        GlobalIndex(self.0.wrapping_add(rhs))
    }
}

impl core::ops::AddAssign<usize> for GlobalIndex {
    fn add_assign(&mut self, rhs: usize) {
        *self = *self + rhs
    }
}

impl PartialOrd for GlobalIndex {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for GlobalIndex {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        let diff = *self - *other;
        diff.cmp(&0)
    }
}
