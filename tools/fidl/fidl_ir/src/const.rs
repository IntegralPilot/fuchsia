// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use serde::Deserialize;

use crate::de::Index;
use crate::{
    Attributes, CompoundIdentifier, CompoundIdentifierOrMember, Literal, PartialTypeConstructor,
    Type,
};

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Const {
    #[serde(flatten)]
    pub attributes: Attributes,
    pub name: CompoundIdentifier,
    #[serde(rename = "type")]
    pub ty: Type,
    pub value: Constant,
    #[serde(rename = "experimental_maybe_from_alias")]
    pub from_alias: Option<PartialTypeConstructor>,
}

impl Index for Const {
    type Key = CompoundIdentifier;

    fn key(&self) -> &Self::Key {
        &self.name
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct Constant {
    pub value: String,
    #[serde(flatten)]
    pub kind: ConstantKind,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ConstantKind {
    Identifier { identifier: CompoundIdentifierOrMember },
    Literal { literal: Literal },
    BinaryOperator,
}
