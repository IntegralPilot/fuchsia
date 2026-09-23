// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::writer::{
    ArithmeticArrayProperty, ArrayProperty, HistogramProperty, InspectType, Node, UintArrayProperty,
};
use diagnostics_hierarchy::{ArrayFormat, ExponentialHistogramParams};
use log::error;
use std::borrow::Cow;

#[derive(Debug, Default)]
/// An exponential histogram property for uint values.
pub struct UintExponentialHistogramProperty {
    array: UintArrayProperty,
    floor: u64,
    initial_step: u64,
    step_multiplier: u64,
    buckets: usize,
}

impl InspectType for UintExponentialHistogramProperty {
    fn into_recorded(self) -> crate::writer::types::RecordedInspectType {
        crate::writer::types::RecordedInspectType::UintArray(self.array)
    }
}

impl UintExponentialHistogramProperty {
    pub(crate) fn new(
        name: Cow<'_, str>,
        params: ExponentialHistogramParams<u64>,
        parent: &Node,
    ) -> Self {
        let slots = params.buckets + ArrayFormat::ExponentialHistogram.extra_slots();
        let array =
            parent.create_uint_array_internal(name, slots, ArrayFormat::ExponentialHistogram);
        array.set(0, params.floor);
        array.set(1, params.initial_step);
        array.set(2, params.step_multiplier);
        Self {
            floor: params.floor,
            initial_step: params.initial_step,
            step_multiplier: params.step_multiplier,
            buckets: params.buckets,
            array,
        }
    }

    fn get_index(&self, value: u64) -> usize {
        let floor = self.floor;
        let mut bucket_end = floor; // The exclusive end of a bucket's range.
        let mut step = self.initial_step;

        let mut index = ArrayFormat::ExponentialHistogram.underflow_bucket_index();
        let overflow_index = ArrayFormat::ExponentialHistogram.overflow_bucket_index(self.buckets);
        while value >= bucket_end && index < overflow_index {
            if let Some(c) = floor.checked_add(step) {
                bucket_end = c;
            } else {
                // Overflow. The next bucket is guaranteed to be choosen, as it
                // contains all possible remaining values for a u64.
                return index + 1;
            }

            step = step.saturating_mul(self.step_multiplier);
            index += 1;
        }
        index
    }
}

impl HistogramProperty for UintExponentialHistogramProperty {
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
                        ArrayFormat::ExponentialHistogram.underflow_bucket_index(),
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
    fn test_uint_exp_histogram() {
        let inspector = Inspector::default();
        let root = inspector.root();
        let node = root.create_child("node");
        {
            let uint_histogram = node.create_uint_exponential_histogram(
                "uint-histogram",
                ExponentialHistogramParams {
                    floor: 1,
                    initial_step: 1,
                    step_multiplier: 2,
                    buckets: 4,
                },
            );
            uint_histogram.insert_multiple(0, 2); // underflow
            uint_histogram.insert(8);
            uint_histogram.insert(500); // overflow
            uint_histogram.array.get_block::<_, Array<Uint>>(|block| {
                for (i, value) in [1, 1, 2, 2, 0, 0, 0, 1, 1].iter().enumerate() {
                    assert_eq!(block.get(i).unwrap(), *value);
                }
            });

            uint_histogram.clear();
            uint_histogram.array.get_block::<_, Array<Uint>>(|block| {
                for (i, value) in [1, 1, 2, 0, 0, 0, 0, 0, 0].iter().enumerate() {
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
    fn overflow_underflow() {
        let inspector = Inspector::default();
        let root = inspector.root();
        let hist = root.create_uint_exponential_histogram(
            "test",
            ExponentialHistogramParams {
                floor: 1,
                initial_step: u64::MAX / 2,
                step_multiplier: 2,
                buckets: 4,
            },
        );

        // | Bucket    | Range                      |
        // |-----------|----------------------------|
        // | Underflow | [0, 1)                     |
        // | 0         | [1, u64::MAX/2 + 1)        |
        // | 1         | [u64::MAX/2 + 1, u64::MAX] |
        // | 2..3      | Empty                      |
        // | Overflow  | Empty                      |

        hist.insert((u64::MAX / 2) + 1);
        hist.insert(0);

        hist.array.get_block::<_, Array<Uint>>(|block| {
            assert_eq!(block.get(0).unwrap(), 1);
            assert_eq!(block.get(1).unwrap(), u64::MAX / 2);
            assert_eq!(block.get(2).unwrap(), 2);

            assert_eq!(block.get(3).unwrap(), 1); // underflow
            assert_eq!(block.get(4).unwrap(), 0); // bucket 0
            assert_eq!(block.get(5).unwrap(), 1); // bucket 1
            assert_eq!(block.get(6).unwrap(), 0); // bucket 2
            assert_eq!(block.get(7).unwrap(), 0); // bucket 3
            assert_eq!(block.get(8).unwrap(), 0); // overflow
        });
    }
}
