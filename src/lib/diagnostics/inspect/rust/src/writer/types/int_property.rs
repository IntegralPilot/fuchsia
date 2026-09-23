// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::writer::{Inner, InnerValueType, InspectType, NumericProperty, Property};
use log::error;

/// Inspect int property data type.
///
/// NOTE: do not rely on PartialEq implementation for true comparison.
/// Instead leverage the reader.
///
/// NOTE: Operations on a Default value are no-ops.
#[derive(Debug, PartialEq, Eq, Default)]
pub struct IntProperty {
    inner: Inner<InnerValueType>,
}

impl InspectType for IntProperty {
    fn into_recorded(self) -> crate::writer::types::RecordedInspectType {
        crate::writer::types::RecordedInspectType::IntProperty(self)
    }
}

crate::impl_inspect_type_internal!(IntProperty);

impl Property<'_> for IntProperty {
    type Type = i64;

    fn set(&self, value: i64) {
        if let Some(ref inner_ref) = self.inner.inner_ref() {
            match inner_ref.state.try_lock() {
                Ok(mut state) => state.set_int_metric(inner_ref.block_index, value),
                Err(err) => error!(err:?; "Failed to set property"),
            }
        }
    }
}

impl NumericProperty<'_> for IntProperty {
    fn add(&self, value: i64) -> Option<i64> {
        if let Some(ref inner_ref) = self.inner.inner_ref() {
            match inner_ref.state.try_lock() {
                Ok(mut state) => {
                    return Some(state.add_int_metric(inner_ref.block_index, value));
                }
                Err(err) => error!(err:?; "Failed to add property"),
            }
        }
        None
    }

    fn subtract(&self, value: i64) -> Option<i64> {
        if let Some(ref inner_ref) = self.inner.inner_ref() {
            match inner_ref.state.try_lock() {
                Ok(mut state) => {
                    return Some(state.subtract_int_metric(inner_ref.block_index, value));
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
    use crate::assert_update_is_atomic;
    use crate::writer::testing_utils::{GetBlockExt, get_state};
    use crate::writer::{Inspector, Node};
    use inspect_format::{BlockType, Int};

    #[fuchsia::test]
    fn int_property() {
        // Create and use a default value.
        let default = IntProperty::default();
        default.add(1);

        let state = get_state(4096);
        let root = Node::new_root(state);
        let node = root.create_child("node");
        {
            let property = node.create_int("property", 1);
            property.get_block::<_, Int>(|property_block| {
                assert_eq!(property_block.block_type(), Some(BlockType::IntValue));
                assert_eq!(property_block.value(), 1);
            });
            node.get_block::<_, inspect_format::Node>(|node_block| {
                assert_eq!(node_block.child_count(), 1);
            });

            property.set(2);
            property.get_block::<_, Int>(|property_block| {
                assert_eq!(property_block.value(), 2);
            });

            assert_eq!(property.subtract(5).unwrap(), -3);
            property.get_block::<_, Int>(|property_block| {
                assert_eq!(property_block.value(), -3);
            });

            assert_eq!(property.add(8).unwrap(), 5);
            property.get_block::<_, Int>(|property_block| {
                assert_eq!(property_block.value(), 5);
            });
        }
        node.get_block::<_, inspect_format::Node>(|node_block| {
            assert_eq!(node_block.child_count(), 0);
        });
    }

    #[fuchsia::test]
    fn property_atomics() {
        let inspector = Inspector::default();
        let property = inspector.root().create_int("property", 5);

        assert_update_is_atomic!(property, |property| {
            property.subtract(1);
            property.subtract(2);
        });
    }
}
