// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use alloc::collections::VecDeque;
use core::ops::Range;

use derivative::Derivative;
use netstack3_base::SeqNum;

/// A generic data structure that keeps track of ordered sequence number ranges.
///
/// Each kept range has associated metadata of type `M`.
#[derive(Debug, Derivative)]
#[derivative(Default(bound = ""))]
#[cfg_attr(test, derive(PartialEq, Eq))]
pub(crate) struct SeqRanges<M> {
    blocks: VecDeque<SeqRange<M>>,
}

impl<M> SeqRanges<M> {
    pub(crate) fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    pub(crate) fn len(&self) -> usize {
        self.blocks.len()
    }

    pub(crate) fn pop_back(&mut self) -> Option<SeqRange<M>> {
        self.blocks.pop_back()
    }

    pub(crate) fn pop_front_if<F: FnOnce(&SeqRange<M>) -> bool>(
        &mut self,
        f: F,
    ) -> Option<SeqRange<M>> {
        let front = self.blocks.front()?;
        if f(front) { self.blocks.pop_front() } else { None }
    }

    fn find_first_after(blocks: &VecDeque<SeqRange<M>>, start: SeqNum) -> usize {
        match blocks.binary_search_by(|block| {
            if block.start() == start {
                return core::cmp::Ordering::Equal;
            }
            if block.start().before(start) {
                core::cmp::Ordering::Less
            } else {
                core::cmp::Ordering::Greater
            }
        }) {
            Ok(r) => {
                // We found the exact same start point, so the first segment
                // whose start is greater must be the next one.
                r + 1
            }
            Err(e) => {
                // When binary search doesn't find the exact place it returns
                // the index where this block should be in, which should be the
                // next greater range.
                e
            }
        }
    }

    /// Inserts `range` into this tracking structure.
    ///
    /// No-op if the range is empty.
    ///
    /// `meta` is attached to the newly created range and all the ranges it
    /// touches, including if `range` is a subset of a currently tracked range.
    ///
    /// Returns `true` iff `range` insertion increases the total number of
    /// tracked bytes contained in `SeqRanges`.
    pub(crate) fn insert(&mut self, range: Range<SeqNum>, meta: M) -> bool
    where
        M: Clone,
    {
        let Some(range) = SeqRange::new(range, meta) else {
            return false;
        };
        self.insert_seq_range(range)
    }

    fn insert_seq_range(&mut self, mut range: SeqRange<M>) -> bool
    where
        M: Clone,
    {
        let Self { blocks } = self;

        if blocks.is_empty() {
            blocks.push_back(range);
            return true;
        }

        // Search for the first segment whose `start` is greater.
        let first_after = Self::find_first_after(blocks, range.start());

        let mut merge_right = 0;
        for block in blocks.range(first_after..blocks.len()) {
            match range.merge_right(block) {
                MergeRightResult::Before => break,
                MergeRightResult::After { merged } => {
                    merge_right += 1;
                    if merged {
                        break;
                    }
                }
            }
        }

        // Given we're always sorted and we know the first range strictly after
        // the inserting one, we only need to check to the left once.
        let merge_left = match first_after
            .checked_sub(1)
            .and_then(|first_before| blocks.get_mut(first_before))
        {
            Some(block) => {
                match block.merge_right(&range) {
                    MergeRightResult::Before => 0,
                    MergeRightResult::After { merged } => {
                        if merged {
                            range.clone_range_from(&block);
                            1
                        } else {
                            // The new range fits entirely within an existing
                            // block. Update the metadata and return early.
                            block.set_meta(range.into_meta());
                            return false;
                        }
                    }
                }
            }
            None => 0,
        };

        if merge_left == 0 && merge_right == 0 {
            // If the new segment cannot merge with any of its neighbors, we
            // add a new entry for it.
            blocks.insert(first_after, range);
        } else {
            // Otherwise, we put the new segment at the left edge of the merge
            // window and remove all other existing segments.
            let left_edge = first_after - merge_left;
            let right_edge = first_after + merge_right;
            blocks[left_edge] = range;
            for i in right_edge..blocks.len() {
                blocks[i - merge_left - merge_right + 1] = blocks[i].clone();
            }
            blocks.truncate(blocks.len() - merge_left - merge_right + 1);
        }

        true
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &SeqRange<M>> + '_ {
        self.blocks.iter()
    }

