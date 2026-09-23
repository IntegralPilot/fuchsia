// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::num::NonZeroI64;

use serde::Deserialize;

use crate::de::Index;

use crate::{Attributes, CompoundIdentifier, Identifier, PartialTypeConstructor, Type, TypeShape};

#[derive(Clone, Debug, Deserialize)]
pub struct Table {
    #[serde(flatten)]
    pub attributes: Attributes,
    pub name: CompoundIdentifier,
    pub naming_context: Vec<String>,
    pub members: Vec<TableMember>,
    #[serde(rename = "resource")]
    pub is_resource: bool,
    #[serde(rename = "type_shape_v2")]
    pub shape: TypeShape,
}

impl Index for Table {
    type Key = CompoundIdentifier;

    fn key(&self) -> &Self::Key {
        &self.name
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct TableMember {
    #[serde(flatten)]
    pub attributes: Attributes,
    pub name: Identifier,
    #[serde(rename = "type")]
    pub ty: Type,
    pub ordinal: NonZeroI64,
    #[serde(rename = "experimental_maybe_from_alias")]
    pub from_alias: Option<PartialTypeConstructor>,
}
