// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Types and utilities for dealing with numerical values.

use core::num::{NonZeroIsize, NonZeroUsize};

/// An `isize` that is strictly positive (greater than 0).
///
/// `PositiveIsize` differs from [`NonZeroUsize`] in that it is guaranteed to
/// fit in an `isize` (i.e. the maximum value is `isize::MAX` as opposed to
/// `usize::MAX`).
#[derive(Copy, Clone, Eq, PartialEq, Debug, Hash, Ord, PartialOrd)]
pub struct PositiveIsize(isize);

impl PositiveIsize {
    /// Creates a new `PositiveIsize` from an `isize` value.
    ///
    /// Returns `None` if `value` is less than or equal to zero.
    pub const fn new(value: isize) -> Option<Self> {
        if value > 0 { Some(Self(value)) } else { None }
    }

    /// Creates a new `PositiveIsize` from a `usize` value.
    ///
    /// Returns `None` if `value` is zero or larger than `isize::MAX`.
    pub const fn new_unsigned(value: usize) -> Option<Self> {
        match NonZeroUsize::new(value) {
            Some(v) => Self::new_nonzero_unsigned(v),
            None => None,
        }
    }

    /// Creates a new `PositiveIsize` from a `NonZeroUsize` value.
    ///
    /// Returns `None` if `value` is zero or larger than `isize::MAX`.
    pub const fn new_nonzero_unsigned(value: NonZeroUsize) -> Option<Self> {
        let value = value.get();
        if value > (isize::MAX as usize) { None } else { Some(Self(value as isize)) }
    }

    /// Returns the `isize` value within this `PositiveIsize`.
    pub const fn get(self) -> isize {
        let Self(v) = self;
        v
    }
}

impl From<PositiveIsize> for isize {
    fn from(PositiveIsize(value): PositiveIsize) -> Self {
        value
    }
}

impl From<PositiveIsize> for usize {
    fn from(PositiveIsize(value): PositiveIsize) -> Self {
        // Conversion is guaranteed to be infallible because `PositiveIsize`
        // creation guarantees: a positive `isize` always fits within a `usize`.
        value as usize
    }
}

impl From<PositiveIsize> for NonZeroIsize {
    fn from(PositiveIsize(value): PositiveIsize) -> Self {
        // SAFETY: value is guaranteed to be nonzero.
        unsafe { NonZeroIsize::new_unchecked(value) }
    }
}

impl From<PositiveIsize> for NonZeroUsize {
    fn from(PositiveIsize(value): PositiveIsize) -> Self {
        // value is guaranteed to be positive.
        let value = value as usize;
        // SAFETY: value is guaranteed to be nonzero.
        unsafe { NonZeroUsize::new_unchecked(value) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_positive_isize() {
        assert_eq!(PositiveIsize::new(0), None);
        assert_eq!(PositiveIsize::new(-1), None);
        assert_eq!(PositiveIsize::new(1), Some(PositiveIsize(1)));
        assert_eq!(PositiveIsize::new(isize::MIN), None);
        assert_eq!(PositiveIsize::new(isize::MAX), Some(PositiveIsize(isize::MAX)));
    }

    #[test]
    fn new_unsigned_positive_isize() {
        assert_eq!(PositiveIsize::new_unsigned(0), None);
        assert_eq!(PositiveIsize::new_unsigned(usize::MAX), None);
        assert_eq!(PositiveIsize::new_unsigned(1), Some(PositiveIsize(1)));
        let max = usize::try_from(isize::MAX).unwrap();
        assert_eq!(PositiveIsize::new_unsigned(max), Some(PositiveIsize(isize::MAX)));
        assert_eq!(PositiveIsize::new_unsigned(max + 1), None);
    }
}
