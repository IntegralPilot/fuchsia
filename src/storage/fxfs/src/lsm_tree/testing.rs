// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::lsm_tree::types::{
    DefaultOrdLowerBound, DefaultOrdUpperBound, FuzzyHash, LayerKey, MergeType, OrdLowerBound,
    OrdUpperBound, SortByU64,
};
use crate::serialized_types::{
    LATEST_VERSION, Version, Versioned, VersionedLatest, versioned_type,
};
use fprint::TypeFingerprint;
use fxfs_macros::{FuzzyHash, SerializeKey, impl_fuzzy_hash};
use serde::{Deserialize, Serialize};
use std::ops::Range;

/// A test key that wraps a `Range<u64>`.
///
/// This key is used across tests in `lsm_tree` to simulate range-based keys
/// (like `Extent` in production).
///
/// Invariants:
/// - `cmp_upper_bound` compares `end` first, then `start` descending (total ordering matching
///   (end, len)).
/// - `cmp_lower_bound` compares `start` only.
/// - `SortByU64` returns `end` to align with `cmp_upper_bound`.
#[derive(
    Clone,
    Eq,
    Hash,
    FuzzyHash,
    PartialEq,
    Debug,
    Serialize,
    Deserialize,
    TypeFingerprint,
    Versioned,
    SerializeKey,
)]
#[cfg_attr(fuzz, derive(arbitrary::Arbitrary))]
pub struct TestKey(pub Range<u64>);

versioned_type! { 1.. => TestKey }

impl SortByU64 for TestKey {
    fn get_leading_u64(&self) -> u64 {
        self.0.end
    }
}

impl LayerKey for TestKey {
    fn merge_type(&self) -> MergeType {
        MergeType::OptimizedMerge
    }

    fn next_key(&self) -> Option<Self> {
        Some(TestKey(self.0.end..self.0.end + 1))
    }

    fn search_key(&self) -> Option<Self> {
        Some(TestKey(self.0.start..self.0.start + 1))
    }

    fn is_search_key(&self) -> bool {
        self.0.end == self.0.start + 1
    }

    fn overlaps(&self, other: &Self) -> bool {
        self.0.start < other.0.end && self.0.end > other.0.start
    }
}

impl OrdUpperBound for TestKey {
    fn cmp_upper_bound(&self, other: &TestKey) -> std::cmp::Ordering {
        self.0.end.cmp(&other.0.end).then(other.0.start.cmp(&self.0.start))
    }
}

impl OrdLowerBound for TestKey {
    fn cmp_lower_bound(&self, other: &Self) -> std::cmp::Ordering {
        self.0.start.cmp(&other.0.start)
    }
}

// Ord is implemented to compare `start` then `end` to provide a total ordering
// that is consistent with `cmp_lower_bound` (which only compares `start`).
// This differs from `cmp_upper_bound` which compares `end` first.
impl Ord for TestKey {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.start.cmp(&other.0.start).then(self.0.end.cmp(&other.0.end))
    }
}

impl PartialOrd for TestKey {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl SortByU64 for i32 {
    fn get_leading_u64(&self) -> u64 {
        (*self as i64 - i32::MIN as i64) as u64
    }
}

impl crate::serialized_types::serialized_key::SerializeKey for i32 {
    fn serialize_key_to<B: crate::serialized_types::varint::Buffer>(
        &self,
        serializer: &mut crate::serialized_types::serialized_key::KeySerializer<'_, B>,
    ) {
        serializer.write_u64(crate::lsm_tree::types::SortByU64::get_leading_u64(self));
    }
    fn deserialize_key_from(
        deserializer: &mut crate::serialized_types::serialized_key::KeyDeserializer<'_>,
    ) -> Result<Self, anyhow::Error> {
        let val = deserializer.read_u64()?;
        Ok((val as i64 + i32::MIN as i64) as i32)
    }
}

impl DefaultOrdUpperBound for i32 {}
impl DefaultOrdLowerBound for i32 {}

impl LayerKey for i32 {
    fn merge_type(&self) -> MergeType {
        MergeType::FullMerge
    }

    fn overlaps(&self, other: &Self) -> bool {
        self == other
    }
}

impl_fuzzy_hash!(i32);
