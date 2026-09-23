// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use cm_rust::{
    CapabilityDecl, CapabilityTypeName, ChildRef, ConfigurationDecl, DictionaryDecl, DirectoryDecl,
    EventStreamDecl, ExposeConfigurationDecl, ExposeDecl, ExposeDeclCommon, ExposeDictionaryDecl,
    ExposeDirectoryDecl, ExposeProtocolDecl, ExposeResolverDecl, ExposeRunnerDecl,
    ExposeServiceDecl, ExposeSource, OfferConfigurationDecl, OfferDecl, OfferDeclCommon,
    OfferDictionaryDecl, OfferProtocolDecl, OfferResolverDecl, OfferRunnerDecl, OfferServiceDecl,
    OfferSource, OfferStorageDecl, ProtocolDecl, RegistrationSource, ResolverDecl, RunnerDecl,
    ServiceDecl, StorageDecl, UseDecl, UseDeclCommon, UseDirectoryDecl, UseProtocolDecl,
    UseServiceDecl, UseSource, UseStorageDecl,
};
use cm_types::{Name, Path, RelativePath};
use fidl_fuchsia_sys2 as fsys;
use from_enum::FromEnum;
use moniker::{ChildName, ExtendedMoniker, Moniker};
use std::fmt;
use thiserror::Error;

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

#[derive(Debug, Error)]
pub enum Error {
    #[error("Invalid framework capability.")]
    InvalidFrameworkCapability {},
    #[error("Invalid builtin capability.")]
    InvalidBuiltinCapability {},
}

#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
#[derive(Debug, Hash, PartialEq, Eq, Clone)]
pub enum AggregateMember {
    Child(ChildRef),
    Collection(Name),
    Parent,
    Self_,
}

impl TryFrom<&OfferDecl> for AggregateMember {
    type Error = ();

    fn try_from(offer: &OfferDecl) -> Result<AggregateMember, ()> {
        match offer.source() {
            // TODO: should we panic instead of filtering when we find something we don't expect?
            OfferSource::Framework => Err(()),
            OfferSource::Parent => Ok(AggregateMember::Parent),
            OfferSource::Child(child) => Ok(AggregateMember::Child(child.clone())),
            OfferSource::Collection(name) => Ok(AggregateMember::Collection(name.clone())),
            OfferSource::Self_ => Ok(AggregateMember::Self_),
            OfferSource::Capability(_name) => Err(()),
            OfferSource::Void => Err(()),
        }
    }
}

impl TryFrom<&cm_rust::ExposeDecl> for AggregateMember {
    type Error = ();

    fn try_from(expose: &cm_rust::ExposeDecl) -> Result<AggregateMember, ()> {
        match expose.source() {
            // TODO: should we panic instead of filtering when we find something we don't expect?
            cm_rust::ExposeSource::Framework => Err(()),
            cm_rust::ExposeSource::Child(child) => Ok(AggregateMember::Child(cm_rust::ChildRef {
                name: child.clone().into(),
                collection: None,
            })),
            cm_rust::ExposeSource::Collection(name) => {
                Ok(AggregateMember::Collection(name.clone()))
            }
            cm_rust::ExposeSource::Self_ => Ok(AggregateMember::Self_),
            cm_rust::ExposeSource::Capability(_name) => Err(()),
            cm_rust::ExposeSource::Void => Err(()),
        }
    }
}

impl TryFrom<&cm_rust::UseDecl> for AggregateMember {
    type Error = ();

    fn try_from(use_: &cm_rust::UseDecl) -> Result<AggregateMember, ()> {
        match use_.source() {
            cm_rust::UseSource::Parent => Ok(AggregateMember::Parent),
            cm_rust::UseSource::Framework => Err(()),
            cm_rust::UseSource::Debug => Err(()),
            cm_rust::UseSource::Self_ => Ok(AggregateMember::Self_),
            cm_rust::UseSource::Capability(_) => Err(()),
            cm_rust::UseSource::Child(name) => {
                Ok(AggregateMember::Child(ChildRef { name: name.clone().into(), collection: None }))
            }
            cm_rust::UseSource::Collection(name) => Ok(AggregateMember::Collection(name.clone())),
            cm_rust::UseSource::Environment => Err(()),
        }
    }
}

