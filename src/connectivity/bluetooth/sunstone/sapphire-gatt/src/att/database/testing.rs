// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

extern crate alloc;

use crate::att::AttributeHandle;
use crate::att::attribute::testing::MockAttribute;
use crate::att::database::Database;
use alloc::collections::BTreeMap;

/// A mock database implementation sorted by 16-bit handle using a B-Tree Map.
///
/// This is for testing purposes only. It uses the `alloc` crate (`BTreeMap`)
/// to simplify handle sorting and query range execution, avoiding the complexity
/// of implementing custom zero-allocation sorted collections in test utilities.
///
/// The production database is implemented by the GATT layer.
pub struct MockDb {
    attributes: BTreeMap<AttributeHandle, MockAttribute>,
}

impl MockDb {
    /// Creates an empty, sorted mock database.
    pub fn new() -> Self {
        Self { attributes: BTreeMap::new() }
    }

    /// Inserts an attribute at the targeted handle.
    pub fn insert(&mut self, handle: AttributeHandle, attr: MockAttribute) {
        self.attributes.insert(handle, attr);
    }

    /// Returns true if the database contains any attributes within the given raw handle range.
    pub fn has_attributes_in_range(&self, start: u16, end: u16) -> bool {
        let Ok(start_h) = AttributeHandle::try_from(start) else {
            return false;
        };
        let Ok(end_h) = AttributeHandle::try_from(end) else {
            return false;
        };
        self.attributes.range(start_h..=end_h).next().is_some()
    }
}

impl Database for MockDb {
    type Attr = MockAttribute;

    fn find_attribute(&self, handle: AttributeHandle) -> Option<&Self::Attr> {
        self.attributes.get(&handle)
    }

    fn query_range(
        &self,
        start: AttributeHandle,
        end: AttributeHandle,
    ) -> impl Iterator<Item = (AttributeHandle, &Self::Attr)> {
        self.attributes.range(start..=end).map(|(&handle, attr)| (handle, attr))
    }
}