    /// Provides an iterator that allows modifying the metadata for each stored
    /// range.
    pub(crate) fn iter_mut(&mut self) -> impl DoubleEndedIterator<Item = &mut SeqRange<M>> + '_ {
        self.blocks.iter_mut()
    }

    /// Trims the ranges discarding any values at or before `value`.
    pub(crate) fn discard_starting_at_or_before(&mut self, value: SeqNum) {
        let Self { blocks } = self;
        let first_after = Self::find_first_after(blocks, value);
        // All the blocks starting at or before `value` can be discarded.
        let _drain = blocks.drain(0..first_after);
    }

    /// Finds the first hole containing or after the sequence number `marker`.
    ///
    /// Returns [`FirstHoleResult::None`] if no hole can be identified, i.e.,
    /// `marker`` is at or above the highest tracked range.
    ///
    /// Returns [`FirstHoleResult::Right`] if the hole is bounded to its
    /// right side by the first stored sequence range,
    /// [`FirstHoleResult::Both`] otherwise.
    pub(crate) fn first_hole_on_or_after(&self, marker: SeqNum) -> FirstHoleResult<'_, M> {
        let Self { blocks } = self;
        let first_after = Self::find_first_after(blocks, marker);
        blocks
            .get(first_after)
            .map(|right| {
                first_after
                    .checked_sub(1)
                    .map(|left| FirstHoleResult::Both(&blocks[left], right))
                    .unwrap_or_else(|| FirstHoleResult::Right(right))
            })
            .unwrap_or(FirstHoleResult::None)
    }

    /// Returns the highest sequence number range stored, if there are any.
    pub(crate) fn last(&self) -> Option<&SeqRange<M>> {
        let Self { blocks } = self;
        blocks.back()
    }

    pub(crate) fn clear(&mut self) {
        let Self { blocks } = self;
        blocks.clear();
    }
}

#[derive(Debug)]
pub(crate) enum FirstHoleResult<'a, M> {
    None,
    Right(&'a SeqRange<M>),
    Both(&'a SeqRange<M>, &'a SeqRange<M>),
}

impl<M: Clone> FromIterator<SeqRange<M>> for SeqRanges<M> {
    fn from_iter<T: IntoIterator<Item = SeqRange<M>>>(iter: T) -> Self {
        let mut ranges = SeqRanges::default();
        for range in iter {
            let _: bool = ranges.insert_seq_range(range);
        }
        ranges
    }
}

mod range {
    use netstack3_base::SackBlock;

    use super::*;

    /// A range kept in [`SeqRanges`].
    #[derive(Debug, Clone)]
    #[cfg_attr(test, derive(PartialEq, Eq))]
    pub(crate) struct SeqRange<M> {
        range: Range<SeqNum>,
        meta: M,
    }

    impl<M> SeqRange<M> {
        pub(crate) fn new(range: Range<SeqNum>, meta: M) -> Option<Self> {
            range.end.after(range.start).then(|| Self { range, meta })
        }

        pub(crate) fn start(&self) -> SeqNum {
            self.range.start
        }

        pub(crate) fn end(&self) -> SeqNum {
            self.range.end
        }

        pub(crate) fn set_meta(&mut self, meta: M) {
            self.meta = meta;
        }

        pub(crate) fn meta(&self) -> &M {
            &self.meta
        }

        pub(crate) fn into_meta(self) -> M {
            self.meta
        }

        pub(super) fn clone_range_from(&mut self, other: &Self) {
            let Self { range, meta: _ } = self;
            *range = other.range.clone();
        }

        pub(crate) fn len(&self) -> u32 {
            let Self { range: Range { start, end }, meta: _ } = self;
            let len = *end - *start;
            // Assert on SeqRange invariant in debug only.
            debug_assert!(len >= 0);
            len as u32
        }

