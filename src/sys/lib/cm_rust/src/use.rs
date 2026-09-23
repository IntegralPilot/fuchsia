// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use cm_rust_derive::{FidlDecl, UseDeclCommon};
use cm_types::{BorrowedSeparatedPath, Name, Path, RelativePath};
use fidl_fuchsia_component_decl as fdecl;
use fidl_fuchsia_io as fio;
use from_enum::FromEnum;
use std::collections::BTreeMap;
use std::fmt;
use std::hash::Hash;
use std::sync::LazyLock;

use crate::{
    Availability, ConfigValue, ConfigValueType, DependencyType, DictionaryValue, EventScope,
    FidlIntoNative, NativeIntoFidl, SourceName, SourcePath,
};

#[cfg(fuchsia_api_level_at_least = "29")]
pub use cm_types::HandleType;

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
#[fidl_decl(fidl_union = "fdecl::Use")]
pub enum UseDecl {
    Service(UseServiceDecl),
    Protocol(UseProtocolDecl),
    Directory(UseDirectoryDecl),
    Storage(UseStorageDecl),
    EventStream(Box<UseEventStreamDecl>),
    #[cfg(fuchsia_api_level_at_least = "HEAD")]
    Runner(UseRunnerDecl),
    Config(Box<UseConfigurationDecl>),
    #[cfg(fuchsia_api_level_at_least = "29")]
    Dictionary(UseDictionaryDecl),
}

#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
#[derive(FidlDecl, UseDeclCommon, Debug, Clone, PartialEq, Eq)]
#[fidl_decl(fidl_table = "fdecl::UseService", source_path = "dictionary")]
pub struct UseServiceDecl {
    pub source: UseSource,
    pub source_name: Name,
    #[fidl_decl(default_preserve_none)]
    pub source_dictionary: RelativePath,
    pub target_path: Path,
    pub dependency_type: DependencyType,
    #[fidl_decl(default)]
    pub availability: Availability,
}

#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
#[derive(FidlDecl, UseDeclCommon, Debug, Clone, PartialEq, Eq)]
#[fidl_decl(fidl_table = "fdecl::UseProtocol", source_path = "dictionary")]
pub struct UseProtocolDecl {
    pub source: UseSource,
    pub source_name: Name,
    #[fidl_decl(default_preserve_none)]
    pub source_dictionary: RelativePath,
    pub target_path: Option<Path>,
    #[cfg(fuchsia_api_level_at_least = "29")]
    pub numbered_handle: Option<HandleType>,
    pub dependency_type: DependencyType,
    #[fidl_decl(default)]
    pub availability: Availability,
}

#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
#[derive(FidlDecl, UseDeclCommon, Debug, Clone, PartialEq, Eq)]
#[fidl_decl(fidl_table = "fdecl::UseDirectory", source_path = "dictionary")]
pub struct UseDirectoryDecl {
    pub source: UseSource,
    pub source_name: Name,
    #[fidl_decl(default_preserve_none)]
    pub source_dictionary: RelativePath,
    pub target_path: Path,

    #[cfg_attr(
        feature = "serde",
        serde(
            deserialize_with = "serde_ext::deserialize_fio_operations",
            serialize_with = "serde_ext::serialize_fio_operations"
        )
    )]
    pub rights: fio::Operations,

    #[fidl_decl(default_preserve_none)]
    pub subdir: RelativePath,
    pub dependency_type: DependencyType,
    #[fidl_decl(default)]
    pub availability: Availability,
}

#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
#[derive(FidlDecl, Debug, Clone, PartialEq, Eq)]
#[fidl_decl(fidl_table = "fdecl::UseStorage", source_path = "name_only")]
pub struct UseStorageDecl {
    pub source_name: Name,
    pub target_path: Path,
    #[fidl_decl(default)]
    pub availability: Availability,
}

impl SourceName for UseStorageDecl {
    fn source_name(&self) -> &Name {
        &self.source_name
    }
}

