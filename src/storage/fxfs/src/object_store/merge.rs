// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::object_record::{
    AttributeId, AttributeKey, ObjectKey, ObjectKeyData, ObjectValue, ProjectProperty,
};
use super::{Extent, ExtentValue};
use crate::log::*;
use crate::lsm_tree::merge::ItemOp::{Discard, Keep, Replace};
use crate::lsm_tree::merge::{MergeLayerIterator, MergeResult};
use crate::lsm_tree::types::Item;

fn merge_extents(
    object_id: u64,
    attribute_id: AttributeId,
    left: &MergeLayerIterator<'_, ObjectKey, ObjectValue>,
    right: &MergeLayerIterator<'_, ObjectKey, ObjectValue>,
    left_key: &Extent,
    right_key: &Extent,
    left_value: &ExtentValue,
    right_value: &ExtentValue,
) -> MergeResult<ObjectKey, ObjectValue> {
    // For now, we don't support/expect two extents with the same key in one layer.
    // One reason you can't merge deleted extents in non-adjacent layers is because you
    // can't decide which layer the merge should end up in.
    //
    // Consider this scenario:
    //   |X-X-X|
    //      |a-a-a|
    //         |X-X-X|
    // If you merge the deleted extents here, you might end up with:
    //   |X-X-X-X-X-X|
    //      |a-a-a|
    // which is clearly incorrect.
    debug_assert!(right.layer_index != left.layer_index);

    if let (ExtentValue::None, ExtentValue::None) = (left_value, right_value) {
        if (left.layer_index as i32 - right.layer_index as i32).abs() == 1 {
            // Two deletions in adjacent layers can be merged.
            return merge_deleted_extents(object_id, attribute_id, left_key, right_key);
        }
    }

    if left_key.end <= right_key.start {
        // Extents don't overlap.
        return MergeResult::EmitLeft;
    }

    // The start of the left extent is <= the start of the right extent, due to merge key ordering.
    //
    // One of the extents has to win. The way we break this tie is by picking the extent from the
    // newest layer (i.e. the layer with the lowest index).
    //
    // Generally, we'll be doing the following:
    //
    //  Old  |----------|
    //  New          |----------|
    //
    // Turns into
    //
    //  Emit  |------|
    //  Old          |--|
    //  New          |----------|

    if right.layer_index < left.layer_index {
        // Right layer is newer.
        debug_assert!(left_key.start < right_key.start);
        return MergeResult::Other {
            emit: Some(
                Item::new(
                    ObjectKey::extent(object_id, attribute_id, left_key.start..right_key.start),
                    ObjectValue::Extent(
                        left_value.shrunk(
                            left_key.end - left_key.start,
                            right_key.start - left_key.start,
                        ),
                    ),
                )
                .boxed(),
            ),
            left: Replace(
                Item::new(
                    ObjectKey::extent(object_id, attribute_id, right_key.start..left_key.end),
                    ObjectValue::Extent(left_value.offset_by(
                        right_key.start - left_key.start,
                        left_key.end - left_key.start,
                    )),
                )
                .boxed(),
            ),
            right: Keep,
        };
    }
    // Left layer is newer.
    if left_key.end >= right_key.end {
        // The left key entirely contains the right key.
        return MergeResult::Other { emit: None, left: Keep, right: Discard };
    }
    MergeResult::Other {
        emit: None,
        left: Keep,
        right: Replace(
            Item::new(
                ObjectKey::extent(object_id, attribute_id, left_key.end..right_key.end),
                ObjectValue::Extent(
                    right_value
                        .offset_by(left_key.end - right_key.start, right_key.end - right_key.start),
                ),
            )
            .boxed(),
        ),
    }
}

// Assumes that the two extents to be merged are on adjacent layers (i.e. layers N, N+1).
fn merge_deleted_extents(
    object_id: u64,
    attribute_id: AttributeId,
    left_key: &Extent,
    right_key: &Extent,
) -> MergeResult<ObjectKey, ObjectValue> {
    if left_key.end < right_key.start {
        // The extents are not adjacent or overlapping.
        return MergeResult::EmitLeft;
    }
    // Both of these are deleted extents which are either adjacent or overlapping, which means
    // we can coalesce the records.
    if left_key.end >= right_key.end {
        // The left deletion eclipses the right, so just keep the left.
        return MergeResult::Other { emit: None, left: Keep, right: Discard };
    }
    MergeResult::Other {
        emit: None,
        left: Discard,
        right: Replace(Box::new(Item::new(
            ObjectKey::extent(object_id, attribute_id, left_key.start..right_key.end),
            ObjectValue::deleted_extent(),
        ))),
    }
}

/// Merge function for items in the object store.
///
/// The most interesting behaviour in this merge function is how extents are handled. Since extents
/// can overlap and replace one another, the merge function generally builds up the most
/// recent view of the extents in the tree, so that the output of a full merge contains no
/// overlapping extents. You can imagine looking down at the extents from the top-most layer.
///
/// A brief example:
///
/// Layer 0   |a-a-a-a|     |b-b-b|
/// Layer 1   |c-c-c-c-c|
/// Layer 2                     |d-d-d-d|
///
/// Merged    |a-a-a-a|c|   |b-b-b|d-d-d|
///
/// Adjacent or overlapping extent deletions in two adjacent layers can be merged into single
/// records (since they do not have a physical offset, so there's no need to keep the physical
/// extents contiguous). We can't merge deletions from non-adjacent layers, since that would
/// cause issues in situations like this:
///
/// Layer 0         |X-X-X|
/// Layer 1   |a-a-a-a-a-a|
/// Layer 2   |X-X-X|
///
/// Merging the two deletions in layers 0 and 2 would either result in the middle extent being
/// fully occluded or not at all (depending on whether we replaced on the left or right layer).
pub fn merge(
    left: &MergeLayerIterator<'_, ObjectKey, ObjectValue>,
    right: &MergeLayerIterator<'_, ObjectKey, ObjectValue>,
) -> MergeResult<ObjectKey, ObjectValue> {
    if left.key().object_id != right.key().object_id {
        return MergeResult::EmitLeft;
    }
    match (left.key(), right.key(), left.value(), right.value()) {
        (
            ObjectKey {
                object_id,
                data: ObjectKeyData::Attribute(left_attr_id, AttributeKey::Extent(left_extent_key)),
            },
            ObjectKey {
                object_id: _,
                data:
                    ObjectKeyData::Attribute(right_attr_id, AttributeKey::Extent(right_extent_key)),
            },
            ObjectValue::Extent(left_extent),
            ObjectValue::Extent(right_extent),
        ) if left_attr_id == right_attr_id => {
            return merge_extents(
                *object_id,
                *left_attr_id,
                left,
                right,
                left_extent_key,
                right_extent_key,
                left_extent,
                right_extent,
            );
        }
        (
            ObjectKey {
                object_id: _,
                data:
                    ObjectKeyData::Project {
                        project_id: left_project_id,
                        property: ProjectProperty::Usage,
                    },
            },
            ObjectKey {
                object_id: _,
                data:
                    ObjectKeyData::Project {
                        project_id: right_project_id,
                        property: ProjectProperty::Usage,
                    },
            },
            ObjectValue::BytesAndNodes { bytes: left_bytes, nodes: left_nodes },
            ObjectValue::BytesAndNodes { bytes: right_bytes, nodes: right_nodes },
        ) if left_project_id == right_project_id => {
            let bytes = left_bytes + right_bytes;
            let nodes = left_nodes + right_nodes;
            // Tombstone the tracking when it goes to zero.
            match (bytes, nodes) {
                (0, 0) => MergeResult::Other { emit: None, left: Discard, right: Discard },
                _ => MergeResult::Other {
                    emit: None,
                    left: Discard,
                    right: Replace(
                        Item::new(right.key().clone(), ObjectValue::BytesAndNodes { bytes, nodes })
                            .boxed(),
                    ),
                },
            }
        }
        // Tombstones (ObjectKeyData::Object) compare before others, so always appear on left.
        (ObjectKey { data: ObjectKeyData::Object, .. }, _, ObjectValue::None, _) => {
            if left.layer_index > right.layer_index {
                warn!("Detected inconsistency: record has been inserted after tombstone");
                MergeResult::EmitLeft
            } else {
                MergeResult::Other { emit: None, left: Keep, right: Discard }
            }
        }
        // Note that identical keys are sorted by layer_index, so left is always newer.
        (left_key, right_key, _, _) if left_key == right_key => {
            debug_assert!(left.layer_index < right.layer_index);
            MergeResult::Other { emit: None, left: Keep, right: Discard }
        }
        _ => MergeResult::EmitLeft,
    }
}

