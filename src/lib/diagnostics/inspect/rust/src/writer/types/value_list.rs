// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::*;
use derivative::Derivative;
use fuchsia_sync::Mutex;

/// An enum representing any standard Inspect type that can be recorded in a `ValueList`.
///
/// Types that are 16 bytes or less are stored directly in the enum. Types that are larger than
/// 16 bytes must be boxed in a `Boxed` variant.
#[derive(Debug)]
pub enum RecordedInspectType {
    Node(Node),
    BoolProperty(BoolProperty),
    BytesProperty(BytesProperty),
    DoubleProperty(DoubleProperty),
    IntProperty(IntProperty),
    StringProperty(StringProperty),
    UintProperty(UintProperty),
    DoubleArray(DoubleArrayProperty),
    IntArray(IntArrayProperty),
    StringArray(StringArrayProperty),
    UintArray(UintArrayProperty),
    LazyNode(LazyNode),
    Boxed(Box<dyn InspectType>),
}

// Ensure that we don't inadvertently increase the size of RecordedInspectType by adding a new
// variant with an unboxed payload larger than 16 bytes.
const _RECORDED_INSPECT_TYPE_SIZE_ASSERTION: () = assert!(
    std::mem::size_of::<RecordedInspectType>() == 24,
    "RecordedInspectType size changed! Expected 24 bytes (1 byte tag + 7 bytes padding + \
     16 bytes max unboxed payload)."
);

type InspectTypeList = Vec<RecordedInspectType>;

/// Holds a list of inspect types that won't change.
#[derive(Derivative)]
#[derivative(Debug, PartialEq)]
pub struct ValueList {
    #[derivative(PartialEq = "ignore")]
    #[derivative(Debug = "ignore")]
    values: Mutex<Option<InspectTypeList>>,
}

impl Default for ValueList {
    fn default() -> Self {
        ValueList::new()
    }
}

impl ValueList {
    /// Creates a new empty value list.
    pub fn new() -> Self {
        Self { values: Mutex::new(None) }
    }

    /// Stores an inspect type that won't change.
    pub fn record(&self, value: impl InspectType + 'static) {
        let converted_value = value.into_recorded();
        let mut values_lock = self.values.lock();
        if let Some(ref mut values) = *values_lock {
            values.push(converted_value);
        } else {
            *values_lock = Some(vec![converted_value]);
        }
    }

    /// Clears all values from ValueList, rendering it empty.
    /// `InspectType` values contained will be dropped.
    pub fn clear(&self) {
        let mut values_lock = self.values.lock();
        *values_lock = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::writer::types::Inspector;
    use diagnostics_assertions::assert_json_diff;

    #[fuchsia::test]
    fn value_list_record() {
        let inspector = Inspector::default();
        let child = inspector.root().create_child("test");
        let value_list = ValueList::new();
        assert!(value_list.values.lock().is_none());
        value_list.record(child);
        assert_eq!(value_list.values.lock().as_ref().unwrap().len(), 1);
    }

    #[fuchsia::test]
    async fn value_list_drop_recorded() {
        let inspector = Inspector::default();
        let child = inspector.root().create_child("test");
        let value_list = ValueList::new();
        assert!(value_list.values.lock().is_none());
        value_list.record(child);
        assert_eq!(value_list.values.lock().as_ref().unwrap().len(), 1);
        assert_json_diff!(inspector, root: {
            test: {},
        });

        value_list.clear();
        assert!(value_list.values.lock().is_none());
        assert_json_diff!(inspector, root: {});
    }
}