        pub(crate) fn cap_right(self, seq: SeqNum) -> Option<Self> {
            let Self { range: Range { start, end }, meta } = self;
            seq.after(start).then(|| Self { range: Range { start, end: end.earliest(seq) }, meta })
        }

        pub(crate) fn to_sack_block(&self) -> SackBlock {
            let Self { range: Range { start, end }, meta: _ } = self;
            // SAFETY: SackBlock requires that end is after start, which is the
            // same invariant held by SeqRange.
            unsafe { SackBlock::new_unchecked(*start, *end) }
        }

        pub(super) fn merge_right(&mut self, other: &Self) -> MergeRightResult {
            if self.range.end.before(other.range.start) {
                return MergeRightResult::Before;
            }

            let merged = self.range.end.before(other.range.end);
            if merged {
                self.range.end = other.range.end;
            }

            MergeRightResult::After { merged }
        }
    }

    pub(super) enum MergeRightResult {
        Before,
        After { merged: bool },
    }
}
use range::MergeRightResult;
pub(crate) use range::SeqRange;

#[cfg(test)]
mod test {
    use super::*;

    use alloc::vec;
    use alloc::vec::Vec;

    use netstack3_base::{SackBlock, WindowSize};
    use proptest::strategy::{Just, Strategy};
    use proptest::test_runner::Config;
    use proptest::{prop_assert, prop_assert_eq, proptest};
    use proptest_support::failed_seeds_no_std;
    use test_case::test_case;

    impl SeqRanges<()> {
        fn insert_u32(&mut self, range: Range<u32>) -> bool {
            let Range { start, end } = range;
            self.insert(SeqNum::new(start)..SeqNum::new(end), ())
        }
    }

    impl FromIterator<Range<u32>> for SeqRanges<()> {
        fn from_iter<T: IntoIterator<Item = Range<u32>>>(iter: T) -> Self {
            let mut ranges = SeqRanges::default();
            for range in iter {
                let _: bool = ranges.insert_u32(range);
            }
            ranges
        }
    }

