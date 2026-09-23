// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A strategy tracks free space and decides where allocations are to be made.
//!
//! Note that strategy *excludes*:
//!
//!   * get/set byte limits
//!   * reservation tracking of future allocations.
//!   * deallocated but not yet usable regions (awaiting flush).
//!   * Any TRIM-specific logic
//!   * Volume Deletion
//!
//! These sorts of higher-level concepts should be implemented in `Allocator`.
//!
//! Strategies should be concerned only with selecting which free regions of disk to hand out.

use crate::object_store::FxfsError;
use anyhow::{Context, Error};
use std::collections::{BTreeMap, BTreeSet, btree_map};
use std::fmt::Debug;
use std::ops::Range;

/// Holds the offsets of extents of a given length.
#[derive(Debug, Default)]
struct SizeBucket {
    /// True if this bucket has overflowed, indicating that there may be more extents of this
    /// size on disk if we look again.
    overflow: bool,
    offsets: BTreeSet<u64>,
}

/// Tests (in allocator.rs) may set max extent size to low values for other reasons.
/// We want to make sure that we never go below a value that is big enough for superblocks
/// so the value we use here is chosen to be larger than the value allocator.rs calculates and
/// a convenient power of two.
const DEFAULT_MAX_EXTENT_SIZE: u64 = 2 << 20;

/// The largest size bucket is a catch-all for extents this size and up so we
/// explicitly keep more than we would otherwise.
const NUM_MAX_SIZED_EXTENTS_TO_KEEP: u64 = 512;

/// Returns the maximum free ranges we keep in RAM for a given range length.
/// We use the reciprocal function (2MiB/len) for this as we tend to see a lot more short
/// extents. One exception is 'DEFAULT_MAX_EXTENT_SIZE'. This is a catch-all bucket so we
/// intentionally bump this to cover the fact that a wide range of allocation sizes may hit this
/// bucket.
fn max_entries(len: u64) -> usize {
    if len < DEFAULT_MAX_EXTENT_SIZE {
        // With a 4kiB block size up to 2MiB (i.e. 512 possible lengths), this works out to be a
        // total of 6748 entries.
        (4 << 20) / len as usize
    } else {
        NUM_MAX_SIZED_EXTENTS_TO_KEEP as usize
    }
}

/// An allocation strategy that returns the smallest extent that is large enough to hold the
/// requested allocation, or the next best if no extent is big enough.
///
/// This strategy either leads to a perfect fit of a free extent to an allocation or a small
/// amount of free space being left over, creating even smaller fragments of free space.
///
/// This tendency to create smaller fragments of free space starts to affect file fragmentation
/// when filesystem use approaches capacity and there are nothing but small fragments of free
/// space remaining.
#[derive(Debug, Default)]
pub struct BestFit {
    /// Holds free storage ranges for the filesystem ordered by length, then start.
    ranges: BTreeMap<u64, SizeBucket>,
    /// A secondary index used to speed up coalescing of 'free' calls. Keyed by end -> start
    by_end: BTreeMap<u64, u64>,
}

impl BestFit {
    /// Tries to assign a set of contiguous `bytes` and returns the range, removing it
    /// from the pool of available bytes and returning it.
    ///
    /// If insufficient contiguous space is available, the largest available range will
    /// be returned. If no bytes are available, None will be returned.
    ///
    /// There are no special requirements on alignment of `bytes` but the caller is generally
    /// encouraged to align to device block size.
    pub fn allocate(&mut self, bytes: u64) -> Result<Range<u64>, FxfsError> {
        let mut result = self.ranges.range_mut(bytes..).next();
        if result.is_none() {
            // Insufficient space. Return the biggest range we have.
            result = self.ranges.iter_mut().rev().next();
        }
        if let Some((&size, size_bucket)) = result {
            if size_bucket.offsets.is_empty() && size_bucket.overflow {
                return Err(FxfsError::NotFound);
            }
            debug_assert!(!size_bucket.offsets.is_empty());
            let offset = size_bucket.offsets.pop_first().unwrap();
            if size_bucket.offsets.is_empty() && !size_bucket.overflow {
                self.ranges.remove(&size);
            }
            self.by_end.remove(&(offset + size));
            if size > bytes {
                self.free(offset + bytes..offset + size).expect("give extra back");
                Ok(offset..offset + bytes)
            } else {
                Ok(offset..offset + size)
            }
        } else {
            Err(FxfsError::NoSpace)
        }
    }

