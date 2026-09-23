// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use serde::Deserialize;

use crate::de::Index;

use crate::{Attributes, CompoundIdentifier, Identifier, PartialTypeConstructor, Type, TypeShape};

#[derive(Clone, Debug, Deserialize)]
pub struct Struct {
    #[serde(flatten)]
    pub attributes: Attributes,
    pub name: CompoundIdentifier,
    pub naming_context: Vec<String>,
    pub members: Vec<StructMember>,
    #[serde(rename = "resource")]
    pub is_resource: bool,
    #[serde(rename = "type_shape_v2")]
    pub shape: TypeShape,
    pub is_empty_success_struct: bool,
}

impl Index for Struct {
    type Key = CompoundIdentifier;

    fn key(&self) -> &Self::Key {
        &self.name
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct StructMember {
    #[serde(flatten)]
    pub attributes: Attributes,
    pub name: Identifier,
    #[serde(rename = "type")]
    pub ty: Type,
    #[serde(rename = "field_shape_v2")]
    pub field_shape: FieldShape,
    #[serde(rename = "experimental_maybe_from_alias")]
    pub from_alias: Option<PartialTypeConstructor>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct FieldShape {
    pub offset: u32,
    pub padding: u32,
}