impl fmt::Display for AggregateMember {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Child(n) => {
                write!(f, "child `{n}`")
            }
            Self::Collection(n) => {
                write!(f, "collection `{n}`")
            }
            Self::Parent => {
                write!(f, "parent")
            }
            Self::Self_ => {
                write!(f, "self")
            }
        }
    }
}

/// Describes the source of a capability, as determined by `find_capability_source`
#[cfg_attr(
    feature = "serde",
    derive(Deserialize, Serialize),
    serde(tag = "type", rename_all = "snake_case")
)]
#[derive(Debug, Clone, PartialEq)]
pub enum CapabilitySource {
    /// This capability originates from the component instance for the given Realm.
    /// point.
    Component(ComponentSource),
    /// This capability originates from "framework". It's implemented by component manager and is
    /// scoped to the realm of the source.
    Framework(FrameworkSource),
    /// This capability originates from the parent of the root component, and is built in to
    /// component manager. `top_instance` is the instance at the top of the tree, i.e.  the
    /// instance representing component manager.
    Builtin(BuiltinSource),
    /// This capability originates from the parent of the root component, and is offered from
    /// component manager's namespace. `top_instance` is the instance at the top of the tree, i.e.
    /// the instance representing component manager.
    Namespace(NamespaceSource),
    /// This capability is provided by the framework based on some other capability.
    Capability(CapabilityToCapabilitySource),
    /// This capability is an aggregate of capabilities over a set of collections and static
    /// children. The instance names in the aggregate service will be anonymized.
    AnonymizedAggregate(AnonymizedAggregateSource),
    /// This capability is a filtered service capability from a single source, such as self or a
    /// child.
    FilteredProvider(FilteredProviderSource),
    /// This capability is a filtered service capability with multiple sources, such as all of the
    /// dynamic children in a collection. The instances in the aggregate service are the union of
    /// the filters.
    FilteredAggregateProvider(FilteredAggregateProviderSource),
    /// This capability originates from "environment". It's implemented by a component instance.
    Environment(EnvironmentSource),
    /// This capability originates from "void". This is only a valid origination for optional
    /// capabilities.
    Void(VoidSource),
    /// The route for this capability extended outside of component manager at the given moniker,
    /// and we thus don't know where the ultimate terminus of it is.
    RemotedAt(RemotedAtSource),
    /// This capability is a storage capability backed by a directory capability
    /// originating in some component.
    StorageBackingDirectory(StorageBackingDirectorySource),
}

#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
#[derive(Debug, PartialEq, Clone)]
pub struct ComponentSource {
    pub capability: ComponentCapability,
    pub moniker: Moniker,
}
#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
#[derive(Debug, PartialEq, Clone)]
pub struct FrameworkSource {
    pub capability: InternalCapability,
    pub moniker: Moniker,
}
#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
#[derive(Debug, PartialEq, Clone)]
pub struct BuiltinSource {
    pub capability: InternalCapability,
}
#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
#[derive(Debug, PartialEq, Clone)]
pub struct NamespaceSource {
    pub capability: ComponentCapability,
}
#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
#[derive(Debug, PartialEq, Clone)]
pub struct CapabilityToCapabilitySource {
    pub source_capability: ComponentCapability,
    pub moniker: Moniker,
}
#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
#[derive(Debug, PartialEq, Clone)]
pub struct AnonymizedAggregateSource {
    pub capability: AggregateCapability,
    pub moniker: Moniker,
    pub members: Vec<AggregateMember>,
    pub instances: Vec<ServiceInstance>,
}
#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
#[derive(Debug, PartialEq, Clone)]
pub struct ServiceInstance {
    pub instance_name: Name,
    pub child_name: String,
    pub child_instance_name: Name,
}

impl From<ServiceInstance> for fsys::ServiceInstance {
    fn from(service_instance: ServiceInstance) -> Self {
        Self {
            instance_name: Some(service_instance.instance_name.to_string()),
            child_name: Some(service_instance.child_name),
            child_instance_name: Some(service_instance.child_instance_name.to_string()),
            ..Default::default()
        }
    }
}