impl UseDeclCommon for UseStorageDecl {
    fn source(&self) -> &UseSource {
        &UseSource::Parent
    }

    fn availability(&self) -> &Availability {
        &self.availability
    }
}

#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
#[derive(FidlDecl, UseDeclCommon, Debug, Clone, PartialEq, Eq, Hash)]
#[fidl_decl(fidl_table = "fdecl::UseEventStream", source_path = "name_only")]
pub struct UseEventStreamDecl {
    pub source_name: Name,
    pub source: UseSource,
    pub scope: Option<Box<[EventScope]>>,
    pub target_path: Path,
    pub filter: Option<BTreeMap<String, DictionaryValue>>,
    #[fidl_decl(default)]
    pub availability: Availability,
}

#[cfg(fuchsia_api_level_at_least = "HEAD")]
#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
#[derive(FidlDecl, Debug, Clone, PartialEq, Eq)]
#[fidl_decl(fidl_table = "fdecl::UseRunner", source_path = "dictionary")]
pub struct UseRunnerDecl {
    pub source: UseSource,
    pub source_name: Name,
    #[fidl_decl(default_preserve_none)]
    pub source_dictionary: RelativePath,
}

#[cfg(fuchsia_api_level_at_least = "29")]
#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
#[derive(FidlDecl, UseDeclCommon, Debug, Clone, PartialEq, Eq)]
#[fidl_decl(fidl_table = "fdecl::UseDictionary", source_path = "dictionary")]
pub struct UseDictionaryDecl {
    pub source: UseSource,
    pub source_name: Name,
    #[fidl_decl(default_preserve_none)]
    pub source_dictionary: RelativePath,
    pub target_path: Path,
    pub dependency_type: DependencyType,
    #[fidl_decl(default)]
    pub availability: Availability,
}

#[cfg(fuchsia_api_level_at_least = "HEAD")]
impl SourceName for UseRunnerDecl {
    fn source_name(&self) -> &Name {
        &self.source_name
    }
}

#[cfg(fuchsia_api_level_at_least = "HEAD")]
impl UseDeclCommon for UseRunnerDecl {
    fn source(&self) -> &UseSource {
        &self.source
    }

    fn availability(&self) -> &Availability {
        &Availability::Required
    }
}

#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
#[derive(FidlDecl, UseDeclCommon, Debug, Clone, PartialEq, Eq)]
#[fidl_decl(fidl_table = "fdecl::UseConfiguration", source_path = "dictionary")]
pub struct UseConfigurationDecl {
    pub source: UseSource,
    pub source_name: Name,
    #[fidl_decl(default_preserve_none)]
    pub source_dictionary: RelativePath,
    pub target_name: Name,
    #[fidl_decl(default)]
    pub availability: Availability,
    pub type_: ConfigValueType,
    pub default: Option<ConfigValue>,
}

impl UseDeclCommon for UseDecl {
    fn source(&self) -> &UseSource {
        match &self {
            UseDecl::Service(u) => u.source(),
            UseDecl::Protocol(u) => u.source(),
            UseDecl::Directory(u) => u.source(),
            UseDecl::Storage(u) => u.source(),
            UseDecl::EventStream(u) => u.source(),
            #[cfg(fuchsia_api_level_at_least = "HEAD")]
            UseDecl::Runner(u) => u.source(),
            UseDecl::Config(u) => u.source(),
            #[cfg(fuchsia_api_level_at_least = "29")]
            UseDecl::Dictionary(u) => u.source(),
        }
    }

    fn availability(&self) -> &Availability {
        match &self {
            UseDecl::Service(u) => u.availability(),
            UseDecl::Protocol(u) => u.availability(),
            UseDecl::Directory(u) => u.availability(),
            UseDecl::Storage(u) => u.availability(),
            UseDecl::EventStream(u) => u.availability(),
            #[cfg(fuchsia_api_level_at_least = "HEAD")]
            UseDecl::Runner(u) => u.availability(),
            UseDecl::Config(u) => u.availability(),
            #[cfg(fuchsia_api_level_at_least = "29")]
            UseDecl::Dictionary(u) => u.availability(),
        }
    }
}

