// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::iter::once;
use std::ops::Range;

/// Represents a mapping from the interval `from` to `to`.
#[derive(Debug, Eq, PartialEq)]
pub struct Mapping {
    pub from: Range<u64>,
    pub to: u64,
}

impl From<(Range<u64>, u64)> for Mapping {
    fn from(value: (Range<u64>, u64)) -> Self {
        let (from, to) = value;
        Self { from, to }
    }
}

impl Mapping {
    pub fn len(&self) -> u64 {
        self.from.end - self.from.start
    }

    // This does *not* check whether the two mappings are adjacent/overlapping.
    fn can_merge(&self, other: &Self) -> bool {
        if self.from.start > other.from.start {
            self.to == other.to + self.from.start - other.from.start
        } else {
            other.to == self.to + other.from.start - self.from.start
        }
    }

    fn split(&mut self, offset: u64) -> Self {
        let tail = Mapping { from: offset..self.from.end, to: self.to + offset - self.from.start };
        self.from.end = offset;
        tail
    }
}

pub trait MappingExt {
    fn insert_contiguous_mappings(&mut self, mappings: Self);
}

impl MappingExt for Vec<Mapping> {
    /// Inserts mappings into the existing mappings, maintaining sorted order.  NOTE: The inserted
    /// mappings *must* be contiguous.
    fn insert_contiguous_mappings(&mut self, mut mappings: Self) {
        if mappings.is_empty() {
            return;
        }

        let start;
        let end;

        // Records whether the mappings we are inserting overlap existing entries that need to be
        // trimmed.
        let mut overlaps_start = false;
        let mut overlaps_end = false;

        // Find the range we need to remove.
        let remove = {
            let first = &mut mappings[0];
            start = first.from.start;
            match self.binary_search_by(|m: &Mapping| m.from.start.cmp(&start)) {
                Err(i) if i > 0 => {
                    let m = &mut self[i - 1];
                    if m.from.end >= start {
                        if m.can_merge(first) {
                            first.to -= first.from.start - m.from.start;
                            first.from.start = m.from.start;
                            i - 1
                        } else {
                            overlaps_start = m.from.end > start;
                            i
                        }
                    } else {
                        i
                    }
                }
                Ok(i) | Err(i) => i,
            }
        }..{
            let last = mappings.last_mut().unwrap();
            end = last.from.end;
            match self.binary_search_by(|m: &Mapping| m.from.end.cmp(&end)) {
                Err(i) => {
                    if let Some(m) = self.get_mut(i) {
                        if m.from.start <= last.from.end {
                            if m.can_merge(last) {
                                last.from.end = m.from.end;
                                i + 1
                            } else {
                                overlaps_end = m.from.start < last.from.end;
                                i
                            }
                        } else {
                            i
                        }
                    } else {
                        i
                    }
                }
                Ok(i) => i + 1,
            }
        };

        if remove.end < remove.start {
            // In this case, a single existing entry needs to be split.
            let tail = self[remove.start - 1].split(end);
            self[remove.start - 1].from.end = start;
            self.splice(remove.start..remove.start, mappings.into_iter().chain(once(tail)));
        } else {
            if overlaps_start {
                self[remove.start - 1].split(start);
            }
            if overlaps_end {
                self[remove.end] = self[remove.end].split(end);
            }
            self.splice(remove, mappings);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Mapping, MappingExt};

    macro_rules! m {
        ($($x:expr),+ $(,)?) => { vec![$(Mapping::from($x)),+] }
    }

    #[test]
    fn test_insert_mappings() {
        let mut mappings = Vec::new();

        mappings.insert_contiguous_mappings(m![(10..20, 100)]);
        assert_eq!(mappings, m![(10..20, 100)]);

        // Inserting before, no overlap, can't be merged.
        mappings.insert_contiguous_mappings(m![(9..10, 101)]);
        assert_eq!(mappings, m![(9..10, 101), (10..20, 100)]);

        // Inserting before, no overlap, can be merged.
        mappings.insert_contiguous_mappings(m![(8..9, 100)]);
        assert_eq!(mappings, m![(8..10, 100), (10..20, 100)]);

        // Inserting before, with overlap, can't be merged.
        mappings.insert_contiguous_mappings(m![(7..9, 90)]);
        assert_eq!(mappings, m![(7..9, 90), (9..10, 101), (10..20, 100)]);

        // Inserting before, with overlap, can be merged.
        mappings.insert_contiguous_mappings(m![(6..8, 89)]);
        assert_eq!(mappings, m![(6..9, 89), (9..10, 101), (10..20, 100)]);

        // Inserting after, no overlap, can't be merged.
        mappings.insert_contiguous_mappings(m![(20..25, 200)]);
        assert_eq!(mappings, m![(6..9, 89), (9..10, 101), (10..20, 100), (20..25, 200)]);

        // Inserting after, no overlap, can be merged.
        mappings.insert_contiguous_mappings(m![(25..26, 205)]);
        assert_eq!(mappings, m![(6..9, 89), (9..10, 101), (10..20, 100), (20..26, 200)]);

        // Inserting after, with overlap, can't be merged.
        mappings.insert_contiguous_mappings(m![(25..27, 300)]);
        assert_eq!(
            mappings,
            m![(6..9, 89), (9..10, 101), (10..20, 100), (20..25, 200), (25..27, 300)]
        );

        // Inserting after, with overlap, can be merged.
        mappings.insert_contiguous_mappings(m![(26..28, 301)]);
        assert_eq!(
            mappings,
            m![(6..9, 89), (9..10, 101), (10..20, 100), (20..25, 200), (25..28, 300)]
        );

        // Insert requires splitting an existing entry.
        mappings.insert_contiguous_mappings(m![(15..16, 400)]);
        assert_eq!(
            mappings,
            m![
                (6..9, 89),
                (9..10, 101),
                (10..15, 100),
                (15..16, 400),
                (16..20, 106),
                (20..25, 200),
                (25..28, 300)
            ]
        );

        // Overwrite an existing entry.
        mappings.insert_contiguous_mappings(m![(6..9, 500)]);
        assert_eq!(
            mappings,
            m![
                (6..9, 500),
                (9..10, 101),
                (10..15, 100),
                (15..16, 400),
                (16..20, 106),
                (20..25, 200),
                (25..28, 300)
            ]
        );

        // Overwrite multiple entries and split the entry at the end.
        mappings.insert_contiguous_mappings(m![(6..14, 600)]);
        assert_eq!(
            mappings,
            m![
                (6..14, 600),
                (14..15, 104),
                (15..16, 400),
                (16..20, 106),
                (20..25, 200),
                (25..28, 300)
            ]
        );

        // Overwrite multiple entries and split the entry at the beginning.
        mappings.insert_contiguous_mappings(m![(10..16, 700)]);
        assert_eq!(
            mappings,
            m![(6..10, 600), (10..16, 700), (16..20, 106), (20..25, 200), (25..28, 300)]
        );

        // Overwrite all the entries.
        mappings.insert_contiguous_mappings(m![(6..28, 800)]);
        assert_eq!(mappings, m![(6..28, 800)]);

        // Insert no mappings.
        mappings.insert_contiguous_mappings(Vec::new());
        assert_eq!(mappings, m![(6..28, 800)]);

        // Overwrite just the beginning of an existing entry.
        mappings.insert_contiguous_mappings(m![(6..10, 900)]);
        assert_eq!(mappings, m![(6..10, 900), (10..28, 804)]);

        // Overwrite just the end of an existing entry.
        mappings.insert_contiguous_mappings(m![(24..28, 1000)]);
        assert_eq!(mappings, m![(6..10, 900), (10..24, 804), (24..28, 1000)]);
    }
}