#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
#[derive(Debug, PartialEq, Clone)]
pub struct FilteredProviderSource {
    pub capability: AggregateCapability,
    pub moniker: Moniker,
    pub service_capability: ComponentCapability,
    pub offer_service_decl: OfferServiceDecl,
}
#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
#[derive(Debug, PartialEq, Clone)]
pub struct FilteredAggregateProviderSource {
    pub capability: AggregateCapability,
    pub moniker: Moniker,
    pub offer_service_decls: Vec<OfferServiceDecl>,
}
#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
#[derive(Debug, PartialEq, Clone)]
pub struct EnvironmentSource {
    pub capability: ComponentCapability,
    pub moniker: Moniker,
}
#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
#[derive(Debug, PartialEq, Clone)]
pub struct VoidSource {
    pub capability: InternalCapability,
    pub moniker: Moniker,
}
#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
#[derive(Debug, PartialEq, Clone)]
pub struct RemotedAtSource {
    pub moniker: Moniker,
    pub type_name: Option<CapabilityTypeName>,
}

#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
#[derive(Debug, PartialEq, Clone)]
pub struct StorageBackingDirectorySource {
    pub capability: ComponentCapability,
    pub moniker: Moniker,
    pub backing_dir_subdir: RelativePath,
    pub storage_subdir: RelativePath,
    pub storage_source_moniker: Moniker,
}

impl CapabilitySource {
    pub fn source_name(&self) -> Option<&Name> {
        match self {
            Self::Component(ComponentSource { capability, .. }) => capability.source_name(),
            Self::Framework(FrameworkSource { capability, .. }) => Some(capability.source_name()),
            Self::Builtin(BuiltinSource { capability }) => Some(capability.source_name()),
            Self::Namespace(NamespaceSource { capability }) => capability.source_name(),
            Self::Capability(CapabilityToCapabilitySource { .. }) => None,
            Self::AnonymizedAggregate(AnonymizedAggregateSource { capability, .. }) => {
                Some(capability.source_name())
            }
            Self::FilteredProvider(FilteredProviderSource { capability, .. })
            | Self::FilteredAggregateProvider(FilteredAggregateProviderSource {
                capability, ..
            }) => Some(capability.source_name()),
            Self::Environment(EnvironmentSource { capability, .. }) => capability.source_name(),
            Self::Void(VoidSource { capability, .. }) => Some(capability.source_name()),
            Self::RemotedAt(_) => None,
            Self::StorageBackingDirectory(StorageBackingDirectorySource { capability, .. }) => {
                capability.source_name()
            }
        }
    }

    /// Returns the type name of the component. This will be `Some` in all cases except for where a
    /// capability route exclusively involves routers created outside of component manager, in
    /// which case this value is not knowable.
    pub fn type_name(&self) -> Option<CapabilityTypeName> {
        match self {
            Self::Component(ComponentSource { capability, .. }) => Some(capability.type_name()),
            Self::Framework(FrameworkSource { capability, .. }) => Some(capability.type_name()),
            Self::Builtin(BuiltinSource { capability }) => Some(capability.type_name()),
            Self::Namespace(NamespaceSource { capability }) => Some(capability.type_name()),
            Self::Capability(CapabilityToCapabilitySource { source_capability, .. }) => {
                Some(source_capability.type_name())
            }
            Self::AnonymizedAggregate(AnonymizedAggregateSource { capability, .. }) => {
                Some(capability.type_name())
            }
            Self::FilteredProvider(FilteredProviderSource { capability, .. })
            | Self::FilteredAggregateProvider(FilteredAggregateProviderSource {
                capability, ..
            }) => Some(capability.type_name()),
            Self::Environment(EnvironmentSource { capability, .. }) => Some(capability.type_name()),
            Self::Void(VoidSource { capability, .. }) => Some(capability.type_name()),
            Self::RemotedAt(RemotedAtSource { type_name, .. }) => type_name.clone(),
            Self::StorageBackingDirectory(StorageBackingDirectorySource { capability, .. }) => {
                Some(capability.type_name())
            }
        }
    }

