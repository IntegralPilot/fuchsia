// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::writer::{ArithmeticArrayProperty, ArrayProperty, Inner, InnerValueType, InspectType};
use log::error;

/// Inspect double array data type.
///
/// NOTE: do not rely on PartialEq implementation for true comparison.
/// Instead leverage the reader.
///
/// NOTE: Operations on a Default value are no-ops.
#[derive(Debug, PartialEq, Eq, Default)]
pub struct DoubleArrayProperty {
    pub(crate) inner: Inner<InnerValueType>,
}

impl InspectType for DoubleArrayProperty {
    fn into_recorded(self) -> crate::writer::types::RecordedInspectType {
        crate::writer::types::RecordedInspectType::DoubleArray(self)
    }
}

crate::impl_inspect_type_internal!(DoubleArrayProperty);

impl ArrayProperty for DoubleArrayProperty {
    type Type<'a> = f64;

    fn set<'a>(&self, index: usize, value: impl Into<Self::Type<'a>>) {
        if let Some(ref inner_ref) = self.inner.inner_ref() {
            match inner_ref.state.try_lock() {
                Ok(mut state) => {
                    state.set_array_double_slot(inner_ref.block_index, index, value.into())
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
                .unwrap_or_else(|err| {
                    error!(err:?; "Failed to clear property.");
                });
        }
    }
}

impl ArithmeticArrayProperty for DoubleArrayProperty {
    fn add<'a>(&self, index: usize, value: Self::Type<'a>)
    where
        Self: 'a,
    {
        if let Some(ref inner_ref) = self.inner.inner_ref() {
            match inner_ref.state.try_lock() {
                Ok(mut state) => {
                    state.add_array_double_slot(inner_ref.block_index, index, value);
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
                    state.subtract_array_double_slot(inner_ref.block_index, index, value);
                }
                Err(err) => error!(err:?; "Failed to subtract property"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Length;
    use crate::writer::Inspector;
    use crate::writer::testing_utils::GetBlockExt;
    use inspect_format::{Array, Double};

    #[fuchsia::test]
    fn test_double_array() {
        // Create and use a default value.
        let default = DoubleArrayProperty::default();
        default.add(1, 1.0);

        let inspector = Inspector::default();
        let root = inspector.root();
        let node = root.create_child("node");
        {
            let array = node.create_double_array("array_property", 5);
            assert_eq!(array.len().unwrap(), 5);

            array.set(0, 5.0);
            array.get_block::<_, Array<Double>>(|block| {
                assert_eq!(block.get(0).unwrap(), 5.0);
            });

            array.add(0, 5.3);
            array.get_block::<_, Array<Double>>(|array_block| {
                assert_eq!(array_block.get(0).unwrap(), 10.3);
            });

            array.subtract(0, 3.4);
            array.get_block::<_, Array<Double>>(|array_block| {
                assert_eq!(array_block.get(0).unwrap(), 6.9);
            });

            array.set(1, 2.5);
            array.set(3, -3.1);

            array.get_block::<_, Array<Double>>(|array_block| {
                for (i, value) in [6.9, 2.5, 0.0, -3.1, 0.0].iter().enumerate() {
                    assert_eq!(array_block.get(i).unwrap(), *value);
                }
            });

            array.clear();
            array.get_block::<_, Array<Double>>(|array_block| {
                for i in 0..5 {
                    assert_eq!(0.0, array_block.get(i).unwrap());
                }
            });

            node.get_block::<_, inspect_format::Node>(|block| {
                assert_eq!(block.child_count(), 1);
            });
        }
        node.get_block::<_, inspect_format::Node>(|block| {
            assert_eq!(block.child_count(), 0);
        });
    }
}
