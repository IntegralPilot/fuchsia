// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::num::NonZeroI64;

use serde::Deserialize;

use crate::de::Index;

use crate::{Attributes, CompoundIdentifier, Identifier, PartialTypeConstructor, Type, TypeShape};

#[derive(Clone, Debug, Deserialize)]
pub struct Union {
    #[serde(flatten)]
    pub attributes: Attributes,
    pub members: Vec<UnionMember>,
    pub name: CompoundIdentifier,
    pub naming_context: Vec<String>,
    #[serde(rename = "resource")]
    pub is_resource: bool,
    pub is_result: bool,
    #[serde(rename = "strict")]
    pub is_strict: bool,
    #[serde(rename = "type_shape_v2")]
    pub shape: TypeShape,
}

impl Index for Union {
    type Key = CompoundIdentifier;

    fn key(&self) -> &Self::Key {
        &self.name
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct UnionMember {
    #[serde(flatten)]
    pub attributes: Attributes,
    pub name: Identifier,
    pub ordinal: NonZeroI64,
    #[serde(rename = "type")]
    pub ty: Type,
    #[serde(rename = "experimental_maybe_from_alias")]
    pub from_alias: Option<PartialTypeConstructor>,
}
