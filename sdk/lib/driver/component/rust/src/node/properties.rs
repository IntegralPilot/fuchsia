// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_driver_framework::NodePropertyValue;

/// A newtype wrapper that can be used to construct [`NodePropertyValue`]-compatible values
/// for [`crate::NodeBuilder::add_property`]
pub struct PropertyValue(pub(crate) NodePropertyValue);

impl From<String> for PropertyValue {
    fn from(value: String) -> Self {
        Self(NodePropertyValue::StringValue(value))
    }
}

impl From<&str> for PropertyValue {
    fn from(value: &str) -> Self {
        Self(NodePropertyValue::StringValue(value.to_owned()))
    }
}

impl From<u32> for PropertyValue {
    fn from(value: u32) -> Self {
        Self(NodePropertyValue::IntValue(value))
    }
}

impl From<bool> for PropertyValue {
    fn from(value: bool) -> Self {
        Self(NodePropertyValue::BoolValue(value))
    }
}

impl PropertyValue {
    /// Creates a value from something that is string-like
    pub fn from_string(value: impl Into<String>) -> Self {
        String::into(value.into())
    }

    /// Creates a value from something that is integer-like
    pub fn from_int(value: impl Into<u32>) -> Self {
        u32::into(value.into())
    }

    /// Creates a value from something that is bool-like
    pub fn from_bool(value: impl Into<bool>) -> Self {
        bool::into(value.into())
    }
}