    pub fn source_moniker(&self) -> ExtendedMoniker {
        match self {
            Self::Component(ComponentSource { moniker, .. })
            | Self::Framework(FrameworkSource { moniker, .. })
            | Self::Capability(CapabilityToCapabilitySource { moniker, .. })
            | Self::Environment(EnvironmentSource { moniker, .. })
            | Self::Void(VoidSource { moniker, .. })
            | Self::StorageBackingDirectory(StorageBackingDirectorySource { moniker, .. })
            | Self::AnonymizedAggregate(AnonymizedAggregateSource { moniker, .. })
            | Self::FilteredProvider(FilteredProviderSource { moniker, .. })
            | Self::FilteredAggregateProvider(FilteredAggregateProviderSource {
                moniker, ..
            })
            | Self::RemotedAt(RemotedAtSource { moniker, .. }) => {
                ExtendedMoniker::ComponentInstance(moniker.clone())
            }
            Self::Builtin(_) | Self::Namespace(_) => ExtendedMoniker::ComponentManager,
        }
    }
}

impl fmt::Display for CapabilitySource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Self::Component(ComponentSource { capability, moniker })
                | Self::StorageBackingDirectory(StorageBackingDirectorySource {
                    capability,
                    moniker,
                    ..
                }) => {
                    format!("{} '{}'", capability, moniker)
                }
                Self::Framework(FrameworkSource { capability, .. }) => capability.to_string(),
                Self::Builtin(BuiltinSource { capability }) => capability.to_string(),
                Self::Namespace(NamespaceSource { capability }) => capability.to_string(),
                Self::FilteredProvider(FilteredProviderSource { capability, .. })
                | Self::FilteredAggregateProvider(FilteredAggregateProviderSource {
                    capability,
                    ..
                }) => capability.to_string(),
                Self::Capability(CapabilityToCapabilitySource { source_capability, .. }) =>
                    format!("{}", source_capability),
                Self::AnonymizedAggregate(AnonymizedAggregateSource {
                    capability,
                    members,
                    moniker,
                    ..
                }) => {
                    format!(
                        "{} from component '{}' aggregated from {}",
                        capability,
                        moniker,
                        members.iter().map(|s| format!("{s}")).collect::<Vec<_>>().join(","),
                    )
                }
                Self::Environment(EnvironmentSource { capability, .. }) => capability.to_string(),
                Self::Void(VoidSource { capability, .. }) => capability.to_string(),
                Self::RemotedAt(RemotedAtSource { moniker, type_name }) => format!(
                    "{} route left component manager at {}",
                    type_name.map(|t| t.to_string()).unwrap_or_else(|| "unknown".to_string()),
                    moniker
                ),
            }
        )
    }
}

/// An individual instance in an aggregate.
#[derive(Debug, Clone, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub enum AggregateInstance {
    Child(ChildName),
    Parent,
    Self_,
}

impl fmt::Display for AggregateInstance {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Child(n) => {
                write!(f, "child `{n}`")
            }
            Self::Parent => {
                write!(f, "parent")
            }
            Self::Self_ => {
                write!(f, "self")
            }
        }
    }
}

/// Describes a capability provided by the component manager which could be a framework capability
/// scoped to a realm, a built-in global capability, or a capability from component manager's own
/// namespace.
#[cfg_attr(feature = "serde", derive(Deserialize, Serialize), serde(rename_all = "snake_case"))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InternalCapability {
    Service(Name),
    Protocol(Name),
    Directory(Name),
    Runner(Name),
    Config(Name),
    EventStream(InternalEventStreamCapability),
    Resolver(Name),
    Storage(Name),
    Dictionary(Name),
}

#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
#[derive(Debug, PartialEq, Clone, Eq)]
pub struct InternalEventStreamCapability {
    pub name: Name,
    pub scope_moniker: Option<String>,
    pub scope: Option<Box<[cm_rust::EventScope]>>,
}

