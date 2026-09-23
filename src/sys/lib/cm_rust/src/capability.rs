// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use cm_rust_derive::FidlDecl;
use cm_types::{Name, Path, RelativePath};
use fidl_fuchsia_component_decl as fdecl;
use fidl_fuchsia_io as fio;
use from_enum::FromEnum;

use crate::{
    CapabilityTypeName, ConfigValue, FidlIntoNative, NativeIntoFidl, StorageDirectorySource,
};

#[cfg(fuchsia_api_level_at_least = "HEAD")]
use cm_types::DeliveryType;

#[cfg(feature = "serde")]
use crate::serde_ext;

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

#[cfg_attr(
    feature = "serde",
    derive(Deserialize, Serialize),
    serde(tag = "type", rename_all = "snake_case")
)]
#[derive(FidlDecl, FromEnum, Debug, Clone, PartialEq, Eq)]
#[fidl_decl(fidl_union = "fdecl::Capability")]
pub enum CapabilityDecl {
    Service(ServiceDecl),
    Protocol(ProtocolDecl),
    Directory(DirectoryDecl),
    Storage(StorageDecl),
    Runner(RunnerDecl),
    Resolver(ResolverDecl),
    EventStream(EventStreamDecl),
    Dictionary(DictionaryDecl),
    Config(ConfigurationDecl),
}

#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
#[derive(FidlDecl, Debug, Clone, PartialEq, Eq)]
#[fidl_decl(fidl_table = "fdecl::Service")]
pub struct ServiceDecl {
    pub name: Name,
    pub source_path: Option<Path>,
}

#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
#[derive(FidlDecl, Debug, Clone, PartialEq, Eq)]
#[fidl_decl(fidl_table = "fdecl::Protocol")]
pub struct ProtocolDecl {
    pub name: Name,
    pub source_path: Option<Path>,
    #[fidl_decl(default)]
    #[cfg(fuchsia_api_level_at_least = "HEAD")]
    pub delivery: DeliveryType,
}

#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
#[derive(FidlDecl, Debug, Clone, PartialEq, Eq)]
#[fidl_decl(fidl_table = "fdecl::Directory")]
pub struct DirectoryDecl {
    pub name: Name,
    pub source_path: Option<Path>,

    #[cfg_attr(
        feature = "serde",
        serde(
            deserialize_with = "serde_ext::deserialize_fio_operations",
            serialize_with = "serde_ext::serialize_fio_operations"
        )
    )]
    pub rights: fio::Operations,
}

#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
#[derive(FidlDecl, Debug, Clone, PartialEq, Eq)]
#[fidl_decl(fidl_table = "fdecl::Storage")]
pub struct StorageDecl {
    pub name: Name,
    pub source: StorageDirectorySource,
    pub backing_dir: Name,
    #[fidl_decl(default_preserve_none)]
    pub subdir: RelativePath,
    #[cfg_attr(feature = "serde", serde(with = "serde_ext::StorageId"))]
    pub storage_id: fdecl::StorageId,
}

#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
#[derive(FidlDecl, Debug, Clone, PartialEq, Eq)]
#[fidl_decl(fidl_table = "fdecl::Runner")]
pub struct RunnerDecl {
    pub name: Name,
    pub source_path: Option<Path>,
}

#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
#[derive(FidlDecl, Debug, Clone, PartialEq, Eq)]
#[fidl_decl(fidl_table = "fdecl::Resolver")]
pub struct ResolverDecl {
    pub name: Name,
    pub source_path: Option<Path>,
}

#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
#[derive(FidlDecl, Debug, Clone, PartialEq, Eq)]
#[fidl_decl(fidl_table = "fdecl::EventStream")]
pub struct EventStreamDecl {
    pub name: Name,
}

#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
#[derive(FidlDecl, Debug, Clone, PartialEq, Eq)]
#[fidl_decl(fidl_table = "fdecl::Dictionary")]
pub struct DictionaryDecl {
    pub name: Name,
    pub source_path: Option<Path>,
}

#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
#[derive(FidlDecl, Debug, Clone, PartialEq, Eq)]
#[fidl_decl(fidl_table = "fdecl::Configuration")]
pub struct ConfigurationDecl {
    pub name: Name,
    pub value: ConfigValue,
}

impl CapabilityDecl {
    pub fn name(&self) -> &Name {
        match self {
            CapabilityDecl::Directory(decl) => &decl.name,
            CapabilityDecl::Protocol(decl) => &decl.name,
            CapabilityDecl::Resolver(decl) => &decl.name,
            CapabilityDecl::Runner(decl) => &decl.name,
            CapabilityDecl::Service(decl) => &decl.name,
            CapabilityDecl::Storage(decl) => &decl.name,
            CapabilityDecl::EventStream(decl) => &decl.name,
            CapabilityDecl::Dictionary(decl) => &decl.name,
            CapabilityDecl::Config(decl) => &decl.name,
        }
    }

    pub fn path(&self) -> Option<&Path> {
        match self {
            CapabilityDecl::Directory(decl) => decl.source_path.as_ref(),
            CapabilityDecl::Protocol(decl) => decl.source_path.as_ref(),
            CapabilityDecl::Resolver(decl) => decl.source_path.as_ref(),
            CapabilityDecl::Runner(decl) => decl.source_path.as_ref(),
            CapabilityDecl::Service(decl) => decl.source_path.as_ref(),
            CapabilityDecl::Storage(_) => None,
            CapabilityDecl::EventStream(_) => None,
            CapabilityDecl::Dictionary(_) => None,
            CapabilityDecl::Config(_) => None,
        }
    }
}

impl From<&CapabilityDecl> for CapabilityTypeName {
    fn from(capability: &CapabilityDecl) -> Self {
        match capability {
            CapabilityDecl::Service(_) => Self::Service,
            CapabilityDecl::Protocol(_) => Self::Protocol,
            CapabilityDecl::Directory(_) => Self::Directory,
            CapabilityDecl::Storage(_) => Self::Storage,
            CapabilityDecl::Runner(_) => Self::Runner,
            CapabilityDecl::Resolver(_) => Self::Resolver,
            CapabilityDecl::EventStream(_) => Self::EventStream,
            CapabilityDecl::Dictionary(_) => Self::Dictionary,
            CapabilityDecl::Config(_) => Self::Config,
        }
    }
}
