// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::writer::{
    ArithmeticArrayProperty, ArrayProperty, DoubleArrayProperty, HistogramProperty, InspectType,
    Node,
};
use diagnostics_hierarchy::{ArrayFormat, LinearHistogramParams};
use log::error;
use std::borrow::Cow;

#[derive(Debug, Default)]
/// A linear histogram property for double values.
pub struct DoubleLinearHistogramProperty {
    array: DoubleArrayProperty,
    floor: f64,
    buckets: usize,
    step_size: f64,
}

impl InspectType for DoubleLinearHistogramProperty {
    fn into_recorded(self) -> crate::writer::types::RecordedInspectType {
        crate::writer::types::RecordedInspectType::DoubleArray(self.array)
    }
}

impl DoubleLinearHistogramProperty {
    pub(crate) fn new(
        name: Cow<'_, str>,
        params: LinearHistogramParams<f64>,
        parent: &Node,
    ) -> Self {
        let slots = params.buckets + ArrayFormat::LinearHistogram.extra_slots();
        let array = parent.create_double_array_internal(name, slots, ArrayFormat::LinearHistogram);
        array.set(0, params.floor);
        array.set(1, params.step_size);
        Self { floor: params.floor, step_size: params.step_size, buckets: params.buckets, array }
    }

    fn get_index(&self, value: f64) -> usize {
        let mut bucket_end = self.floor; // The exclusive end of a bucket's range.
        let mut index = ArrayFormat::LinearHistogram.underflow_bucket_index();
        let overflow_index = ArrayFormat::LinearHistogram.overflow_bucket_index(self.buckets);
        while value >= bucket_end && index < overflow_index {
            bucket_end += self.step_size;
            index += 1;
        }
        index
    }
}

impl HistogramProperty for DoubleLinearHistogramProperty {
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
    use inspect_format::{Array, Double};

    #[fuchsia::test]
    fn double_linear_histogram() {
        let inspector = Inspector::default();
        let root = inspector.root();
        let node = root.create_child("node");
        {
            let double_histogram = node.create_double_linear_histogram(
                "double-histogram",
                LinearHistogramParams { floor: 10.0, step_size: 5.0, buckets: 5 },
            );
            double_histogram.insert_multiple(0.0, 2); // underflow
            double_histogram.insert(25.3);
            double_histogram.insert(500.0); // overflow
            double_histogram.array.get_block::<_, Array<Double>>(|block| {
                for (i, value) in [10.0, 5.0, 2.0, 0.0, 0.0, 0.0, 1.0, 0.0, 1.0].iter().enumerate()
                {
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
}
