// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Memory-efficient collections optimized for dynamically sized types.

mod packed_item;
mod packed_map;
mod packed_map_builder;
mod packed_vec;

pub use packed_item::PackedItem;
pub use packed_map::{Iter, PackedMap};
pub use packed_map_builder::PackedMapBuilder;
pub use packed_vec::PackedVec;

pub(crate) fn compute_range_indices<T, R, F>(
    len: usize,
    range: R,
    mut index_of: F,
) -> std::ops::Range<usize>
where
    R: std::ops::RangeBounds<T>,
    F: FnMut(&T) -> Result<usize, usize>,
{
    let start = match range.start_bound() {
        std::ops::Bound::Included(bound) => index_of(bound).unwrap_or_else(|e| e),
        std::ops::Bound::Excluded(bound) => match index_of(bound) {
            Ok(idx) => idx + 1,
            Err(idx) => idx,
        },
        std::ops::Bound::Unbounded => 0,
    };
    let end = match range.end_bound() {
        std::ops::Bound::Included(bound) => match index_of(bound) {
            Ok(idx) => idx + 1,
            Err(idx) => idx,
        },
        std::ops::Bound::Excluded(bound) => index_of(bound).unwrap_or_else(|e| e),
        std::ops::Bound::Unbounded => len,
    };

    let start = std::cmp::min(start, len);
    let end = std::cmp::max(start, std::cmp::min(end, len));
    start..end
}
