// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use serde::Deserialize;

use crate::{CompoundIdentifier, HandleRights, HandleSubtype, PrimSubtype, TypeShape};

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndpointRole {
    Client,
    Server,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Type {
    #[serde(flatten)]
    pub kind: TypeKind,
    #[serde(rename = "type_shape_v2")]
    pub shape: TypeShape,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "kind_v2", rename_all = "snake_case")]
pub enum TypeKind {
    Array {
        element_type: Box<Type>,
        element_count: u32,
        #[serde(rename = "experimental_maybe_from_alias")]
        from_alias: Option<PartialTypeConstructor>,
    },
    Vector {
        element_type: Box<Type>,
        #[serde(default, rename = "maybe_element_count")]
        element_count: Option<u32>,
        nullable: bool,
        #[serde(rename = "experimental_maybe_from_alias")]
        from_alias: Option<PartialTypeConstructor>,
    },
    String {
        #[serde(default, rename = "maybe_element_count")]
        element_count: Option<u32>,
        nullable: bool,
    },
    Handle {
        nullable: bool,
        rights: HandleRights,
        subtype: HandleSubtype,
        resource_identifier: String,
    },
    Endpoint {
        nullable: bool,
        role: EndpointRole,
        protocol: CompoundIdentifier,
        protocol_transport: String,
    },
    Primitive {
        subtype: PrimSubtype,
    },
    Identifier {
        identifier: CompoundIdentifier,
        nullable: bool,
        #[serde(default = "default_protocol_transport")]
        protocol_transport: String,
    },
    Internal {
        subtype: InternalSubtype,
    },
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InternalSubtype {
    FrameworkError,
}

fn default_protocol_transport() -> String {
    "Channel".to_string()
}

#[derive(Clone, Debug, Deserialize)]
pub struct PartialTypeConstructor {
    pub name: CompoundIdentifier,
    pub args: Vec<PartialTypeConstructor>,
    #[serde(rename = "nullable")]
    pub is_nullable: bool,
}
