// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::writer::{
    ArithmeticArrayProperty, ArrayProperty, HistogramProperty, InspectType, IntArrayProperty, Node,
};
use diagnostics_hierarchy::{ArrayFormat, ExponentialHistogramParams};
use log::error;
use std::borrow::Cow;

#[derive(Debug, Default)]
/// An exponential histogram property for int values.
pub struct IntExponentialHistogramProperty {
    array: IntArrayProperty,
    floor: i64,
    initial_step: i64,
    step_multiplier: i64,
    buckets: usize,
}

impl InspectType for IntExponentialHistogramProperty {
    fn into_recorded(self) -> crate::writer::types::RecordedInspectType {
        crate::writer::types::RecordedInspectType::IntArray(self.array)
    }
}

impl IntExponentialHistogramProperty {
    pub(crate) fn new(
        name: Cow<'_, str>,
        params: ExponentialHistogramParams<i64>,
        parent: &Node,
    ) -> Self {
        let slots = params.buckets + ArrayFormat::ExponentialHistogram.extra_slots();
        let array =
            parent.create_int_array_internal(name, slots, ArrayFormat::ExponentialHistogram);
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

    fn get_index(&self, value: i64) -> usize {
        // Use a larger data type to support histograms of total width larger
        // than i64::MAX (e.g. histogram range spanning from i64::MIN to
        // i64::MAX, a width of u64::MAX).
        let value = value as i128;
        let floor = self.floor as i128;
        let mut bucket_end = floor; // Exclusive end of the current bucket.
        let mut step = self.initial_step as i128;

        let mut index = ArrayFormat::ExponentialHistogram.underflow_bucket_index();
        let overflow_index = ArrayFormat::ExponentialHistogram.overflow_bucket_index(self.buckets);

        while value >= bucket_end && index < overflow_index {
            if let Some(c) = floor.checked_add(step) {
                bucket_end = c;
            } else {
                // Overflow. The next bucket contains all possible remaining
                // values for an i128; it is guaranteed to be choosen.
                return index + 1;
            }

            if bucket_end > i64::MAX as i128 {
                // The next bucket extends beyond representable i64; it is
                // guaranteed to be choosen.
                return index + 1;
            }

            step = step.saturating_mul(self.step_multiplier as i128);
            index += 1;
        }
        index
    }
}

impl HistogramProperty for IntExponentialHistogramProperty {
    type Type = i64;

    fn insert(&self, value: i64) {
        self.insert_multiple(value, 1);
    }

    fn insert_multiple(&self, value: i64, count: usize) {
        self.array.add(self.get_index(value), count as i64);
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
    use inspect_format::{Array, Int};

    #[fuchsia::test]
    fn test_int_exp_histogram() {
        let inspector = Inspector::default();
        let root = inspector.root();
        let node = root.create_child("node");
        {
            let int_histogram = node.create_int_exponential_histogram(
                "int-histogram",
                ExponentialHistogramParams {
                    floor: 1,
                    initial_step: 1,
                    step_multiplier: 2,
                    buckets: 4,
                },
            );
            int_histogram.insert_multiple(-1, 2); // underflow
            int_histogram.insert(8);
            int_histogram.insert(500); // overflow
            int_histogram.array.get_block::<_, Array<Int>>(|block| {
                for (i, value) in [1, 1, 2, 2, 0, 0, 0, 1, 1].iter().enumerate() {
                    assert_eq!(block.get(i).unwrap(), *value);
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
    fn exp_histogram_insert() {
        let inspector = Inspector::default();
        let root = inspector.root();
        let hist = root.create_int_exponential_histogram(
            "test",
            ExponentialHistogramParams {
                floor: 0,
                initial_step: 2,
                step_multiplier: 4,
                buckets: 4,
            },
        );
        for i in -200..200 {
            hist.insert(i);
        }
        hist.array.get_block::<_, Array<Int>>(|block| {
            assert_eq!(block.get(0).unwrap(), 0);
            assert_eq!(block.get(1).unwrap(), 2);
            assert_eq!(block.get(2).unwrap(), 4);

            // Buckets
            let i = 3;
            assert_eq!(block.get(i).unwrap(), 200);
            assert_eq!(block.get(i + 1).unwrap(), 2);
            assert_eq!(block.get(i + 2).unwrap(), 6);
            assert_eq!(block.get(i + 3).unwrap(), 24);
            assert_eq!(block.get(i + 4).unwrap(), 96);
            assert_eq!(block.get(i + 5).unwrap(), 72);
        });
    }

    #[fuchsia::test]
    fn overflow_underflow() {
        let inspector = Inspector::default();
        let root = inspector.root();
        let hist = root.create_int_exponential_histogram(
            "test",
            ExponentialHistogramParams {
                floor: 0,
                initial_step: i64::MAX / 2,
                step_multiplier: 2,
                buckets: 4,
            },
        );

        // this will get multiplied by initial step and overflow
        hist.insert((i64::MAX / 2) + 1);

        hist.insert(-5);

        hist.array.get_block::<_, Array<Int>>(|block| {
            assert_eq!(block.get(0).unwrap(), 0);
            assert_eq!(block.get(1).unwrap(), i64::MAX / 2);
            assert_eq!(block.get(2).unwrap(), 2);

            assert_eq!(block.get(3).unwrap(), 1);
            assert_eq!(block.get(4).unwrap(), 0);
            assert_eq!(block.get(5).unwrap(), 1);
            assert_eq!(block.get(6).unwrap(), 0);
            assert_eq!(block.get(7).unwrap(), 0);
            assert_eq!(block.get(8).unwrap(), 0);
        });
    }
}
