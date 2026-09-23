// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Contains [`ExtentMappingIterator`] which is intended for remapping extent records from an
//! "inner" volume into a parent "outer" volume. The inner volume must be written as a contiguous
//! file within the outer volume, and the outer volume must not contain any other object records.
//!
//! This allows installation of objects from the inner volume into the outer volume without copying
//! extent data. When this process is complete, there may be unused portions of the backing file
//! which may be safely deallocated.
//!
//! *NOTE*: Due to fragmentation of the inner file, extent records may need to be split when
//! installing them in the outer volume. See [`ExtentMappingIterator`] for details.

use crate::errors::FxfsError;
use crate::object_store::{
    AttributeKey, ExtentValue, FileExtent, ItemRef, LayerIterator, ObjectKey, ObjectKeyData,
    ObjectValue,
};
use crate::range::RangeExt as _;
use anyhow::{Context as _, Error};
use futures::future::BoxFuture;
use std::num::NonZero;
use std::ops::Range;

/// Implementation of an iterator that maps extent records from an inner volume to their physical
/// on-device locations. The inner volume is expected to be stored within a contiguous file owned by
/// the outer volume. This can be used to "install" object records from the inner volume directly
/// into the outer volume without copying extent data.
///
/// *NOTE*: Extent records in the inner file may be fragmented when being emitted as the storage
/// backing them may not be contiguous. For example, let's say we want to install a file which
/// occupies a single extent, say from block 0 to 10. It may be backed on disk by blocks 0-5 and
/// 15-20, so we will need to emit two extent records for the inner file when it is installed:
///
///               0          5          10         15         20
///     inner:    |xxxxxxxxxx|yyyyyyyyyy|----------|----------|
///     outer:    |xxxxxxxxxx|----------|----------|yyyyyyyyyy|
///
/// Thus while in the inner volume the file originally had a single extent (0-10), when installed,
/// it will have two extents backing that same source range (0-5, 15-20).
#[derive(Debug)]
pub struct ExtentMappingIterator<I: LayerIterator<ObjectKey, ObjectValue>> {
    /// The inner objects which we're going to iterate over and remap/split.
    inner_iterator: I,
    /// The extents which back the objects referenced by [`Self::inner_iterator`]. These objects are
    /// stored within a file backed by these extents. This means the device offsets of these extents
    /// refer to the *logical* offset within the backing file.
    backing_extents: Vec<FileExtent>,
    /// Determines what to emit for the current item pointed to by [`Self::inner_iterator`].
    state: MappingState,
}

/// The state of the [`ExtentMappingIterator`], which determines how to transform the underlying
/// record pointed to by [`ExtentMappingIterator::inner_iterator`].
#[derive(Debug)]
enum MappingState {
    /// The inner item, if any, should be emitted without modification.
    Passthrough,
    /// The inner item should be emitted with a remapped device offset.
    Remap { value: ObjectValue },
    /// The inner item is being split into multiple records because the backing extent is too small
    /// to cover the inner extent. `next_extent_idx` refers to the *next* backing extent we should
    /// use for the remaining range, or None if this is the last fragment we will emit.
    Split { key: ObjectKey, value: ObjectValue, next_extent_idx: Option<NonZero<usize>> },
}

impl<I: LayerIterator<ObjectKey, ObjectValue>> ExtentMappingIterator<I> {
    pub fn new(inner_iterator: I, backing_extents: Vec<FileExtent>) -> Result<Self, Error> {
        Self::validate_extents(&backing_extents)?;
        let mut this = Self { inner_iterator, backing_extents, state: MappingState::Passthrough };
        this.process_item()?;
        Ok(this)
    }

    /// Validates that there is at least one backing extent, that all backing extents are
    /// logically contiguous, and that the first extent starts from offset zero.
    fn validate_extents(backing_extents: &Vec<FileExtent>) -> Result<(), Error> {
        // We expect the inner volume to be backed by at least one extent.
        if backing_extents.is_empty() {
            return Err(FxfsError::Inconsistent).context("backing extents are empty");
        }
        if backing_extents[0].logical_offset() != 0 {
            return Err(FxfsError::Inconsistent)
                .context("first extents should start from offset 0");
        }
        // The backing extents should be logically contiguous (i.e. there should be no holes).
        for [prev, next] in backing_extents.array_windows() {
            if prev.logical_range().end != next.logical_range().start {
                return Err(FxfsError::Inconsistent).context("backing extents must be contiguous");
            }
        }
        Ok(())
    }

