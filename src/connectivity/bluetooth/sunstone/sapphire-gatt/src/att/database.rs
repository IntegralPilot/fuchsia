// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#[cfg(test)]
pub mod testing;

use crate::att::AttributeHandle;
use crate::att::attribute::Attribute;

/// The interface representing an Attribute Protocol (ATT) Database.
///
/// Implementations provide attribute search and query capabilities sorted by handle.
pub trait Database {
    /// The concrete attribute type stored in this database.
    ///
    /// An associated type is used instead of a trait object (`dyn Attribute`) because
    /// `Attribute` contains async methods (making it not object-safe) and to avoid
    /// dynamic dispatch overhead. It also keeps this trait generic and decoupled from
    /// any specific concrete implementation (like `AttributeType`).
    type Attr: Attribute;

    /// Searches for an attribute in the database matching the handle.
    fn find_attribute(&self, handle: AttributeHandle) -> Option<&Self::Attr>;

    /// Queries an inclusive range of attributes (`start..=end`) in handle-sorted order.
    fn query_range(
        &self,
        start: AttributeHandle,
        end: AttributeHandle,
    ) -> impl Iterator<Item = (AttributeHandle, &Self::Attr)>;
}
