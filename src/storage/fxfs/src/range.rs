// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::errors::FxfsError;
use anyhow::{Error, ensure};
use std::fmt::Debug;
use std::ops::{Range, Rem, Sub};

pub trait RangeExt<T> {
    /// Returns whether the range is valid (i.e. start <= end).
    fn is_valid(&self) -> bool;

    /// Returns the length of the range, or an error if the range is `!RangeExt::is_valid()`.
    /// Since this is intended to be used primarily for possibly-untrusted serialized ranges, the
    /// error returned is FxfsError::Inconsistent.
    fn length(&self) -> Result<T, Error>;

    /// Returns the length of the range.
    ///
    /// # Safety
    ///
    /// The range must be valid (i.e. [`RangeExt::is_valid()`] must be true).
    unsafe fn unchecked_length(&self) -> T;

    /// Splits the half-open range `[range.start, range.end)` into the ranges `[range.start,
    /// split_point)` and `[split_point, range.end)`. If either of the new ranges would be empty,
    /// then `None` is returned in its place and `Some(range)` is returned for the other. `range`
    /// must not be empty.
    fn split(self, split_point: T) -> (Option<Range<T>>, Option<Range<T>>);
}

impl<T: Sub<Output = T> + Copy + Ord + Debug + Rem<Output = T> + PartialEq + Default> RangeExt<T>
    for Range<T>
{
    fn is_valid(&self) -> bool {
        self.start <= self.end
    }

    fn length(&self) -> Result<T, Error> {
        ensure!(self.is_valid(), FxfsError::Inconsistent);
        Ok(self.end - self.start)
    }

    unsafe fn unchecked_length(&self) -> T {
        self.end - self.start
    }

    fn split(self, split_point: T) -> (Option<Range<T>>, Option<Range<T>>) {
        debug_assert!(!self.is_empty());
        if split_point <= self.start {
            (None, Some(self))
        } else if split_point >= self.end {
            (Some(self), None)
        } else {
            (Some(self.start..split_point), Some(split_point..self.end))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::RangeExt;

    #[test]
    fn test_split_range() {
        assert_eq!((10..20).split(0), (None, Some(10..20)));
        assert_eq!((10..20).split(9), (None, Some(10..20)));
        assert_eq!((10..20).split(10), (None, Some(10..20)));
        assert_eq!((10..20).split(11), (Some(10..11), Some(11..20)));
        assert_eq!((10..20).split(15), (Some(10..15), Some(15..20)));
        assert_eq!((10..20).split(19), (Some(10..19), Some(19..20)));
        assert_eq!((10..20).split(20), (Some(10..20), None));
        assert_eq!((10..20).split(25), (Some(10..20), None));
    }
}
