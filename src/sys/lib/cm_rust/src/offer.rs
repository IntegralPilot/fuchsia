// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use cm_rust_derive::{FidlDecl, OfferDeclCommon, OfferDeclCommonNoAvailability};
use cm_types::{BorrowedSeparatedPath, Name, RelativePath};
use fidl_fuchsia_component_decl as fdecl;
use fidl_fuchsia_io as fio;
use from_enum::FromEnum;
use std::fmt;
use std::hash::Hash;
use std::sync::LazyLock;

use crate::{
    Availability, CapabilityTypeName, ChildRef, DependencyType, EventScope, FidlIntoNative,
    NameMapping, NativeIntoFidl, SourceName, SourcePath,
};

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
#[fidl_decl(fidl_union = "fdecl::Offer")]
pub enum OfferDecl {
    Service(Box<OfferServiceDecl>),
    Protocol(OfferProtocolDecl),
    Directory(Box<OfferDirectoryDecl>),
    Storage(OfferStorageDecl),
    Runner(OfferRunnerDecl),
    Resolver(OfferResolverDecl),
    EventStream(Box<OfferEventStreamDecl>),
    Dictionary(OfferDictionaryDecl),
    Config(OfferConfigurationDecl),
}

#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
#[derive(FidlDecl, OfferDeclCommon, Debug, Clone, PartialEq, Eq)]
#[fidl_decl(fidl_table = "fdecl::OfferEventStream", source_path = "name_only")]
pub struct OfferEventStreamDecl {
    pub source: OfferSource,
    pub scope: Option<Box<[EventScope]>>,
    pub source_name: Name,
    pub target: OfferTarget,
    pub target_name: Name,
    #[fidl_decl(default)]
    pub availability: Availability,
}

#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
#[derive(FidlDecl, OfferDeclCommon, Debug, Clone, PartialEq, Eq)]
#[fidl_decl(fidl_table = "fdecl::OfferService", source_path = "dictionary")]
pub struct OfferServiceDecl {
    pub source: OfferSource,
    pub source_name: Name,
    #[fidl_decl(default_preserve_none)]
    pub source_dictionary: RelativePath,
    pub target: OfferTarget,
    pub target_name: Name,
    pub source_instance_filter: Option<Box<[Name]>>,
    pub renamed_instances: Option<Box<[NameMapping]>>,
    #[fidl_decl(default)]
    pub availability: Availability,
    #[cfg(fuchsia_api_level_at_least = "HEAD")]
    #[fidl_decl(default)]
    pub dependency_type: DependencyType,
}

#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
#[derive(FidlDecl, OfferDeclCommon, Debug, Clone, PartialEq, Eq)]
#[fidl_decl(fidl_table = "fdecl::OfferProtocol", source_path = "dictionary")]
pub struct OfferProtocolDecl {
    pub source: OfferSource,
    pub source_name: Name,
    #[fidl_decl(default_preserve_none)]
    pub source_dictionary: RelativePath,
    pub target: OfferTarget,
    pub target_name: Name,
    pub dependency_type: DependencyType,
    #[fidl_decl(default)]
    pub availability: Availability,
}

#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
#[derive(FidlDecl, OfferDeclCommon, Debug, Clone, PartialEq, Eq)]
#[fidl_decl(fidl_table = "fdecl::OfferDirectory", source_path = "dictionary")]
pub struct OfferDirectoryDecl {
    pub source: OfferSource,
    pub source_name: Name,
    #[fidl_decl(default_preserve_none)]
    pub source_dictionary: RelativePath,
    pub target: OfferTarget,
    pub target_name: Name,
    pub dependency_type: DependencyType,

    #[cfg_attr(
        feature = "serde",
        serde(
            deserialize_with = "serde_ext::deserialize_opt_fio_operations",
            serialize_with = "serde_ext::serialize_opt_fio_operations"
        )
    )]
    pub rights: Option<fio::Operations>,

    #[fidl_decl(default_preserve_none)]
    pub subdir: RelativePath,
    #[fidl_decl(default)]
    pub availability: Availability,
}