    proptest! {
        #![proptest_config(Config {
            // Add all failed seeds here.
            failure_persistence: failed_seeds_no_std!(
                "cc f621ca7d3a2b108e0dc41f7169ad028f4329b79e90e73d5f68042519a9f63999",
                "cc c449aebed201b4ec4f137f3c224f20325f4cfee0b7fd596d9285176b6d811aa9"
            ),
            ..Config::default()
        })]

        #[test]
        fn seq_ranges_insert(insertions in proptest::collection::vec(insertions(), 200)) {
            let mut seq_ranges = SeqRanges::<()>::default();
            let mut num_insertions_performed = 0;
            let mut min_seq = SeqNum::new(WindowSize::MAX.into());
            let mut max_seq = SeqNum::new(0);
            for Range { start, end } in insertions {
                if min_seq.after(start) {
                    min_seq = start;
                }
                if max_seq.before(end) {
                    max_seq = end;
                }
                // assert that it's impossible to have more entries than the
                // number of insertions performed.
                prop_assert!(seq_ranges.blocks.len() <= num_insertions_performed);
                let _: bool = seq_ranges.insert(start..end, ());
                num_insertions_performed += 1;

                // assert that the ranges are sorted and don't overlap with
                // each other.
                for i in 1..seq_ranges.blocks.len() {
                    prop_assert!(
                        seq_ranges.blocks[i-1].end().before(seq_ranges.blocks[i].start())
                    );
                }
            }
            prop_assert_eq!(seq_ranges.blocks.front().unwrap().start(), min_seq);
            prop_assert_eq!(seq_ranges.blocks.back().unwrap().end(), max_seq);
        }

        // Test that the invariants between SackBlock and SeqRange creation
        // match. Supports unsafe block in SeqRange::to_sack_block.
        #[test]
        fn seq_range_to_sack_block((start, end) in sequence_numbers()) {
            prop_assert_eq!(
                SeqRange::new(start..end, ()).map(|sr| sr.to_sack_block()),
                SackBlock::try_new(start, end).ok()
            );
        }

    }

    fn insertions() -> impl Strategy<Value = Range<SeqNum>> {
        (0..u32::from(WindowSize::MAX)).prop_flat_map(|start| {
            (start + 1..=u32::from(WindowSize::MAX)).prop_flat_map(move |end| {
                Just(Range { start: SeqNum::new(start), end: SeqNum::new(end) })
            })
        })
    }

    fn sequence_numbers() -> impl Strategy<Value = (SeqNum, SeqNum)> {
        (0u32..5).prop_flat_map(|start| {
            (0u32..5).prop_flat_map(move |end| Just((SeqNum::new(start), SeqNum::new(end))))
        })
    }

    #[test]
    fn insert_return() {
        let mut sr = SeqRanges::default();
        assert!(sr.insert_u32(10..20));

        assert!(!sr.insert_u32(10..20));
        assert!(!sr.insert_u32(11..20));
        assert!(!sr.insert_u32(11..12));
        assert!(!sr.insert_u32(19..20));

        assert!(sr.insert_u32(0..5));
        assert!(sr.insert_u32(25..35));
        assert!(sr.insert_u32(5..7));
        assert!(sr.insert_u32(22..25));

        assert!(sr.insert_u32(7..22));
        assert!(!sr.insert_u32(0..35));
    }

    #[test_case(&[], 0 => Vec::<Range<u32>>::new(); "empty")]
    #[test_case(&[10..20], 0 => vec![10..20]; "before 1")]
    #[test_case(&[10..20, 30..40], 0 => vec![10..20, 30..40]; "before 2")]
    #[test_case(&[10..20], 10 =>  Vec::<Range<u32>>::new(); "same 1")]
    #[test_case(&[10..20, 30..40], 10 => vec![30..40]; "same 2")]
    #[test_case(&[10..20], 20 =>  Vec::<Range<u32>>::new(); "after 1")]
    #[test_case(&[10..20, 30..40], 20 => vec![30..40]; "after 2")]
    #[test_case(&[10..20, 30..40], 30 =>  Vec::<Range<u32>>::new(); "after 3")]
    #[test_case(&[10..20, 30..40], 15 =>  vec![30..40]; "mid 1")]
    #[test_case(&[10..20, 30..40], 35 =>  Vec::<Range<u32>>::new(); "mid 2")]
    fn discard_starting_at_or_before(ranges: &[Range<u32>], discard: u32) -> Vec<Range<u32>> {
        let mut sr = ranges.iter().cloned().collect::<SeqRanges<()>>();
        sr.discard_starting_at_or_before(SeqNum::new(discard));
        sr.iter()
            .map(|seq_range| u32::from(seq_range.start())..u32::from(seq_range.end()))
            .collect()
    }

    #[test_case(&[], 0 => (None, None))]
    #[test_case(&[10..20, 30..40], 0 => (None, Some(10)))]
    #[test_case(&[10..20, 30..40], 10 => (Some(20), Some(30)))]
    #[test_case(&[10..20, 30..40], 20 => (Some(20), Some(30)))]
    #[test_case(&[10..20, 30..40], 30 => (None, None))]
    #[test_case(&[10..20, 30..40], 40 => (None, None))]
    #[test_case(&[10..20, 30..40, 50..60], 30 => (Some(40), Some(50)))]
    #[test_case(&[10..20, 30..40, 50..60], 35 => (Some(40), Some(50)))]
    #[test_case(&[10..20, 30..40, 50..60], 40 => (Some(40), Some(50)))]
    fn first_hole_on_or_after(ranges: &[Range<u32>], marker: u32) -> (Option<u32>, Option<u32>) {
        match ranges
            .iter()
            .cloned()
            .collect::<SeqRanges<()>>()
            .first_hole_on_or_after(SeqNum::new(marker))
        {
            FirstHoleResult::None => (None, None),
            FirstHoleResult::Right(right) => (None, Some(u32::from(right.start()))),
            FirstHoleResult::Both(left, right) => {
                (Some(u32::from(left.end())), Some(u32::from(right.start())))
            }
        }
    }
}
