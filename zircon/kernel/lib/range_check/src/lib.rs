// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Range checking and interval arithmetic utilities mirroring `<kernel/range_check.h>`.

#![no_std]

use core::ops::Range;

/// Constructs a half-open `Range<u64>` from `(offset, len)`.
///
/// Returns `None` if `offset + len` overflows `u64`.
pub const fn from_offset_len(offset: u64, len: u64) -> Option<Range<u64>> {
    match offset.checked_add(len) {
        Some(end) => Some(offset..end),
        None => None,
    }
}

/// Constructs a half-open `Range<usize>` from `(offset, len)`.
///
/// Returns `None` if `offset + len` overflows `usize`.
pub const fn from_offset_len_usize(offset: usize, len: usize) -> Option<Range<usize>> {
    match offset.checked_add(len) {
        Some(end) => Some(offset..end),
        None => None,
    }
}

/// Returns true if `inner` is fully contained inside `outer`.
///
/// Both ranges are treated as half-open intervals `[start, end)`.
/// An empty `inner` range is considered contained within `outer` iff `inner.start` lies within `outer.start..=outer.end`.
pub fn in_range<T: Ord + Copy>(inner: &Range<T>, outer: &Range<T>) -> bool {
    inner.start >= outer.start && inner.end <= outer.end && inner.start <= inner.end
}

/// Returns true if the range `[offset, offset + len)` is fully inside `[0, max)`.
///
/// Returns `false` on arithmetic overflow or if out of bounds.
pub fn in_range_max(offset: u64, len: u64, max: u64) -> bool {
    let Some(range) = from_offset_len(offset, len) else {
        return false;
    };
    in_range(&range, &(0..max))
}

/// Returns true if the range `[offset, offset + len)` is fully inside `[min, max)`.
///
/// Returns `false` on arithmetic overflow, underflow, or if out of bounds.
pub fn in_range_min_max(offset: u64, len: u64, min: u64, max: u64) -> bool {
    let Some(range) = from_offset_len(offset, len) else {
        return false;
    };
    in_range(&range, &(min..max))
}

/// Trims `range` so that it fits within `0..trim_to_len`.
///
/// Returns `Some(trimmed_range)` if `range.start <= trim_to_len`.
/// Returns `None` if `range.start > trim_to_len` or `range.start > range.end`.
pub fn trim_range<T: Ord + Copy>(range: &Range<T>, trim_to_len: T) -> Option<Range<T>> {
    if range.start > trim_to_len || range.start > range.end {
        return None;
    }
    let end = core::cmp::min(range.end, trim_to_len);
    Some(range.start..end)
}

/// Trims `[offset, offset + len)` to `[0, trim_to_len)`.
///
/// Returns `Some(trimmed_len)` on success, or `None` if `offset > trim_to_len` or on overflow.
pub fn trim_range_offset_len(offset: u64, len: u64, trim_to_len: u64) -> Option<u64> {
    let range = from_offset_len(offset, len)?;
    let trimmed = trim_range(&range, trim_to_len)?;
    Some(trimmed.end - trimmed.start)
}

/// Determines if two half-open ranges overlap.
///
/// Empty ranges (where `start >= end`) do not overlap with any range.
pub fn intersects<T: Ord + Copy>(r1: &Range<T>, r2: &Range<T>) -> bool {
    r1.start < r1.end && r2.start < r2.end && r1.start < r2.end && r2.start < r1.end
}

/// Determines if two `(offset, len)` pairs overlap as half-open ranges `[offset, offset + len)`.
///
/// Returns `false` if either length is zero or if an integer overflow occurs on `offset + len`.
pub fn intersects_offset_len(offset1: u64, len1: u64, offset2: u64, len2: u64) -> bool {
    let Some(r1) = from_offset_len(offset1, len1) else {
        return false;
    };
    let Some(r2) = from_offset_len(offset2, len2) else {
        return false;
    };
    intersects(&r1, &r2)
}

/// Computes the intersection of two half-open ranges, returning `Some(intersection)` if they
/// overlap, or `None` if they do not.
pub fn get_intersect<T: Ord + Copy>(r1: &Range<T>, r2: &Range<T>) -> Option<Range<T>> {
    if !intersects(r1, r2) {
        return None;
    }
    let start = core::cmp::max(r1.start, r2.start);
    let end = core::cmp::min(r1.end, r2.end);
    Some(start..end)
}

/// Computes the intersection of two `(offset, len)` pairs, returning `Some((offset, len))`
/// if they overlap, or `None` if they do not.
pub fn get_intersect_offset_len(
    offset1: u64,
    len1: u64,
    offset2: u64,
    len2: u64,
) -> Option<(u64, u64)> {
    let r1 = from_offset_len(offset1, len1)?;
    let r2 = from_offset_len(offset2, len2)?;
    let intersection = get_intersect(&r1, &r2)?;
    Some((intersection.start, intersection.end - intersection.start))
}

/// Extension trait providing range-checking methods directly on `Range<T>`.
pub trait RangeExt<T> {
    /// Returns true if this range is fully contained inside `outer`.
    fn in_range(&self, outer: &Range<T>) -> bool;