    /// Finds the backing extent for a given logical offset within the file.
    fn find_backing_extent(&self, logical_offset: u64) -> Result<(usize, &FileExtent), Error> {
        let index = match self
            .backing_extents
            .binary_search_by(|e| e.logical_offset().cmp(&logical_offset))
        {
            Ok(i) => i,
            Err(i) => {
                if i == 0 {
                    return Err(FxfsError::Inconsistent)
                        .context("inner device offset precedes all backing file extents");
                }
                i - 1
            }
        };
        let backing_extent = &self.backing_extents[index];
        let backed_logical_range = backing_extent.logical_range();
        if logical_offset >= backed_logical_range.end {
            return Err(FxfsError::Inconsistent)
                .context("inner device offset exceeds bounds of backing file");
        }
        Ok((index, backing_extent))
    }

    fn process_item(&mut self) -> Result<(), Error> {
        // If we have no more items to handle, stay in the passthrough state.
        let Some(inner_item) = self.inner_iterator.get() else {
            self.state = MappingState::Passthrough;
            return Ok(());
        };
        // Passthrough all items that aren't extent records.
        if !matches!(inner_item.value, ObjectValue::Extent(ExtentValue::Some { .. })) {
            self.state = MappingState::Passthrough;
            return Ok(());
        }
        // Find the extent backing the device offset of the inner extent. This is equivalent to the
        // *logical* offset within the backing file.
        let logical_offset = *inner_item.value.device_offset();
        let (backing_extent_idx, backing_extent) = self.find_backing_extent(logical_offset)?;
        // Remap the inner device offset to match the physical location on disk in the backing file.
        let mut value = inner_item.value.clone();
        *value.device_offset_mut() = backing_extent.device_range().start
            + (logical_offset - backing_extent.logical_offset());
        // If the backing extent is large enough to cover the entire inner extent, we can emit the
        // remapped record directly. If it's too small, we need to split the record up.
        let available_backing_len = backing_extent.logical_range().end - logical_offset;
        let remaining_inner_len = inner_item.key.extent_range().length()?;
        let state = if available_backing_len >= remaining_inner_len {
            MappingState::Remap { value }
        } else {
            let mut key = inner_item.key.clone();
            let extent_range = key.extent_range_mut();
            extent_range.end = extent_range.start + available_backing_len;
            if (backing_extent_idx + 1) >= self.backing_extents.len() {
                return Err(FxfsError::Inconsistent).context("not enough extents in backing file");
            }
            let next_extent_idx = NonZero::new(backing_extent_idx + 1);
            MappingState::Split { key, value, next_extent_idx }
        };
        self.state = state;
        Ok(())
    }

    fn update_split(&mut self) -> Result<(), Error> {
        let MappingState::Split { ref mut key, ref mut value, ref mut next_extent_idx } =
            self.state
        else {
            unreachable!()
        };
        let backing_extent_idx = next_extent_idx.unwrap().get();
        let backing_extent = &self.backing_extents[backing_extent_idx];
        // Update the range key and extent value (device offset) accordingly.
        let extent_range = key.extent_range_mut();
        extent_range.start = extent_range.end;
        extent_range.end = self.inner_iterator.get().unwrap().key.extent_range().end;
        *value.device_offset_mut() = backing_extent.device_range().start;
        // Finish splitting if the next extent covers the remainder, otherwise continue splitting.
        let remaining_inner_len = extent_range.end - extent_range.start;
        if backing_extent.length() >= remaining_inner_len {
            *next_extent_idx = None;
        } else {
            extent_range.end = extent_range.start + backing_extent.length();
            if (backing_extent_idx + 1) >= self.backing_extents.len() {
                return Err(FxfsError::Inconsistent).context("not enough extents in backing file");
            }
            *next_extent_idx = NonZero::new(backing_extent_idx + 1);
        };
        Ok(())
    }
}

