// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use serde::Deserialize;

use crate::de::Index;
use crate::{Attributes, CompoundIdentifier, PartialTypeConstructor, Type};

#[derive(Clone, Debug, Deserialize)]
pub struct TypeAlias {
    #[serde(flatten)]
    pub attributes: Attributes,
    pub name: CompoundIdentifier,
    #[serde(rename = "type")]
    pub ty: Type,
    #[serde(rename = "experimental_maybe_from_alias")]
    pub from_alias: Option<PartialTypeConstructor>,
}

impl Index for TypeAlias {
    type Key = CompoundIdentifier;

    fn key(&self) -> &Self::Key {
        &self.name
    }
}