#[cfg(test)]
mod tests {
    use super::merge;
    use crate::checksum::Checksums;
    use crate::lsm_tree::types::{Item, LayerIterator, MergeableKey, Value};
    use crate::lsm_tree::{LSMTree, Query};
    use crate::object_store::extent::MIN_BLOCK_SIZE;
    use crate::object_store::extent_record::ExtentValue;
    use crate::object_store::object_record::{AttributeKey, ObjectKey, ObjectValue, Timestamp};
    use crate::object_store::{AttributeId, ProjectId, VOLUME_DATA_KEY_ID};
    use anyhow::Error;

    async fn test_merge<K: MergeableKey, V: Value + PartialEq>(
        tree: &LSMTree<K, V>,
        layer0: &[Item<K, V>],
        layer1: &[Item<K, V>],
        expected: &[Item<K, V>],
    ) {
        for item in layer1 {
            tree.insert(item.clone()).expect("insert error");
        }
        tree.seal();
        for item in layer0 {
            tree.insert(item.clone()).expect("insert error");
        }
        let layer_set = tree.layer_set();
        let mut merger = layer_set.merger();
        let mut iter = merger.query(Query::FullScan).await.expect("seek failed");
        for e in expected {
            assert_eq!(iter.get().expect("get failed"), e.as_item_ref());
            iter.advance().await.expect("advance failed");
        }
        assert!(iter.get().is_none());
    }

    #[fuchsia::test]
    async fn test_merge_extents_non_overlapping() -> Result<(), Error> {
        let object_id = 0;
        let attr_id = AttributeId::TEST_ID;
        let tree = LSMTree::new(merge, None);

        tree.insert(Item::new(
            ObjectKey::extent(object_id, attr_id, 0..512),
            ObjectValue::Extent(ExtentValue::new_raw(0, VOLUME_DATA_KEY_ID)),
        ))
        .expect("insert error");
        tree.seal();

        tree.insert(Item::new(
            ObjectKey::extent(object_id, attr_id, 512..1024),
            ObjectValue::Extent(ExtentValue::new_raw(16384, VOLUME_DATA_KEY_ID)),
        ))
        .expect("insert error");

        let layer_set = tree.layer_set();
        let mut merger = layer_set.merger();
        let mut iter = merger.query(Query::FullScan).await?;
        assert_eq!(iter.get().unwrap().key, &ObjectKey::extent(object_id, attr_id, 0..512));
        iter.advance().await?;
        assert_eq!(iter.get().unwrap().key, &ObjectKey::extent(object_id, attr_id, 512..1024));
        iter.advance().await?;
        assert!(iter.get().is_none());
        Ok(())
    }

    #[fuchsia::test]
    async fn test_merge_extents_rewrite_right() -> Result<(), Error> {
        let object_id = 0;
        let attr_id = AttributeId::TEST_ID;
        let tree = LSMTree::new(merge, None);

        tree.insert(Item::new(
            ObjectKey::extent(object_id, attr_id, 0..1024),
            ObjectValue::Extent(ExtentValue::new_raw(0, VOLUME_DATA_KEY_ID)),
        ))
        .expect("insert error");
        tree.seal();

        tree.insert(Item::new(
            ObjectKey::extent(object_id, attr_id, 512..1024),
            ObjectValue::Extent(ExtentValue::new_raw(16384, VOLUME_DATA_KEY_ID)),
        ))
        .expect("insert error");

        let layer_set = tree.layer_set();
        let mut merger = layer_set.merger();
        let mut iter = merger.query(Query::FullScan).await?;
        assert_eq!(iter.get().unwrap().key, &ObjectKey::extent(object_id, attr_id, 0..512));
        assert_eq!(
            iter.get().unwrap().value,
            &ObjectValue::Extent(ExtentValue::new_raw(0, VOLUME_DATA_KEY_ID))
        );
        iter.advance().await?;
        assert_eq!(iter.get().unwrap().key, &ObjectKey::extent(object_id, attr_id, 512..1024));
        assert_eq!(
            iter.get().unwrap().value,
            &ObjectValue::Extent(ExtentValue::new_raw(16384, VOLUME_DATA_KEY_ID))
        );
        iter.advance().await?;
        assert!(iter.get().is_none());
        Ok(())
    }

    #[fuchsia::test]
    async fn test_merge_extents_rewrite_left() -> Result<(), Error> {
        let object_id = 0;
        let attr_id = AttributeId::TEST_ID;
        let tree = LSMTree::new(merge, None);

        tree.insert(Item::new(
            ObjectKey::extent(object_id, attr_id, 0..1024),
            ObjectValue::Extent(ExtentValue::with_checksum(
                0,
                Checksums::fletcher(vec![1, 2]),
                VOLUME_DATA_KEY_ID,
            )),
        ))
        .expect("insert error");
        tree.seal();

        tree.insert(Item::new(
            ObjectKey::extent(object_id, attr_id, 0..512),
            ObjectValue::Extent(ExtentValue::with_checksum(
                16384,
                Checksums::fletcher(vec![3]),
                VOLUME_DATA_KEY_ID,
            )),
        ))
        .expect("insert error");

        let layer_set = tree.layer_set();
        let mut merger = layer_set.merger();
        let mut iter = merger.query(Query::FullScan).await?;
        assert_eq!(iter.get().unwrap().key, &ObjectKey::extent(object_id, attr_id, 0..512));
        assert_eq!(
            iter.get().unwrap().value,
            &ObjectValue::Extent(ExtentValue::with_checksum(
                16384,
                Checksums::fletcher(vec![3]),
                VOLUME_DATA_KEY_ID
            ))
        );
        iter.advance().await?;
        assert_eq!(iter.get().unwrap().key, &ObjectKey::extent(object_id, attr_id, 512..1024));
        assert_eq!(
            iter.get().unwrap().value,
            &ObjectValue::Extent(ExtentValue::with_checksum(
                512,
                Checksums::fletcher(vec![2]),
                VOLUME_DATA_KEY_ID
            ))
        );
        iter.advance().await?;
        assert!(iter.get().is_none());
        Ok(())
    }