impl InternalCapability {
    pub fn new(type_name: CapabilityTypeName, name: Name) -> Self {
        match type_name {
            CapabilityTypeName::Directory => InternalCapability::Directory(name),
            CapabilityTypeName::EventStream => InternalCapability::Directory(name),
            CapabilityTypeName::Protocol => InternalCapability::Protocol(name),
            CapabilityTypeName::Resolver => InternalCapability::Resolver(name),
            CapabilityTypeName::Runner => InternalCapability::Runner(name),
            CapabilityTypeName::Service => InternalCapability::Service(name),
            CapabilityTypeName::Storage => InternalCapability::Storage(name),
            CapabilityTypeName::Dictionary => InternalCapability::Dictionary(name),
            CapabilityTypeName::Config => InternalCapability::Config(name),
        }
    }

    /// Returns a name for the capability type.
    pub fn type_name(&self) -> CapabilityTypeName {
        match self {
            InternalCapability::Service(_) => CapabilityTypeName::Service,
            InternalCapability::Protocol(_) => CapabilityTypeName::Protocol,
            InternalCapability::Directory(_) => CapabilityTypeName::Directory,
            InternalCapability::Runner(_) => CapabilityTypeName::Runner,
            InternalCapability::Config(_) => CapabilityTypeName::Config,
            InternalCapability::EventStream(_) => CapabilityTypeName::EventStream,
            InternalCapability::Resolver(_) => CapabilityTypeName::Resolver,
            InternalCapability::Storage(_) => CapabilityTypeName::Storage,
            InternalCapability::Dictionary(_) => CapabilityTypeName::Dictionary,
        }
    }

    pub fn source_name(&self) -> &Name {
        match self {
            InternalCapability::Service(name) => &name,
            InternalCapability::Protocol(name) => &name,
            InternalCapability::Directory(name) => &name,
            InternalCapability::Runner(name) => &name,
            InternalCapability::Config(name) => &name,
            InternalCapability::EventStream(InternalEventStreamCapability { name, .. }) => &name,
            InternalCapability::Resolver(name) => &name,
            InternalCapability::Storage(name) => &name,
            InternalCapability::Dictionary(name) => &name,
        }
    }

    /// Returns true if this is a protocol with name that matches `name`.
    pub fn matches_protocol(&self, name: &Name) -> bool {
        match self {
            Self::Protocol(source_name) => source_name == name,
            _ => false,
        }
    }
}

impl fmt::Display for InternalCapability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} '{}' from component manager", self.type_name(), self.source_name())
    }
}

impl From<CapabilityDecl> for InternalCapability {
    fn from(capability: CapabilityDecl) -> Self {
        match capability {
            CapabilityDecl::Service(c) => c.into(),
            CapabilityDecl::Protocol(c) => c.into(),
            CapabilityDecl::Directory(c) => c.into(),
            CapabilityDecl::Storage(c) => c.into(),
            CapabilityDecl::Runner(c) => c.into(),
            CapabilityDecl::Resolver(c) => c.into(),
            CapabilityDecl::EventStream(c) => c.into(),
            CapabilityDecl::Dictionary(c) => c.into(),
            CapabilityDecl::Config(c) => c.into(),
        }
    }
}

impl From<ServiceDecl> for InternalCapability {
    fn from(service: ServiceDecl) -> Self {
        Self::Service(service.name)
    }
}

impl From<ProtocolDecl> for InternalCapability {
    fn from(protocol: ProtocolDecl) -> Self {
        Self::Protocol(protocol.name)
    }
}

impl From<DirectoryDecl> for InternalCapability {
    fn from(directory: DirectoryDecl) -> Self {
        Self::Directory(directory.name)
    }
}

impl From<RunnerDecl> for InternalCapability {
    fn from(runner: RunnerDecl) -> Self {
        Self::Runner(runner.name)
    }
}

impl From<ResolverDecl> for InternalCapability {
    fn from(resolver: ResolverDecl) -> Self {
        Self::Resolver(resolver.name)
    }
}

impl From<EventStreamDecl> for InternalCapability {
    fn from(event: EventStreamDecl) -> Self {
        Self::EventStream(InternalEventStreamCapability {
            name: event.name,
            scope_moniker: None,
            scope: None,
        })
    }
}

