// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::writer::{
    ArithmeticArrayProperty, ArrayProperty, DoubleArrayProperty, HistogramProperty, InspectType,
    Node,
};
use diagnostics_hierarchy::{ArrayFormat, ExponentialHistogramParams};
use log::error;
use std::borrow::Cow;

#[derive(Debug, Default)]
/// An exponential histogram property for double values.
pub struct DoubleExponentialHistogramProperty {
    array: DoubleArrayProperty,
    floor: f64,
    initial_step: f64,
    step_multiplier: f64,
    buckets: usize,
}

impl InspectType for DoubleExponentialHistogramProperty {
    fn into_recorded(self) -> crate::writer::types::RecordedInspectType {
        crate::writer::types::RecordedInspectType::DoubleArray(self.array)
    }
}

impl DoubleExponentialHistogramProperty {
    pub(crate) fn new(
        name: Cow<'_, str>,
        params: ExponentialHistogramParams<f64>,
        parent: &Node,
    ) -> Self {
        let slots = params.buckets + ArrayFormat::ExponentialHistogram.extra_slots();
        let array =
            parent.create_double_array_internal(name, slots, ArrayFormat::ExponentialHistogram);
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

    fn get_index(&self, value: f64) -> usize {
        if value < self.floor {
            return ArrayFormat::ExponentialHistogram.underflow_bucket_index();
        }

        let mut index = ArrayFormat::ExponentialHistogram.underflow_bucket_index() + 1;
        let overflow_index = ArrayFormat::ExponentialHistogram.overflow_bucket_index(self.buckets);

        // Exclusive bucket end is: initial_step * step_multiplier^i + floor
        // where i is the bucket index, excluding underflow and overflow.
        let mut bucket_end = self.initial_step + self.floor;
        let mut multiplier = 1.0;

        while value >= bucket_end && index < overflow_index {
            multiplier *= self.step_multiplier;
            // mul_add is necessary when a bucket's exclusive end is f64::MAX.
            bucket_end = self.initial_step.mul_add(multiplier, self.floor);
            index += 1;
        }
        index
    }
}

impl HistogramProperty for DoubleExponentialHistogramProperty {
    type Type = f64;

    fn insert(&self, value: f64) {
        self.insert_multiple(value, 1);
    }

    fn insert_multiple(&self, value: f64, count: usize) {
        self.array.add(self.get_index(value), count as f64);
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
    use inspect_format::{Array, Double};

    #[fuchsia::test]
    fn test_double_exp_histogram() {
        let inspector = Inspector::default();
        let root = inspector.root();
        let node = root.create_child("node");
        {
            let double_histogram = node.create_double_exponential_histogram(
                "double-histogram",
                ExponentialHistogramParams {
                    floor: 1.0,
                    initial_step: 1.0,
                    step_multiplier: 2.0,
                    buckets: 4,
                },
            );
            double_histogram.insert_multiple(0.0, 2); // underflow
            double_histogram.insert(8.3);
            double_histogram.insert(500.0); // overflow
            double_histogram.array.get_block::<_, Array<Double>>(|block| {
                for (i, value) in [1.0, 1.0, 2.0, 2.0, 0.0, 0.0, 0.0, 1.0, 1.0].iter().enumerate() {
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
    fn overflow_underflow() {
        let inspector = Inspector::default();
        let root = inspector.root();
        let hist = root.create_double_exponential_histogram(
            "test",
            ExponentialHistogramParams {
                floor: f64::MIN,
                initial_step: f64::MAX,
                step_multiplier: 2.0,
                buckets: 4,
            },
        );

        // | Bucket    | Range         |
        // | --------- | ------------- |
        // | Underflow | (-inf, MIN)   |
        // |         0 | [MIN, 0)      |
        // |         1 | [0, MAX)      |
        // |         2 | MAX           |
        // |         3 | Empty         |
        // |  Overflow | +inf          |

        hist.insert_multiple(f64::NEG_INFINITY, 1);
        hist.insert_multiple(f64::MIN, 2);
        hist.insert_multiple(0.0, 3);
        hist.insert_multiple(f64::MAX, 4);
        hist.insert_multiple(f64::INFINITY, 5);

        hist.array.get_block::<_, Array<Double>>(|block| {
            assert_eq!(block.get(0).unwrap(), f64::MIN);
            assert_eq!(block.get(1).unwrap(), f64::MAX);
            assert_eq!(block.get(2).unwrap(), 2.0);

            assert_eq!(block.get(3).unwrap(), 1.0); // underflow bucket
            assert_eq!(block.get(4).unwrap(), 2.0); // bucket 0
            assert_eq!(block.get(5).unwrap(), 3.0); // bucket 1
            assert_eq!(block.get(6).unwrap(), 4.0); // bucket 2
            assert_eq!(block.get(7).unwrap(), 0.0); // bucket 3
            assert_eq!(block.get(8).unwrap(), 5.0); // overflow bucket
        });
    }
}