    #[fuchsia::test]
    async fn test_merge_extents_rewrite_middle() -> Result<(), Error> {
        let object_id = 0;
        let attr_id = AttributeId::TEST_ID;
        let tree = LSMTree::new(merge, None);

        tree.insert(Item::new(
            ObjectKey::extent(object_id, attr_id, 0..2048),
            ObjectValue::Extent(ExtentValue::with_checksum(
                0,
                Checksums::fletcher(vec![1, 2, 3, 4]),
                VOLUME_DATA_KEY_ID,
            )),
        ))
        .expect("insert error");
        tree.seal();

        tree.insert(Item::new(
            ObjectKey::extent(object_id, attr_id, 1024..1536),
            ObjectValue::Extent(ExtentValue::with_checksum(
                16384,
                Checksums::fletcher(vec![5]),
                VOLUME_DATA_KEY_ID,
            )),
        ))
        .expect("insert error");

        let layer_set = tree.layer_set();
        let mut merger = layer_set.merger();
        let mut iter = merger.query(Query::FullScan).await?;
        assert_eq!(iter.get().unwrap().key, &ObjectKey::extent(object_id, attr_id, 0..1024));
        assert_eq!(
            iter.get().unwrap().value,
            &ObjectValue::Extent(ExtentValue::with_checksum(
                0,
                Checksums::fletcher(vec![1, 2]),
                VOLUME_DATA_KEY_ID
            ))
        );
        iter.advance().await?;
        assert_eq!(iter.get().unwrap().key, &ObjectKey::extent(object_id, attr_id, 1024..1536));
        assert_eq!(
            iter.get().unwrap().value,
            &ObjectValue::Extent(ExtentValue::with_checksum(
                16384,
                Checksums::fletcher(vec![5]),
                VOLUME_DATA_KEY_ID
            ))
        );
        iter.advance().await?;
        assert_eq!(iter.get().unwrap().key, &ObjectKey::extent(object_id, attr_id, 1536..2048));
        assert_eq!(
            iter.get().unwrap().value,
            &ObjectValue::Extent(ExtentValue::with_checksum(
                1536,
                Checksums::fletcher(vec![4]),
                VOLUME_DATA_KEY_ID
            ))
        );
        iter.advance().await?;
        assert!(iter.get().is_none());
        Ok(())
    }

    #[fuchsia::test]
    async fn test_merge_extents_rewrite_eclipses() -> Result<(), Error> {
        let object_id = 0;
        let attr_id = AttributeId::TEST_ID;
        let tree = LSMTree::new(merge, None);

        tree.insert(Item::new(
            ObjectKey::extent(object_id, attr_id, 1024..1536),
            ObjectValue::Extent(ExtentValue::new_raw(0, VOLUME_DATA_KEY_ID)),
        ))
        .expect("insert error");
        tree.seal();

        tree.insert(Item::new(
            ObjectKey::extent(object_id, attr_id, 0..2048),
            ObjectValue::Extent(ExtentValue::new_raw(16384, VOLUME_DATA_KEY_ID)),
        ))
        .expect("insert error");

        let layer_set = tree.layer_set();
        let mut merger = layer_set.merger();
        let mut iter = merger.query(Query::FullScan).await?;
        assert_eq!(iter.get().unwrap().key, &ObjectKey::extent(object_id, attr_id, 0..2048));
        assert_eq!(
            iter.get().unwrap().value,
            &ObjectValue::Extent(ExtentValue::new_raw(16384, VOLUME_DATA_KEY_ID))
        );
        iter.advance().await?;
        assert!(iter.get().is_none());
        Ok(())
    }

    #[fuchsia::test]
    async fn test_merge_extents_delete_left() -> Result<(), Error> {
        let object_id = 0;
        let attr_id = AttributeId::TEST_ID;
        let tree = LSMTree::new(merge, None);

        tree.insert(Item::new(
            ObjectKey::extent(object_id, attr_id, 0..1024),
            ObjectValue::Extent(ExtentValue::new_raw(0, VOLUME_DATA_KEY_ID)),
        ))
        .expect("insert error");
        tree.seal();

        tree.insert(Item::new(
            ObjectKey::extent(object_id, attr_id, 0..512),
            ObjectValue::deleted_extent(),
        ))
        .expect("insert error");

        let layer_set = tree.layer_set();
        let mut merger = layer_set.merger();
        let mut iter = merger.query(Query::FullScan).await?;
        assert_eq!(iter.get().unwrap().key, &ObjectKey::extent(object_id, attr_id, 0..512));
        assert_eq!(iter.get().unwrap().value, &ObjectValue::deleted_extent());
        iter.advance().await?;
        assert_eq!(iter.get().unwrap().key, &ObjectKey::extent(object_id, attr_id, 512..1024));
        assert_eq!(
            iter.get().unwrap().value,
            &ObjectValue::Extent(ExtentValue::new_raw(512, VOLUME_DATA_KEY_ID))
        );
        iter.advance().await?;
        assert!(iter.get().is_none());
        Ok(())
    }

    #[fuchsia::test]
    async fn test_merge_extents_delete_right() -> Result<(), Error> {
        let object_id = 0;
        let attr_id = AttributeId::TEST_ID;
        let tree = LSMTree::new(merge, None);

        tree.insert(Item::new(
            ObjectKey::extent(object_id, attr_id, 0..1024),
            ObjectValue::Extent(ExtentValue::new_raw(0, VOLUME_DATA_KEY_ID)),
        ))
        .expect("insert error");
        tree.seal();

        tree.insert(Item::new(
            ObjectKey::extent(object_id, attr_id, 512..1024),
            ObjectValue::deleted_extent(),
        ))
        .expect("insert error");

        let layer_set = tree.layer_set();
        let mut merger = layer_set.merger();
        let mut iter = merger.query(Query::FullScan).await?;
        assert_eq!(iter.get().unwrap().key, &ObjectKey::extent(object_id, attr_id, 0..512));
        assert_eq!(
            iter.get().unwrap().value,
            &ObjectValue::Extent(ExtentValue::new_raw(0, VOLUME_DATA_KEY_ID))
        );
        iter.advance().await?;
        assert_eq!(iter.get().unwrap().key, &ObjectKey::extent(object_id, attr_id, 512..1024));
        assert_eq!(iter.get().unwrap().value, &ObjectValue::deleted_extent());
        iter.advance().await?;
        assert!(iter.get().is_none());
        Ok(())
    }

