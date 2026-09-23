// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::writer::{Inner, InnerValueType, InspectType, NumericProperty, Property};
use log::error;

/// Inspect double property type.
///
/// NOTE: do not rely on PartialEq implementation for true comparison.
/// Instead leverage the reader.
///
/// NOTE: Operations on a Default value are no-ops.
#[derive(Debug, PartialEq, Eq, Default)]
pub struct DoubleProperty {
    inner: Inner<InnerValueType>,
}

impl InspectType for DoubleProperty {
    fn into_recorded(self) -> crate::writer::types::RecordedInspectType {
        crate::writer::types::RecordedInspectType::DoubleProperty(self)
    }
}

crate::impl_inspect_type_internal!(DoubleProperty);

impl Property<'_> for DoubleProperty {
    type Type = f64;

    fn set(&self, value: f64) {
        if let Some(ref inner_ref) = self.inner.inner_ref() {
            match inner_ref.state.try_lock() {
                Ok(mut state) => {
                    state.set_double_metric(inner_ref.block_index, value);
                }
                Err(err) => error!(err:?; "Failed to set property"),
            }
        }
    }
}

impl NumericProperty<'_> for DoubleProperty {
    fn add(&self, value: f64) -> Option<f64> {
        if let Some(ref inner_ref) = self.inner.inner_ref() {
            match inner_ref.state.try_lock() {
                Ok(mut state) => {
                    return Some(state.add_double_metric(inner_ref.block_index, value));
                }
                Err(err) => error!(err:?; "Failed to add property"),
            }
        }
        None
    }

    fn subtract(&self, value: f64) -> Option<f64> {
        if let Some(ref inner_ref) = self.inner.inner_ref() {
            match inner_ref.state.try_lock() {
                Ok(mut state) => {
                    return Some(state.subtract_double_metric(inner_ref.block_index, value));
                }
                Err(err) => error!(err:?; "Failed to subtract property"),
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::writer::Node;
    use crate::writer::testing_utils::{GetBlockExt, get_state};
    use inspect_format::{BlockType, Double};

    #[fuchsia::test]
    fn double_property() {
        // Create and use a default value.
        let default = DoubleProperty::default();
        default.add(1.0);

        let state = get_state(4096);
        let root = Node::new_root(state);
        let node = root.create_child("node");
        {
            let property = node.create_double("property", 1.0);
            property.get_block::<_, Double>(|property_block| {
                assert_eq!(property_block.block_type(), Some(BlockType::DoubleValue));
                assert_eq!(property_block.value(), 1.0);
            });
            node.get_block::<_, inspect_format::Node>(|node_block| {
                assert_eq!(node_block.child_count(), 1);
            });

            property.set(2.0);
            property.get_block::<_, Double>(|property_block| {
                assert_eq!(property_block.value(), 2.0);
            });

            assert_eq!(property.subtract(5.5).unwrap(), -3.5);
            property.get_block::<_, Double>(|property_block| {
                assert_eq!(property_block.value(), -3.5);
            });

            assert_eq!(property.add(8.1).unwrap(), 4.6);
            property.get_block::<_, Double>(|property_block| {
                assert_eq!(property_block.value(), 4.6);
            });
        }
        node.get_block::<_, inspect_format::Node>(|node_block| {
            assert_eq!(node_block.child_count(), 0);
        });
    }
}
