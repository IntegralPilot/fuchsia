// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::writer::{Inner, InnerValueType, InspectType, Property};

/// Inspect API Bool Property data type.
///
/// NOTE: do not rely on PartialEq implementation for true comparison.
/// Instead leverage the reader.
///
/// NOTE: Operations on a Default value are no-ops.
#[derive(Debug, PartialEq, Eq, Default)]
pub struct BoolProperty {
    inner: Inner<InnerValueType>,
}

impl Property<'_> for BoolProperty {
    type Type = bool;

    fn set(&self, value: bool) {
        if let Some(ref inner_ref) = self.inner.inner_ref()
            && let Ok(mut state) = inner_ref.state.try_lock()
        {
            state.set_bool(inner_ref.block_index, value);
        }
    }
}

impl InspectType for BoolProperty {
    fn into_recorded(self) -> crate::writer::types::RecordedInspectType {
        crate::writer::types::RecordedInspectType::BoolProperty(self)
    }
}

crate::impl_inspect_type_internal!(BoolProperty);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::writer::Node;
    use crate::writer::testing_utils::{GetBlockExt, get_state};
    use inspect_format::{BlockType, Bool};

    #[fuchsia::test]
    fn bool_property() {
        // Create and use a default value.
        let default = BoolProperty::default();
        default.set(true);

        let state = get_state(4096);
        let root = Node::new_root(state);
        let node = root.create_child("node");
        {
            let property = node.create_bool("property", true);
            property.get_block::<_, Bool>(|block| {
                assert_eq!(block.block_type(), Some(BlockType::BoolValue));
                assert!(block.value());
            });
            node.get_block::<_, inspect_format::Node>(|block| {
                assert_eq!(block.child_count(), 1);
            });

            property.set(false);
            property.get_block::<_, Bool>(|block| {
                assert!(!block.value());
            });
        }
        node.get_block::<_, inspect_format::Node>(|block| {
            assert_eq!(block.child_count(), 0);
        });
    }
}