impl<I: LayerIterator<ObjectKey, ObjectValue>> LayerIterator<ObjectKey, ObjectValue>
    for ExtentMappingIterator<I>
{
    async fn advance(&mut self) -> Result<(), Error> {
        if matches!(self.state, MappingState::Split { next_extent_idx: Some(_), .. }) {
            self.update_split()
        } else {
            self.inner_iterator.advance().await?;
            self.process_item()
        }
    }

    fn advance_dyn<'a>(&'a mut self) -> Result<Option<BoxFuture<'a, Result<(), Error>>>, Error> {
        Ok(Some(Box::pin(self.advance())))
    }

    fn get(&self) -> Option<ItemRef<'_, ObjectKey, ObjectValue>> {
        self.inner_iterator.get().map(|inner_item| match &self.state {
            MappingState::Passthrough => inner_item,
            MappingState::Remap { value } => ItemRef { key: inner_item.key, value },
            MappingState::Split { key, value, .. } => ItemRef { key, value },
        })
    }
}

/// Extension trait to more concisely obtain a reference to the range of an extent key.
trait ExtentExt {
    /// Reference to the logical range for this extent. Will panic if key is not an extent.
    fn extent_range(&self) -> &Range<u64>;

    /// Mutable reference to the logical range for this extent. Will panic if key is not an extent.
    fn extent_range_mut(&mut self) -> &mut Range<u64>;
}

impl ExtentExt for ObjectKey {
    fn extent_range(&self) -> &Range<u64> {
        let ObjectKey { data: ObjectKeyData::Attribute(_, AttributeKey::Extent(extent)), .. } =
            self
        else {
            unreachable!()
        };
        extent
    }

    fn extent_range_mut(&mut self) -> &mut Range<u64> {
        let ObjectKey { data: ObjectKeyData::Attribute(_, AttributeKey::Extent(extent)), .. } =
            self
        else {
            unreachable!()
        };
        extent
    }
}

/// Extension trait to more concisely obtain a reference to the device offset of an extent value.
trait ExtentValueExt {
    /// Reference to the device offset for this extent. Will panic if not an extent.
    fn device_offset(&self) -> &u64;

    /// Mutable reference to the device offset for this extent. Will panic if not an extent.
    fn device_offset_mut(&mut self) -> &mut u64;
}

impl ExtentValueExt for ObjectValue {
    fn device_offset(&self) -> &u64 {
        let ObjectValue::Extent(ExtentValue::Some { device_offset, .. }) = self else {
            unreachable!()
        };
        device_offset
    }

    fn device_offset_mut(&mut self) -> &mut u64 {
        let ObjectValue::Extent(ExtentValue::Some { device_offset, .. }) = self else {
            unreachable!()
        };
        device_offset
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::object_store::{AttributeId, ObjectDescriptor};

    /// Simple test iterator that yields records from a slice of object key/value pairs.
    #[derive(Debug, Default)]
    struct TestIterator<'a> {
        objects: &'a [(ObjectKey, ObjectValue)],
        index: usize,
    }

    impl<'a> LayerIterator<ObjectKey, ObjectValue> for TestIterator<'a> {
        async fn advance(&mut self) -> Result<(), Error> {
            let _ = self.advance_dyn()?;
            Ok(())
        }