impl From<StorageDecl> for InternalCapability {
    fn from(storage: StorageDecl) -> Self {
        Self::Storage(storage.name)
    }
}

impl From<ConfigurationDecl> for InternalCapability {
    fn from(config: ConfigurationDecl) -> Self {
        Self::Config(config.name)
    }
}

impl From<DictionaryDecl> for InternalCapability {
    fn from(dictionary: DictionaryDecl) -> Self {
        Self::Dictionary(dictionary.name)
    }
}

/// A capability being routed from a component.
#[cfg_attr(
    feature = "serde",
    derive(Deserialize, Serialize),
    serde(tag = "type", rename_all = "snake_case")
)]
#[derive(FromEnum, Clone, Debug, PartialEq, Eq)]
pub enum ComponentCapability {
    Use_(UseDecl),
    /// Models a capability used from the environment.
    Environment(EnvironmentCapability),
    Expose(ExposeDecl),
    Offer(OfferDecl),
    Protocol(ProtocolDecl),
    Directory(DirectoryDecl),
    Storage(StorageDecl),
    Runner(RunnerDecl),
    Resolver(ResolverDecl),
    Service(ServiceDecl),
    EventStream(EventStreamDecl),
    Dictionary(DictionaryDecl),
    Config(ConfigurationDecl),
}

impl ComponentCapability {
    /// Returns a name for the capability type.
    pub fn type_name(&self) -> CapabilityTypeName {
        match self {
            ComponentCapability::Use_(use_) => use_.into(),
            ComponentCapability::Environment(env) => match env {
                EnvironmentCapability::Runner(_) => CapabilityTypeName::Runner,
                EnvironmentCapability::Resolver(_) => CapabilityTypeName::Resolver,
                EnvironmentCapability::Debug(_) => CapabilityTypeName::Protocol,
            },
            ComponentCapability::Expose(expose) => expose.into(),
            ComponentCapability::Offer(offer) => offer.into(),
            ComponentCapability::Protocol(_) => CapabilityTypeName::Protocol,
            ComponentCapability::Directory(_) => CapabilityTypeName::Directory,
            ComponentCapability::Storage(_) => CapabilityTypeName::Storage,
            ComponentCapability::Runner(_) => CapabilityTypeName::Runner,
            ComponentCapability::Config(_) => CapabilityTypeName::Config,
            ComponentCapability::Resolver(_) => CapabilityTypeName::Resolver,
            ComponentCapability::Service(_) => CapabilityTypeName::Service,
            ComponentCapability::EventStream(_) => CapabilityTypeName::EventStream,
            ComponentCapability::Dictionary(_) => CapabilityTypeName::Dictionary,
        }
    }

    /// Return the source path of the capability, if one exists.
    pub fn source_path(&self) -> Option<&Path> {
        match self {
            ComponentCapability::Storage(_) => None,
            ComponentCapability::Protocol(protocol) => protocol.source_path.as_ref(),
            ComponentCapability::Directory(directory) => directory.source_path.as_ref(),
            ComponentCapability::Runner(runner) => runner.source_path.as_ref(),
            ComponentCapability::Resolver(resolver) => resolver.source_path.as_ref(),
            ComponentCapability::Service(service) => service.source_path.as_ref(),
            _ => None,
        }
    }