/// The common properties of a [Use](fdecl::Use) declaration.
pub trait UseDeclCommon: SourceName + SourcePath + Send + Sync {
    fn source(&self) -> &UseSource;
    fn availability(&self) -> &Availability;
}

#[cfg_attr(feature = "serde", derive(Deserialize, Serialize), serde(rename_all = "snake_case"))]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum UseSource {
    Parent,
    Framework,
    Debug,
    Self_,
    Capability(Name),
    Child(Name),
    Collection(Name),
    #[cfg(fuchsia_api_level_at_least = "HEAD")]
    Environment,
}

impl std::fmt::Display for UseSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Framework => write!(f, "framework"),
            Self::Parent => write!(f, "parent"),
            Self::Debug => write!(f, "debug environment"),
            Self::Self_ => write!(f, "self"),
            Self::Capability(c) => write!(f, "capability `{}`", c),
            Self::Child(c) => write!(f, "child `#{}`", c),
            Self::Collection(c) => write!(f, "collection `#{}`", c),
            #[cfg(fuchsia_api_level_at_least = "HEAD")]
            Self::Environment => write!(f, "environment"),
        }
    }
}

impl FidlIntoNative<UseSource> for fdecl::Ref {
    fn fidl_into_native(self) -> UseSource {
        match self {
            fdecl::Ref::Parent(_) => UseSource::Parent,
            fdecl::Ref::Framework(_) => UseSource::Framework,
            fdecl::Ref::Debug(_) => UseSource::Debug,
            fdecl::Ref::Self_(_) => UseSource::Self_,
            // cm_fidl_validator should have already validated this
            fdecl::Ref::Capability(c) => UseSource::Capability(c.name.parse().unwrap()),
            fdecl::Ref::Child(c) => UseSource::Child(c.name.parse().unwrap()),
            fdecl::Ref::Collection(c) => UseSource::Collection(c.name.parse().unwrap()),
            #[cfg(fuchsia_api_level_at_least = "HEAD")]
            fdecl::Ref::Environment(_) => UseSource::Environment,
            _ => panic!("invalid UseSource variant"),
        }
    }
}

impl NativeIntoFidl<fdecl::Ref> for UseSource {
    fn native_into_fidl(self) -> fdecl::Ref {
        match self {
            UseSource::Parent => fdecl::Ref::Parent(fdecl::ParentRef {}),
            UseSource::Framework => fdecl::Ref::Framework(fdecl::FrameworkRef {}),
            UseSource::Debug => fdecl::Ref::Debug(fdecl::DebugRef {}),
            UseSource::Self_ => fdecl::Ref::Self_(fdecl::SelfRef {}),
            UseSource::Capability(name) => {
                fdecl::Ref::Capability(fdecl::CapabilityRef { name: name.to_string() })
            }
            UseSource::Child(name) => {
                fdecl::Ref::Child(fdecl::ChildRef { name: name.to_string(), collection: None })
            }
            UseSource::Collection(name) => {
                fdecl::Ref::Collection(fdecl::CollectionRef { name: name.to_string() })
            }
            #[cfg(fuchsia_api_level_at_least = "HEAD")]
            UseSource::Environment => fdecl::Ref::Environment(fdecl::EnvironmentRef {}),
        }
    }
}

#[cfg(fuchsia_api_level_at_least = "29")]
impl FidlIntoNative<HandleType> for u8 {
    fn fidl_into_native(self) -> HandleType {
        self.into()
    }
}

#[cfg(fuchsia_api_level_at_least = "29")]
impl NativeIntoFidl<u8> for HandleType {
    fn native_into_fidl(self) -> u8 {
        self.into()
    }
}