        fn advance_dyn<'b>(
            &'b mut self,
        ) -> Result<Option<BoxFuture<'b, Result<(), Error>>>, Error> {
            self.index = std::cmp::min(self.index + 1, self.objects.len());
            Ok(None)
        }

        fn get(&self) -> Option<ItemRef<'_, ObjectKey, ObjectValue>> {
            if self.index >= self.objects.len() {
                None
            } else {
                let (key, value) = &self.objects[self.index];
                Some(ItemRef { key, value })
            }
        }
    }

    /// Helper function that verifies the correct set of object records are emitted.
    async fn validate_extent_mapping(
        inner_objects: &[(ObjectKey, ObjectValue)],
        backing_extents: &[FileExtent],
        expected_output: &[(ObjectKey, ObjectValue)],
    ) {
        let mut iter = ExtentMappingIterator::new(
            TestIterator { objects: inner_objects, index: 0 },
            backing_extents.to_vec(),
        )
        .unwrap();

        for (expected_key, expected_value) in expected_output {
            let ItemRef { key, value, .. } =
                iter.get().expect("iterator did not yield enough items");
            assert_eq!(key, expected_key);
            assert_eq!(value, expected_value);
            iter.advance().await.unwrap();
        }
        assert!(iter.get().is_none(), "iterator yielded too many items");
    }

    /// Verifies that a single extent in the inner file is split into two extents when backed by
    /// discontiguous extents on the physical device.
    ///
    ///          File (Logical View)
    /// +--------------------------------------+
    /// |         Object 1 (0..100)            |
    /// +--------------------------------------+
    /// 0                  50                100
    ///
    ///          Device (Physical View)
    /// +------------------+ ... +--------------------+
    /// | Object 1 (0..50) |     | Object 1 (50..100) |
    /// +------------------+ ... +--------------------+
    /// 1000             1050   2000               2050
    ///
    #[fuchsia::test]
    async fn test_extent_mapper_split() {
        let inner_objects =
            [(ObjectKey::extent(1, AttributeId::TEST_ID, 0..100), ObjectValue::extent(0, 0))];
        let backing_extents = [
            FileExtent::new(/*logical_offset*/ 0, /*device_range*/ 1000..1050).unwrap(),
            FileExtent::new(/*logical_offset*/ 50, /*device_range*/ 2000..2050).unwrap(),
        ];
        let expected_objects = [
            // Object 1 should be split since it's backed by a non-contiguous physical range.
            (ObjectKey::extent(1, AttributeId::TEST_ID, 0..50), ObjectValue::extent(1000, 0)),
            (ObjectKey::extent(1, AttributeId::TEST_ID, 50..100), ObjectValue::extent(2000, 0)),
        ];

        validate_extent_mapping(&inner_objects, &backing_extents, &expected_objects).await;
    }

    /// Verifies that we preserve original fragmentation when remapping, and only introduce new
    /// fragments when required.
    ///
    ///          File (Logical View)
    /// +------------------+------------------------------------------------------+
    /// |  Obj 1 (0..100)  |  Obj 2 (0..50)  | Obj 2 (50..100) | Obj 2 (100..150) |
    /// +------------------+------------------------------------------------------+
    /// 0                 100               150               200               250
    ///
    ///          Device (Physical View)
    /// +---------------+ ... +-----------------+ ... +------------------------------+ ... +
    /// | Obj 1 (0..50) |     | Obj 1 (50..100) |     |        Obj 2 (0..150)        |     |
    /// +---------------+ ... +-----------------+ ... +------------------------------+ ... +
    /// 1000         1050     2000           2050     3000                        3150    4000
    ///
    /// Obj 2 should end up with 3 extents covering ranges 3000..3050, 3050..3100, and 3100..3150.
    #[fuchsia::test]
    async fn test_extent_mapper_preserves_existing_fragments() {
        let inner_objects = [
            (ObjectKey::extent(1, AttributeId::TEST_ID, 0..100), ObjectValue::extent(0, 0)),
            (ObjectKey::extent(2, AttributeId::TEST_ID, 0..50), ObjectValue::extent(100, 0)),
            (ObjectKey::extent(2, AttributeId::TEST_ID, 50..100), ObjectValue::extent(150, 0)),
            (ObjectKey::extent(2, AttributeId::TEST_ID, 100..150), ObjectValue::extent(200, 0)),
        ];

        let backing_extents = [
            // Contains object 1's data from range 0..50
            FileExtent::new(/*logical_offset*/ 0, /*device_range*/ 1000..1050).unwrap(),
            // Contains object 1's data from range 50..100
            FileExtent::new(/*logical_offset*/ 50, /*device_range*/ 2000..2050).unwrap(),
            // Contains object 2's data from range 0..150.
            FileExtent::new(/*logical_offset*/ 100, /*device_range*/ 3000..4000).unwrap(),
        ];

        let expected_objects = [
            // Object 1 should be split since it's backed by a non-contiguous physical range.
            (ObjectKey::extent(1, AttributeId::TEST_ID, 0..50), ObjectValue::extent(1000, 0)),
            (ObjectKey::extent(1, AttributeId::TEST_ID, 50..100), ObjectValue::extent(2000, 0)),
            // Object 2 is covered by the remaining range starting from 3000. We don't handle
            // defragmentation at this point, so although both the logical and physical ranges
            // backing this object are contiguous, we still emit three extent records. These should
            // be merged upon the next compaction for us.
            (ObjectKey::extent(2, AttributeId::TEST_ID, 0..50), ObjectValue::extent(3000, 0)),
            (ObjectKey::extent(2, AttributeId::TEST_ID, 50..100), ObjectValue::extent(3050, 0)),
            (ObjectKey::extent(2, AttributeId::TEST_ID, 100..150), ObjectValue::extent(3100, 0)),
        ];

        validate_extent_mapping(&inner_objects, &backing_extents, &expected_objects).await;
    }

    /// Verifies that we correctly map extents if they aren't physically laid out in order.
    #[fuchsia::test]
    async fn test_extent_mapper_out_of_order() {
        let inner_objects =
            [(ObjectKey::extent(1, AttributeId::TEST_ID, 0..100), ObjectValue::extent(0, 0))];
        let backing_extents = [
            FileExtent::new(/*logical_offset*/ 0, /*device_range*/ 2000..2050).unwrap(),
            FileExtent::new(/*logical_offset*/ 50, /*device_range*/ 1000..1050).unwrap(),
        ];
        let expected_objects = [
            (ObjectKey::extent(1, AttributeId::TEST_ID, 0..50), ObjectValue::extent(2000, 0)),
            (ObjectKey::extent(1, AttributeId::TEST_ID, 50..100), ObjectValue::extent(1000, 0)),
        ];

        validate_extent_mapping(&inner_objects, &backing_extents, &expected_objects).await;
    }

    /// Verifies that non-extent records are passed through the iterator unmodified.
    #[fuchsia::test]
    async fn test_extent_mapper_emits_other_records_unmodified() {
        use crate::object_store::DirType;
        let objects = [
            (
                ObjectKey::child(1, "test", DirType::Normal),
                ObjectValue::child(1, ObjectDescriptor::Directory),
            ),
            (ObjectKey::graveyard_entry(2, 3), ObjectValue::None),
        ];
        // The inner volume itself must be backed by at least one extent, even if the object records
        // within do not have any extent records.
        let backing_extents =
            [FileExtent::new(/*logical_offset*/ 0, /*device_range*/ 2000..2050).unwrap()];
        // We expect the same set of input/output objects since we don't have any extent records.
        validate_extent_mapping(&objects, &backing_extents, &objects).await;
    }

    /// We should fail to create the iterator if the backing extents aren't contiguous.
    #[fuchsia::test]
    async fn test_backing_extents_must_be_contiguous() {
        let backing_extents = [
            FileExtent::new(/*logical_offset*/ 0, /*device_range*/ 2000..2050).unwrap(),
            FileExtent::new(/*logical_offset*/ 100, /*device_range*/ 1000..1050).unwrap(),
        ];
        let e = ExtentMappingIterator::new(
            TestIterator { objects: &[], index: 0 },
            backing_extents.to_vec(),
        )
        .unwrap_err();
        assert_eq!(e.downcast::<FxfsError>().unwrap(), FxfsError::Inconsistent);
    }

    /// We should fail to create the iterator without any backing extents.
    #[fuchsia::test]
    async fn test_backing_extents_are_not_empty() {
        // There should be at least one extent to make the iterator.
        let e = ExtentMappingIterator::new(TestIterator { objects: &[], index: 0 }, vec![])
            .unwrap_err();
        assert_eq!(e.downcast::<FxfsError>().unwrap(), FxfsError::Inconsistent);
    }

    /// The iterator should check that there are enough backing extents to emit a split record.
    #[fuchsia::test]
    async fn test_extent_mapper_backing_extents_too_small() {
        let inner_objects =
            [(ObjectKey::extent(1, AttributeId::TEST_ID, 0..100), ObjectValue::extent(0, 0))];
        let backing_extents =
            [FileExtent::new(/*logical_offset*/ 0, /*device_range*/ 1000..1050).unwrap()];
        let e = ExtentMappingIterator::new(
            TestIterator { objects: &inner_objects, index: 0 },
            backing_extents.to_vec(),
        )
        .unwrap_err();
        assert_eq!(e.downcast::<FxfsError>().unwrap(), FxfsError::Inconsistent);
    }
}