#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
#[derive(FidlDecl, OfferDeclCommon, Debug, Clone, PartialEq, Eq)]
#[fidl_decl(fidl_table = "fdecl::OfferStorage", source_path = "name_only")]
pub struct OfferStorageDecl {
    pub source: OfferSource,
    pub source_name: Name,
    pub target: OfferTarget,
    pub target_name: Name,
    #[fidl_decl(default)]
    pub availability: Availability,
}

#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
#[derive(FidlDecl, Debug, Clone, PartialEq, Eq)]
#[fidl_decl(fidl_table = "fdecl::OfferRunner", source_path = "dictionary")]
pub struct OfferRunnerDecl {
    pub source: OfferSource,
    pub source_name: Name,
    #[fidl_decl(default_preserve_none)]
    pub source_dictionary: RelativePath,
    pub target: OfferTarget,
    pub target_name: Name,
}

#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
#[derive(FidlDecl, OfferDeclCommonNoAvailability, Debug, Clone, PartialEq, Eq)]
#[fidl_decl(fidl_table = "fdecl::OfferResolver", source_path = "dictionary")]
pub struct OfferResolverDecl {
    pub source: OfferSource,
    pub source_name: Name,
    #[fidl_decl(default_preserve_none)]
    pub source_dictionary: RelativePath,
    pub target: OfferTarget,
    pub target_name: Name,
}

#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
#[derive(FidlDecl, OfferDeclCommon, Debug, Clone, PartialEq, Eq)]
#[fidl_decl(fidl_table = "fdecl::OfferDictionary", source_path = "dictionary")]
pub struct OfferDictionaryDecl {
    pub source: OfferSource,
    pub source_name: Name,
    #[fidl_decl(default_preserve_none)]
    pub source_dictionary: RelativePath,
    pub target: OfferTarget,
    pub target_name: Name,
    pub dependency_type: DependencyType,
    #[fidl_decl(default)]
    pub availability: Availability,
}

#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
#[derive(FidlDecl, OfferDeclCommon, Debug, Clone, PartialEq, Eq)]
#[fidl_decl(fidl_table = "fdecl::OfferConfiguration", source_path = "dictionary")]
pub struct OfferConfigurationDecl {
    pub source: OfferSource,
    pub source_name: Name,
    #[fidl_decl(default_preserve_none)]
    pub source_dictionary: RelativePath,
    pub target: OfferTarget,
    pub target_name: Name,
    #[fidl_decl(default)]
    pub availability: Availability,
}

impl SourceName for OfferDecl {
    fn source_name(&self) -> &Name {
        match &self {
            OfferDecl::Service(o) => o.source_name(),
            OfferDecl::Protocol(o) => o.source_name(),
            OfferDecl::Directory(o) => o.source_name(),
            OfferDecl::Storage(o) => o.source_name(),
            OfferDecl::Runner(o) => o.source_name(),
            OfferDecl::Resolver(o) => o.source_name(),
            OfferDecl::EventStream(o) => o.source_name(),
            OfferDecl::Dictionary(o) => o.source_name(),
            OfferDecl::Config(o) => o.source_name(),
        }
    }
}

impl SourcePath for OfferDecl {
    fn source_path(&self) -> BorrowedSeparatedPath<'_> {
        match &self {
            OfferDecl::Service(o) => o.source_path(),
            OfferDecl::Protocol(o) => o.source_path(),
            OfferDecl::Directory(o) => o.source_path(),
            OfferDecl::Storage(o) => o.source_path(),
            OfferDecl::Runner(o) => o.source_path(),
            OfferDecl::Resolver(o) => o.source_path(),
            OfferDecl::EventStream(o) => o.source_path(),
            OfferDecl::Dictionary(o) => o.source_path(),
            OfferDecl::Config(o) => o.source_path(),
        }
    }
}