    /// Returns true if this range overlaps with `other`.
    fn intersects(&self, other: &Range<T>) -> bool;

    /// Computes the intersection of this range with `other`.
    fn intersect(&self, other: &Range<T>) -> Option<Range<T>>;

    /// Trims this range to fit within `0..trim_to_len`.
    fn trim(&self, trim_to_len: T) -> Option<Range<T>>;
}

impl<T: Ord + Copy> RangeExt<T> for Range<T> {
    fn in_range(&self, outer: &Range<T>) -> bool {
        in_range(self, outer)
    }

    fn intersects(&self, other: &Range<T>) -> bool {
        intersects(self, other)
    }

    fn intersect(&self, other: &Range<T>) -> Option<Range<T>> {
        get_intersect(self, other)
    }

    fn trim(&self, trim_to_len: T) -> Option<Range<T>> {
        trim_range(self, trim_to_len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_in_range_basic() {
        // [0, 1024) is within [0, 4096)
        assert!(in_range_max(0, 1024, 4096));
        assert!(in_range_min_max(0, 1024, 0, 4096));
        assert!((0..1024).in_range(&(0..4096)));

        // [0, 1024) is not within [1, 4096)
        assert!(!in_range_min_max(0, 1024, 1, 4096));
        assert!(!(0..1024).in_range(&(1..4096)));

        // [0, 1024) is within [0, 1024)
        assert!(in_range_min_max(0, 1024, 0, 1024));
        assert!((0..1024).in_range(&(0..1024)));

        // [0, 1024) is not within [0, 1023)
        assert!(!in_range_min_max(0, 1024, 0, 1023));
        assert!(!(0..1024).in_range(&(0..1023)));

        // offset < min tests
        assert!(!in_range_min_max(32768, 1024, 524288, 1048576));
        assert!(!(32768..33792).in_range(&(524288..1048576)));

        // Right overlap, left overlap, full overlap
        assert!(!in_range_min_max(4000, 1000, 4500, 5500));
        assert!(!in_range_min_max(5000, 1000, 4500, 5500));
        assert!(!in_range_min_max(4000, 2000, 4500, 5500));
    }

    #[test]
    fn test_in_range_overflow() {
        assert!(!in_range_max(u64::MAX - 10, 20, u64::MAX));
        assert!(!in_range_min_max(u64::MAX - 10, 20, 0, u64::MAX));
    }

    #[test]
    fn test_trim_range() {
        assert_eq!(trim_range_offset_len(0, 1000, 500), Some(500));
        assert_eq!(trim_range_offset_len(0, 1000, 2000), Some(1000));
        assert_eq!(trim_range_offset_len(500, 1000, 1000), Some(500));
        assert_eq!(trim_range_offset_len(1000, 1000, 1000), Some(0));
        assert_eq!(trim_range_offset_len(1500, 1000, 1000), None);

        assert_eq!((0..1000).trim(500), Some(0..500));
        assert_eq!((0..1000).trim(2000), Some(0..1000));
        assert_eq!((1500..2500).trim(1000), None);
    }

    #[test]
    fn test_intersects() {
        // Disjoint ranges
        assert!(!intersects_offset_len(0, 10, 10, 10));
        assert!(!intersects_offset_len(10, 10, 0, 10));
        assert!(!(0..10).intersects(&(10..20)));
        assert!(!(10..20).intersects(&(0..10)));

        // Overlapping ranges
        assert!(intersects_offset_len(0, 10, 5, 10));
        assert!(intersects_offset_len(5, 10, 0, 10));
        assert!((0..10).intersects(&(5..15)));
        assert!((5..15).intersects(&(0..10)));

        // Fully contained
        assert!(intersects_offset_len(0, 20, 5, 5));
        assert!(intersects_offset_len(5, 5, 0, 20));
        assert!((0..20).intersects(&(5..10)));
        assert!((5..10).intersects(&(0..20)));

        // Zero-length regions do not intersect
        assert!(!intersects_offset_len(5, 0, 0, 20));
        assert!(!intersects_offset_len(0, 20, 5, 0));
        assert!(!(5..5).intersects(&(0..20)));
        assert!(!(0..20).intersects(&(5..5)));
    }

    #[test]
    fn test_get_intersect() {
        assert_eq!(get_intersect_offset_len(0, 10, 10, 10), None);
        assert_eq!(get_intersect_offset_len(0, 10, 5, 10), Some((5, 5)));
        assert_eq!(get_intersect_offset_len(5, 10, 0, 10), Some((5, 5)));
        assert_eq!(get_intersect_offset_len(0, 20, 5, 10), Some((5, 10)));
        assert_eq!(get_intersect_offset_len(5, 10, 0, 20), Some((5, 10)));

        assert_eq!((0..10).intersect(&(10..20)), None);
        assert_eq!((0..10).intersect(&(5..15)), Some(5..10));
        assert_eq!((5..15).intersect(&(0..10)), Some(5..10));
        assert_eq!((0..20).intersect(&(5..15)), Some(5..15));
    }
}