    #[fuchsia::test]
    async fn test_merge_extents_delete_middle() -> Result<(), Error> {
        let object_id = 0;
        let attr_id = AttributeId::TEST_ID;
        let tree = LSMTree::new(merge, None);

        tree.insert(Item::new(
            ObjectKey::extent(object_id, attr_id, 0..2048),
            ObjectValue::Extent(ExtentValue::new_raw(0, VOLUME_DATA_KEY_ID)),
        ))
        .expect("insert error");
        tree.seal();

        tree.insert(Item::new(
            ObjectKey::extent(object_id, attr_id, 1024..1536),
            ObjectValue::deleted_extent(),
        ))
        .expect("insert error");

        let layer_set = tree.layer_set();
        let mut merger = layer_set.merger();
        let mut iter = merger.query(Query::FullScan).await?;
        assert_eq!(iter.get().unwrap().key, &ObjectKey::extent(object_id, attr_id, 0..1024));
        assert_eq!(
            iter.get().unwrap().value,
            &ObjectValue::Extent(ExtentValue::new_raw(0, VOLUME_DATA_KEY_ID))
        );
        iter.advance().await?;
        assert_eq!(iter.get().unwrap().key, &ObjectKey::extent(object_id, attr_id, 1024..1536));
        assert_eq!(iter.get().unwrap().value, &ObjectValue::deleted_extent());
        iter.advance().await?;
        assert_eq!(iter.get().unwrap().key, &ObjectKey::extent(object_id, attr_id, 1536..2048));
        assert_eq!(
            iter.get().unwrap().value,
            &ObjectValue::Extent(ExtentValue::new_raw(1536, VOLUME_DATA_KEY_ID))
        );
        iter.advance().await?;
        assert!(iter.get().is_none());
        Ok(())
    }

    #[fuchsia::test]
    async fn test_merge_extents_delete_eclipses() -> Result<(), Error> {
        let object_id = 0;
        let attr_id = AttributeId::TEST_ID;
        let tree = LSMTree::new(merge, None);

        tree.insert(Item::new(
            ObjectKey::extent(object_id, attr_id, 1024..1536),
            ObjectValue::Extent(ExtentValue::new_raw(0, VOLUME_DATA_KEY_ID)),
        ))
        .expect("insert error");
        tree.seal();

        tree.insert(Item::new(
            ObjectKey::extent(object_id, attr_id, 0..2048),
            ObjectValue::deleted_extent(),
        ))
        .expect("insert error");

        let layer_set = tree.layer_set();
        let mut merger = layer_set.merger();
        let mut iter = merger.query(Query::FullScan).await?;
        assert_eq!(iter.get().unwrap().key, &ObjectKey::extent(object_id, attr_id, 0..2048));
        assert_eq!(iter.get().unwrap().value, &ObjectValue::deleted_extent());
        iter.advance().await?;
        assert!(iter.get().is_none());
        Ok(())
    }

    #[fuchsia::test]
    async fn test_merge_deleted_extents_new_layer_joins_two_deletions() -> Result<(), Error> {
        // Old layer:  [----]    [----]
        // New layer:       [----]
        // Merged:     [--------------]
        let object_id = 0;
        let attr_id = AttributeId::TEST_ID;
        let tree = LSMTree::new(merge, None);

        tree.insert(Item::new(
            ObjectKey::extent(object_id, attr_id, 0..512),
            ObjectValue::deleted_extent(),
        ))
        .expect("insert error");
        tree.insert(Item::new(
            ObjectKey::extent(object_id, attr_id, 1024..1536),
            ObjectValue::deleted_extent(),
        ))
        .expect("insert error");
        tree.seal();

        tree.insert(Item::new(
            ObjectKey::extent(object_id, attr_id, 512..1024),
            ObjectValue::deleted_extent(),
        ))
        .expect("insert error");
        tree.seal();

        let layer_set = tree.layer_set();
        let mut merger = layer_set.merger();
        let mut iter = merger.query(Query::FullScan).await?;
        assert_eq!(iter.get().unwrap().key, &ObjectKey::extent(object_id, attr_id, 0..1536));
        assert_eq!(iter.get().unwrap().value, &ObjectValue::deleted_extent());
        iter.advance().await?;
        assert!(iter.get().is_none());
        Ok(())
    }

    #[fuchsia::test]
    async fn test_merge_deleted_extents_new_layer_joined_by_old_deletion() -> Result<(), Error> {
        // Old layer:       [----]
        // New layer:  [----]    [----]
        // Merged:     [--------------]
        let object_id = 0;
        let attr_id = AttributeId::TEST_ID;
        let tree = LSMTree::new(merge, None);

        tree.insert(Item::new(
            ObjectKey::extent(object_id, attr_id, 512..1024),
            ObjectValue::deleted_extent(),
        ))
        .expect("insert error");
        tree.seal();

        tree.insert(Item::new(
            ObjectKey::extent(object_id, attr_id, 0..512),
            ObjectValue::deleted_extent(),
        ))
        .expect("insert error");
        tree.insert(Item::new(
            ObjectKey::extent(object_id, attr_id, 1024..1536),
            ObjectValue::deleted_extent(),
        ))
        .expect("insert error");
        tree.seal();

        let layer_set = tree.layer_set();
        let mut merger = layer_set.merger();
        let mut iter = merger.query(Query::FullScan).await?;
        assert_eq!(iter.get().unwrap().key, &ObjectKey::extent(object_id, attr_id, 0..1536));
        assert_eq!(iter.get().unwrap().value, &ObjectValue::deleted_extent());
        iter.advance().await?;
        assert!(iter.get().is_none());
        Ok(())
    }

    #[fuchsia::test]
    async fn test_merge_deleted_extents_overlapping_newest_on_right() -> Result<(), Error> {
        let object_id = 0;
        let attr_id = AttributeId::TEST_ID;
        let tree = LSMTree::new(merge, None);

        tree.insert(Item::new(
            ObjectKey::extent(object_id, attr_id, 0..1024),
            ObjectValue::deleted_extent(),
        ))
        .expect("insert error");
        tree.seal();

        tree.insert(Item::new(
            ObjectKey::extent(object_id, attr_id, 512..1536),
            ObjectValue::deleted_extent(),
        ))
        .expect("insert error");

        let layer_set = tree.layer_set();
        let mut merger = layer_set.merger();
        let mut iter = merger.query(Query::FullScan).await?;
        assert_eq!(iter.get().unwrap().key, &ObjectKey::extent(object_id, attr_id, 0..1536));
        assert_eq!(iter.get().unwrap().value, &ObjectValue::deleted_extent());
        iter.advance().await?;
        assert!(iter.get().is_none());
        Ok(())
    }