impl OfferDeclCommon for OfferDecl {
    fn target_name(&self) -> &Name {
        match &self {
            OfferDecl::Service(o) => o.target_name(),
            OfferDecl::Protocol(o) => o.target_name(),
            OfferDecl::Directory(o) => o.target_name(),
            OfferDecl::Storage(o) => o.target_name(),
            OfferDecl::Runner(o) => o.target_name(),
            OfferDecl::Resolver(o) => o.target_name(),
            OfferDecl::EventStream(o) => o.target_name(),
            OfferDecl::Dictionary(o) => o.target_name(),
            OfferDecl::Config(o) => o.target_name(),
        }
    }

    fn target(&self) -> &OfferTarget {
        match &self {
            OfferDecl::Service(o) => o.target(),
            OfferDecl::Protocol(o) => o.target(),
            OfferDecl::Directory(o) => o.target(),
            OfferDecl::Storage(o) => o.target(),
            OfferDecl::Runner(o) => o.target(),
            OfferDecl::Resolver(o) => o.target(),
            OfferDecl::EventStream(o) => o.target(),
            OfferDecl::Dictionary(o) => o.target(),
            OfferDecl::Config(o) => o.target(),
        }
    }

    fn source(&self) -> &OfferSource {
        match &self {
            OfferDecl::Service(o) => o.source(),
            OfferDecl::Protocol(o) => o.source(),
            OfferDecl::Directory(o) => o.source(),
            OfferDecl::Storage(o) => o.source(),
            OfferDecl::Runner(o) => o.source(),
            OfferDecl::Resolver(o) => o.source(),
            OfferDecl::EventStream(o) => o.source(),
            OfferDecl::Dictionary(o) => o.source(),
            OfferDecl::Config(o) => o.source(),
        }
    }

    fn availability(&self) -> &Availability {
        match &self {
            OfferDecl::Service(o) => o.availability(),
            OfferDecl::Protocol(o) => o.availability(),
            OfferDecl::Directory(o) => o.availability(),
            OfferDecl::Storage(o) => o.availability(),
            OfferDecl::Runner(o) => o.availability(),
            OfferDecl::Resolver(o) => o.availability(),
            OfferDecl::EventStream(o) => o.availability(),
            OfferDecl::Dictionary(o) => o.availability(),
            OfferDecl::Config(o) => o.availability(),
        }
    }
}

impl SourceName for OfferRunnerDecl {
    fn source_name(&self) -> &Name {
        &self.source_name
    }
}

impl OfferDeclCommon for OfferRunnerDecl {
    fn target_name(&self) -> &Name {
        &self.target_name
    }

    fn target(&self) -> &OfferTarget {
        &self.target
    }

    fn source(&self) -> &OfferSource {
        &self.source
    }

    fn availability(&self) -> &Availability {
        &Availability::Required
    }
}

/// The common properties of an [Offer](fdecl::Offer) declaration.
pub trait OfferDeclCommon: SourceName + SourcePath + fmt::Debug + Send + Sync {
    fn target_name(&self) -> &Name;
    fn target(&self) -> &OfferTarget;
    fn source(&self) -> &OfferSource;
    fn availability(&self) -> &Availability;
}

impl From<&OfferDecl> for CapabilityTypeName {
    fn from(offer_decl: &OfferDecl) -> Self {
        match offer_decl {
            OfferDecl::Service(_) => Self::Service,
            OfferDecl::Protocol(_) => Self::Protocol,
            OfferDecl::Directory(_) => Self::Directory,
            OfferDecl::Storage(_) => Self::Storage,
            OfferDecl::Runner(_) => Self::Runner,
            OfferDecl::Resolver(_) => Self::Resolver,
            OfferDecl::EventStream(_) => Self::EventStream,
            OfferDecl::Dictionary(_) => Self::Dictionary,
            OfferDecl::Config(_) => Self::Config,
        }
    }
}

#[cfg_attr(feature = "serde", derive(Deserialize, Serialize), serde(rename_all = "snake_case"))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OfferSource {
    Framework,
    Parent,
    Child(ChildRef),
    Collection(Name),
    Self_,
    Capability(Name),
    Void,
}

