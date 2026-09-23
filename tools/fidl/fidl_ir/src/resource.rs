// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use serde::Deserialize;

use crate::de::Index;
use crate::{Attributes, CompoundIdentifier, Identifier, Type};

#[derive(Clone, Debug, Deserialize)]
pub struct Resource {
    #[serde(flatten)]
    pub attributes: Attributes,
    pub name: CompoundIdentifier,
    pub properties: Vec<ResourceProperty>,
    #[serde(rename = "type")]
    pub ty: Type,
}

impl Index for Resource {
    type Key = CompoundIdentifier;

    fn key(&self) -> &Self::Key {
        &self.name
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct ResourceProperty {
    pub name: Identifier,
    #[serde(rename = "type")]
    pub ty: Type,
}
