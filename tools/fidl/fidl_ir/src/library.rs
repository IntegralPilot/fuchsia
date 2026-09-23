// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::collections::HashMap;

use serde::Deserialize;

use crate::{
    Bits, CompoundIdentifier, Const, DeclType, Enum, LibraryDependency, Protocol, Resource,
    Service, Struct, Table, TypeAlias, Union,
};

/// A FIDL library.
#[derive(Deserialize)]
pub struct Library {
    pub name: String,
    #[serde(deserialize_with = "crate::de::index")]
    pub alias_declarations: HashMap<CompoundIdentifier, TypeAlias>,
    #[serde(deserialize_with = "crate::de::index")]
    pub bits_declarations: HashMap<CompoundIdentifier, Bits>,
    #[serde(deserialize_with = "crate::de::index")]
    pub const_declarations: HashMap<CompoundIdentifier, Const>,
    #[serde(deserialize_with = "crate::de::index")]
    pub enum_declarations: HashMap<CompoundIdentifier, Enum>,
    #[serde(deserialize_with = "crate::de::index")]
    pub protocol_declarations: HashMap<CompoundIdentifier, Protocol>,
    #[serde(deserialize_with = "crate::de::index")]
    pub service_declarations: HashMap<CompoundIdentifier, Service>,
    #[serde(deserialize_with = "crate::de::index")]
    pub struct_declarations: HashMap<CompoundIdentifier, Struct>,
    #[serde(deserialize_with = "crate::de::index")]
    pub external_struct_declarations: HashMap<CompoundIdentifier, Struct>,
    #[serde(deserialize_with = "crate::de::index")]
    pub table_declarations: HashMap<CompoundIdentifier, Table>,
    #[serde(deserialize_with = "crate::de::index")]
    pub union_declarations: HashMap<CompoundIdentifier, Union>,
    pub declaration_order: Vec<CompoundIdentifier>,
    pub declarations: HashMap<CompoundIdentifier, DeclType>,
    #[serde(deserialize_with = "crate::de::index")]
    pub library_dependencies: HashMap<String, LibraryDependency>,
    #[serde(deserialize_with = "crate::de::index", rename = "experimental_resource_declarations")]
    pub resource_declarations: HashMap<CompoundIdentifier, Resource>,
}
