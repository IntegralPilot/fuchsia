// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::BLOCK_SIZE;
use anyhow::{Error, ensure};
use std::borrow::Borrow;
use std::ops::Range;

const TYPE_MASK: u64 = 0xc0000000_00000000;
const REGULAR: u64 = 0x00000000_00000000;
const SPARSE: u64 = 0x80000000_00000000;

// Regular extents are densely bit-packed into a single 64-bit hardware command (LSB 0):
//   Bits 62-63 (2 most significant bits): Type identifier (`REGULAR`)
//   Bits 32-61 (30 bits): Extent length in `BLOCK_SIZE` units (4096 bytes)
//   Bits 0-31  (32 least significant bits): Target device offset block address (in `BLOCK_SIZE`
//               units), relative to `base_device_offset`
// Therefore, the maximum contiguous chunk that can fit into a single regular command is 30 bits.
const MAX_REGULAR_EXTENT_BLOCKS: u64 = 0x3fff_ffff;

// Sparse extents pack their length into the remaining 62 bits not occupied by the type header.
const MAX_SPARSE_EXTENT_BLOCKS: u64 = !TYPE_MASK;

/// Represents a logical extent and its optional physical device starting offset.
/// Both `logical_range` boundaries and `device_offset` must always be a multiple of
/// `BLOCK_SIZE` (4096 bytes). The physical device range length is identical to `logical_range`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Extent {
    logical_range: Range<u64>,
    device_offset: Option<u64>,
}

impl Extent {
    /// Returns the logical range of this extent.
    pub fn logical_range(&self) -> Range<u64> {
        self.logical_range.clone()
    }

    /// Returns the optional physical device offset of this extent. If None, this extent is sparse.
    pub fn device_offset(&self) -> Option<u64> {
        self.device_offset
    }

    /// Returns true if this extent is sparse (unbacked by physical storage).
    pub fn is_sparse(&self) -> bool {
        self.device_offset.is_none()
    }

    /// Returns the logical length of this extent in bytes.
    pub fn len(&self) -> u64 {
        self.logical_range.end - self.logical_range.start
    }

    /// Creates a new `Extent`.
    ///
    /// # Panics
    ///
    /// Panics if `logical_range.start`, `logical_range.end`, or `device_offset`
    /// (when `Some`) is not a multiple of `BLOCK_SIZE` (4096 bytes), or if
    /// `logical_range.start > logical_range.end`.
    pub fn new(logical_range: Range<u64>, device_offset: Option<u64>) -> Self {
        Self::try_new(logical_range, device_offset).unwrap()
    }

    /// Creates a new `Extent`, returning an `Error` if the alignment is invalid.
    pub fn try_new(logical_range: Range<u64>, device_offset: Option<u64>) -> Result<Self, Error> {
        ensure!(
            logical_range.start % BLOCK_SIZE == 0 && logical_range.end % BLOCK_SIZE == 0,
            "logical_range boundaries must be a multiple of BLOCK_SIZE (4096 bytes), got {:?}",
            logical_range
        );
        ensure!(
            logical_range.start <= logical_range.end,
            "logical_range.start must be <= logical_range.end, got {:?}",
            logical_range
        );

        let length_blocks = (logical_range.end - logical_range.start) / BLOCK_SIZE;
        if device_offset.is_some() {
            ensure!(
                length_blocks <= MAX_REGULAR_EXTENT_BLOCKS,
                "Extent length bounds exceed maximum encodeable length"
            );
        } else {
            ensure!(
                length_blocks <= MAX_SPARSE_EXTENT_BLOCKS,
                "Extent length bounds exceed maximum encodeable length"
            );
        }
        Ok(Self { logical_range, device_offset })
    }
}

/// Compact in-memory representation of a single extent boundary (16 bytes).
/// By storing the ending logical offset (`end_logical_offset`), the start of extent `i`
/// is `0` for `i == 0` or `entries[i - 1].end_logical_offset` for `i > 0`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ExtentEntry {
    end_logical_offset: u64,
    device_offset: u64, // u64::MAX sentinel indicates a SPARSE (unbacked) extent.
}

impl ExtentEntry {
    const SPARSE_DEVICE_OFFSET: u64 = u64::MAX;