    /// This is effectively an optimized path for "remove" followed by "free" on the same range.
    /// Returns true if this resulted in changes to overall free space.
    pub fn force_free(&mut self, range: Range<u64>) -> Result<bool, Error> {
        let mut to_add = Vec::new();
        let mut last = range.start;
        for (&end, &offset) in self.by_end.range(range.start + 1..) {
            if offset >= range.end {
                break;
            }
            if offset > last {
                to_add.push(last..offset);
            }
            last = end;
            assert!(end > range.start);
        }
        if last < range.end {
            to_add.push(last..range.end);
        }
        Ok(if to_add.is_empty() {
            false
        } else {
            for range in to_add {
                self.free(range)?;
            }
            true
        })
    }

    /// Removes all free extents in the given range.
    pub fn remove(&mut self, range: Range<u64>) {
        let mut to_add = Vec::new();
        let mut to_remove = Vec::new();
        for (&end, &offset) in self.by_end.range(range.start + 1..) {
            if offset >= range.end {
                break;
            }
            assert!(end > range.start);
            to_remove.push(offset..end);
            if offset < range.start {
                to_add.push(offset..range.start);
            }
            if end > range.end {
                to_add.push(range.end..end);
            }
        }
        for range in to_remove {
            self.remove_range(range);
        }
        for range in to_add {
            self.free(range).unwrap();
        }
    }

    /// Internal helper function. Assumes range exists.
    fn remove_range(&mut self, range: Range<u64>) {
        let btree_map::Entry::Occupied(mut size_bucket) =
            self.ranges.entry(range.end - range.start)
        else {
            unreachable!()
        };
        size_bucket.get_mut().offsets.remove(&range.start);
        if size_bucket.get().offsets.is_empty() && !size_bucket.get().overflow {
            size_bucket.remove_entry();
        }
        assert_eq!(Some(range.start), self.by_end.remove(&range.end));
    }

    /// Returns the set of buckets that have overflow markers.
    pub fn overflow_markers(&self) -> Vec<u64> {
        self.ranges
            .iter()
            .filter_map(|(&size, size_bucket)| size_bucket.overflow.then_some(size))
            .collect()
    }

    // Overflow markers persist until removed.
    // Before we refresh data from disk, we need to remove these markers as there is no guarantee
    // that we will overflow the same bucket every time we refresh.
    pub fn reset_overflow_markers(&mut self) {
        let mut to_remove = Vec::new();
        for (&size, size_bucket) in self.ranges.iter_mut() {
            size_bucket.overflow = false;
            if size_bucket.offsets.is_empty() {
                to_remove.push(size);
            }
        }
        for size in to_remove {
            self.ranges.remove(&size);
        }
    }

    /// Internal helper function. Inserts range if room to do so, appends overflow marker if needed.
    fn insert_range(&mut self, range: Range<u64>) -> Result<(), Error> {
        let len = range.end - range.start;
        let entry = self.ranges.entry(len).or_default();
        if !entry.offsets.insert(range.start) {
            // This might happen if there is some high-level logic error that causes a range to be
            // freed twice. It may indicate a disk corruption or a software bug and shouldn't happen
            // under normal circumstances. While the low-level data structure held here will not be
            // compromised by this error, it is probably not safe to continue if a condition such
            // as this is detected, thus we return "Inconsistent".
            return Err(FxfsError::Inconsistent).context("Range already in 'ranges'.");
        }
        let other = self.by_end.insert(range.end, range.start);
        if let Some(_other) = other {
            // self.by_end and self.ranges should always remain consistant.
            // If this occurs it is almost certainly a software bug and continued use of this
            // data structure will likely lead to undefined behaviour.
            return Err(FxfsError::Inconsistent)
                .context("Range already in 'by_end'. Potential logic bug.");
        };
        if entry.offsets.len() > max_entries(len) {
            let last = entry.offsets.pop_last().unwrap();
            if self.by_end.remove(&(last + len)) != Some(last) {
                return Err(FxfsError::Inconsistent)
                    .context("Expected range missing or invalid 'by_end'");
            };
            entry.overflow = true;
        }
        Ok(())
    }