    #[fuchsia::test]
    async fn test_merge_deleted_extents_overlapping_newest_on_left() -> Result<(), Error> {
        let object_id = 0;
        let attr_id = AttributeId::TEST_ID;
        let tree = LSMTree::new(merge, None);

        tree.insert(Item::new(
            ObjectKey::extent(object_id, attr_id, 512..1536),
            ObjectValue::deleted_extent(),
        ))
        .expect("insert error");
        tree.seal();
        tree.insert(Item::new(
            ObjectKey::extent(object_id, attr_id, 0..1024),
            ObjectValue::deleted_extent(),
        ))
        .expect("insert error");

        let layer_set = tree.layer_set();
        let mut merger = layer_set.merger();
        let mut iter = merger.query(Query::FullScan).await?;
        assert_eq!(iter.get().unwrap().key, &ObjectKey::extent(object_id, attr_id, 0..1536));
        assert_eq!(iter.get().unwrap().value, &ObjectValue::deleted_extent());
        iter.advance().await?;
        assert!(iter.get().is_none());
        Ok(())
    }

    #[fuchsia::test]
    async fn test_merge_deleted_extents_new_layer_contained_in_old() -> Result<(), Error> {
        // Old layer:  [--------------]
        // New layer:       [----]
        // Merged:     [--------------]
        let object_id = 0;
        let attr_id = AttributeId::TEST_ID;
        let tree = LSMTree::new(merge, None);

        tree.insert(Item::new(
            ObjectKey::extent(object_id, attr_id, 0..1536),
            ObjectValue::deleted_extent(),
        ))
        .expect("insert error");
        tree.seal();

        tree.insert(Item::new(
            ObjectKey::extent(object_id, attr_id, 512..1024),
            ObjectValue::deleted_extent(),
        ))
        .expect("insert error");
        tree.seal();

        let layer_set = tree.layer_set();
        let mut merger = layer_set.merger();
        let mut iter = merger.query(Query::FullScan).await?;
        assert_eq!(iter.get().unwrap().key, &ObjectKey::extent(object_id, attr_id, 0..1536));
        assert_eq!(iter.get().unwrap().value, &ObjectValue::deleted_extent());
        iter.advance().await?;
        assert!(iter.get().is_none());
        Ok(())
    }

    #[fuchsia::test]
    async fn test_merge_deleted_extents_new_layer_eclipses_old() -> Result<(), Error> {
        // Old layer:       [----]
        // New layer:  [--------------]
        // Merged:     [--------------]
        let object_id = 0;
        let attr_id = AttributeId::TEST_ID;
        let tree = LSMTree::new(merge, None);

        tree.insert(Item::new(
            ObjectKey::extent(object_id, attr_id, 512..1024),
            ObjectValue::deleted_extent(),
        ))
        .expect("insert error");
        tree.seal();

        tree.insert(Item::new(
            ObjectKey::extent(object_id, attr_id, 0..1536),
            ObjectValue::deleted_extent(),
        ))
        .expect("insert error");
        tree.seal();

        let layer_set = tree.layer_set();
        let mut merger = layer_set.merger();
        let mut iter = merger.query(Query::FullScan).await?;
        assert_eq!(iter.get().unwrap().key, &ObjectKey::extent(object_id, attr_id, 0..1536));
        assert_eq!(iter.get().unwrap().value, &ObjectValue::deleted_extent());
        iter.advance().await?;
        assert!(iter.get().is_none());
        Ok(())
    }

    #[fuchsia::test]
    async fn test_merge_deleted_extents_does_not_coalesce_if_not_adjacent_layers()
    -> Result<(), Error> {
        // Layer 0:  [XXXXX]
        // Layer 1:  [--------------]
        // Layer 2:        [XXXXXXXX]
        //  Merged:  [XXXXX|--------]
        let object_id = 0;
        let attr_id = AttributeId::TEST_ID;
        let tree = LSMTree::<ObjectKey, ObjectValue>::new(merge, None);

        tree.insert(Item::new(
            ObjectKey::extent(object_id, attr_id, 512..1024),
            ObjectValue::deleted_extent(),
        ))
        .expect("insert error");
        tree.seal();

        tree.insert(Item::new(
            ObjectKey::extent(object_id, attr_id, 0..1024),
            ObjectValue::Extent(ExtentValue::new_raw(0, VOLUME_DATA_KEY_ID)),
        ))
        .expect("insert error");
        tree.seal();

        tree.insert(Item::new(
            ObjectKey::extent(object_id, attr_id, 0..512),
            ObjectValue::deleted_extent(),
        ))
        .expect("insert error");

        let layer_set = tree.layer_set();
        let mut merger = layer_set.merger();
        let mut iter = merger.query(Query::FullScan).await?;
        assert_eq!(iter.get().unwrap().key, &ObjectKey::extent(object_id, attr_id, 0..512));
        assert_eq!(iter.get().unwrap().value, &ObjectValue::deleted_extent());
        iter.advance().await?;
        assert_eq!(iter.get().unwrap().key, &ObjectKey::extent(object_id, attr_id, 512..1024));
        assert_eq!(
            iter.get().unwrap().value,
            &ObjectValue::Extent(ExtentValue::new_raw(512, VOLUME_DATA_KEY_ID))
        );
        iter.advance().await?;
        assert!(iter.get().is_none());
        Ok(())
    }

    #[fuchsia::test]
    async fn test_merge_deleted_extents_does_not_coalesce_if_not_adjacent_deletions()
    -> Result<(), Error> {
        // Layer 0:  [XXXXX|--------]
        // Layer 1:           [XXXXX]
        //  Merged:  [XXXXX|--------]
        let object_id = 0;
        let attr_id = AttributeId::TEST_ID;
        let tree = LSMTree::<ObjectKey, ObjectValue>::new(merge, None);

        tree.insert(Item::new(
            ObjectKey::extent(object_id, attr_id, 1024..1536),
            ObjectValue::Extent(ExtentValue::new_raw(0, VOLUME_DATA_KEY_ID)),
        ))
        .expect("insert error");
        tree.seal();

        tree.insert(Item::new(
            ObjectKey::extent(object_id, attr_id, 0..512),
            ObjectValue::deleted_extent(),
        ))
        .expect("insert error");
        tree.insert(Item::new(
            ObjectKey::extent(object_id, attr_id, 512..1536),
            ObjectValue::Extent(ExtentValue::new_raw(0, VOLUME_DATA_KEY_ID)),
        ))
        .expect("insert error");

        let layer_set = tree.layer_set();
        let mut merger = layer_set.merger();
        let mut iter = merger.query(Query::FullScan).await?;
        assert_eq!(iter.get().unwrap().key, &ObjectKey::extent(object_id, attr_id, 0..512));
        assert_eq!(iter.get().unwrap().value, &ObjectValue::deleted_extent());
        iter.advance().await?;
        assert_eq!(iter.get().unwrap().key, &ObjectKey::extent(object_id, attr_id, 512..1536));
        assert_eq!(
            iter.get().unwrap().value,
            &ObjectValue::Extent(ExtentValue::new_raw(0, VOLUME_DATA_KEY_ID))
        );
        iter.advance().await?;
        assert!(iter.get().is_none());
        Ok(())
    }