    /// Return the name of the capability, if this is a capability declaration.
    pub fn source_name(&self) -> Option<&Name> {
        match self {
            ComponentCapability::Storage(storage) => Some(&storage.name),
            ComponentCapability::Protocol(protocol) => Some(&protocol.name),
            ComponentCapability::Directory(directory) => Some(&directory.name),
            ComponentCapability::Runner(runner) => Some(&runner.name),
            ComponentCapability::Config(config) => Some(&config.name),
            ComponentCapability::Resolver(resolver) => Some(&resolver.name),
            ComponentCapability::Service(service) => Some(&service.name),
            ComponentCapability::EventStream(event) => Some(&event.name),
            ComponentCapability::Dictionary(dictionary) => Some(&dictionary.name),
            ComponentCapability::Use_(use_) => match use_ {
                UseDecl::Protocol(UseProtocolDecl { source_name, .. }) => Some(source_name),
                UseDecl::Directory(UseDirectoryDecl { source_name, .. }) => Some(source_name),
                UseDecl::Storage(UseStorageDecl { source_name, .. }) => Some(source_name),
                UseDecl::Service(UseServiceDecl { source_name, .. }) => Some(source_name),
                UseDecl::Config(config) => Some(&config.source_name),
                _ => None,
            },
            ComponentCapability::Environment(env_cap) => match env_cap {
                EnvironmentCapability::Runner(EnvironmentCapabilityData {
                    source_name, ..
                }) => Some(source_name),
                EnvironmentCapability::Resolver(EnvironmentCapabilityData {
                    source_name, ..
                }) => Some(source_name),
                EnvironmentCapability::Debug(EnvironmentCapabilityData { source_name, .. }) => {
                    Some(source_name)
                }
            },
            ComponentCapability::Expose(expose) => match expose {
                ExposeDecl::Protocol(ExposeProtocolDecl { source_name, .. }) => Some(source_name),
                ExposeDecl::Directory(ExposeDirectoryDecl { source_name, .. }) => Some(source_name),
                ExposeDecl::Runner(ExposeRunnerDecl { source_name, .. }) => Some(source_name),
                ExposeDecl::Resolver(ExposeResolverDecl { source_name, .. }) => Some(source_name),
                ExposeDecl::Service(ExposeServiceDecl { source_name, .. }) => Some(source_name),
                ExposeDecl::Config(ExposeConfigurationDecl { source_name, .. }) => {
                    Some(source_name)
                }
                ExposeDecl::Dictionary(ExposeDictionaryDecl { source_name, .. }) => {
                    Some(source_name)
                }
            },
            ComponentCapability::Offer(offer) => match offer {
                OfferDecl::Protocol(OfferProtocolDecl { source_name, .. }) => Some(source_name),
                OfferDecl::Directory(directory) => Some(&directory.source_name),
                OfferDecl::Runner(OfferRunnerDecl { source_name, .. }) => Some(source_name),
                OfferDecl::Storage(OfferStorageDecl { source_name, .. }) => Some(source_name),
                OfferDecl::Resolver(OfferResolverDecl { source_name, .. }) => Some(source_name),
                OfferDecl::Service(service) => Some(&service.source_name),
                OfferDecl::Config(OfferConfigurationDecl { source_name, .. }) => Some(source_name),
                OfferDecl::EventStream(event_stream) => Some(&event_stream.source_name),
                OfferDecl::Dictionary(OfferDictionaryDecl { source_name, .. }) => Some(source_name),
            },
        }
    }

    pub fn source_capability_name(&self) -> Option<&Name> {
        match self {
            ComponentCapability::Offer(OfferDecl::Protocol(OfferProtocolDecl {
                source: OfferSource::Capability(name),
                ..
            })) => Some(name),
            ComponentCapability::Expose(ExposeDecl::Protocol(ExposeProtocolDecl {
                source: ExposeSource::Capability(name),
                ..
            })) => Some(name),
            ComponentCapability::Use_(UseDecl::Protocol(UseProtocolDecl {
                source: UseSource::Capability(name),
                ..
            })) => Some(name),
            _ => None,
        }
    }

    /// Returns the path or name of the capability as a string, useful for debugging.
    pub fn source_id(&self) -> String {
        self.source_name()
            .map(|p| format!("{}", p))
            .or_else(|| self.source_path().map(|p| format!("{}", p)))
            .unwrap_or_default()
    }
}

impl From<CapabilityDecl> for ComponentCapability {
    fn from(capability: CapabilityDecl) -> Self {
        match capability {
            CapabilityDecl::Service(c) => ComponentCapability::Service(c),
            CapabilityDecl::Protocol(c) => ComponentCapability::Protocol(c),
            CapabilityDecl::Directory(c) => ComponentCapability::Directory(c),
            CapabilityDecl::Storage(c) => ComponentCapability::Storage(c),
            CapabilityDecl::Runner(c) => ComponentCapability::Runner(c),
            CapabilityDecl::Resolver(c) => ComponentCapability::Resolver(c),
            CapabilityDecl::EventStream(c) => ComponentCapability::EventStream(c),
            CapabilityDecl::Dictionary(c) => ComponentCapability::Dictionary(c),
            CapabilityDecl::Config(c) => ComponentCapability::Config(c),
        }
    }
}