    /// Adds an arbitrary range of bytes to the pool of available ranges.
    ///
    /// Note that we keep these ranges in a map keyed by their length. To bound the size of this
    /// map we only track ranges up to N blocks long (up to 2MB). Longer ranges
    /// are broken up into ranges of this size.
    pub fn free(&mut self, mut range: Range<u64>) -> Result<(), Error> {
        // If there is a free range immediately before this one, merge with it.
        let mut iter = self.by_end.range(range.start..);
        let mut next_item = iter.next();
        if let Some((&end, &start)) = next_item {
            if end == range.start {
                self.remove_range(start..end);
                range.start = start;
                iter = self.by_end.range(range.start + 1..);
                next_item = iter.next();
            } else if start < range.end {
                // There exists a range already that overlaps with this.
                // We have a double free or freeing of overlapping allocations.
                // While the data-structure here remains valid after this error, the fact that
                // an overlapping free was attempted indicates that the filesystem is not in a
                // safe state to continue, thus we return "Inconsistent".
                return Err(FxfsError::Inconsistent).context("overlapping free");
            }
        }
        // If there is a free range immediately after this one, merge with it.
        if let Some((&end, &start)) = next_item {
            if start == range.end {
                self.remove_range(start..end);
                range.end = end;
            }
        }

        // We don't allow ranges longer than maximum extent size, but if we need to split such
        // a range, we want the smaller fragment to come first (pushing small fragments together at
        // the start of the device).
        while (range.end - range.start) >= DEFAULT_MAX_EXTENT_SIZE {
            self.insert_range(range.end - DEFAULT_MAX_EXTENT_SIZE..range.end)
                .context("adding max_extent_size fragment")?;
            range.end -= DEFAULT_MAX_EXTENT_SIZE;
        }
        if range.start < range.end {
            self.insert_range(range).context("adding final range")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn allocate() {
        let mut bestfit = BestFit::default();
        bestfit.free(0..0).unwrap(); // NOOP
        bestfit.free(0..100).unwrap();
        assert_eq!(bestfit.allocate(10), Ok(0..10));
        assert_eq!(bestfit.allocate(10), Ok(10..20));
        assert_eq!(bestfit.allocate(10), Ok(20..30));
        assert_eq!(bestfit.allocate(10), Ok(30..40));
        assert_eq!(bestfit.allocate(10), Ok(40..50));
        // Make some holes.
        bestfit.free(30..40).unwrap();
        bestfit.free(10..20).unwrap();
        // Holes get filled first.
        assert_eq!(bestfit.allocate(10), Ok(10..20));
        assert_eq!(bestfit.allocate(10), Ok(30..40));
        assert_eq!(bestfit.allocate(10), Ok(50..60));
        // Free a contiguous bunch of allocations at once.
        bestfit.free(0..50).unwrap();
        // Return less than requested.
        assert_eq!(bestfit.allocate(100), Ok(0..50));
        // Return all remaining space.
        assert_eq!(bestfit.allocate(100), Ok(60..100));
        // No space left. Return None.
        assert_eq!(bestfit.allocate(100), Err(FxfsError::NoSpace));
        // Now we have some more back.
        bestfit.free(50..100).unwrap();
        assert_eq!(bestfit.allocate(100), Ok(50..100));
    }

    #[test]
    fn remove() {
        let mut bestfit = BestFit::default();
        bestfit.free(0..100).unwrap();
        bestfit.remove(25..50);
        assert_eq!(bestfit.allocate(30), Ok(50..80));
        assert_eq!(bestfit.allocate(25), Ok(0..25));

        // Test removal across disjoint extents.
        let mut bestfit = BestFit::default();
        bestfit.free(0..20).unwrap();
        bestfit.free(30..50).unwrap();
        bestfit.remove(10..40);
        assert_eq!(bestfit.allocate(10), Ok(0..10));
        assert_eq!(bestfit.allocate(10), Ok(40..50));
        assert_eq!(bestfit.allocate(10), Err(FxfsError::NoSpace));

        // Test coalescing of adjacent available ranges if the request is large.
        let mut bestfit = BestFit::default();
        let end = DEFAULT_MAX_EXTENT_SIZE * (NUM_MAX_SIZED_EXTENTS_TO_KEEP + 1) + 10000;
        bestfit.free(0..end).unwrap();
        // We slice from the back so we have sizes: 10000,2M,2M,2M,2M,...
        // Free up most of the 2MB tail entries -- we just needed to trigger an overflow marker.
        bestfit.remove(DEFAULT_MAX_EXTENT_SIZE * 3 + 10000..end);
        // Free up the firs two slices worth. (This exercises unaligned removal.)
        bestfit.remove(0..DEFAULT_MAX_EXTENT_SIZE * 2);
        // Allocate the last 2MB extent.
        assert_eq!(
            bestfit.allocate(DEFAULT_MAX_EXTENT_SIZE),
            Ok(DEFAULT_MAX_EXTENT_SIZE * 2 + 10000..DEFAULT_MAX_EXTENT_SIZE * 3 + 10000)
        );
        // Allocate all the space.
        assert_eq!(
            bestfit.allocate(10000),
            Ok(DEFAULT_MAX_EXTENT_SIZE * 2..DEFAULT_MAX_EXTENT_SIZE * 2 + 10000)
        );
        // Now there is insufficient space, but we should get the overflow marker.
        assert_eq!(
            bestfit.allocate(DEFAULT_MAX_EXTENT_SIZE),
            Err(FxfsError::NotFound) // Overflow.
        );
        // Reset the overflow marker. We should see NoSpace.
        bestfit.reset_overflow_markers();
        assert_eq!(bestfit.allocate(DEFAULT_MAX_EXTENT_SIZE), Err(FxfsError::NoSpace));
    }

    #[test]
    fn coalescing_free() {
        let mut bestfit = BestFit::default();
        // Free some bytes at the start and end.
        bestfit.free(0..10).unwrap();
        bestfit.free(20..32).unwrap();
        // Now free the space in the middle, which should coalesce with ranges on both sides.
        bestfit.free(10..20).unwrap();
        // Confirm that we can allocate one block of 32 bytes. This will fail if coalescing
        // didn't occur.
        bestfit.remove(0..32);
        assert_eq!(bestfit.allocate(32), Err(FxfsError::NoSpace));
    }

    #[test]
    fn max_range() {
        let mut bestfit = BestFit::default();
        bestfit.free(10..10 + 10 * DEFAULT_MAX_EXTENT_SIZE).unwrap();

        // We can't allocate bigger than DEFAULT_MAX_EXTENT_SIZE.
        assert_eq!(
            bestfit.allocate(2 * DEFAULT_MAX_EXTENT_SIZE),
            Ok(10..10 + DEFAULT_MAX_EXTENT_SIZE)
        );

        // Make sure that coalescing still works properly
        bestfit.free(10..10 + DEFAULT_MAX_EXTENT_SIZE).unwrap();
        assert_eq!(bestfit.allocate(10), Ok(10..20));
        assert_eq!(bestfit.allocate(10), Ok(20..30));
        assert_eq!(
            bestfit.allocate(DEFAULT_MAX_EXTENT_SIZE - 20),
            Ok(30..10 + DEFAULT_MAX_EXTENT_SIZE)
        );
    }

    #[test]
    fn fragmenting_free() {
        // It shouldn't matter how we allocate and free here so long as there are no overlaps.
        let mut bestfit = BestFit::default();
        bestfit.free(0..100).unwrap();
        assert_eq!(bestfit.allocate(30), Ok(0..30));
        assert!(bestfit.free(20..30).is_ok());
        assert_eq!(bestfit.allocate(10), Ok(20..30));
        assert!(bestfit.free(0..30).is_ok());
        assert!(bestfit.free(0..30).is_err());

        // Merge left and right and middle.
        let mut bestfit = BestFit::default();
        bestfit.free(10..20).unwrap();
        bestfit.free(30..40).unwrap();
        bestfit.free(0..10).unwrap(); // before
        bestfit.free(40..50).unwrap(); // after
        bestfit.free(20..30).unwrap(); // middle

        // Check all combinations of overlaps.
        let mut bestfit = BestFit::default();
        bestfit.free(10..20).unwrap();
        assert!(bestfit.free(9..11).is_err());
        assert!(bestfit.free(19..21).is_err());
        assert!(bestfit.free(11..29).is_err());
        assert!(bestfit.free(10..20).is_err());
        assert!(bestfit.free(9..10).is_ok());
        assert!(bestfit.free(20..21).is_ok());
    }

    #[test]
    fn force_free() {
        let mut bestfit = BestFit::default();
        bestfit.free(0..100).unwrap();
        assert_eq!(bestfit.force_free(0..100).unwrap(), false);
        assert_eq!(bestfit.force_free(10..100).unwrap(), false);
        assert_eq!(bestfit.force_free(0..90).unwrap(), false);
        assert_eq!(bestfit.force_free(10..90).unwrap(), false);

        let mut bestfit = BestFit::default();
        bestfit.free(0..40).unwrap();
        bestfit.free(60..100).unwrap();
        assert_eq!(bestfit.force_free(10..90).unwrap(), true);
        assert_eq!(bestfit.allocate(100), Ok(0..100));

        let mut bestfit = BestFit::default();
        bestfit.free(0..40).unwrap();
        bestfit.free(60..100).unwrap();
        assert_eq!(bestfit.force_free(10..100).unwrap(), true);
        assert_eq!(bestfit.allocate(100), Ok(0..100));

        let mut bestfit = BestFit::default();
        bestfit.free(10..40).unwrap();
        bestfit.free(60..100).unwrap();
        assert_eq!(bestfit.force_free(0..110).unwrap(), true);
        assert_eq!(bestfit.allocate(110), Ok(0..110));

        let mut bestfit = BestFit::default();
        bestfit.free(10..40).unwrap();
        bestfit.free(60..100).unwrap();
        assert_eq!(bestfit.force_free(10..110).unwrap(), true);
        assert_eq!(bestfit.allocate(100), Ok(10..110));

        let mut bestfit = BestFit::default();
        bestfit.free(10..40).unwrap();
        bestfit.free(60..100).unwrap();
        assert_eq!(bestfit.force_free(0..100).unwrap(), true);
        assert_eq!(bestfit.allocate(100), Ok(0..100));

        let mut bestfit = BestFit::default();
        assert_eq!(bestfit.force_free(0..100).unwrap(), true);
        assert_eq!(bestfit.allocate(100), Ok(0..100));
    }

    #[test]
    fn test_overflow_changes() {
        // Test for b/345105394 conditions.
        // This doesn't exactly recreate the differences though because
        let mut bestfit = BestFit::default();
        let mut ofs: u64 = 0;
        // Fill 256kb.
        for i in 0..max_entries(256 << 10) as u64 {
            assert!(bestfit.force_free(i * 2 * (256 << 10)..(i * 2 + 1) * (256 << 10)).unwrap());
        }
        ofs += (256 << 10) * max_entries(256 << 10) as u64 * 3;

        // Overflow 768kb
        for i in 0..max_entries(768 << 10) as u64 + 1 {
            assert!(
                bestfit
                    .force_free(ofs + i * 2 * (768 << 10)..ofs + (i * 2 + 1) * (768 << 10))
                    .unwrap()
            );
        }
        ofs += (768 << 10) * (max_entries(768 << 10) + 1) as u64 * 3;

        // Overflow 512kb.
        for i in 0..max_entries(512 << 10) as u64 + 1 {
            assert!(
                bestfit
                    .force_free(ofs + i * 2 * (512 << 10)..ofs + (i * 2 + 1) * (512 << 10))
                    .unwrap()
            );
        }
        // We want to put the next free adjacent to the tail of the last 512kb block, so
        // we drop the "+1" and add 512kb.
        ofs += (512 << 10) * max_entries(512 << 10) as u64 * 2 + (512 << 10);

        assert_eq!(bestfit.overflow_markers(), vec![524288, 786432]);

        // This is a 256kB free just after a 512kB free, so it will form a 768kb free.
        assert!(bestfit.force_free(ofs..ofs + (256 << 10)).unwrap());
        // In the actual bug, we would not add this until after the next flush, so
        // force_free would not return true and the only change would be the overflow marker.
        assert_eq!(bestfit.overflow_markers(), vec![262144, 524288, 786432]);
    }
}