    #[fuchsia::test]
    async fn test_merge_deleted_extent_into_overwrites_extents() -> Result<(), Error> {
        let object_id = 0;
        let attr_id = AttributeId::TEST_ID;
        let tree = LSMTree::<ObjectKey, ObjectValue>::new(merge, None);

        tree.insert(Item::new(
            ObjectKey::extent(object_id, attr_id, 0..1024),
            ObjectValue::Extent(ExtentValue::new_raw(0, VOLUME_DATA_KEY_ID)),
        ))
        .expect("insert error");
        tree.insert(Item::new(
            ObjectKey::extent(object_id, attr_id, 1024..2048),
            ObjectValue::Extent(ExtentValue::new_raw(16384, VOLUME_DATA_KEY_ID)),
        ))
        .expect("insert error");
        let key = ObjectKey::extent(object_id, attr_id, 512..1536);
        tree.merge_into(
            Item::new(key.clone(), ObjectValue::deleted_extent()),
            &key.key_for_merge_into(),
        );

        let layer_set = tree.layer_set();
        let mut merger = layer_set.merger();
        let mut iter = merger.query(Query::FullScan).await?;
        assert_eq!(iter.get().unwrap().key, &ObjectKey::extent(object_id, attr_id, 0..512));
        assert_eq!(
            iter.get().unwrap().value,
            &ObjectValue::Extent(ExtentValue::new_raw(0, VOLUME_DATA_KEY_ID))
        );
        iter.advance().await?;
        assert_eq!(iter.get().unwrap().key, &ObjectKey::extent(object_id, attr_id, 512..1536));
        assert_eq!(iter.get().unwrap().value, &ObjectValue::deleted_extent());
        iter.advance().await?;
        assert_eq!(iter.get().unwrap().key, &ObjectKey::extent(object_id, attr_id, 1536..2048));
        assert_eq!(
            iter.get().unwrap().value,
            &ObjectValue::Extent(ExtentValue::new_raw(16896, VOLUME_DATA_KEY_ID))
        );
        iter.advance().await?;
        assert!(iter.get().is_none());
        Ok(())
    }

    #[fuchsia::test]
    async fn test_merge_deleted_extent_into_merges_with_other_deletions() -> Result<(), Error> {
        let object_id = 0;
        let attr_id = AttributeId::TEST_ID;
        let tree = LSMTree::<ObjectKey, ObjectValue>::new(merge, None);

        tree.insert(Item::new(
            ObjectKey::extent(object_id, attr_id, 0..1024),
            ObjectValue::deleted_extent(),
        ))
        .expect("insert error");
        tree.insert(Item::new(
            ObjectKey::extent(object_id, attr_id, 1024..2048),
            ObjectValue::deleted_extent(),
        ))
        .expect("insert error");

        let key = ObjectKey::extent(object_id, attr_id, 512..1536);
        tree.merge_into(
            Item::new(key.clone(), ObjectValue::deleted_extent()),
            &key.key_for_merge_into(),
        );

        let layer_set = tree.layer_set();
        let mut merger = layer_set.merger();
        let mut iter = merger.query(Query::FullScan).await?;
        assert_eq!(iter.get().unwrap().key, &ObjectKey::extent(object_id, attr_id, 0..2048));
        assert_eq!(iter.get().unwrap().value, &ObjectValue::deleted_extent());
        iter.advance().await?;
        assert!(iter.get().is_none());
        Ok(())
    }

    #[fuchsia::test]
    async fn test_merge_size_records() {
        let left = &[Item::new(
            ObjectKey::attribute(1, AttributeId::TEST_ID, AttributeKey::Attribute),
            ObjectValue::attribute(5, false),
        )];
        let right = &[Item::new(
            ObjectKey::attribute(1, AttributeId::TEST_ID, AttributeKey::Attribute),
            ObjectValue::attribute(10, false),
        )];
        let tree = LSMTree::new(merge, None);
        test_merge(&tree, left, right, left).await;
    }

    #[fuchsia::test]
    async fn test_different_attributes_not_merged() {
        let left = Item::new(
            ObjectKey::attribute(1, AttributeId::TEST_ID, AttributeKey::Attribute),
            ObjectValue::attribute(5, false),
        );
        let right = Item::new(
            ObjectKey::attribute(1, AttributeId::TEST_ID.next(), AttributeKey::Attribute),
            ObjectValue::attribute(10, false),
        );
        let tree = LSMTree::new(merge, None);
        test_merge(&tree, &[left.clone()], &[right.clone()], &[left, right]).await;

        let left = Item::new(
            ObjectKey::extent(1, AttributeId::TEST_ID, 0..100),
            ObjectValue::Extent(ExtentValue::new_raw(0, VOLUME_DATA_KEY_ID)),
        );
        let right = Item::new(
            ObjectKey::extent(1, AttributeId::TEST_ID.next(), 0..100),
            ObjectValue::Extent(ExtentValue::new_raw(1, VOLUME_DATA_KEY_ID)),
        );
        let tree = LSMTree::new(merge, None);
        test_merge(&tree, &[left.clone()], &[right.clone()], &[left, right]).await;
    }

    #[fuchsia::test]
    async fn test_tombstone_discards_all_other_records() {
        let tombstone = Item::new(ObjectKey::object(1), ObjectValue::None);
        let other_object = Item::new(
            ObjectKey::object(2),
            ObjectValue::file(
                1,
                0,
                Timestamp::default(),
                Timestamp::default(),
                Timestamp::default(),
                Timestamp::default(),
                None,
                None,
            ),
        );
        let tree = LSMTree::new(merge, None);
        test_merge(
            &tree,
            &[tombstone.clone()],
            &[
                Item::new(
                    ObjectKey::object(1),
                    ObjectValue::file(
                        1,
                        100,
                        Timestamp::default(),
                        Timestamp::default(),
                        Timestamp::default(),
                        Timestamp::default(),
                        None,
                        None,
                    ),
                ),
                Item::new(
                    ObjectKey::attribute(1, AttributeId::TEST_ID, AttributeKey::Attribute),
                    ObjectValue::attribute(100, false),
                ),
                other_object.clone(),
            ],
            &[tombstone, other_object],
        )
        .await;
    }

    #[fuchsia::test]
    async fn test_extent_overlapping_boundaries() {
        use crate::object_store::VOLUME_DATA_KEY_ID;
        let object_id = 1;
        let attr_id = AttributeId::TEST_ID;
        let base = Item::new(
            ObjectKey::extent(object_id, attr_id, 50..100),
            ObjectValue::Extent(ExtentValue::new_raw(0, VOLUME_DATA_KEY_ID)),
        );

        // 1. Same end, start off-by-one.
        // 49..100 (older, val 2) vs 50..100 (newer, val 1).
        // Yields 49..50 (val 2), 50..100 (val 1).
        let tree = LSMTree::new(merge, None);
        test_merge(
            &tree,
            &[base.clone()],
            &[Item::new(
                ObjectKey::extent(object_id, attr_id, 49..100),
                ObjectValue::Extent(ExtentValue::new_raw(16384, VOLUME_DATA_KEY_ID)),
            )],
            &[
                Item::new(
                    ObjectKey::extent(object_id, attr_id, 49..50),
                    ObjectValue::Extent(ExtentValue::new_raw(16384, VOLUME_DATA_KEY_ID)),
                ),
                base.clone(),
            ],
        )
        .await;

        // 51..100 (older, val 2) vs 50..100 (newer, val 1).
        // Yields 50..100 (val 1).
        let tree = LSMTree::new(merge, None);
        test_merge(
            &tree,
            &[base.clone()],
            &[Item::new(
                ObjectKey::extent(object_id, attr_id, 51..100),
                ObjectValue::Extent(ExtentValue::new_raw(16384, VOLUME_DATA_KEY_ID)),
            )],
            &[base.clone()],
        )
        .await;
    }

