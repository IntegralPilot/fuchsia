// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use serde::Deserialize;

use crate::de::Index;
use crate::{
    Attributes, CompoundIdentifier, Constant, Identifier, IntType, PrimSubtype, Type, TypeKind,
};

#[derive(Clone, Debug, Deserialize)]
pub struct Bits {
    #[serde(flatten)]
    pub attributes: Attributes,
    pub name: CompoundIdentifier,
    pub naming_context: Vec<String>,
    pub members: Vec<BitsMember>,
    #[serde(rename = "strict")]
    pub is_strict: bool,
    #[serde(rename = "type")]
    pub ty: Type,
}

impl Bits {
    pub fn subtype(&self) -> IntType {
        let Type { kind: TypeKind::Primitive { subtype }, .. } = &self.ty else {
            panic!("invalid non-integral primitive subtype for bits");
        };

        match subtype {
            PrimSubtype::Uint8 => IntType::Uint8,
            PrimSubtype::Uint16 => IntType::Uint16,
            PrimSubtype::Uint32 => IntType::Uint32,
            PrimSubtype::Uint64 => IntType::Uint64,
            _ => unreachable!(),
        }
    }
}

impl Index for Bits {
    type Key = CompoundIdentifier;

    fn key(&self) -> &Self::Key {
        &self.name
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct BitsMember {
    #[serde(flatten)]
    pub attributes: Attributes,
    pub name: Identifier,
    pub value: Constant,
}
