// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::writer::{ArithmeticArrayProperty, ArrayProperty, Inner, InnerValueType, InspectType};
use log::error;

/// Inspect uint array data type.
///
/// NOTE: do not rely on PartialEq implementation for true comparison.
/// Instead leverage the reader.
///
/// NOTE: Operations on a Default value are no-ops.
#[derive(Debug, PartialEq, Eq, Default)]
pub struct UintArrayProperty {
    pub(crate) inner: Inner<InnerValueType>,
}

impl InspectType for UintArrayProperty {
    fn into_recorded(self) -> crate::writer::types::RecordedInspectType {
        crate::writer::types::RecordedInspectType::UintArray(self)
    }
}

crate::impl_inspect_type_internal!(UintArrayProperty);

impl ArrayProperty for UintArrayProperty {
    type Type<'a> = u64;

    fn set<'a>(&self, index: usize, value: impl Into<Self::Type<'a>>) {
        if let Some(ref inner_ref) = self.inner.inner_ref() {
            match inner_ref.state.try_lock() {
                Ok(mut state) => {
                    state.set_array_uint_slot(inner_ref.block_index, index, value.into())
                }
                Err(err) => error!(err:?; "Failed to set property"),
            }
        }
    }

    fn clear(&self) {
        if let Some(ref inner_ref) = self.inner.inner_ref() {
            inner_ref
                .state
                .try_lock()
                .and_then(|mut state| state.clear_array(inner_ref.block_index, 0))
                .unwrap_or_else(|e| {
                    error!("Failed to clear property. Error: {:?}", e);
                });
        }
    }
}

impl ArithmeticArrayProperty for UintArrayProperty {
    fn add<'a>(&self, index: usize, value: Self::Type<'a>)
    where
        Self: 'a,
    {
        if let Some(ref inner_ref) = self.inner.inner_ref() {
            match inner_ref.state.try_lock() {
                Ok(mut state) => {
                    state.add_array_uint_slot(inner_ref.block_index, index, value);
                }
                Err(err) => error!(err:?; "Failed to add property"),
            }
        }
    }

    fn subtract<'a>(&self, index: usize, value: Self::Type<'a>)
    where
        Self: 'a,
    {
        if let Some(ref inner_ref) = self.inner.inner_ref() {
            match inner_ref.state.try_lock() {
                Ok(mut state) => {
                    state.subtract_array_uint_slot(inner_ref.block_index, index, value);
                }
                Err(err) => error!(err:?; "Failed to subtract property"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::writer::testing_utils::GetBlockExt;
    use crate::writer::{Inspector, Length};
    use inspect_format::{Array, Uint};

    #[fuchsia::test]
    fn test_uint_array() {
        // Create and use a default value.
        let default = UintArrayProperty::default();
        default.add(1, 1);

        let inspector = Inspector::default();
        let root = inspector.root();
        let node = root.create_child("node");
        {
            let array = node.create_uint_array("array_property", 5);
            assert_eq!(array.len().unwrap(), 5);

            array.set(0, 5u64);
            array.get_block::<_, Array<Uint>>(|array_block| {
                assert_eq!(array_block.get(0).unwrap(), 5);
            });

            array.add(0, 5);
            array.get_block::<_, Array<Uint>>(|array_block| {
                assert_eq!(array_block.get(0).unwrap(), 10);
            });

            array.subtract(0, 3);
            array.get_block::<_, Array<Uint>>(|array_block| {
                assert_eq!(array_block.get(0).unwrap(), 7);
            });

            array.set(1, 2u64);
            array.set(3, 3u64);

            array.get_block::<_, Array<Uint>>(|array_block| {
                for (i, value) in [7, 2, 0, 3, 0].iter().enumerate() {
                    assert_eq!(array_block.get(i).unwrap(), *value);
                }
            });

            array.clear();
            array.get_block::<_, Array<Uint>>(|array_block| {
                for i in 0..5 {
                    assert_eq!(0, array_block.get(i).unwrap());
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