impl std::fmt::Display for OfferSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Framework => write!(f, "framework"),
            Self::Parent => write!(f, "parent"),
            Self::Child(c) => write!(f, "child `#{}`", c),
            Self::Collection(c) => write!(f, "collection `#{}`", c),
            Self::Self_ => write!(f, "self"),
            Self::Capability(c) => write!(f, "capability `{}`", c),
            Self::Void => write!(f, "void"),
        }
    }
}

impl FidlIntoNative<OfferSource> for fdecl::Ref {
    fn fidl_into_native(self) -> OfferSource {
        match self {
            fdecl::Ref::Parent(_) => OfferSource::Parent,
            fdecl::Ref::Self_(_) => OfferSource::Self_,
            fdecl::Ref::Child(c) => OfferSource::Child(c.fidl_into_native()),
            // cm_fidl_validator should have already validated this
            fdecl::Ref::Collection(c) => OfferSource::Collection(c.name.parse().unwrap()),
            fdecl::Ref::Framework(_) => OfferSource::Framework,
            // cm_fidl_validator should have already validated this
            fdecl::Ref::Capability(c) => OfferSource::Capability(c.name.parse().unwrap()),
            fdecl::Ref::VoidType(_) => OfferSource::Void,
            _ => panic!("invalid OfferSource variant"),
        }
    }
}

impl NativeIntoFidl<fdecl::Ref> for OfferSource {
    fn native_into_fidl(self) -> fdecl::Ref {
        match self {
            OfferSource::Parent => fdecl::Ref::Parent(fdecl::ParentRef {}),
            OfferSource::Self_ => fdecl::Ref::Self_(fdecl::SelfRef {}),
            OfferSource::Child(c) => fdecl::Ref::Child(c.native_into_fidl()),
            OfferSource::Collection(name) => {
                fdecl::Ref::Collection(fdecl::CollectionRef { name: name.native_into_fidl() })
            }
            OfferSource::Framework => fdecl::Ref::Framework(fdecl::FrameworkRef {}),
            OfferSource::Capability(name) => {
                fdecl::Ref::Capability(fdecl::CapabilityRef { name: name.to_string() })
            }
            OfferSource::Void => fdecl::Ref::VoidType(fdecl::VoidRef {}),
        }
    }
}

#[cfg_attr(feature = "serde", derive(Deserialize, Serialize), serde(rename_all = "snake_case"))]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum OfferTarget {
    Child(ChildRef),
    Collection(Name),
    Capability(Name),
}

impl std::fmt::Display for OfferTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Child(c) => write!(f, "child `#{}`", c),
            Self::Collection(c) => write!(f, "collection `#{}`", c),
            Self::Capability(c) => write!(f, "capability `#{}`", c),
        }
    }
}

impl FidlIntoNative<OfferTarget> for fdecl::Ref {
    fn fidl_into_native(self) -> OfferTarget {
        match self {
            fdecl::Ref::Child(c) => OfferTarget::Child(c.fidl_into_native()),
            // cm_fidl_validator should have already validated this
            fdecl::Ref::Collection(c) => OfferTarget::Collection(c.name.parse().unwrap()),
            fdecl::Ref::Capability(c) => OfferTarget::Capability(c.name.parse().unwrap()),
            _ => panic!("invalid OfferTarget variant"),
        }
    }
}

impl NativeIntoFidl<fdecl::Ref> for OfferTarget {
    fn native_into_fidl(self) -> fdecl::Ref {
        match self {
            OfferTarget::Child(c) => fdecl::Ref::Child(c.native_into_fidl()),
            OfferTarget::Collection(collection_name) => {
                fdecl::Ref::Collection(fdecl::CollectionRef {
                    name: collection_name.native_into_fidl(),
                })
            }
            OfferTarget::Capability(capability_name) => {
                fdecl::Ref::Capability(fdecl::CapabilityRef {
                    name: capability_name.native_into_fidl(),
                })
            }
        }
    }
}