    // Tests merging across three LSM tree layers (top, middle, base) with various overlapping
    // combinations. `calculate_expected` constructs a 1-D point oracle where the newest layer
    // (top = 1, middle = 2, base = 3) wins for each byte position, and aggregates continuous
    // segments to verify against the merger iterator output and layer advance depths.
    #[fuchsia::test]
    async fn test_extent_complex_multi_layer() {
        use crate::object_store::extent_record::ExtentValue;
        use crate::object_store::object_record::{
            AttributeKey, ObjectKey, ObjectKeyData, ObjectValue,
        };

        let object_id = 1;
        let attr_id = AttributeId::TEST_ID;

        let top_options = vec![49..50, 50..51, 51..52, 98..99, 99..100, 100..101, 40..95];

        let middle_range = 50..100;

        let base_options = vec![49..101, 49..100, 50..101, 48..102, 100..101, 100..102, 30..90];

        let scale_range =
            |r: std::ops::Range<u64>| r.start * MIN_BLOCK_SIZE..r.end * MIN_BLOCK_SIZE;

        let calculate_expected = |top: std::ops::Range<u64>,
                                  middle: std::ops::Range<u64>,
                                  base: std::ops::Range<u64>| {
            let mut points = vec![0; 200];
            for x in 0u64..200u64 {
                if top.contains(&x) {
                    points[x as usize] = 1;
                } else if middle.contains(&x) {
                    points[x as usize] = 2;
                } else if base.contains(&x) {
                    points[x as usize] = 3;
                }
            }

            let mut result = Vec::new();
            let mut current_val = 0;
            let mut start = 0;
            let mut max_layers_needed = 0;
            let mut start_x = None;

            for x in 0..200 {
                let val = points[x];

                let layers_needed = if top.contains(&(x as u64)) {
                    1
                } else if middle.contains(&(x as u64)) {
                    2
                } else {
                    3
                };

                if val != current_val {
                    if current_val != 0 {
                        result.push((start as u64..x as u64, current_val, 3 - max_layers_needed));
                    }
                    current_val = val;
                    start = x;
                    if start_x.is_none() && val != 0 {
                        start_x = Some(x);
                    }
                }

                if let Some(sx) = start_x {
                    if x >= sx {
                        max_layers_needed = std::cmp::max(max_layers_needed, layers_needed);
                    }
                }
            }
            if current_val != 0 {
                result.push((start as u64..200, current_val, 3 - max_layers_needed));
            }

            result
        };

        for top_range in top_options {
            for base_range in &base_options {
                let expected =
                    calculate_expected(top_range.clone(), middle_range.clone(), base_range.clone());

                let tree = LSMTree::new(merge, None);

                // Base layer (Layer 2)
                tree.insert(Item::new(
                    ObjectKey::extent(object_id, attr_id, scale_range(base_range.clone())),
                    ObjectValue::Extent(ExtentValue::new_raw(0, 3)), // key_id = 3
                ))
                .expect("insert error");
                tree.seal();

                // Middle layer (Layer 1)
                tree.insert(Item::new(
                    ObjectKey::extent(object_id, attr_id, scale_range(middle_range.clone())),
                    ObjectValue::Extent(ExtentValue::new_raw(0, 2)), // key_id = 2
                ))
                .expect("insert error");
                tree.seal();

                // Top layer (Layer 0)
                tree.insert(Item::new(
                    ObjectKey::extent(object_id, attr_id, scale_range(top_range.clone())),
                    ObjectValue::Extent(ExtentValue::new_raw(0, 1)), // key_id = 1
                ))
                .expect("insert error");

                let layer_set = tree.layer_set();

                // Start search with full range of expected results.
                let mut merger = layer_set.merger();
                let mut iter = merger
                    .query(Query::LimitedRange(&ObjectKey::extent(
                        object_id,
                        attr_id,
                        scale_range(expected[0].0.start..expected.last().unwrap().0.end),
                    )))
                    .await
                    .expect("seek failed");

                assert_eq!(iter.pending_iterators_len(), expected[0].2);

                for e in expected {
                    let crate::object_store::ItemRef { key, value, .. } =
                        iter.get().expect("get failed");
                    if let ObjectKeyData::Attribute(aid, AttributeKey::Extent(extent)) = &key.data {
                        assert_eq!(aid, &attr_id);
                        assert_eq!(&**extent, &scale_range(e.0));
                    } else {
                        panic!("Unexpected key type");
                    }
                    if let ObjectValue::Extent(ExtentValue::Some { key_id, .. }) = value {
                        assert_eq!(key_id, &e.1);
                    } else {
                        panic!("Unexpected value type");
                    }
                    assert_eq!(iter.pending_iterators_len(), e.2);
                    iter.advance().await.expect("advance failed");
                }
                assert_eq!(iter.pending_iterators_len(), 0);
                assert!(iter.get().is_none());
            }
        }
    }

    #[fuchsia::test]
    async fn test_next_key_behavior() -> Result<(), Error> {
        let object_id = 1;
        let attr_id = AttributeId::TEST_ID;
        let tree = LSMTree::new(merge, None);

        // Layer 1 (older)
        tree.insert(Item::new(
            ObjectKey::extent(object_id, attr_id, 50 * MIN_BLOCK_SIZE..101 * MIN_BLOCK_SIZE),
            ObjectValue::Extent(ExtentValue::new_raw(0, VOLUME_DATA_KEY_ID)),
        ))?;
        tree.seal();

        // Layer 0 (newer)
        tree.insert(Item::new(
            ObjectKey::extent(object_id, attr_id, 0..100 * MIN_BLOCK_SIZE),
            ObjectValue::Extent(ExtentValue::new_raw(16384, VOLUME_DATA_KEY_ID)),
        ))?;

        let layer_set = tree.layer_set();
        let mut merger = layer_set.merger();

        let mut iter = merger
            .query(Query::LimitedRange(&ObjectKey::extent(
                object_id,
                attr_id,
                0..100 * MIN_BLOCK_SIZE,
            )))
            .await?;

        let item = iter.get().expect("missing item");
        assert_eq!(item.key, &ObjectKey::extent(object_id, attr_id, 0..100 * MIN_BLOCK_SIZE));

        iter.advance().await.expect("advance failed");

        let item = iter.get().expect("missing item");
        assert_eq!(
            item.key,
            &ObjectKey::extent(object_id, attr_id, 100 * MIN_BLOCK_SIZE..101 * MIN_BLOCK_SIZE,)
        );

        iter.advance().await.expect("advance failed");
        assert!(iter.get().is_none());

        Ok(())
    }