    fn is_sparse(&self) -> bool {
        self.device_offset == Self::SPARSE_DEVICE_OFFSET
    }
}

fn encode_regular(length_blocks: u32, target_block: u32) -> u64 {
    REGULAR | ((length_blocks as u64 & MAX_REGULAR_EXTENT_BLOCKS) << 32) | (target_block as u64)
}

fn encode_sparse(length_blocks: u64) -> u64 {
    SPARSE | (length_blocks & !TYPE_MASK)
}

/// An iterator over a subset of `Extent`s in an `Extents` container.
#[derive(Debug, Clone)]
pub struct ExtentsIterator<'a> {
    extents: &'a Extents,
    index: usize,
}

impl<'a> Iterator for ExtentsIterator<'a> {
    type Item = Extent;

    fn next(&mut self) -> Option<Self::Item> {
        if self.index < self.extents.entries.len() {
            let res = self.extents.entry_to_result(self.index);
            self.index += 1;
            Some(res)
        } else {
            None
        }
    }
}

/// Container for active mappings associated with a session.
/// Extents are stored in compact form (`ExtentEntry`, 16 bytes) in a boxed slice
/// sorted by ascending `end_logical_offset` to support clean O(log N) binary search lookups.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Extents {
    base_device_offset: u64,
    entries: Box<[ExtentEntry]>,
}

impl Extents {
    /// Returns the base device offset of this container.
    pub fn base_device_offset(&self) -> u64 {
        self.base_device_offset
    }

    /// Creates an `Extents` container from an iterator of `Extent`s, returning an `Error`
    /// if validation fails.
    pub fn try_new(
        extents: impl IntoIterator<Item = impl Borrow<Extent>>,
        base_device_offset: u64,
    ) -> Result<Self, Error> {
        let iter = extents.into_iter();
        let (lower_bound, _) = iter.size_hint();
        let mut entries = Vec::with_capacity(lower_bound);
        let mut current_logical_offset = 0u64;

        for extent in iter {
            let extent = extent.borrow();
            ensure!(
                extent.logical_range.start == current_logical_offset,
                "Extents must be contiguous and start at 0: expected start \
                 {current_logical_offset}, got {}",
                extent.logical_range.start
            );
            ensure!(
                extent.logical_range.start < extent.logical_range.end,
                "Extent logical range must be non-empty, got {:?}",
                extent.logical_range
            );
            ensure!(
                extent.logical_range.start % BLOCK_SIZE == 0
                    && extent.logical_range.end % BLOCK_SIZE == 0,
                "logical_range boundaries must be a multiple of BLOCK_SIZE ({BLOCK_SIZE} bytes), \
                 got {:?}",
                extent.logical_range
            );
            let length_blocks = extent.len() / BLOCK_SIZE;
            if let Some(dev_offset) = extent.device_offset {
                ensure!(
                    dev_offset >= base_device_offset,
                    "device_offset ({dev_offset}) must be >= base_device_offset \
                     ({base_device_offset})"
                );
                let relative_offset = dev_offset - base_device_offset;
                ensure!(
                    relative_offset % BLOCK_SIZE == 0,
                    "Relative device offset ({dev_offset} - {base_device_offset} = \
                     {relative_offset}) must be a multiple of BLOCK_SIZE ({BLOCK_SIZE} bytes)"
                );
                let target_block = relative_offset / BLOCK_SIZE;
                ensure!(
                    target_block <= u32::MAX as u64,
                    "Relative device offset block index exceeds u32::MAX"
                );
                ensure!(
                    length_blocks <= MAX_REGULAR_EXTENT_BLOCKS,
                    "Extent length bounds exceed maximum encodeable length"
                );
                current_logical_offset = extent.logical_range.end;
                entries.push(ExtentEntry {
                    end_logical_offset: current_logical_offset,
                    device_offset: dev_offset,
                });
            } else {
                ensure!(
                    length_blocks <= MAX_SPARSE_EXTENT_BLOCKS,
                    "Extent length bounds exceed maximum encodeable length"
                );
                current_logical_offset = extent.logical_range.end;
                entries.push(ExtentEntry {
                    end_logical_offset: current_logical_offset,
                    device_offset: ExtentEntry::SPARSE_DEVICE_OFFSET,
                });
            }
        }

        Ok(Self { base_device_offset, entries: entries.into_boxed_slice() })
    }