impl TryInto<CapabilityDecl> for ComponentCapability {
    type Error = ();

    fn try_into(self) -> Result<CapabilityDecl, Self::Error> {
        match self {
            Self::Service(c) => Ok(CapabilityDecl::Service(c)),
            Self::Protocol(c) => Ok(CapabilityDecl::Protocol(c)),
            Self::Directory(c) => Ok(CapabilityDecl::Directory(c)),
            Self::Storage(c) => Ok(CapabilityDecl::Storage(c)),
            Self::Runner(c) => Ok(CapabilityDecl::Runner(c)),
            Self::Resolver(c) => Ok(CapabilityDecl::Resolver(c)),
            Self::EventStream(c) => Ok(CapabilityDecl::EventStream(c)),
            Self::Dictionary(c) => Ok(CapabilityDecl::Dictionary(c)),
            Self::Config(c) => Ok(CapabilityDecl::Config(c)),
            _ => Err(()),
        }
    }
}

impl fmt::Display for ComponentCapability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} '{}' from component", self.type_name(), self.source_id())
    }
}

#[cfg_attr(
    feature = "serde",
    derive(Deserialize, Serialize),
    serde(tag = "type", rename_all = "snake_case")
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EnvironmentCapability {
    Runner(EnvironmentCapabilityData),
    Resolver(EnvironmentCapabilityData),
    Debug(EnvironmentCapabilityData),
}

impl EnvironmentCapability {
    pub fn registration_source(&self) -> &RegistrationSource {
        match self {
            Self::Runner(EnvironmentCapabilityData { source, .. })
            | Self::Resolver(EnvironmentCapabilityData { source, .. })
            | Self::Debug(EnvironmentCapabilityData { source, .. }) => &source,
        }
    }
}

#[cfg_attr(feature = "serde", derive(Deserialize, Serialize))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnvironmentCapabilityData {
    source_name: Name,
    source: RegistrationSource,
}

/// Describes a capability provided by component manager that is an aggregation
/// of multiple instances of a capability.
#[cfg_attr(
    feature = "serde",
    derive(Deserialize, Serialize),
    serde(tag = "type", rename_all = "snake_case")
)]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum AggregateCapability {
    Service(Name),
}

impl AggregateCapability {
    /// Returns a name for the capability type.
    pub fn type_name(&self) -> CapabilityTypeName {
        match self {
            AggregateCapability::Service(_) => CapabilityTypeName::Service,
        }
    }

    pub fn source_name(&self) -> &Name {
        match self {
            AggregateCapability::Service(name) => &name,
        }
    }
}

impl fmt::Display for AggregateCapability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "aggregate {} '{}'", self.type_name(), self.source_name())
    }
}

impl From<ServiceDecl> for AggregateCapability {
    fn from(service: ServiceDecl) -> Self {
        Self::Service(service.name)
    }
}

/// The list of declarations for capabilities from component manager's namespace.
pub type NamespaceCapabilities = Vec<CapabilityDecl>;

/// The list of declarations for capabilities offered by component manager as built-in capabilities.
pub type BuiltinCapabilities = Vec<CapabilityDecl>;

#[cfg(test)]
mod tests {
    use super::*;
    use cm_rust::StorageDirectorySource;
    use fidl_fuchsia_component_decl as fdecl;

    #[test]
    fn capability_type_name() {
        let storage_capability = ComponentCapability::Storage(StorageDecl {
            name: "foo".parse().unwrap(),
            source: StorageDirectorySource::Parent,
            backing_dir: "bar".parse().unwrap(),
            subdir: Default::default(),
            storage_id: fdecl::StorageId::StaticInstanceIdOrMoniker,
        });
        assert_eq!(storage_capability.type_name(), CapabilityTypeName::Storage);
    }
}