    #[fuchsia::test]
    async fn test_merge_project_usage() {
        let tree = LSMTree::new(merge, None);
        let key = ObjectKey::project_usage(5, ProjectId::new(6).unwrap());

        tree.insert(Item::new(key.clone(), ObjectValue::BytesAndNodes { bytes: 100, nodes: 1000 }))
            .expect("insert error");
        tree.merge_into(
            Item::new(key.clone(), ObjectValue::BytesAndNodes { bytes: 4, nodes: 8 }),
            &key,
        );
        tree.seal();

        tree.merge_into(
            Item::new(key.clone(), ObjectValue::BytesAndNodes { bytes: -1, nodes: -2 }),
            &key,
        );
        tree.seal();

        tree.merge_into(
            Item::new(key.clone(), ObjectValue::BytesAndNodes { bytes: 16, nodes: 32 }),
            &key,
        );

        let layer_set = tree.layer_set();
        let mut merger = layer_set.merger();
        let mut iter = merger.query(Query::FullScan).await.unwrap();
        assert_eq!(iter.get().unwrap().key, &key);
        assert_eq!(
            iter.get().unwrap().value,
            &ObjectValue::BytesAndNodes { bytes: 119, nodes: 1038 }
        );
        iter.advance().await.unwrap();
        assert!(iter.get().is_none());
    }

    #[fuchsia::test]
    async fn test_merge_project_usage_gap_layer() {
        let tree = LSMTree::new(merge, None);
        let key = ObjectKey::project_usage(5, ProjectId::new(6).unwrap());
        let key2 = ObjectKey::project_usage(5, ProjectId::new(7).unwrap());

        tree.insert(Item::new(key.clone(), ObjectValue::BytesAndNodes { bytes: 100, nodes: 1000 }))
            .expect("insert error");
        tree.merge_into(
            Item::new(key.clone(), ObjectValue::BytesAndNodes { bytes: 4, nodes: 8 }),
            &key,
        );
        tree.seal();

        assert_eq!(
            tree.find_value(&key).await.expect("Find").unwrap(),
            ObjectValue::BytesAndNodes { bytes: 104, nodes: 1008 }
        );

        tree.merge_into(
            Item::new(key2.clone(), ObjectValue::BytesAndNodes { bytes: 13, nodes: 17 }),
            &key,
        );
        tree.seal();

        assert_eq!(
            tree.find_value(&key).await.expect("Find").unwrap(),
            ObjectValue::BytesAndNodes { bytes: 104, nodes: 1008 }
        );

        tree.merge_into(
            Item::new(key.clone(), ObjectValue::BytesAndNodes { bytes: 16, nodes: 32 }),
            &key,
        );

        let layer_set = tree.layer_set();
        let mut merger = layer_set.merger();
        let mut iter = merger.query(Query::FullScan).await.unwrap();
        assert_eq!(iter.get().unwrap().key, &key);
        assert_eq!(
            iter.get().unwrap().value,
            &ObjectValue::BytesAndNodes { bytes: 120, nodes: 1040 }
        );
        iter.advance().await.unwrap();
        assert_eq!(iter.get().unwrap().key, &key2);
        assert_eq!(iter.get().unwrap().value, &ObjectValue::BytesAndNodes { bytes: 13, nodes: 17 });
        iter.advance().await.unwrap();
        assert!(iter.get().is_none());

        assert_eq!(
            tree.find_value(&key).await.expect("Find").unwrap(),
            ObjectValue::BytesAndNodes { bytes: 120, nodes: 1040 }
        );
    }

    #[fuchsia::test]
    async fn test_merge_project_usage_to_zero() {
        let tree = LSMTree::new(merge, None);
        let key = ObjectKey::project_usage(5, ProjectId::new(6).unwrap());

        tree.insert(Item::new(key.clone(), ObjectValue::BytesAndNodes { bytes: 4, nodes: 8 }))
            .expect("insert error");
        tree.seal();

        tree.merge_into(
            Item::new(key.clone(), ObjectValue::BytesAndNodes { bytes: -4, nodes: -8 }),
            &key,
        );
        tree.seal();

        let layer_set = tree.layer_set();
        let mut merger = layer_set.merger();
        let iter = merger.query(Query::FullScan).await.unwrap();
        assert!(iter.get().is_none());
    }

    #[fuchsia::test]
    async fn test_merge_project_usage_recover_from_zero() {
        let tree = LSMTree::new(merge, None);
        let key = ObjectKey::project_usage(5, ProjectId::new(6).unwrap());

        tree.insert(Item::new(key.clone(), ObjectValue::BytesAndNodes { bytes: 4, nodes: 8 }))
            .expect("insert error");
        tree.seal();

        tree.merge_into(
            Item::new(key.clone(), ObjectValue::BytesAndNodes { bytes: -4, nodes: -8 }),
            &key,
        );
        tree.seal();

        tree.merge_into(
            Item::new(key.clone(), ObjectValue::BytesAndNodes { bytes: 20, nodes: 40 }),
            &key,
        );
        tree.seal();

        let layer_set = tree.layer_set();
        let mut merger = layer_set.merger();
        let mut iter = merger.query(Query::FullScan).await.unwrap();
        assert_eq!(iter.get().unwrap().key, &key);
        assert_eq!(iter.get().unwrap().value, &ObjectValue::BytesAndNodes { bytes: 20, nodes: 40 });
        iter.advance().await.unwrap();
        assert!(iter.get().is_none());
    }

    #[fuchsia::test]
    async fn test_merge_project_usage_layer_merge_to_negative() {
        let tree = LSMTree::new(merge, None);
        let key = ObjectKey::project_usage(5, ProjectId::new(6).unwrap());

        tree.insert(Item::new(key.clone(), ObjectValue::BytesAndNodes { bytes: 4, nodes: 8 }))
            .expect("insert error");
        tree.seal();

        tree.merge_into(
            Item::new(key.clone(), ObjectValue::BytesAndNodes { bytes: 16, nodes: 32 }),
            &key,
        );
        tree.seal();

        // As we merge from the newest layer down this will drop bytes below zero and nodes to
        // exactly zero during the merge process.
        tree.merge_into(
            Item::new(key.clone(), ObjectValue::BytesAndNodes { bytes: -18, nodes: -32 }),
            &key,
        );
        tree.seal();

        let layer_set = tree.layer_set();
        let mut merger = layer_set.merger();
        let mut iter = merger.query(Query::FullScan).await.unwrap();
        assert_eq!(iter.get().unwrap().key, &key);
        assert_eq!(iter.get().unwrap().value, &ObjectValue::BytesAndNodes { bytes: 2, nodes: 8 });
        iter.advance().await.unwrap();
        assert!(iter.get().is_none());
    }
}