    /// Encodes this `Extents` container into 64-bit mapping descriptors relative to its base
    /// device offset.
    pub fn encode(&self) -> impl Iterator<Item = u64> + '_ {
        let mut prev_logical = 0u64;
        self.entries.iter().map(move |entry| {
            let length_blocks = (entry.end_logical_offset - prev_logical) / BLOCK_SIZE;
            prev_logical = entry.end_logical_offset;
            if entry.is_sparse() {
                encode_sparse(length_blocks)
            } else {
                let relative_offset = entry.device_offset - self.base_device_offset;
                let target_block = (relative_offset / BLOCK_SIZE) as u32;
                encode_regular(length_blocks as u32, target_block)
            }
        })
    }

    /// Encodes an `Extents` container into 64-bit mapping descriptors.
    pub fn encode_extents(extents: &Extents) -> impl Iterator<Item = u64> + '_ {
        extents.encode()
    }

    /// Encodes an `Extents` container into 64-bit mapping descriptors relative to its base
    /// device offset.
    pub fn encode_extents_with_base_offset(extents: &Extents) -> impl Iterator<Item = u64> + '_ {
        extents.encode()
    }

    /// Decodes a sequence of 64-bit mapping descriptors into a compact `Extents` container,
    /// offsetting regular extents by `base_device_offset`.
    /// Returns `None` if an unknown mapping descriptor type is encountered or if an arithmetic
    /// overflow occurs while decoding.
    pub fn from_encoded(
        encoded: impl IntoIterator<Item = u64>,
        base_device_offset: u64,
    ) -> Option<Self> {
        let iter = encoded.into_iter();
        let (lower_bound, _) = iter.size_hint();
        let mut entries = Vec::with_capacity(lower_bound);
        let mut current_logical_offset = 0u64;

        for val in iter {
            let kind = val & TYPE_MASK;
            if kind == REGULAR {
                let length_blocks = ((val & !TYPE_MASK) >> 32) as u64;
                let target_block = (val & 0xffff_ffff) as u64;
                let length_bytes = length_blocks.checked_mul(BLOCK_SIZE)?;
                current_logical_offset = current_logical_offset.checked_add(length_bytes)?;
                let device_offset =
                    base_device_offset.checked_add(target_block.checked_mul(BLOCK_SIZE)?)?;
                entries.push(ExtentEntry {
                    end_logical_offset: current_logical_offset,
                    device_offset,
                });
            } else if kind == SPARSE {
                let length_blocks = (val & !TYPE_MASK) as u64;
                let length_bytes = length_blocks.checked_mul(BLOCK_SIZE)?;
                current_logical_offset = current_logical_offset.checked_add(length_bytes)?;
                entries.push(ExtentEntry {
                    end_logical_offset: current_logical_offset,
                    device_offset: ExtentEntry::SPARSE_DEVICE_OFFSET,
                });
            } else {
                return None;
            }
        }

        Some(Self { base_device_offset, entries: entries.into_boxed_slice() })
    }

    /// Returns an iterator over all extents whose logical range ends after `start_offset`,
    /// jumping directly to the first overlapping extent in O(log N) time via binary search.
    pub fn iter_extents(&self, start_offset: u64) -> ExtentsIterator<'_> {
        let index = self.entries.partition_point(|e| e.end_logical_offset <= start_offset);
        ExtentsIterator { extents: self, index }
    }

    /// Maps a logical byte offset to the corresponding `Extent` in O(log N) time
    /// using binary search, translating logical and physical ranges to start at `offset`.
    ///
    /// # Panics
    ///
    /// Panics if `offset` is not a multiple of `BLOCK_SIZE` (4096 bytes).
    pub fn map(&self, offset: u64) -> Option<Extent> {
        assert!(
            offset % BLOCK_SIZE == 0,
            "offset must be a multiple of BLOCK_SIZE (4096 bytes), got {offset}"
        );
        let idx = self.entries.partition_point(|e| e.end_logical_offset <= offset);
        if idx >= self.entries.len() {
            return None;
        }

        let entry = &self.entries[idx];
        let start_logical = self.entry_start_offset(idx);
        let end_logical = entry.end_logical_offset;

        let offset_within = offset.checked_sub(start_logical)?;
        let device_offset = if entry.is_sparse() {
            None
        } else {
            Some(entry.device_offset.checked_add(offset_within)?)
        };

        Some(Extent { logical_range: offset..end_logical, device_offset })
    }

    /// Returns all mappings as full `Extent` structs.
    pub fn mappings(&self) -> Vec<Extent> {
        (0..self.entries.len()).map(|i| self.entry_to_result(i)).collect()
    }

    fn entry_start_offset(&self, idx: usize) -> u64 {
        if idx == 0 { 0 } else { self.entries[idx - 1].end_logical_offset }
    }

    fn entry_to_result(&self, idx: usize) -> Extent {
        let entry = &self.entries[idx];
        let start_logical = self.entry_start_offset(idx);
        let end_logical = entry.end_logical_offset;
        let device_offset = if entry.is_sparse() { None } else { Some(entry.device_offset) };
        Extent { logical_range: start_logical..end_logical, device_offset }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_decode_regular() {
        let extents = Extents::try_new(
            [
                Extent::new(0..(4 * BLOCK_SIZE), Some(10 * BLOCK_SIZE)),
                Extent::new((4 * BLOCK_SIZE)..(6 * BLOCK_SIZE), Some(100 * BLOCK_SIZE)),
            ],
            0,
        )
        .unwrap();
        let encoded = Extents::encode_extents(&extents);
        let extents_container = Extents::from_encoded(encoded, 0).expect("from_encoded failed");

        let decoded = extents_container.mappings();
        assert_eq!(decoded.len(), 2);

        assert_eq!(decoded[0].logical_range, 0..(4 * BLOCK_SIZE));
        assert_eq!(decoded[0].device_offset, Some(10 * BLOCK_SIZE));

        assert_eq!(decoded[1].logical_range, (4 * BLOCK_SIZE)..(6 * BLOCK_SIZE));
        assert_eq!(decoded[1].device_offset, Some(100 * BLOCK_SIZE));
    }

    #[test]
    fn test_encode_decode_sparse() {
        let extents = Extents::try_new(
            [
                Extent::new(0..(2 * BLOCK_SIZE), Some(50 * BLOCK_SIZE)),
                Extent::new((2 * BLOCK_SIZE)..(5 * BLOCK_SIZE), None),
                Extent::new((5 * BLOCK_SIZE)..(6 * BLOCK_SIZE), Some(200 * BLOCK_SIZE)),
            ],
            0,
        )
        .unwrap();
        let encoded = Extents::encode_extents(&extents);
        let extents_container = Extents::from_encoded(encoded, 0).expect("from_encoded failed");

        let decoded = extents_container.mappings();
        assert_eq!(decoded.len(), 3);

        assert_eq!(decoded[0].logical_range, 0..(2 * BLOCK_SIZE));
        assert_eq!(decoded[0].device_offset, Some(50 * BLOCK_SIZE));

        assert_eq!(decoded[1].logical_range, (2 * BLOCK_SIZE)..(5 * BLOCK_SIZE));
        assert_eq!(decoded[1].device_offset, None);

        assert_eq!(decoded[2].logical_range, (5 * BLOCK_SIZE)..(6 * BLOCK_SIZE));
        assert_eq!(decoded[2].device_offset, Some(200 * BLOCK_SIZE));
    }

    #[test]
    fn test_encode_decode_non_aligned_start() {
        let base_device_offset = 17408u64; // e.g. LBA 34 on 512-byte sector disk
        let extents = Extents::try_new(
            [
                Extent::new(0..(4 * BLOCK_SIZE), Some(base_device_offset)),
                Extent::new(
                    (4 * BLOCK_SIZE)..(6 * BLOCK_SIZE),
                    Some(base_device_offset + 100 * BLOCK_SIZE),
                ),
            ],
            base_device_offset,
        )
        .unwrap();
        let encoded = Extents::encode_extents_with_base_offset(&extents);
        let extents_container =
            Extents::from_encoded(encoded, base_device_offset).expect("from_encoded failed");

        let decoded = extents_container.mappings();
        assert_eq!(decoded.len(), 2);
        assert_eq!(decoded[0].logical_range, 0..(4 * BLOCK_SIZE));
        assert_eq!(decoded[0].device_offset, Some(base_device_offset));
        assert_eq!(decoded[1].logical_range, (4 * BLOCK_SIZE)..(6 * BLOCK_SIZE));
        assert_eq!(decoded[1].device_offset, Some(base_device_offset + 100 * BLOCK_SIZE));

        let mapped = extents_container.map(0).expect("should map at 0");
        assert_eq!(mapped.device_offset, Some(base_device_offset));

        let mapped_next = extents_container.map(4 * BLOCK_SIZE).expect("should map next");
        assert_eq!(mapped_next.device_offset, Some(base_device_offset + 100 * BLOCK_SIZE));
    }

    #[test]
    fn test_extents_validation_relative_offset_unaligned_fails() {
        let base_device_offset = 17408u64;
        // 17408 + 500 is not aligned to BLOCK_SIZE relative to base_device_offset
        let result = Extents::try_new(
            [Extent::new(0..BLOCK_SIZE, Some(base_device_offset + 500))],
            base_device_offset,
        );
        assert!(result.is_err());
        assert!(
            result.unwrap_err().to_string().contains("Relative device offset"),
            "Error should mention relative device offset"
        );
    }

    #[test]
    fn test_extents_validation_device_offset_smaller_than_base_fails() {
        let base_device_offset = 17408u64;
        let result = Extents::try_new([Extent::new(0..BLOCK_SIZE, Some(0))], base_device_offset);
        assert!(result.is_err());
        assert!(
            result.unwrap_err().to_string().contains("must be >= base_device_offset"),
            "Error should mention dev_offset >= base_device_offset"
        );
    }

    #[test]
    fn test_extents_validation_non_contiguous_logical_fails() {
        let result = Extents::try_new(
            [
                Extent::new(0..BLOCK_SIZE, Some(0)),
                Extent::new((2 * BLOCK_SIZE)..(3 * BLOCK_SIZE), Some(BLOCK_SIZE)),
            ],
            0,
        );
        assert!(result.is_err());
        assert!(
            result.unwrap_err().to_string().contains("must be contiguous and start at 0"),
            "Error should mention non-contiguous start"
        );
    }

    #[test]
    fn test_binary_search_map_logical_offset() {
        let extents = Extents::try_new(
            [
                Extent::new(0..(10 * BLOCK_SIZE), Some(100 * BLOCK_SIZE)),
                Extent::new((10 * BLOCK_SIZE)..(20 * BLOCK_SIZE), Some(200 * BLOCK_SIZE)),
                Extent::new((20 * BLOCK_SIZE)..(30 * BLOCK_SIZE), Some(300 * BLOCK_SIZE)),
            ],
            0,
        )
        .unwrap();
        let encoded = Extents::encode_extents(&extents);
        let extents_container = Extents::from_encoded(encoded, 0).expect("from_encoded failed");

        let mapped = extents_container.map(0).expect("should map at offset 0");
        assert_eq!(mapped.logical_range, 0..(10 * BLOCK_SIZE));
        assert_eq!(mapped.device_offset, Some(100 * BLOCK_SIZE));

        let mapped_mid = extents_container
            .map(12 * BLOCK_SIZE)
            .expect("should map inside second extent via binary search");
        assert_eq!(mapped_mid.logical_range, (12 * BLOCK_SIZE)..(20 * BLOCK_SIZE));
        assert_eq!(mapped_mid.device_offset, Some(202 * BLOCK_SIZE));
    }

    #[test]
    fn test_map_out_of_bounds() {
        let extents =
            Extents::try_new([Extent::new(0..(2 * BLOCK_SIZE), Some(10 * BLOCK_SIZE))], 0).unwrap();
        let encoded = Extents::encode_extents(&extents);
        let extents_container = Extents::from_encoded(encoded, 0).expect("from_encoded failed");

        assert!(extents_container.map(2 * BLOCK_SIZE).is_none());
        assert!(extents_container.map(100 * BLOCK_SIZE).is_none());
    }

    #[test]
    fn test_binary_search_iter_extents() {
        let extents = Extents::try_new(
            [
                Extent::new(0..(2 * BLOCK_SIZE), Some(10 * BLOCK_SIZE)),
                Extent::new((2 * BLOCK_SIZE)..(4 * BLOCK_SIZE), Some(20 * BLOCK_SIZE)),
                Extent::new((4 * BLOCK_SIZE)..(6 * BLOCK_SIZE), Some(30 * BLOCK_SIZE)),
            ],
            0,
        )
        .unwrap();
        let encoded = Extents::encode_extents(&extents);
        let extents_container = Extents::from_encoded(encoded, 0).expect("from_encoded failed");

        let results: Vec<_> = extents_container.iter_extents(3 * BLOCK_SIZE).collect();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].logical_range, (2 * BLOCK_SIZE)..(4 * BLOCK_SIZE));
        assert_eq!(results[1].logical_range, (4 * BLOCK_SIZE)..(6 * BLOCK_SIZE));
    }

    #[test]
    fn test_exact_boundary_queries() {
        let extents = Extents::try_new(
            [
                Extent::new(0..(10 * BLOCK_SIZE), Some(100 * BLOCK_SIZE)),
                Extent::new((10 * BLOCK_SIZE)..(20 * BLOCK_SIZE), Some(200 * BLOCK_SIZE)),
            ],
            0,
        )
        .unwrap();
        let encoded = Extents::encode_extents(&extents);
        let extents_container = Extents::from_encoded(encoded, 0).expect("from_encoded failed");

        let mapped = extents_container.map(10 * BLOCK_SIZE).expect("should map at exact boundary");
        assert_eq!(mapped.logical_range, (10 * BLOCK_SIZE)..(20 * BLOCK_SIZE));
        assert_eq!(mapped.device_offset, Some(200 * BLOCK_SIZE));

        let results: Vec<_> = extents_container.iter_extents(10 * BLOCK_SIZE).collect();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].logical_range, (10 * BLOCK_SIZE)..(20 * BLOCK_SIZE));
    }

    #[test]
    #[should_panic(expected = "multiple of BLOCK_SIZE")]
    fn test_extent_new_unaligned_logical_start_panics() {
        Extent::new(1..(2 * BLOCK_SIZE), Some(10 * BLOCK_SIZE));
    }

    #[test]
    #[should_panic(expected = "multiple of BLOCK_SIZE")]
    fn test_extent_new_unaligned_logical_end_panics() {
        Extent::new(0..(2 * BLOCK_SIZE + 1), Some(10 * BLOCK_SIZE));
    }

    #[test]
    #[should_panic(expected = "multiple of BLOCK_SIZE")]
    fn test_map_unaligned_offset_panics() {
        Extents::default().map(500);
    }

    #[test]
    fn test_iter_extents_unaligned_start_offset() {
        let extents = Extents::try_new(
            [
                Extent::new(0..(10 * BLOCK_SIZE), Some(100 * BLOCK_SIZE)),
                Extent::new((10 * BLOCK_SIZE)..(20 * BLOCK_SIZE), Some(200 * BLOCK_SIZE)),
            ],
            0,
        )
        .unwrap();
        let encoded = Extents::encode_extents(&extents);
        let extents_container = Extents::from_encoded(encoded, 0).expect("from_encoded failed");
        let results: Vec<_> = extents_container.iter_extents(500).collect();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].logical_range, 0..(10 * BLOCK_SIZE));
    }

    #[test]
    fn test_encode_extents_regular_length_overflow_errors() {
        let result = Extent::try_new(
            0..((MAX_REGULAR_EXTENT_BLOCKS + 1) * BLOCK_SIZE),
            Some(10 * BLOCK_SIZE),
        );
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().to_string(),
            "Extent length bounds exceed maximum encodeable length"
        );
    }

    #[test]
    fn test_from_encoded_unknown_kind_returns_none() {
        let unknown_descriptor = 0x40000000_00000000;
        assert!(Extents::from_encoded([unknown_descriptor], 0).is_none());
    }
}
