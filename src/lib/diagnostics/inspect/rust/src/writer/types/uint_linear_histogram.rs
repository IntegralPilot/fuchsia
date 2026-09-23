// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::writer::{
    ArithmeticArrayProperty, ArrayProperty, HistogramProperty, InspectType, Node, UintArrayProperty,
};
use diagnostics_hierarchy::{ArrayFormat, LinearHistogramParams};
use log::error;
use std::borrow::Cow;

#[derive(Debug, Default)]
/// A linear histogram property for unsigned integer values.
pub struct UintLinearHistogramProperty {
    array: UintArrayProperty,
    floor: u64,
    buckets: usize,
    step_size: u64,
}

impl InspectType for UintLinearHistogramProperty {
    fn into_recorded(self) -> crate::writer::types::RecordedInspectType {
        crate::writer::types::RecordedInspectType::UintArray(self.array)
    }
}

impl UintLinearHistogramProperty {
    pub(crate) fn new(
        name: Cow<'_, str>,
        params: LinearHistogramParams<u64>,
        parent: &Node,
    ) -> Self {
        let slots = params.buckets + ArrayFormat::LinearHistogram.extra_slots();
        let array = parent.create_uint_array_internal(name, slots, ArrayFormat::LinearHistogram);
        array.set(0, params.floor);
        array.set(1, params.step_size);
        Self { floor: params.floor, step_size: params.step_size, buckets: params.buckets, array }
    }

    fn get_index(&self, value: u64) -> usize {
        let mut bucket_end = self.floor; // The exclusive end of a bucket's range.
        let mut index = ArrayFormat::LinearHistogram.underflow_bucket_index();
        let overflow_index = ArrayFormat::LinearHistogram.overflow_bucket_index(self.buckets);
        while value >= bucket_end && index < overflow_index {
            bucket_end = bucket_end.saturating_add(self.step_size);
            index += 1;
        }
        index
    }
}

impl HistogramProperty for UintLinearHistogramProperty {
    type Type = u64;

    fn insert(&self, value: u64) {
        self.insert_multiple(value, 1);
    }

    fn insert_multiple(&self, value: u64, count: usize) {
        self.array.add(self.get_index(value), count as u64);
    }

    fn clear(&self) {
        if let Some(ref inner_ref) = self.array.inner.inner_ref() {
            // Ensure we don't delete the array slots that contain histogram metadata.
            inner_ref
                .state
                .try_lock()
                .and_then(|mut state| {
                    // Clear histogram buckets starting at first bucket, which
                    // is the underflow bucket.
                    state.clear_array(
                        inner_ref.block_index,
                        ArrayFormat::LinearHistogram.underflow_bucket_index(),
                    )
                })
                .unwrap_or_else(|err| {
                    error!(err:?; "Failed to clear property");
                });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::writer::Inspector;
    use crate::writer::testing_utils::GetBlockExt;
    use inspect_format::{Array, Uint};

    #[fuchsia::test]
    fn uint_linear_histogram() {
        let inspector = Inspector::default();
        let root = inspector.root();
        let node = root.create_child("node");
        {
            let uint_histogram = node.create_uint_linear_histogram(
                "uint-histogram",
                LinearHistogramParams { floor: 10, step_size: 5, buckets: 5 },
            );
            uint_histogram.insert_multiple(0, 2); // underflow
            uint_histogram.insert(25);
            uint_histogram.insert(500); // overflow
            uint_histogram.array.get_block::<_, Array<Uint>>(|block| {
                for (i, value) in [10, 5, 2, 0, 0, 0, 1, 0, 1].iter().enumerate() {
                    assert_eq!(block.get(i).unwrap(), *value);
                }
            });

            uint_histogram.clear();
            uint_histogram.array.get_block::<_, Array<Uint>>(|block| {
                for (i, value) in [10, 5, 0, 0, 0, 0, 0, 0, 0].iter().enumerate() {
                    assert_eq!(*value, block.get(i).unwrap());
                }
            });

            node.get_block::<_, inspect_format::Node>(|node_block| {
                assert_eq!(node_block.child_count(), 1);
            });
        }
        node.get_block::<_, inspect_format::Node>(|node_block| {
            assert_eq!(node_block.child_count(), 0);
        });
    }

    #[fuchsia::test]
    fn underflow() {
        let inspector = Inspector::default();
        let root = inspector.root();
        let hist = root.create_uint_linear_histogram(
            "test",
            LinearHistogramParams { floor: 0, step_size: u64::MAX / 2, buckets: 4 },
        );

        // | Bucket    | Range                  |
        // |-----------|------------------------|
        // | Underflow | Empty                  |
        // | 0         | [0, u64::MAX/2)        |
        // | 1         | [u64::MAX/2, u64::MAX) |
        // | 2..3      | Empty                  |
        // | Overflow  | [u64::MAX, +inf)       |

        hist.insert(0); // increment bucket 0
        hist.insert(u64::MAX / 2); // increment bucket 1
        hist.insert(u64::MAX); // increment overflow bucket

        hist.array.get_block::<_, Array<Uint>>(|block| {
            assert_eq!(block.get(0).unwrap(), 0);
            assert_eq!(block.get(1).unwrap(), u64::MAX / 2);

            assert_eq!(block.get(2).unwrap(), 0); // underflow
            assert_eq!(block.get(3).unwrap(), 1); // bucket 0
            assert_eq!(block.get(4).unwrap(), 1); // bucket 1
            assert_eq!(block.get(5).unwrap(), 0); // bucket 2
            assert_eq!(block.get(6).unwrap(), 0); // bucket 3
            assert_eq!(block.get(7).unwrap(), 1); // overflow
        });
    }
}
