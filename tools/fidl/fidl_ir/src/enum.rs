// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use serde::Deserialize;

use crate::de::Index;

use crate::{Attributes, CompoundIdentifier, Constant, Identifier, IntType};

#[derive(Clone, Debug, Deserialize)]
pub struct Enum {
    #[serde(flatten)]
    pub attributes: Attributes,
    pub members: Vec<EnumMember>,
    pub name: CompoundIdentifier,
    pub naming_context: Vec<String>,
    #[serde(rename = "strict")]
    pub is_strict: bool,
    #[serde(rename = "type")]
    pub ty: IntType,
}

impl Index for Enum {
    type Key = CompoundIdentifier;

    fn key(&self) -> &Self::Key {
        &self.name
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct EnumMember {
    #[serde(flatten)]
    pub attributes: Attributes,
    pub name: Identifier,
    pub value: Constant,
}
