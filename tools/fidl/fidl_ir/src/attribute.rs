// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::collections::HashMap;

use serde::Deserialize;

use crate::Constant;
use crate::de::Index;

#[derive(Clone, Debug, Deserialize)]
pub struct Attributes {
    #[serde(default, rename = "maybe_attributes", deserialize_with = "crate::de::index")]
    pub attributes: HashMap<String, Attribute>,
}

impl Attributes {
    /// Get an attribute value, if any.
    pub fn get_value(&self, name: &str) -> Option<&str> {
        Some(self.attributes.get(name)?.args.get("value")?.value.value.as_str())
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct Attribute {
    #[serde(default, rename = "arguments", deserialize_with = "crate::de::index")]
    pub args: HashMap<String, AttributeArg>,
    pub name: String,
}

impl Index for Attribute {
    type Key = String;

    fn key(&self) -> &Self::Key {
        &self.name
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct AttributeArg {
    pub name: String,
    pub value: Constant,
}

impl Index for AttributeArg {
    type Key = String;

    fn key(&self) -> &Self::Key {
        &self.name
    }
}
