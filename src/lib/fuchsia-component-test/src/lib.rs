// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::error::Error;
use crate::local_component_runner::LocalComponentRunnerBuilder;
use anyhow::{Context as _, format_err};
use assert_matches::assert_matches;
use cm_rust::{FidlIntoNative, NativeIntoFidl};
use component_events::events::Started;
use component_events::matcher::EventMatcher;
use fidl::endpoints::{
    self, ClientEnd, DiscoverableProtocolMarker, Proxy, ServerEnd, ServiceMarker, create_proxy,
};
use fuchsia_component::client as fclient;
use futures::future::BoxFuture;
use futures::{FutureExt, TryFutureExt, TryStreamExt};
use log::*;
use std::collections::HashMap;
use std::fmt::{self, Display, Formatter};
use {
    fidl_fuchsia_component as fcomponent, fidl_fuchsia_component_decl as fdecl,
    fidl_fuchsia_component_test as ftest, fidl_fuchsia_io as fio, fidl_fuchsia_mem as fmem,
    fidl_fuchsia_sys2 as fsys, fuchsia_async as fasync,
};

pub mod new {
    pub use super::*;
}

/// The default name of the child component collection that contains built topologies.
pub const DEFAULT_COLLECTION_NAME: &'static str = "realm_builder";

const REALM_BUILDER_SERVER_CHILD_NAME: &'static str = "realm_builder_server";

pub mod error;
mod local_component_runner;

pub use local_component_runner::LocalComponentHandles;

/// The source or destination of a capability route.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Ref {
    value: RefInner,

    /// The path to the realm this ref exists in, if known. When set, this ref may not be used
    /// outside of the realm it is scoped to.
    scope: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum RefInner {
    Capability(String),
    Child(String),
    Collection(String),
    Dictionary(Box<Self>, String),
    Debug,
    Framework,
    Parent,
    Self_,
    Void,
}

impl Ref {
    pub fn capability(name: impl Into<String>) -> Ref {
        Ref { value: RefInner::Capability(name.into()), scope: None }
    }
    pub fn child(name: impl Into<String>) -> Ref {
        Ref { value: RefInner::Child(name.into()), scope: None }
    }

    pub fn collection(name: impl Into<String>) -> Ref {
        Ref { value: RefInner::Collection(name.into()), scope: None }
    }

    pub fn debug() -> Ref {
        Ref { value: RefInner::Debug, scope: None }
    }

    pub fn framework() -> Ref {
        Ref { value: RefInner::Framework, scope: None }
    }

    pub fn parent() -> Ref {
        Ref { value: RefInner::Parent, scope: None }
    }

    pub fn self_() -> Ref {
        Ref { value: RefInner::Self_, scope: None }
    }

    pub fn void() -> Ref {
        Ref { value: RefInner::Void, scope: None }
    }

    /// A reference to a dictionary, possibly nested.
    /// `base` represents the first segment of the dictionary path, and `remainder` captures the
    /// rest of the path. `base` is a `Ref` so it is compatible with `Ref` objects returned by
    /// previous RealmBuilder API calls. For example, if you had "parent/diagnostics" in CML,
    /// `base` would be `Ref::parent()` and `remainder` is `"diagnostics"`.
    pub fn dictionary(base: impl Into<Ref>, remainder: impl Into<String>) -> Ref {
        let base = base.into();
        let remainder = remainder.into();
        assert_matches!(
            base.value,
            RefInner::Self_
                | RefInner::Parent
                | RefInner::Framework
                | RefInner::Void
                | RefInner::Child(_),
            "invalid dictionary root {base}"
        );
        Self { value: RefInner::Dictionary(Box::new(base.value), remainder), scope: None }
    }

    #[allow(clippy::result_large_err)] // TODO(https://fxbug.dev/401254890)
    fn check_scope(&self, realm_scope: &Vec<String>) -> Result<(), Error> {
        if let Some(ref_scope) = self.scope.as_ref() {
            if ref_scope != realm_scope {
                return Err(Error::RefUsedInWrongRealm(self.clone(), realm_scope.join("/")));
            }
        }
        Ok(())
    }

    /// Converts this `Ref` to a fidl `Ref`, and a dictionary path if the `Ref` is `Dictionary`.
    pub fn into_fidl(self, ctx: RefContext) -> (fdecl::Ref, Option<String>) {
        self.value.into_fidl(ctx)
    }
}

impl RefInner {
    fn into_fidl(self, ctx: RefContext) -> (fdecl::Ref, Option<String>) {
        let mut path_out = None;
        let ref_ = match self {
            RefInner::Capability(name) => fdecl::Ref::Capability(fdecl::CapabilityRef { name }),
            RefInner::Child(name) => fdecl::Ref::Child(fdecl::ChildRef { name, collection: None }),
            RefInner::Collection(name) => fdecl::Ref::Collection(fdecl::CollectionRef { name }),
            RefInner::Debug => fdecl::Ref::Debug(fdecl::DebugRef {}),
            RefInner::Framework => fdecl::Ref::Framework(fdecl::FrameworkRef {}),
            RefInner::Parent => fdecl::Ref::Parent(fdecl::ParentRef {}),
            RefInner::Self_ => fdecl::Ref::Self_(fdecl::SelfRef {}),
            RefInner::Void => fdecl::Ref::VoidType(fdecl::VoidRef {}),
            RefInner::Dictionary(base_ref, path) => {
                match ctx {
                    RefContext::Source => {
                        // `base_ref` cannot be Dictionary so this will only recurse once
                        let (ref_, _) = base_ref.into_fidl(ctx);
                        path_out = Some(path);
                        ref_
                    }
                    RefContext::Target => {
                        if !matches!(*base_ref, RefInner::Self_) || path.split("/").count() != 1 {
                            panic!(
                                "dictionary in `to` must have the form \
                                   `self/<dictionary_name>, was {}/{}",
                                base_ref, path
                            )
                        }
                        fdecl::Ref::Capability(fdecl::CapabilityRef { name: path })
                    }
                }
            }
        };
        (ref_, path_out)
    }
}

impl Display for RefInner {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            RefInner::Capability(name) => {
                write!(f, "#{name}")
            }
            RefInner::Child(name) => {
                write!(f, "#{name}")
            }
            RefInner::Collection(name) => {
                write!(f, "#{name}")
            }
            RefInner::Debug => {
                write!(f, "debug")
            }
            RefInner::Framework => {
                write!(f, "framework")
            }
            RefInner::Parent => {
                write!(f, "parent")
            }
            RefInner::Self_ => {
                write!(f, "self")
            }
            RefInner::Void => {
                write!(f, "void")
            }
            RefInner::Dictionary(base_ref, path) => {
                write!(f, "{base_ref}/{path}")
            }
        }
    }
}

/// Designates whether a [`Ref`] is being used in the context of a source or target, which is
/// necessary to convert it to `fdecl::Ref` properly.
#[derive(Clone, Copy, Debug)]
pub enum RefContext {
    Source,
    Target,
}

/// A SubRealmBuilder may be referenced as a child in a route, in order to route a capability to or
/// from the sub realm.
impl From<&SubRealmBuilder> for Ref {
    fn from(input: &SubRealmBuilder) -> Ref {
        // It should not be possible for library users to access the top-level SubRealmBuilder,
        // which means that this realm_path.last() will always return Some
        let mut scope = input.realm_path.clone();
        let child_name = scope.pop().expect("this should be impossible");
        Ref { value: RefInner::Child(child_name), scope: Some(scope) }
    }
}

impl From<&ChildRef> for Ref {
    fn from(input: &ChildRef) -> Ref {
        Ref { value: RefInner::Child(input.name.clone()), scope: input.scope.clone() }
    }
}

impl From<&CollectionRef> for Ref {
    fn from(input: &CollectionRef) -> Ref {
        Ref { value: RefInner::Collection(input.name.clone()), scope: input.scope.clone() }
    }
}

impl Display for Ref {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match &self.value {
            RefInner::Capability(name) => {
                write!(f, "capability {}", name)?;
            }
            RefInner::Child(name) => {
                write!(f, "child {}", name)?;
            }
            RefInner::Collection(name) => {
                write!(f, "collection {}", name)?;
            }
            RefInner::Debug => {
                write!(f, "debug")?;
            }
            RefInner::Framework => {
                write!(f, "framework")?;
            }
            RefInner::Parent => {
                write!(f, "parent")?;
            }
            RefInner::Self_ => {
                write!(f, "self")?;
            }
            RefInner::Void => {
                write!(f, "void")?;
            }
            RefInner::Dictionary(base_ref, path) => {
                write!(f, "{base_ref}/{path}")?;
            }
        }
        if let Some(ref_scope) = self.scope.as_ref() {
            write!(f, " in realm {:?}", ref_scope.join("/"))?;
        }
        Ok(())
    }
}

/// A reference to a child in a realm. This struct will be returned when a child is added to a
/// realm, and may be used in subsequent calls to `RealmBuilder` or `SubRealmBuilder` to reference
/// the child that was added.
#[derive(Debug, Clone, PartialEq)]
pub struct ChildRef {
    name: String,
    scope: Option<Vec<String>>,
}

impl ChildRef {
    fn new(name: String, scope: Vec<String>) -> Self {
        ChildRef { name, scope: Some(scope) }
    }

    #[allow(clippy::result_large_err)] // TODO(https://fxbug.dev/401254890)
    fn check_scope(&self, realm_scope: &Vec<String>) -> Result<(), Error> {
        if let Some(ref_scope) = self.scope.as_ref() {
            if ref_scope != realm_scope {
                return Err(Error::RefUsedInWrongRealm(self.into(), realm_scope.join("/")));
            }
        }
        Ok(())
    }
}

impl From<String> for ChildRef {
    fn from(input: String) -> ChildRef {
        ChildRef { name: input, scope: None }
    }
}

impl From<&str> for ChildRef {
    fn from(input: &str) -> ChildRef {
        ChildRef { name: input.to_string(), scope: None }
    }
}

impl From<&SubRealmBuilder> for ChildRef {
    fn from(input: &SubRealmBuilder) -> ChildRef {
        // It should not be possible for library users to access the top-level SubRealmBuilder,
        // which means that this realm_path.last() will always return Some
        let mut scope = input.realm_path.clone();
        let child_name = scope.pop().expect("this should be impossible");
        ChildRef { name: child_name, scope: Some(scope) }
    }
}

impl From<&ChildRef> for ChildRef {
    fn from(input: &ChildRef) -> ChildRef {
        input.clone()
    }
}

impl Display for ChildRef {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "#{}", self.name)
    }
}

/// A reference to a collection in a realm. This struct will be returned when a collection is added to a
/// realm, and may be used in subsequent calls to `RealmBuilder` or `SubRealmBuilder` to reference
/// the collection that was added.
#[derive(Debug, Clone, PartialEq)]
pub struct CollectionRef {
    name: String,
    scope: Option<Vec<String>>,
}

impl CollectionRef {
    fn new(name: String, scope: Vec<String>) -> Self {
        CollectionRef { name, scope: Some(scope) }
    }
}

impl From<String> for CollectionRef {
    fn from(input: String) -> CollectionRef {
        CollectionRef { name: input, scope: None }
    }
}

impl From<&str> for CollectionRef {
    fn from(input: &str) -> CollectionRef {
        CollectionRef { name: input.to_string(), scope: None }
    }
}

impl From<&SubRealmBuilder> for CollectionRef {
    fn from(input: &SubRealmBuilder) -> CollectionRef {
        // It should not be possible for library users to access the top-level SubRealmBuilder,
        // which means that this realm_path.last() will always return Some
        let mut scope = input.realm_path.clone();
        let collection_name = scope.pop().expect("this should be impossible");
        CollectionRef { name: collection_name, scope: Some(scope) }
    }
}

impl From<&CollectionRef> for CollectionRef {
    fn from(input: &CollectionRef) -> CollectionRef {
        input.clone()
    }
}

/// A capability, which may be routed between different components with a `Route`.
pub struct Capability;

impl Capability {
    /// Creates a new protocol capability, whose name is derived from a protocol marker.
    pub fn protocol<P: DiscoverableProtocolMarker>() -> ProtocolCapability {
        Self::protocol_by_name(P::PROTOCOL_NAME)
    }

    /// Creates a new protocol capability.
    pub fn protocol_by_name(name: impl Into<String>) -> ProtocolCapability {
        ProtocolCapability {
            name: name.into(),
            as_: None,
            type_: fdecl::DependencyType::Strong,
            path: None,
            availability: None,
        }
    }

    /// Creates a new configuration capability.
    pub fn configuration(name: impl Into<String>) -> ConfigurationCapability {
        ConfigurationCapability { name: name.into(), as_: None, availability: None }
    }

    /// Creates a new directory capability.
    pub fn directory(name: impl Into<String>) -> DirectoryCapability {
        DirectoryCapability {
            name: name.into(),
            as_: None,
            type_: fdecl::DependencyType::Strong,
            rights: None,
            subdir: None,
            path: None,
            availability: None,
        }
    }

    /// Creates a new storage capability.
    pub fn storage(name: impl Into<String>) -> StorageCapability {
        StorageCapability { name: name.into(), as_: None, path: None, availability: None }
    }

    /// Creates a new service capability, whose name is derived from a protocol marker.
    pub fn service<S: ServiceMarker>() -> ServiceCapability {
        Self::service_by_name(S::SERVICE_NAME)
    }

    /// Creates a new service capability.
    pub fn service_by_name(name: impl Into<String>) -> ServiceCapability {
        ServiceCapability { name: name.into(), as_: None, path: None, availability: None }
    }

    /// Creates a new event_stream capability.
    pub fn event_stream(name: impl Into<String>) -> EventStream {
        EventStream { name: name.into(), rename: None, path: None, scope: None }
    }

    /// Creates a new dictionary capability.
    pub fn dictionary(name: impl Into<String>) -> DictionaryCapability {
        DictionaryCapability { name: name.into(), as_: None, availability: None, path: None }
    }

    /// Creates a new resolver capability.
    pub fn resolver(name: impl Into<String>) -> ResolverCapability {
        ResolverCapability { name: name.into(), as_: None, path: None }
    }

    /// Creates a new runner capability.
    pub fn runner(name: impl Into<String>) -> RunnerCapability {
        RunnerCapability { name: name.into(), as_: None, path: None }
    }
}

/// A protocol capability, which may be routed between components. Created by
/// `Capability::protocol`.
#[derive(Debug, Clone, PartialEq)]
pub struct ProtocolCapability {
    name: String,
    as_: Option<String>,
    type_: fdecl::DependencyType,
    path: Option<String>,
    availability: Option<fdecl::Availability>,
}

impl ProtocolCapability {
    /// The name the targets will see the directory capability as.
    pub fn as_(mut self, as_: impl Into<String>) -> Self {
        self.as_ = Some(as_.into());
        self
    }

    /// Marks any offers involved in this route as "weak", which will cause this route to be
    /// ignored when determining shutdown ordering.
    pub fn weak(mut self) -> Self {
        self.type_ = fdecl::DependencyType::Weak;
        self
    }

    /// The path at which this protocol capability will be provided or used. Only relevant if the
    /// route's source or target is a local component, as these are the only components
    /// that realm builder will generate a modern component manifest for.
    pub fn path(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into());
        self
    }

    /// Marks the availability of this capability as "optional", which allows either this or a
    /// parent offer to have a source of `void`.
    pub fn optional(mut self) -> Self {
        self.availability = Some(fdecl::Availability::Optional);
        self
    }

    /// Marks the availability of this capability to be the same as the availability expectations
    /// set in the target.
    pub fn availability_same_as_target(mut self) -> Self {
        self.availability = Some(fdecl::Availability::SameAsTarget);
        self
    }
}

impl Into<ftest::Capability> for ProtocolCapability {
    fn into(self) -> ftest::Capability {
        ftest::Capability::Protocol(ftest::Protocol {
            name: Some(self.name),
            as_: self.as_,
            type_: Some(self.type_),
            path: self.path,
            availability: self.availability,
            ..Default::default()
        })
    }
}

/// A configuration capability, which may be routed between components. Created by
/// `Capability::configuration`.
#[derive(Debug, Clone, PartialEq)]
pub struct ConfigurationCapability {
    name: String,
    as_: Option<String>,
    availability: Option<fdecl::Availability>,
}

impl ConfigurationCapability {
    /// Renames a configuration capability
    pub fn as_(mut self, name: impl Into<String>) -> Self {
        self.as_ = Some(name.into());
        self
    }

    /// Marks the availability of this configuration as "optional", which allows either this or a
    /// parent offer to have a source of `void`.
    pub fn optional(mut self) -> Self {
        self.availability = Some(fdecl::Availability::Optional);
        self
    }

    /// Marks the availability of this configuration to be the same as the availability expectations
    /// set in the target.
    pub fn availability_same_as_target(mut self) -> Self {
        self.availability = Some(fdecl::Availability::SameAsTarget);
        self
    }
}

impl Into<ftest::Capability> for ConfigurationCapability {
    fn into(self) -> ftest::Capability {
        ftest::Capability::Config(ftest::Config {
            name: Some(self.name),
            as_: self.as_,
            availability: self.availability,
            ..Default::default()
        })
    }
}

/// A directory capability, which may be routed between components. Created by
/// `Capability::directory`.
#[derive(Debug, Clone, PartialEq)]
pub struct DirectoryCapability {
    name: String,
    as_: Option<String>,
    type_: fdecl::DependencyType,
    rights: Option<fio::Operations>,
    subdir: Option<String>,
    path: Option<String>,
    availability: Option<fdecl::Availability>,
}

impl DirectoryCapability {
    /// The name the targets will see the directory capability as.
    pub fn as_(mut self, as_: impl Into<String>) -> Self {
        self.as_ = Some(as_.into());
        self
    }

    /// Marks any offers involved in this route as "weak", which will cause this route to be
    /// ignored when determining shutdown ordering.
    pub fn weak(mut self) -> Self {
        self.type_ = fdecl::DependencyType::Weak;
        self
    }

    /// The rights the target will be allowed to use when accessing the directory.
    pub fn rights(mut self, rights: fio::Operations) -> Self {
        self.rights = Some(rights);
        self
    }

    /// The sub-directory of the directory that the target will be given access to.
    pub fn subdir(mut self, subdir: impl Into<String>) -> Self {
        self.subdir = Some(subdir.into());
        self
    }

    /// The path at which this directory will be provided or used. Only relevant if the route's
    /// source or target is a local component.
    pub fn path(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into());
        self
    }

    /// Marks the availability of this capability as "optional", which allows either this or a
    /// parent offer to have a source of `void`.
    pub fn optional(mut self) -> Self {
        self.availability = Some(fdecl::Availability::Optional);
        self
    }

    /// Marks the availability of this capability to be the same as the availability expectations
    /// set in the target.
    pub fn availability_same_as_target(mut self) -> Self {
        self.availability = Some(fdecl::Availability::SameAsTarget);
        self
    }
}

impl Into<ftest::Capability> for DirectoryCapability {
    fn into(self) -> ftest::Capability {
        ftest::Capability::Directory(ftest::Directory {
            name: Some(self.name),
            as_: self.as_,
            type_: Some(self.type_),
            rights: self.rights,
            subdir: self.subdir,
            path: self.path,
            availability: self.availability,
            ..Default::default()
        })
    }
}

/// A storage capability, which may be routed between components. Created by
/// `Capability::storage`.
#[derive(Debug, Clone, PartialEq)]
pub struct StorageCapability {
    name: String,
    as_: Option<String>,
    path: Option<String>,
    availability: Option<fdecl::Availability>,
}

impl StorageCapability {
    /// The name the targets will see the storage capability as.
    pub fn as_(mut self, as_: impl Into<String>) -> Self {
        self.as_ = Some(as_.into());
        self
    }

    /// The path at which this storage will be used. Only relevant if the route's target is a local
    /// component.
    pub fn path(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into());
        self
    }

    /// Marks the availability of this capability as "optional", which allows either this or a
    /// parent offer to have a source of `void`.
    pub fn optional(mut self) -> Self {
        self.availability = Some(fdecl::Availability::Optional);
        self
    }

    /// Marks the availability of this capability to be the same as the availability expectations
    /// set in the target.
    pub fn availability_same_as_target(mut self) -> Self {
        self.availability = Some(fdecl::Availability::SameAsTarget);
        self
    }
}

impl Into<ftest::Capability> for StorageCapability {
    fn into(self) -> ftest::Capability {
        ftest::Capability::Storage(ftest::Storage {
            name: Some(self.name),
            as_: self.as_,
            path: self.path,
            availability: self.availability,
            ..Default::default()
        })
    }
}

/// A service capability, which may be routed between components. Created by
/// `Capability::service`.
#[derive(Debug, Clone, PartialEq)]
pub struct ServiceCapability {
    name: String,
    as_: Option<String>,
    path: Option<String>,
    availability: Option<fdecl::Availability>,
}

impl ServiceCapability {
    /// The name the targets will see the service capability as.
    pub fn as_(mut self, as_: impl Into<String>) -> Self {
        self.as_ = Some(as_.into());
        self
    }

    /// The path at which this service capability will be provided or used. Only relevant if the
    /// route's source or target is a local component, as these are the only components that realm
    /// builder will generate a modern component manifest for.
    pub fn path(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into());
        self
    }

    /// Marks the availability of this capability as "optional", which allows either this or a
    /// parent offer to have a source of `void`.
    pub fn optional(mut self) -> Self {
        self.availability = Some(fdecl::Availability::Optional);
        self
    }

    /// Marks the availability of this capability to be the same as the availability expectations
    /// set in the target.
    pub fn availability_same_as_target(mut self) -> Self {
        self.availability = Some(fdecl::Availability::SameAsTarget);
        self
    }
}

impl Into<ftest::Capability> for ServiceCapability {
    fn into(self) -> ftest::Capability {
        ftest::Capability::Service(ftest::Service {
            name: Some(self.name),
            as_: self.as_,
            path: self.path,
            availability: self.availability,
            ..Default::default()
        })
    }
}

impl Into<ftest::Capability> for EventStream {
    fn into(self) -> ftest::Capability {
        ftest::Capability::EventStream(ftest::EventStream {
            name: Some(self.name),
            as_: self.rename,
            scope: self.scope.map(|scopes| {
                scopes
                    .into_iter()
                    .map(|scope| {
                        let (scope, _) = scope.into_fidl(RefContext::Source);
                        scope
                    })
                    .collect()
            }),
            path: self.path,
            ..Default::default()
        })
    }
}

/// A dictionary capability, which may be routed between components. Created by
/// `Capability::dictionary`.
#[derive(Debug, Clone, PartialEq)]
pub struct DictionaryCapability {
    name: String,
    as_: Option<String>,
    availability: Option<fdecl::Availability>,
    path: Option<String>,
}

impl DictionaryCapability {
    /// The name the targets will see the dictionary capability as.
    pub fn as_(mut self, as_: impl Into<String>) -> Self {
        self.as_ = Some(as_.into());
        self
    }

    /// Marks the availability of this capability as "optional", which allows either this or a
    /// parent offer to have a source of `void`.
    pub fn optional(mut self) -> Self {
        self.availability = Some(fdecl::Availability::Optional);
        self
    }

    /// Marks the availability of this capability to be the same as the availability expectations
    /// set in the target.
    pub fn availability_same_as_target(mut self) -> Self {
        self.availability = Some(fdecl::Availability::SameAsTarget);
        self
    }

    /// The path at which this dictionary will be used. Only relevant if the route's target is a
    /// local component.
    pub fn path(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into());
        self
    }
}

impl Into<ftest::Capability> for DictionaryCapability {
    fn into(self) -> ftest::Capability {
        ftest::Capability::Dictionary(ftest::Dictionary {
            name: Some(self.name),
            as_: self.as_,
            availability: self.availability,
            path: self.path,
            ..Default::default()
        })
    }
}

/// A resolver capability, which may be routed between components. Created by
/// `Capability::resolver`.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolverCapability {
    name: String,
    as_: Option<String>,
    path: Option<String>,
}

impl ResolverCapability {
    /// The name the targets will see the dictionary capability as.
    pub fn as_(mut self, as_: impl Into<String>) -> Self {
        self.as_ = Some(as_.into());
        self
    }

    /// The path at which this protocol capability will be provided or used. Only relevant if the
    /// route's source or target is a local component, as these are the only components
    /// that realm builder will generate a modern component manifest for.
    pub fn path(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into());
        self
    }
}

impl Into<ftest::Capability> for ResolverCapability {
    fn into(self) -> ftest::Capability {
        ftest::Capability::Resolver(ftest::Resolver {
            name: Some(self.name),
            as_: self.as_,
            path: self.path,
            ..Default::default()
        })
    }
}

/// A runner capability, which may be routed between components. Created by
/// `Capability::runner`.
#[derive(Debug, Clone, PartialEq)]
pub struct RunnerCapability {
    name: String,
    as_: Option<String>,
    path: Option<String>,
}

impl RunnerCapability {
    /// The name the targets will see the dictionary capability as.
    pub fn as_(mut self, as_: impl Into<String>) -> Self {
        self.as_ = Some(as_.into());
        self
    }

    /// The path at which this protocol capability will be provided or used. Only relevant if the
    /// route's source or target is a local component, as these are the only components
    /// that realm builder will generate a modern component manifest for.
    pub fn path(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into());
        self
    }
}

impl Into<ftest::Capability> for RunnerCapability {
    fn into(self) -> ftest::Capability {
        ftest::Capability::Runner(ftest::Runner {
            name: Some(self.name),
            as_: self.as_,
            path: self.path,
            ..Default::default()
        })
    }
}

/// A route of one or more capabilities from one point in the realm to one or more targets.
#[derive(Debug, Clone, PartialEq)]
pub struct Route {
    capabilities: Vec<ftest::Capability>,
    from: Option<Ref>,
    to: Vec<Ref>,
}

impl Route {
    pub fn new() -> Self {
        Self { capabilities: vec![], from: None, to: vec![] }
    }

    /// Adds a capability to this route. Must be called at least once.
    pub fn capability(mut self, capability: impl Into<ftest::Capability>) -> Self {
        self.capabilities.push(capability.into());
        self
    }

    /// Adds a source to this route. Must be called exactly once. Will panic if called a second
    /// time.
    ///
    /// `Ref::dictionary` with a path may be supplied to route a capability from a dictionary. For
    /// example:
    ///
    /// ```
    /// Route::new()
    ///     .capability(Capability::protocol::<flogger::LogSinkMarker>())
    ///     .from(Ref::dictionary(Ref::parent(), "diagnostics"))
    ///     .to(Ref::dictionary(Ref::self_(), "diagnostics"))
    /// ```
    pub fn from(mut self, from: impl Into<Ref>) -> Self {
        let from = from.into();
        if self.from.is_some() {
            panic!("from is already set for this route");
        }
        self.from = Some(from);
        self
    }

    /// Adds a target to this route. Must be called at least once.
    pub fn to(mut self, to: impl Into<Ref>) -> Self {
        self.to.push(to.into());
        self
    }
}

/// A running instance of a created realm. When this struct is dropped the realm is destroyed,
/// along with any components that were in the realm.
pub struct RealmInstance {
    /// The root component of this realm instance, which can be used to access exposed capabilities
    /// from the realm.
    pub root: ScopedInstance,

    // We want to ensure that the local component runner remains alive for as long as the realm
    // exists, so the ScopedInstance is bundled up into a struct along with the local component
    // runner's task.
    local_component_runner_task: Option<fasync::Task<()>>,
}

impl Drop for RealmInstance {
    /// To ensure local components are shutdown in an orderly manner (i.e. after their dependent
    /// clients) upon `drop`, keep the local_component_runner_task alive in an async task until the
    /// destroy_waiter synchronously destroys the realm.
    ///
    /// Remember that you *must* keep a life reference to a `RealmInstance` to ensure that your
    /// realm stays running.
    fn drop(&mut self) {
        if !self.root.destroy_waiter_taken() {
            let destroy_waiter = self.root.take_destroy_waiter();
            let local_component_runner_task = self.local_component_runner_task.take();
            fasync::Task::spawn(async move {
                // move the local component runner task into this block
                let _local_component_runner_task = local_component_runner_task;
                // There's nothing to be done if we fail to destroy the child, perhaps someone
                // else already destroyed it for us. Ignore any error we could get here.
                let _ = destroy_waiter.await;
            })
            .detach();
        }
        // Check if this is what you wanted. If you expected the realm to live longer than it did,
        // you must keep a live reference to it.
        debug!("RealmInstance is now shut down - the realm will be destroyed.");
    }
}

impl RealmInstance {
    /// Destroys the realm instance, returning only once realm destruction is complete.
    ///
    /// This function can be useful to call when it's important to ensure a realm accessing a
    /// global resource is stopped before proceeding, or to ensure that realm destruction doesn't
    /// race with process (and thus local component implementations) termination.
    pub async fn destroy(mut self) -> Result<(), Error> {
        if self.root.destroy_waiter_taken() {
            return Err(Error::DestroyWaiterTaken);
        }
        let _local_component_runner_task = self.local_component_runner_task.take();
        let destroy_waiter = self.root.take_destroy_waiter();
        drop(self);
        destroy_waiter.await.map_err(Error::FailedToDestroyChild)?;
        Ok(())
    }

    /// Connects to the `fuchsia.sys2.LifecycleController` protocol exposed by a nested
    /// component manager and attempts to start the root component. This should only be used
    /// when a realm is built in a nested component manager in debug mode.
    pub async fn start_component_tree(&self) -> Result<(), Error> {
        let lifecycle_controller: fsys::LifecycleControllerProxy = self
            .root
            .connect_to_protocol_at_exposed_dir()
            .map_err(|e| Error::CannotStartRootComponent(e))?;
        let (_, binder_server) = fidl::endpoints::create_endpoints::<fcomponent::BinderMarker>();
        lifecycle_controller.start_instance("./", binder_server).await?.map_err(|e| {
            Error::CannotStartRootComponent(format_err!("received error status: {:?}", e))
        })?;
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct RealmBuilderParams {
    component_realm_proxy: Option<fcomponent::RealmProxy>,
    collection_name: Option<String>,
    fragment_only_url: Option<String>,
    pkg_dir_proxy: Option<fio::DirectoryProxy>,
    start: bool,
    realm_name: Option<String>,
}

impl RealmBuilderParams {
    pub fn new() -> Self {
        Self {
            component_realm_proxy: None,
            collection_name: None,
            fragment_only_url: None,
            pkg_dir_proxy: None,
            start: true,
            realm_name: None,
        }
    }

    pub fn with_realm_proxy(mut self, realm_proxy: fcomponent::RealmProxy) -> Self {
        self.component_realm_proxy = Some(realm_proxy);
        self
    }

    pub fn in_collection(mut self, collection_name: impl Into<String>) -> Self {
        self.collection_name = Some(collection_name.into());
        self
    }

    pub fn from_relative_url(mut self, fragment_only_url: impl Into<String>) -> Self {
        self.fragment_only_url = Some(fragment_only_url.into());
        self
    }

    pub fn with_pkg_dir_proxy(mut self, pkg_dir_proxy: fio::DirectoryProxy) -> Self {
        self.pkg_dir_proxy = Some(pkg_dir_proxy);
        self
    }

    pub fn start(mut self, start_on_build: bool) -> Self {
        self.start = start_on_build;
        self
    }

    pub fn realm_name(mut self, realm_name: impl Into<String>) -> Self {
        self.realm_name = Some(realm_name.into());
        self
    }
}

/// The `RealmBuilder` struct can be used to assemble and create a component realm at runtime.
/// For more information on what can be done with this struct, please see the [documentation on
/// fuchsia.dev](https://fuchsia.dev/fuchsia-src/development/testing/components/realm_builder)
#[derive(Debug)]
pub struct RealmBuilder {
    root_realm: SubRealmBuilder,
    builder_proxy: ftest::BuilderProxy,
    component_realm_proxy: fcomponent::RealmProxy,
    local_component_runner_builder: LocalComponentRunnerBuilder,
    collection_name: String,
    start: bool,
}

impl RealmBuilder {
    /// Creates a new, empty Realm Builder.
    pub async fn new() -> Result<Self, Error> {
        Self::with_params(RealmBuilderParams::new()).await
    }

    pub async fn with_params(params: RealmBuilderParams) -> Result<Self, Error> {
        let component_realm_proxy = match params.component_realm_proxy {
            Some(r) => r,
            None => fclient::connect_to_protocol::<fcomponent::RealmMarker>()
                .map_err(Error::ConnectToServer)?,
        };
        let pkg_dir_proxy = match params.pkg_dir_proxy {
            Some(p) => p,
            None => fuchsia_fs::directory::open_in_namespace(
                "/pkg",
                fuchsia_fs::PERM_READABLE | fuchsia_fs::PERM_EXECUTABLE,
            )
            .map_err(Error::FailedToOpenPkgDir)?,
        };
        let collection_name =
            params.collection_name.unwrap_or_else(|| DEFAULT_COLLECTION_NAME.into());
        let realm_name = params.realm_name.unwrap_or_else(|| {
            let id: u64 = rand::random();
            format!("auto-{:x}", id)
        });
        Self::create(
            component_realm_proxy,
            collection_name,
            params.fragment_only_url,
            pkg_dir_proxy,
            params.start,
            realm_name,
        )
        .await
    }

    async fn create(
        component_realm_proxy: fcomponent::RealmProxy,
        collection_name: String,
        fragment_only_url: Option<String>,
        pkg_dir_proxy: fio::DirectoryProxy,
        start: bool,
        realm_name: String,
    ) -> Result<Self, Error> {
        let (exposed_dir_proxy, exposed_dir_server_end) =
            endpoints::create_proxy::<fio::DirectoryMarker>();
        component_realm_proxy
            .open_exposed_dir(
                &fdecl::ChildRef {
                    name: REALM_BUILDER_SERVER_CHILD_NAME.to_string(),
                    collection: None,
                },
                exposed_dir_server_end,
            )
            .await?
            .map_err(|e| {
                Error::ConnectToServer(format_err!("failed to open exposed dir: {:?}", e))
            })?;
        let realm_builder_factory_proxy = fclient::connect_to_protocol_at_dir_root::<
            ftest::RealmBuilderFactoryMarker,
        >(&exposed_dir_proxy)
        .map_err(Error::ConnectToServer)?;

        let (realm_proxy, realm_server_end) = create_proxy::<ftest::RealmMarker>();
        let (builder_proxy, builder_server_end) = create_proxy::<ftest::BuilderMarker>();
        match fragment_only_url {
            Some(fragment_only_url) => {
                realm_builder_factory_proxy
                    .create_from_relative_url(
                        ClientEnd::from(pkg_dir_proxy.into_channel().unwrap().into_zx_channel()),
                        &fragment_only_url,
                        realm_server_end,
                        builder_server_end,
                    )
                    .await??;
            }
            None => {
                realm_builder_factory_proxy
                    .create(
                        ClientEnd::from(pkg_dir_proxy.into_channel().unwrap().into_zx_channel()),
                        realm_server_end,
                        builder_server_end,
                    )
                    .await??;
            }
        }
        Self::build_struct(
            component_realm_proxy,
            realm_proxy,
            builder_proxy,
            collection_name,
            start,
            realm_name,
        )
    }

    #[allow(clippy::result_large_err)] // TODO(https://fxbug.dev/401254890)
    fn build_struct(
        component_realm_proxy: fcomponent::RealmProxy,
        realm_proxy: ftest::RealmProxy,
        builder_proxy: ftest::BuilderProxy,
        collection_name: String,
        start: bool,
        realm_name: String,
    ) -> Result<Self, Error> {
        let local_component_runner_builder = LocalComponentRunnerBuilder::new();
        Ok(Self {
            root_realm: SubRealmBuilder {
                realm_proxy,
                realm_path: vec![],
                local_component_runner_builder: local_component_runner_builder.clone(),
                name: realm_name,
            },
            component_realm_proxy,
            builder_proxy,
            local_component_runner_builder,
            collection_name,
            start,
        })
    }

    /// Initializes the realm, but doesn't create it. Returns the root URL and the task managing
    /// local component implementations. The caller should pass the URL into
    /// `fuchsia.component.Realm#CreateChild`, and keep the task alive until after
    /// `fuchsia.component.Realm#DestroyChild` has been called.
    pub async fn initialize(self) -> Result<(String, fasync::Task<()>), Error> {
        let (component_runner_client_end, local_component_runner_task) =
            self.local_component_runner_builder.build().await?;
        let root_url = self.builder_proxy.build(component_runner_client_end).await??;
        Ok((root_url, local_component_runner_task))
    }

    /// Creates this realm in a child component collection. By default this happens in the
    /// [`DEFAULT_COLLECTION_NAME`] collection with an autogenerated name for the instance.
    ///
    /// Also by default, after creation it starts the child component by connecting to the
    /// fuchsia.component.Binder protocol exposed from the root realm, which gets added
    /// automatically by the server.
    pub async fn build(self) -> Result<RealmInstance, Error> {
        let (component_runner_client_end, local_component_runner_task) =
            self.local_component_runner_builder.build().await?;
        let root_url = self.builder_proxy.build(component_runner_client_end).await??;
        let factory = ScopedInstanceFactory::new(self.collection_name)
            .with_realm_proxy(self.component_realm_proxy);
        let root = factory
            .new_named_instance(self.root_realm.name, root_url)
            .await
            .map_err(Error::FailedToCreateChild)?;
        let realm =
            RealmInstance { root, local_component_runner_task: Some(local_component_runner_task) };
        if self.start {
            realm.root.connect_to_binder().map_err(Error::FailedToBind)?;
        }
        Ok(realm)
    }

    /// Initializes the created realm under an instance of component manager,
    /// specified by the given fragment-only URL. Returns the realm containing
    /// component manager.
    ///
    /// This function should be used to modify the component manager realm.
    /// Otherwise, to directly build the created realm under an instance of
    /// component manager, use `build_in_nested_component_manager()`.
    ///
    /// NOTE: Any routes passed through from the parent need to be routed to
    /// "#realm_builder" in the test component's CML file.
    pub async fn with_nested_component_manager(
        self,
        component_manager_fragment_only_url: &str,
    ) -> Result<Self, Error> {
        self.root_realm.with_nested_component_manager(component_manager_fragment_only_url).await?;
        Ok(self)
    }

    /// Launches a nested component manager which will run the created realm
    /// (along with any local components in the realm). This component manager
    /// _must_ be referenced by a fragment-only URL.
    ///
    /// This function checks for any protocol routes from `parent` and arranges for them to be
    /// passed through component_manager.
    ///
    /// NOTE: Currently, passthrough only supports protocol capabilities.
    pub async fn build_in_nested_component_manager(
        self,
        component_manager_fragment_only_url: &str,
    ) -> Result<RealmInstance, Error> {
        let component_manager_realm =
            self.with_nested_component_manager(component_manager_fragment_only_url).await?;
        let cm_instance = component_manager_realm.build().await?;
        Ok(cm_instance)
    }

    // Note: the RealmBuilder functions below this line all forward to the implementations in
    // SubRealmBuilder. It would be easier to hold these definitions in a common trait that both
    // structs implemented, but then anyone using RealmBuilder would have to use the trait
    // regardless of if they want sub-realm support or not. This approach, which slightly more
    // tedious, is slightly more convenient for users (one less import they have to remember).

    pub async fn add_child_realm(
        &self,
        name: impl Into<String>,
        options: ChildOptions,
    ) -> Result<SubRealmBuilder, Error> {
        self.root_realm.add_child_realm(name, options).await
    }

    pub async fn add_child_realm_from_relative_url(
        &self,
        name: impl Into<String>,
        relative_url: impl Into<String>,
        options: ChildOptions,
    ) -> Result<SubRealmBuilder, Error> {
        self.root_realm.add_child_realm_from_relative_url(name, relative_url, options).await
    }

    pub async fn add_child_realm_from_decl(
        &self,
        name: impl Into<String>,
        decl: cm_rust::ComponentDecl,
        options: ChildOptions,
    ) -> Result<SubRealmBuilder, Error> {
        self.root_realm.add_child_realm_from_decl(name, decl, options).await
    }

    /// Adds a new component with a local implementation to the realm
    pub async fn add_local_child(
        &self,
        name: impl Into<String>,
        local_component_implementation: impl Fn(
            LocalComponentHandles,
        )
            -> BoxFuture<'static, Result<(), anyhow::Error>>
        + Sync
        + Send
        + 'static,
        options: ChildOptions,
    ) -> Result<ChildRef, Error> {
        self.root_realm.add_local_child(name, local_component_implementation, options).await
    }

    /// Adds a new component to the realm by URL
    pub async fn add_child(
        &self,
        name: impl Into<String>,
        url: impl Into<String>,
        options: ChildOptions,
    ) -> Result<ChildRef, Error> {
        self.root_realm.add_child(name, url, options).await
    }

    /// Adds a new component to the realm with the given component declaration
    pub async fn add_child_from_decl(
        &self,
        name: impl Into<String>,
        decl: cm_rust::ComponentDecl,
        options: ChildOptions,
    ) -> Result<ChildRef, Error> {
        self.root_realm.add_child_from_decl(name, decl, options).await
    }

    /// Returns a copy of the decl for a child in this realm. This operation is
    /// only supported for:
    ///
    /// * A component with a local implementation
    /// * A component added with a fragment-only component URL (typically,
    ///   components bundled in the same package as the realm builder client,
    ///   sharing the same `/pkg` directory, for example,
    ///   `#meta/other-component.cm`; see
    ///   https://fuchsia.dev/fuchsia-src/reference/components/url#relative-fragment-only).
    /// * An automatically generated realm (such as the root)
    pub async fn get_component_decl(
        &self,
        name: impl Into<ChildRef>,
    ) -> Result<cm_rust::ComponentDecl, Error> {
        self.root_realm.get_component_decl(name).await
    }

    /// Replaces the decl for a child of this realm. This operation is only
    /// supported for:
    ///
    /// * A component with a local implementation
    /// * A component added with a fragment-only component URL (typically,
    ///   components bundled in the same package as the realm builder client,
    ///   sharing the same `/pkg` directory, for example,
    ///   `#meta/other-component.cm`; see
    ///   https://fuchsia.dev/fuchsia-src/reference/components/url#relative-fragment-only).
    /// * An automatically generated realm (such as the root)
    pub async fn replace_component_decl(
        &self,
        name: impl Into<ChildRef>,
        decl: cm_rust::ComponentDecl,
    ) -> Result<(), Error> {
        self.root_realm.replace_component_decl(name, decl).await
    }

    /// Returns a copy the decl for this realm
    pub async fn get_realm_decl(&self) -> Result<cm_rust::ComponentDecl, Error> {
        self.root_realm.get_realm_decl().await
    }

    /// Replaces the decl for this realm
    pub async fn replace_realm_decl(&self, decl: cm_rust::ComponentDecl) -> Result<(), Error> {
        self.root_realm.replace_realm_decl(decl).await
    }

    /// Adds a route between components within the realm
    pub async fn add_route(&self, route: Route) -> Result<(), Error> {
        self.root_realm.add_route(route).await
    }

    /// Load the component's structured config values from its package before applying overrides.
    pub async fn init_mutable_config_from_package(
        &self,
        name: impl Into<ChildRef>,
    ) -> Result<(), Error> {
        self.root_realm.init_mutable_config_from_package(name).await
    }

    /// Allow setting config values without loading any packaged values first.
    pub async fn init_mutable_config_to_empty(
        &self,
        name: impl Into<ChildRef>,
    ) -> Result<(), Error> {
        self.root_realm.init_mutable_config_to_empty(name).await
    }

    /// Replaces a value of a given configuration field
    pub async fn set_config_value(
        &self,
        name: impl Into<ChildRef>,
        key: &str,
        value: cm_rust::ConfigValue,
    ) -> Result<(), Error> {
        self.root_realm.set_config_value(name, key, value).await
    }

    /// Creates and routes a read-only directory capability to the given targets. The directory
    /// capability will have the given name, and anyone accessing the directory will see the given
    /// contents.
    pub async fn read_only_directory(
        &self,
        directory_name: impl Into<String>,
        to: Vec<impl Into<Ref>>,
        directory_contents: DirectoryContents,
    ) -> Result<(), Error> {
        self.root_realm.read_only_directory(directory_name, to, directory_contents).await
    }

    /// Creates and routes a new storage capability to the provided targets. Optionally can connect
    /// the provided channel to the `fuchsia.component.StorageAdmin` protocol for this storage
    /// capability.
    #[cfg(fuchsia_api_level_at_least = "HEAD")]
    pub async fn add_storage(
        &self,
        storage_name: impl Into<String>,
        to: Vec<impl Into<Ref>>,
        storage_admin: Option<ServerEnd<fcomponent::StorageAdminMarker>>,
    ) -> Result<(), Error> {
        self.root_realm.add_storage(storage_name, to, storage_admin).await
    }

    /// Adds a Capability to the root realm.
    pub async fn add_capability(&self, capability: cm_rust::CapabilityDecl) -> Result<(), Error> {
        self.root_realm.add_capability(capability).await
    }

    /// Adds a Collection to the root realm.
    pub async fn add_collection(
        &self,
        collection: cm_rust::CollectionDecl,
    ) -> Result<CollectionRef, Error> {
        self.root_realm.add_collection(collection).await
    }

    /// Adds a Environment to the root realm.
    pub async fn add_environment(
        &self,
        environment: cm_rust::EnvironmentDecl,
    ) -> Result<(), Error> {
        self.root_realm.add_environment(environment).await
    }
}

#[derive(Debug, Clone)]
pub struct SubRealmBuilder {
    realm_proxy: ftest::RealmProxy,
    realm_path: Vec<String>,
    local_component_runner_builder: LocalComponentRunnerBuilder,
    name: String,
}

impl SubRealmBuilder {
    pub async fn add_child_realm(
        &self,
        name: impl Into<String>,
        options: ChildOptions,
    ) -> Result<Self, Error> {
        let name: String = name.into();
        let (child_realm_proxy, child_realm_server_end) = create_proxy::<ftest::RealmMarker>();
        self.realm_proxy.add_child_realm(&name, &options.into(), child_realm_server_end).await??;

        let mut child_path = self.realm_path.clone();
        child_path.push(name.clone());
        Ok(SubRealmBuilder {
            realm_proxy: child_realm_proxy,
            realm_path: child_path,
            local_component_runner_builder: self.local_component_runner_builder.clone(),
            name,
        })
    }

    pub async fn add_child_realm_from_relative_url(
        &self,
        name: impl Into<String>,
        relative_url: impl Into<String>,
        options: ChildOptions,
    ) -> Result<SubRealmBuilder, Error> {
        let name: String = name.into();
        let (child_realm_proxy, child_realm_server_end) = create_proxy::<ftest::RealmMarker>();
        self.realm_proxy
            .add_child_realm_from_relative_url(
                &name,
                &relative_url.into(),
                &options.into(),
                child_realm_server_end,
            )
            .await??;

        let mut child_path = self.realm_path.clone();
        child_path.push(name.clone());
        Ok(SubRealmBuilder {
            realm_proxy: child_realm_proxy,
            realm_path: child_path,
            local_component_runner_builder: self.local_component_runner_builder.clone(),
            name,
        })
    }

    pub async fn add_child_realm_from_decl(
        &self,
        name: impl Into<String>,
        decl: cm_rust::ComponentDecl,
        options: ChildOptions,
    ) -> Result<SubRealmBuilder, Error> {
        let name: String = name.into();
        let (child_realm_proxy, child_realm_server_end) = create_proxy::<ftest::RealmMarker>();
        self.realm_proxy
            .add_child_realm_from_decl(
                &name,
                &decl.native_into_fidl(),
                &options.into(),
                child_realm_server_end,
            )
            .await??;

        let mut child_path = self.realm_path.clone();
        child_path.push(name.clone());
        Ok(SubRealmBuilder {
            realm_proxy: child_realm_proxy,
            realm_path: child_path,
            local_component_runner_builder: self.local_component_runner_builder.clone(),
            name,
        })
    }

    /// Adds a new local component to the realm
    pub async fn add_local_child<M>(
        &self,
        name: impl Into<String>,
        local_component_implementation: M,
        options: ChildOptions,
    ) -> Result<ChildRef, Error>
    where
        M: Fn(LocalComponentHandles) -> BoxFuture<'static, Result<(), anyhow::Error>>
            + Sync
            + Send
            + 'static,
    {
        let name: String = name.into();
        self.realm_proxy.add_local_child(&name, &options.into()).await??;

        let mut child_path = self.realm_path.clone();
        child_path.push(name.clone());
        self.local_component_runner_builder
            .register_local_component(child_path.join("/"), local_component_implementation)
            .await?;

        Ok(ChildRef::new(name, self.realm_path.clone()))
    }

    /// Adds a new component to the realm by URL
    pub async fn add_child(
        &self,
        name: impl Into<String>,
        url: impl Into<String>,
        options: ChildOptions,
    ) -> Result<ChildRef, Error> {
        let name: String = name.into();
        self.realm_proxy.add_child(&name, &url.into(), &options.into()).await??;
        Ok(ChildRef::new(name, self.realm_path.clone()))
    }

    /// Adds a new component to the realm with the given component declaration
    pub async fn add_child_from_decl(
        &self,
        name: impl Into<String>,
        decl: cm_rust::ComponentDecl,
        options: ChildOptions,
    ) -> Result<ChildRef, Error> {
        let name: String = name.into();
        self.realm_proxy
            .add_child_from_decl(&name, &decl.native_into_fidl(), &options.into())
            .await??;
        Ok(ChildRef::new(name, self.realm_path.clone()))
    }

    /// Returns a copy the decl for a child in this realm
    pub async fn get_component_decl(
        &self,
        child_ref: impl Into<ChildRef>,
    ) -> Result<cm_rust::ComponentDecl, Error> {
        let child_ref: ChildRef = child_ref.into();
        child_ref.check_scope(&self.realm_path)?;
        let decl = self.realm_proxy.get_component_decl(&child_ref.name).await??;
        Ok(decl.fidl_into_native())
    }

    /// Replaces the decl for a child of this realm
    pub async fn replace_component_decl(
        &self,
        child_ref: impl Into<ChildRef>,
        decl: cm_rust::ComponentDecl,
    ) -> Result<(), Error> {
        let child_ref: ChildRef = child_ref.into();
        child_ref.check_scope(&self.realm_path)?;
        self.realm_proxy
            .replace_component_decl(&child_ref.name, &decl.native_into_fidl())
            .await??;
        Ok(())
    }

    /// Returns a copy the decl for this realm
    pub async fn get_realm_decl(&self) -> Result<cm_rust::ComponentDecl, Error> {
        Ok(self.realm_proxy.get_realm_decl().await??.fidl_into_native())
    }

    /// Replaces the decl for this realm
    pub async fn replace_realm_decl(&self, decl: cm_rust::ComponentDecl) -> Result<(), Error> {
        self.realm_proxy.replace_realm_decl(&decl.native_into_fidl()).await?.map_err(Into::into)
    }

    /// Load the packaged structured config values for the component.
    pub async fn init_mutable_config_from_package(
        &self,
        child_ref: impl Into<ChildRef>,
    ) -> Result<(), Error> {
        let child_ref = child_ref.into();
        child_ref.check_scope(&self.realm_path)?;
        self.realm_proxy
            .init_mutable_config_from_package(&child_ref.name)
            .await?
            .map_err(Into::into)
    }

    /// Load the packaged structured config values for the component.
    pub async fn init_mutable_config_to_empty(
        &self,
        child_ref: impl Into<ChildRef>,
    ) -> Result<(), Error> {
        let child_ref = child_ref.into();
        child_ref.check_scope(&self.realm_path)?;
        self.realm_proxy.init_mutable_config_to_empty(&child_ref.name).await?.map_err(Into::into)
    }

    /// Replaces a value of a given configuration field
    pub async fn set_config_value(
        &self,
        child_ref: impl Into<ChildRef>,
        key: &str,
        value: cm_rust::ConfigValue,
    ) -> Result<(), Error> {
        let child_ref: ChildRef = child_ref.into();
        child_ref.check_scope(&self.realm_path)?;
        self.realm_proxy
            .set_config_value(
                &child_ref.name,
                key,
                &cm_rust::ConfigValueSpec { value }.native_into_fidl(),
            )
            .await?
            .map_err(Into::into)
    }

    /// Adds a route between components within the realm
    pub async fn add_route(&self, route: Route) -> Result<(), Error> {
        let from = route.from.ok_or(Error::MissingSource)?;

        #[allow(unused_mut)] // Mutable not needed if not at API level NEXT
        let mut capabilities = route.capabilities;
        for target in &route.to {
            target.check_scope(&self.realm_path)?;
        }
        from.check_scope(&self.realm_path)?;
        let (route_source, from_dictionary) = from.into_fidl(RefContext::Source);
        if !capabilities.is_empty() {
            let route_targets = route
                .to
                .into_iter()
                .map(|to| {
                    let (to, path) = to.into_fidl(RefContext::Target);
                    assert_matches!(path, None);
                    to
                })
                .collect::<Vec<fdecl::Ref>>();
            // If we don't name the future with `let` and then await it in a second step, rustc
            // will decide this function is not Send and then Realm Builder won't be usable on
            // multi-threaded executors. This is caused by the mutable references held in the
            // future generated by `add_route`.
            #[cfg(fuchsia_api_level_at_least = "25")]
            let fut = self.realm_proxy.add_route_from_dictionary(
                &capabilities,
                &route_source,
                from_dictionary.as_ref().map(|s| s.as_str()).unwrap_or("."),
                &route_targets,
            );
            #[cfg(not(fuchsia_api_level_at_least = "25"))]
            let fut = self.realm_proxy.add_route(&capabilities, &route_source, &route_targets);
            fut.await??;
        }
        Ok(())
    }

    /// Creates and routes a read-only directory capability to the given targets. The directory
    /// capability will have the given name, and anyone accessing the directory will see the given
    /// contents.
    pub async fn read_only_directory(
        &self,
        directory_name: impl Into<String>,
        to: Vec<impl Into<Ref>>,
        directory_contents: DirectoryContents,
    ) -> Result<(), Error> {
        let to: Vec<Ref> = to.into_iter().map(|t| t.into()).collect();
        for target in &to {
            target.check_scope(&self.realm_path)?;
        }
        let to = to
            .into_iter()
            .map(|to| {
                let (to, path) = to.into_fidl(RefContext::Target);
                assert_matches!(path, None);
                to
            })
            .collect::<Vec<_>>();

        let fut = self.realm_proxy.read_only_directory(
            &directory_name.into(),
            &to,
            directory_contents.into(),
        );
        fut.await??;
        Ok(())
    }

    #[cfg(fuchsia_api_level_at_least = "HEAD")]
    pub async fn add_storage(
        &self,
        storage_name: impl Into<String>,
        to: Vec<impl Into<Ref>>,
        storage_admin: Option<ServerEnd<fcomponent::StorageAdminMarker>>,
    ) -> Result<(), Error> {
        let to: Vec<Ref> = to.into_iter().map(Into::into).collect();
        for target in &to {
            target.check_scope(&self.realm_path)?;
        }
        let to = to
            .into_iter()
            .map(|to| {
                let (to, path) = to.into_fidl(RefContext::Target);
                assert_matches!(path, None);
                to
            })
            .collect::<Vec<_>>();

        let fut = self.realm_proxy.add_storage(&storage_name.into(), &to, storage_admin);
        fut.await??;
        Ok(())
    }

    /// Adds a Configuration Capability to the root realm and routes it to the given targets.
    pub async fn add_capability(&self, capability: cm_rust::CapabilityDecl) -> Result<(), Error> {
        self.realm_proxy.add_capability(&capability.native_into_fidl()).await??;
        Ok(())
    }

    /// Adds a Collection to the root realm.
    pub async fn add_collection(
        &self,
        collection: cm_rust::CollectionDecl,
    ) -> Result<CollectionRef, Error> {
        let name = collection.name.clone().into();
        self.realm_proxy.add_collection(&collection.native_into_fidl()).await??;
        Ok(CollectionRef::new(name, self.realm_path.clone()))
    }

    /// Adds a Environment to the root realm.
    pub async fn add_environment(
        &self,
        environment: cm_rust::EnvironmentDecl,
    ) -> Result<(), Error> {
        self.realm_proxy.add_environment(&environment.native_into_fidl()).await??;
        Ok(())
    }

    /// Initializes the created realm under an instance of component manager,
    /// specified by the given fragment-only URL.
    ///
    /// This function should be used to modify the component manager realm.
    /// Otherwise, to directly build the created realm under an instance of
    /// component manager, use `build_in_nested_component_manager()`.
    ///
    /// NOTE: Any routes passed through from the parent need to be routed to
    /// "#realm_builder" in the test component's CML file.
    pub async fn with_nested_component_manager(
        &self,
        component_manager_fragment_only_url: &str,
    ) -> Result<(), Error> {
        self.realm_proxy
            .use_nested_component_manager(component_manager_fragment_only_url)
            .await?
            .map_err(Into::into)
    }
}

impl Display for SubRealmBuilder {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "#{}", self.name)
    }
}

/// Contains the contents of a read-only directory that Realm Builder should provide to a realm.
/// Used with the `RealmBuilder::read_only_directory` function.
pub struct DirectoryContents {
    contents: HashMap<String, fmem::Buffer>,
}

impl DirectoryContents {
    pub fn new() -> Self {
        Self { contents: HashMap::new() }
    }

    pub fn add_file(mut self, path: impl Into<String>, contents: impl Into<Vec<u8>>) -> Self {
        let contents: Vec<u8> = contents.into();
        let vmo = zx::Vmo::create(4096).expect("failed to create a VMO");
        vmo.write(&contents, 0).expect("failed to write to VMO");
        let buffer = fmem::Buffer { vmo, size: contents.len() as u64 };
        self.contents.insert(path.into(), buffer);
        self
    }
}

impl Clone for DirectoryContents {
    fn clone(&self) -> Self {
        let mut new_self = Self::new();
        for (path, buf) in self.contents.iter() {
            new_self.contents.insert(
                path.clone(),
                fmem::Buffer {
                    vmo: buf
                        .vmo
                        .create_child(zx::VmoChildOptions::SNAPSHOT_AT_LEAST_ON_WRITE, 0, buf.size)
                        .expect("failed to clone VMO"),
                    size: buf.size,
                },
            );
        }
        new_self
    }
}

impl From<DirectoryContents> for ftest::DirectoryContents {
    fn from(input: DirectoryContents) -> ftest::DirectoryContents {
        ftest::DirectoryContents {
            entries: input
                .contents
                .into_iter()
                .map(|(path, buf)| ftest::DirectoryEntry { file_path: path, file_contents: buf })
                .collect(),
        }
    }
}

/// Represents an event stream capability per RFC-0121
/// see https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs/0121_component_events
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EventStream {
    name: String,
    scope: Option<Vec<Ref>>,
    rename: Option<String>,
    path: Option<String>,
}

impl EventStream {
    /// Creates a new event stream capability.
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into(), scope: None, rename: None, path: None }
    }

    /// Downscopes an event stream to only handle events
    /// from the specified Refs.
    pub fn with_scope(mut self, scope: impl Into<Ref>) -> Self {
        self.scope.get_or_insert(vec![]).push(scope.into());
        self
    }

    /// The path at which this event_stream capability will be provided or
    /// used. Only relevant if the route's source or target is a local
    /// component, as these are the only components that realm builder will generate
    /// a modern component manifest for.
    pub fn path(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into());
        self
    }

    /// Renames an event stream capability
    pub fn as_(mut self, name: impl Into<String>) -> Self {
        self.rename = Some(name.into());
        self
    }
}

/// The properties for a child being added to a realm
#[derive(Debug, Clone)]
pub struct ChildOptions {
    startup: fdecl::StartupMode,
    environment: Option<String>,
    on_terminate: fdecl::OnTerminate,
    config_overrides: Option<Vec<fdecl::ConfigOverride>>,
}

impl ChildOptions {
    pub fn new() -> Self {
        Self {
            startup: fdecl::StartupMode::Lazy,
            environment: None,
            on_terminate: fdecl::OnTerminate::None,
            config_overrides: None,
        }
    }

    pub fn eager(mut self) -> Self {
        self.startup = fdecl::StartupMode::Eager;
        self
    }

    pub fn environment(mut self, environment: impl Into<String>) -> Self {
        self.environment = Some(environment.into());
        self
    }

    pub fn reboot_on_terminate(mut self) -> Self {
        self.on_terminate = fdecl::OnTerminate::Reboot;
        self
    }

    pub fn config_overrides(
        mut self,
        config_overrides: impl Into<Vec<fdecl::ConfigOverride>>,
    ) -> Self {
        self.config_overrides = Some(config_overrides.into());
        self
    }
}

impl Into<ftest::ChildOptions> for ChildOptions {
    fn into(self) -> ftest::ChildOptions {
        ftest::ChildOptions {
            startup: Some(self.startup),
            environment: self.environment,
            on_terminate: Some(self.on_terminate),
            config_overrides: self.config_overrides,
            ..Default::default()
        }
    }
}

/// Manages the creation of new components within a collection.
pub struct ScopedInstanceFactory {
    realm_proxy: Option<fcomponent::RealmProxy>,
    collection_name: String,
}

impl ScopedInstanceFactory {
    /// Creates a new factory that creates components in the specified collection.
    pub fn new(collection_name: impl Into<String>) -> Self {
        ScopedInstanceFactory { realm_proxy: None, collection_name: collection_name.into() }
    }

    /// Use `realm_proxy` instead of the fuchsia.component.Realm protocol in this component's
    /// incoming namespace. This can be used to start component's in a collection belonging
    /// to another component.
    pub fn with_realm_proxy(mut self, realm_proxy: fcomponent::RealmProxy) -> Self {
        self.realm_proxy = Some(realm_proxy);
        self
    }

    /// Creates and binds to a new component just like `new_named_instance`, but uses an
    /// autogenerated name for the instance.
    pub async fn new_instance(
        &self,
        url: impl Into<String>,
    ) -> Result<ScopedInstance, anyhow::Error> {
        let id: u64 = rand::random();
        let child_name = format!("auto-{:x}", id);
        self.new_named_instance(child_name, url).await
    }

    /// Creates and binds to a new component named `child_name` with `url`.
    /// A ScopedInstance is returned on success, representing the component's lifetime and
    /// providing access to the component's exposed capabilities.
    ///
    /// When the ScopedInstance is dropped, the component will be asynchronously stopped _and_
    /// destroyed.
    ///
    /// This is useful for tests that wish to create components that should be torn down at the
    /// end of the test, or to explicitly control the lifecycle of a component.
    pub async fn new_named_instance(
        &self,
        child_name: impl Into<String>,
        url: impl Into<String>,
    ) -> Result<ScopedInstance, anyhow::Error> {
        let realm = if let Some(realm_proxy) = self.realm_proxy.as_ref() {
            realm_proxy.clone()
        } else {
            fclient::realm().context("Failed to connect to Realm service")?
        };
        let child_name = child_name.into();
        let collection_ref = fdecl::CollectionRef { name: self.collection_name.clone() };
        let child_decl = fdecl::Child {
            name: Some(child_name.clone()),
            url: Some(url.into()),
            startup: Some(fdecl::StartupMode::Lazy),
            ..Default::default()
        };
        let (controller_proxy, controller) = create_proxy::<fcomponent::ControllerMarker>();
        let child_args = fcomponent::CreateChildArgs {
            numbered_handles: None,
            controller: Some(controller),
            ..Default::default()
        };
        let () = realm
            .create_child(&collection_ref, &child_decl, child_args)
            .await
            .context("CreateChild FIDL failed.")?
            .map_err(|e| format_err!("Failed to create child: {:?}", e))?;
        let child_ref = fdecl::ChildRef {
            name: child_name.clone(),
            collection: Some(self.collection_name.clone()),
        };
        let (exposed_dir, server) = endpoints::create_proxy::<fio::DirectoryMarker>();
        let () = realm
            .open_exposed_dir(&child_ref, server)
            .await
            .context("OpenExposedDir FIDL failed.")?
            .map_err(|e|
                // NOTE: There could be a flake here that if the collection is single-run, and the
                // child we created is short-lived, it's possible that the child has already run
                // and terminated, and "open_exposed_dir" would fail with an Internal error.
                format_err!("Failed to open exposed dir of child: {:?}", e))?;
        Ok(ScopedInstance {
            realm,
            child_name,
            collection: self.collection_name.clone(),
            exposed_dir,
            destroy_channel: None,
            controller_proxy,
        })
    }
}

/// RAII object that keeps a component instance alive until it's dropped, and provides convenience
/// functions for using the instance. Components v2 only.
#[must_use = "Dropping `ScopedInstance` will cause the component instance to be stopped and destroyed."]
pub struct ScopedInstance {
    realm: fcomponent::RealmProxy,
    child_name: String,
    collection: String,
    exposed_dir: fio::DirectoryProxy,
    destroy_channel: Option<
        futures::channel::oneshot::Sender<
            Result<
                fidl::client::QueryResponseFut<fcomponent::RealmDestroyChildResult>,
                anyhow::Error,
            >,
        >,
    >,
    controller_proxy: fcomponent::ControllerProxy,
}

impl ScopedInstance {
    /// Creates and binds to a new component just like `new_with_name`, but uses an autogenerated
    /// name for the instance.
    pub async fn new(coll: String, url: String) -> Result<Self, anyhow::Error> {
        ScopedInstanceFactory::new(coll).new_instance(url).await
    }

    /// Creates and binds to a new component named `child_name` in a collection `coll` with `url`,
    /// and returning an object that represents the component's lifetime and can be used to access
    /// the component's exposed directory. When the object is dropped, it will be asynchronously
    /// stopped _and_ destroyed. This is useful for tests that wish to create components that
    /// should be torn down at the end of the test. Components v2 only.
    pub async fn new_with_name(
        child_name: String,
        collection: String,
        url: String,
    ) -> Result<Self, anyhow::Error> {
        ScopedInstanceFactory::new(collection).new_named_instance(child_name, url).await
    }

    /// Returns true if the component is currently running.
    pub async fn is_started(&self) -> Result<bool, anyhow::Error> {
        Ok(self
            .controller_proxy
            .is_started()
            .await
            .context("failed to use controller proxy")?
            .map_err(|e| format_err!("failed to determine if component is started: {:?}", e))?)
    }

    /// Starts the component. An error will be returned if the component is already running.
    pub async fn start(&self) -> Result<ExecutionController, anyhow::Error> {
        self.start_with_args(fcomponent::StartChildArgs::default()).await
    }

    /// Starts the component with the provided start arguments. An error will be returned if the
    /// component is already running.
    pub async fn start_with_args(
        &self,
        args: fcomponent::StartChildArgs,
    ) -> Result<ExecutionController, anyhow::Error> {
        let (execution_proxy, execution_server_end) =
            create_proxy::<fcomponent::ExecutionControllerMarker>();
        self.controller_proxy
            .start(args, execution_server_end)
            .await
            .context("failed to use controller proxy")?
            .map_err(|e| format_err!("failed to start component: {:?}", e))?;
        Ok(ExecutionController::new(execution_proxy))
    }

    pub fn controller(&self) -> &fcomponent::ControllerProxy {
        &self.controller_proxy
    }

    /// Connect to exposed fuchsia.component.Binder protocol of instance, thus
    /// triggering it to start.
    /// Note: This will only work if the component exposes this protocol in its
    /// manifest.
    pub fn connect_to_binder(&self) -> Result<fcomponent::BinderProxy, anyhow::Error> {
        let binder: fcomponent::BinderProxy = self
            .connect_to_protocol_at_exposed_dir()
            .context("failed to connect to fuchsia.component.Binder")?;

        Ok(binder)
    }

    /// Same as `connect_to_binder` except that it will block until the
    /// component has started.
    /// Note: This function expects that the instance has not been started yet.
    /// If the instance has been started before this method is invoked, then
    /// this method will block forever waiting for the Started event.
    /// REQUIRED: The manifest of the component executing this code must use
    /// the "started" event_stream.
    pub async fn start_with_binder_sync(&self) -> Result<(), anyhow::Error> {
        let mut event_stream = component_events::events::EventStream::open()
            .await
            .context("failed to create EventSource")?;

        let _: ClientEnd<fcomponent::BinderMarker> = self
            .connect_to_protocol_at_exposed_dir()
            .context("failed to connect to fuchsia.component.Binder")?;

        let _ = EventMatcher::ok()
            .moniker(self.moniker())
            .wait::<Started>(&mut event_stream)
            .await
            .context("failed to observe Started event")?;

        Ok(())
    }

    /// Connect to an instance of a FIDL protocol hosted in the component's exposed directory`,
    pub fn connect_to_protocol_at_exposed_dir<T: fclient::Connect>(
        &self,
    ) -> Result<T, anyhow::Error> {
        T::connect_at_dir_root(&self.exposed_dir)
    }

    /// Connect to an instance of a FIDL protocol hosted in the component's exposed directory`,
    pub fn connect_to_named_protocol_at_exposed_dir<P: DiscoverableProtocolMarker>(
        &self,
        protocol_name: &str,
    ) -> Result<P::Proxy, anyhow::Error> {
        fclient::connect_to_named_protocol_at_dir_root::<P>(&self.exposed_dir, protocol_name)
    }

    /// Connects to an instance of a FIDL protocol hosted in the component's exposed directory
    /// using the given `server_end`.
    pub fn connect_request_to_protocol_at_exposed_dir<P: DiscoverableProtocolMarker>(
        &self,
        server_end: ServerEnd<P>,
    ) -> Result<(), anyhow::Error> {
        self.connect_request_to_named_protocol_at_exposed_dir(
            P::PROTOCOL_NAME,
            server_end.into_channel(),
        )
    }

    /// Connects to an instance of a FIDL protocol called `protocol_name` hosted in the component's
    /// exposed directory using the given `server_end`.
    pub fn connect_request_to_named_protocol_at_exposed_dir(
        &self,
        protocol_name: &str,
        server_end: zx::Channel,
    ) -> Result<(), anyhow::Error> {
        self.exposed_dir
            .open(protocol_name, fio::Flags::PROTOCOL_SERVICE, &Default::default(), server_end)
            .map_err(Into::into)
    }

    /// Returns a reference to the component's read-only exposed directory.
    pub fn get_exposed_dir(&self) -> &fio::DirectoryProxy {
        &self.exposed_dir
    }

    /// Returns true if `take_destroy_waiter` has already been called.
    pub fn destroy_waiter_taken(&self) -> bool {
        self.destroy_channel.is_some()
    }

    /// Returns a future which can be awaited on for destruction to complete after the
    /// `ScopedInstance` is dropped. Panics if called multiple times.
    pub fn take_destroy_waiter(
        &mut self,
    ) -> impl futures::Future<Output = Result<(), anyhow::Error>> + use<> {
        if self.destroy_channel.is_some() {
            panic!("destroy waiter already taken");
        }
        let (sender, receiver) = futures::channel::oneshot::channel();
        self.destroy_channel = Some(sender);
        receiver.err_into().and_then(futures::future::ready).and_then(
            |fidl_fut: fidl::client::QueryResponseFut<_>| {
                fidl_fut.map(|r: Result<Result<(), fidl_fuchsia_component::Error>, fidl::Error>| {
                    r.context("DestroyChild FIDL error")?
                        .map_err(|e| format_err!("Failed to destroy child: {:?}", e))
                })
            },
        )
    }

    /// Return the name of this instance.
    pub fn child_name(&self) -> &str {
        self.child_name.as_str()
    }

    /// Returns the moniker of this instance relative to the calling component.
    pub fn moniker(&self) -> String {
        format!("./{}:{}", self.collection, self.child_name)
    }
}

impl Drop for ScopedInstance {
    fn drop(&mut self) {
        let Self { realm, collection, child_name, destroy_channel, .. } = self;
        let child_ref =
            fdecl::ChildRef { name: child_name.clone(), collection: Some(collection.clone()) };
        // DestroyChild also stops the component.
        //
        // Calling destroy child within drop guarantees that the message
        // goes out to the realm regardless of there existing a waiter on
        // the destruction channel.
        let result = Ok(realm.destroy_child(&child_ref));
        if let Some(chan) = destroy_channel.take() {
            let () = chan.send(result).unwrap_or_else(|result| {
                warn!("Failed to send result for destroyed scoped instance. Result={:?}", result);
            });
        }
    }
}

/// A controller used to influence and observe a specific execution of a component. The component
/// will be stopped when this is dropped if it is still running from the `start` call that created
/// this controller. If the component has already stopped, or even been restarted by some other
/// action, then dropping this will do nothing.
pub struct ExecutionController {
    execution_proxy: fcomponent::ExecutionControllerProxy,
    execution_event_stream: fcomponent::ExecutionControllerEventStream,
}

impl ExecutionController {
    fn new(execution_proxy: fcomponent::ExecutionControllerProxy) -> Self {
        let execution_event_stream = execution_proxy.take_event_stream();
        Self { execution_proxy, execution_event_stream }
    }

    /// Initiates a stop action and waits for the component to stop. If the component has already
    /// stopped, then this will immediately return the stopped payload.
    pub async fn stop(self) -> Result<fcomponent::StoppedPayload, anyhow::Error> {
        // The only possible error that could be received here is if the channel is closed, which
        // would happen because the component has already stopped. Since the error signifies that
        // the component is already in the state that we want, we can safely ignore it.
        let _ = self.execution_proxy.stop();
        self.wait_for_stop().await
    }

    /// Waits for the current execution of the component to stop.
    pub async fn wait_for_stop(mut self) -> Result<fcomponent::StoppedPayload, anyhow::Error> {
        loop {
            match self.execution_event_stream.try_next().await {
                Ok(Some(fcomponent::ExecutionControllerEvent::OnStop { stopped_payload })) => {
                    return Ok(stopped_payload);
                }
                Ok(Some(fcomponent::ExecutionControllerEvent::_UnknownEvent {
                    ordinal, ..
                })) => {
                    warn!(ordinal:%; "fuchsia.component/ExecutionController delivered unknown event");
                }
                Ok(None) => {
                    return Err(format_err!("ExecutionController closed and no OnStop received"));
                }
                Err(e) => {
                    return Err(format_err!("failed to wait for OnStop: {:?}", e));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use fidl::endpoints::create_proxy_and_stream;
    use fidl_fuchsia_component as fcomponent;
    use futures::channel::mpsc;
    use futures::future::pending;
    use futures::{SinkExt, StreamExt};

    // To ensure that the expected value of any new member is explicitly
    // specified, avoid using `..Default::default()`. To do this, we must work
    // around fidlgen_rust's mechanism for ensuring that adding FIDL `table`
    // fields does not break source.
    #[fuchsia::test]
    fn child_options_to_fidl() {
        let options: ftest::ChildOptions = ChildOptions::new().into();
        assert_eq!(
            options,
            ftest::ChildOptions {
                // Only include values that must be set to pass the test.
                startup: Some(fdecl::StartupMode::Lazy),
                on_terminate: Some(fdecl::OnTerminate::None),
                ..Default::default()
            },
        );
        assert_eq!(
            options,
            ftest::ChildOptions {
                startup: Some(fdecl::StartupMode::Lazy),
                environment: None,
                on_terminate: Some(fdecl::OnTerminate::None),
                config_overrides: None,
                __source_breaking: fidl::marker::SourceBreaking,
            },
        );
        let options: ftest::ChildOptions = ChildOptions::new().eager().into();
        assert_eq!(
            options,
            ftest::ChildOptions {
                startup: Some(fdecl::StartupMode::Eager),
                environment: None,
                on_terminate: Some(fdecl::OnTerminate::None),
                config_overrides: None,
                __source_breaking: fidl::marker::SourceBreaking,
            },
        );
        let options: ftest::ChildOptions = ChildOptions::new().environment("test_env").into();
        assert_eq!(
            options,
            ftest::ChildOptions {
                startup: Some(fdecl::StartupMode::Lazy),
                environment: Some("test_env".to_string()),
                on_terminate: Some(fdecl::OnTerminate::None),
                config_overrides: None,
                __source_breaking: fidl::marker::SourceBreaking,
            },
        );
        let options: ftest::ChildOptions = ChildOptions::new().reboot_on_terminate().into();
        assert_eq!(
            options,
            ftest::ChildOptions {
                startup: Some(fdecl::StartupMode::Lazy),
                environment: None,
                on_terminate: Some(fdecl::OnTerminate::Reboot),
                config_overrides: None,
                __source_breaking: fidl::marker::SourceBreaking,
            },
        );

        let mut config_overrides: Vec<fdecl::ConfigOverride> = vec![];
        config_overrides.push(fdecl::ConfigOverride {
            key: Some("mystring".to_string()),
            value: Some(fdecl::ConfigValue::Single(fdecl::ConfigSingleValue::String(
                "Fuchsia".to_string(),
            ))),
            ..Default::default()
        });
        config_overrides.push(fdecl::ConfigOverride {
            key: Some("mynumber".to_string()),
            value: Some(fdecl::ConfigValue::Single(fdecl::ConfigSingleValue::Uint64(200))),
            ..Default::default()
        });
        let options: ftest::ChildOptions =
            ChildOptions::new().config_overrides(config_overrides.clone()).into();
        assert_eq!(
            options,
            ftest::ChildOptions {
                startup: Some(fdecl::StartupMode::Lazy),
                environment: None,
                on_terminate: Some(fdecl::OnTerminate::None),
                config_overrides: Some(config_overrides),
                __source_breaking: fidl::marker::SourceBreaking,
            },
        );
    }

    #[fuchsia::test]
    async fn child_scope_prevents_cross_realm_usage() {
        let (builder, _server_task, receive_server_requests) = new_realm_builder_and_server_task();
        let child_a = builder.add_child("a", "test://a", ChildOptions::new()).await.unwrap();
        let child_realm_b = builder.add_child_realm("b", ChildOptions::new()).await.unwrap();
        let child_c = child_realm_b.add_child("c", "test://c", ChildOptions::new()).await.unwrap();

        assert_matches!(
            builder.add_route(
                Route::new()
                    .capability(Capability::protocol::<fcomponent::RealmMarker>())
                    .from(&child_a)
                    .to(&child_c)
            ).await,
            Err(Error::RefUsedInWrongRealm(ref_, _)) if ref_ == (&child_c).into()
        );

        assert_matches!(
            child_realm_b.add_route(
                Route::new()
                    .capability(Capability::protocol::<fcomponent::RealmMarker>())
                    .from(&child_a)
                    .to(&child_c)
            ).await,
            Err(Error::RefUsedInWrongRealm(ref_, _)) if ref_ == (&child_a).into()
        );

        assert_matches!(
            builder.get_component_decl(&child_c).await,
            Err(Error::RefUsedInWrongRealm(ref_, _)) if ref_ == (&child_c).into()
        );

        assert_matches!(
            child_realm_b.get_component_decl(&child_a).await,
            Err(Error::RefUsedInWrongRealm(ref_, _)) if ref_ == (&child_a).into()
        );

        assert_matches!(
            builder.replace_component_decl(&child_c, cm_rust::ComponentDecl::default()).await,
            Err(Error::RefUsedInWrongRealm(ref_, _)) if ref_ == (&child_c).into()
        );

        assert_matches!(
            child_realm_b.replace_component_decl(&child_a, cm_rust::ComponentDecl::default()).await,
            Err(Error::RefUsedInWrongRealm(ref_, _)) if ref_ == (&child_a).into()
        );

        // There should be two server requests from the initial add child calls, and then none of
        // the following lines in this test should have sent any requests to the server.
        let sub_realm_receiver = confirm_num_server_requests(receive_server_requests, 2).remove(0);
        confirm_num_server_requests(sub_realm_receiver, 1);
    }

    #[fuchsia::test]
    async fn child_ref_construction() {
        let (builder, _server_task, receive_server_requests) = new_realm_builder_and_server_task();
        let child_realm_a = builder.add_child_realm("a", ChildOptions::new()).await.unwrap();
        let child_realm_b = child_realm_a.add_child_realm("b", ChildOptions::new()).await.unwrap();

        let child_ref_a: ChildRef = (&child_realm_a).into();
        let child_ref_b: ChildRef = (&child_realm_b).into();

        assert_eq!(child_ref_a, ChildRef::new("a".to_string(), vec![]),);

        assert_eq!(child_ref_b, ChildRef::new("b".to_string(), vec!["a".to_string()]),);

        let child_ref_c = builder.add_child("c", "test://c", ChildOptions::new()).await.unwrap();
        let child_ref_d =
            child_realm_a.add_child("d", "test://d", ChildOptions::new()).await.unwrap();
        let child_ref_e =
            child_realm_b.add_child("e", "test://e", ChildOptions::new()).await.unwrap();

        assert_eq!(child_ref_c, ChildRef::new("c".to_string(), vec![]),);

        assert_eq!(child_ref_d, ChildRef::new("d".to_string(), vec!["a".to_string()]),);

        assert_eq!(
            child_ref_e,
            ChildRef::new("e".to_string(), vec!["a".to_string(), "b".to_string()]),
        );

        // There should be two server requests from the initial add child calls, and then none of
        // the following lines in this test should have sent any requests to the server.
        confirm_num_server_requests(receive_server_requests, 2);
    }

    #[fuchsia::test]
    async fn protocol_capability_construction() {
        assert_eq!(
            Capability::protocol_by_name("test"),
            ProtocolCapability {
                name: "test".to_string(),
                as_: None,
                type_: fdecl::DependencyType::Strong,
                path: None,
                availability: None,
            },
        );
        assert_eq!(
            Capability::protocol::<ftest::RealmBuilderFactoryMarker>(),
            ProtocolCapability {
                name: ftest::RealmBuilderFactoryMarker::PROTOCOL_NAME.to_string(),
                as_: None,
                type_: fdecl::DependencyType::Strong,
                path: None,
                availability: None,
            },
        );
        assert_eq!(
            Capability::protocol_by_name("test").as_("test2"),
            ProtocolCapability {
                name: "test".to_string(),
                as_: Some("test2".to_string()),
                type_: fdecl::DependencyType::Strong,
                path: None,
                availability: None,
            },
        );
        assert_eq!(
            Capability::protocol_by_name("test").weak(),
            ProtocolCapability {
                name: "test".to_string(),
                as_: None,
                type_: fdecl::DependencyType::Weak,
                path: None,
                availability: None,
            },
        );
        assert_eq!(
            Capability::protocol_by_name("test").path("/svc/test2"),
            ProtocolCapability {
                name: "test".to_string(),
                as_: None,
                type_: fdecl::DependencyType::Strong,
                path: Some("/svc/test2".to_string()),
                availability: None,
            },
        );
        assert_eq!(
            Capability::protocol_by_name("test").optional(),
            ProtocolCapability {
                name: "test".to_string(),
                as_: None,
                type_: fdecl::DependencyType::Strong,
                path: None,
                availability: Some(fdecl::Availability::Optional),
            },
        );
        assert_eq!(
            Capability::protocol_by_name("test").availability_same_as_target(),
            ProtocolCapability {
                name: "test".to_string(),
                as_: None,
                type_: fdecl::DependencyType::Strong,
                path: None,
                availability: Some(fdecl::Availability::SameAsTarget),
            },
        );
    }

    #[fuchsia::test]
    async fn directory_capability_construction() {
        assert_eq!(
            Capability::directory("test"),
            DirectoryCapability {
                name: "test".to_string(),
                as_: None,
                type_: fdecl::DependencyType::Strong,
                rights: None,
                subdir: None,
                path: None,
                availability: None,
            },
        );
        assert_eq!(
            Capability::directory("test").as_("test2"),
            DirectoryCapability {
                name: "test".to_string(),
                as_: Some("test2".to_string()),
                type_: fdecl::DependencyType::Strong,
                rights: None,
                subdir: None,
                path: None,
                availability: None,
            },
        );
        assert_eq!(
            Capability::directory("test").weak(),
            DirectoryCapability {
                name: "test".to_string(),
                as_: None,
                type_: fdecl::DependencyType::Weak,
                rights: None,
                subdir: None,
                path: None,
                availability: None,
            },
        );
        assert_eq!(
            Capability::directory("test").rights(fio::RX_STAR_DIR),
            DirectoryCapability {
                name: "test".to_string(),
                as_: None,
                type_: fdecl::DependencyType::Strong,
                rights: Some(fio::RX_STAR_DIR),
                subdir: None,
                path: None,
                availability: None,
            },
        );
        assert_eq!(
            Capability::directory("test").subdir("test2"),
            DirectoryCapability {
                name: "test".to_string(),
                as_: None,
                type_: fdecl::DependencyType::Strong,
                rights: None,
                subdir: Some("test2".to_string()),
                path: None,
                availability: None,
            },
        );
        assert_eq!(
            Capability::directory("test").path("/test2"),
            DirectoryCapability {
                name: "test".to_string(),
                as_: None,
                type_: fdecl::DependencyType::Strong,
                rights: None,
                subdir: None,
                path: Some("/test2".to_string()),
                availability: None,
            },
        );
        assert_eq!(
            Capability::directory("test").optional(),
            DirectoryCapability {
                name: "test".to_string(),
                as_: None,
                type_: fdecl::DependencyType::Strong,
                rights: None,
                subdir: None,
                path: None,
                availability: Some(fdecl::Availability::Optional),
            },
        );
        assert_eq!(
            Capability::directory("test").availability_same_as_target(),
            DirectoryCapability {
                name: "test".to_string(),
                as_: None,
                type_: fdecl::DependencyType::Strong,
                rights: None,
                subdir: None,
                path: None,
                availability: Some(fdecl::Availability::SameAsTarget),
            },
        );
    }

    #[fuchsia::test]
    async fn storage_capability_construction() {
        assert_eq!(
            Capability::storage("test"),
            StorageCapability {
                name: "test".to_string(),
                as_: None,
                path: None,
                availability: None
            },
        );
        assert_eq!(
            Capability::storage("test").as_("test2"),
            StorageCapability {
                name: "test".to_string(),
                as_: Some("test2".to_string()),
                path: None,
                availability: None,
            },
        );
        assert_eq!(
            Capability::storage("test").path("/test2"),
            StorageCapability {
                name: "test".to_string(),
                as_: None,
                path: Some("/test2".to_string()),
                availability: None,
            },
        );
        assert_eq!(
            Capability::storage("test").optional(),
            StorageCapability {
                name: "test".to_string(),
                as_: None,
                path: None,
                availability: Some(fdecl::Availability::Optional),
            },
        );
        assert_eq!(
            Capability::storage("test").availability_same_as_target(),
            StorageCapability {
                name: "test".to_string(),
                as_: None,
                path: None,
                availability: Some(fdecl::Availability::SameAsTarget),
            },
        );
    }

    #[fuchsia::test]
    async fn service_capability_construction() {
        assert_eq!(
            Capability::service_by_name("test"),
            ServiceCapability {
                name: "test".to_string(),
                as_: None,
                path: None,
                availability: None
            },
        );
        assert_eq!(
            Capability::service_by_name("test").as_("test2"),
            ServiceCapability {
                name: "test".to_string(),
                as_: Some("test2".to_string()),
                path: None,
                availability: None,
            },
        );
        assert_eq!(
            Capability::service_by_name("test").path("/svc/test2"),
            ServiceCapability {
                name: "test".to_string(),
                as_: None,
                path: Some("/svc/test2".to_string()),
                availability: None,
            },
        );
        assert_eq!(
            Capability::service_by_name("test").optional(),
            ServiceCapability {
                name: "test".to_string(),
                as_: None,
                path: None,
                availability: Some(fdecl::Availability::Optional),
            },
        );
        assert_eq!(
            Capability::service_by_name("test").availability_same_as_target(),
            ServiceCapability {
                name: "test".to_string(),
                as_: None,
                path: None,
                availability: Some(fdecl::Availability::SameAsTarget),
            },
        );
    }

    #[fuchsia::test]
    async fn dictionary_capability_construction() {
        assert_eq!(
            Capability::dictionary("test"),
            DictionaryCapability {
                name: "test".to_string(),
                as_: None,
                availability: None,
                path: None
            },
        );
        assert_eq!(
            Capability::dictionary("test").as_("test2"),
            DictionaryCapability {
                name: "test".to_string(),
                as_: Some("test2".to_string()),
                availability: None,
                path: None,
            },
        );
        assert_eq!(
            Capability::dictionary("test").optional(),
            DictionaryCapability {
                name: "test".to_string(),
                as_: None,
                availability: Some(fdecl::Availability::Optional),
                path: None,
            },
        );
        assert_eq!(
            Capability::dictionary("test").availability_same_as_target(),
            DictionaryCapability {
                name: "test".to_string(),
                as_: None,
                availability: Some(fdecl::Availability::SameAsTarget),
                path: None,
            },
        );
        assert_eq!(
            Capability::dictionary("test").path("/data"),
            DictionaryCapability {
                name: "test".to_string(),
                as_: None,
                availability: None,
                path: Some("/data".to_string()),
            },
        );
    }

    #[fuchsia::test]
    async fn route_construction() {
        assert_eq!(
            Route::new()
                .capability(Capability::protocol_by_name("test"))
                .capability(Capability::protocol_by_name("test2"))
                .from(Ref::child("a"))
                .to(Ref::collection("b"))
                .to(Ref::parent()),
            Route {
                capabilities: vec![
                    Capability::protocol_by_name("test").into(),
                    Capability::protocol_by_name("test2").into(),
                ],
                from: Some(Ref::child("a").into()),
                to: vec![Ref::collection("b").into(), Ref::parent().into(),],
            },
        );
    }

    #[derive(Debug)]
    enum ServerRequest {
        AddChild {
            name: String,
            url: String,
            options: ftest::ChildOptions,
        },
        AddChildFromDecl {
            name: String,
            decl: fdecl::Component,
            options: ftest::ChildOptions,
        },
        AddLocalChild {
            name: String,
            options: ftest::ChildOptions,
        },
        AddChildRealm {
            name: String,
            options: ftest::ChildOptions,
            receive_requests: mpsc::UnboundedReceiver<ServerRequest>,
        },
        AddChildRealmFromRelativeUrl {
            name: String,
            relative_url: String,
            options: ftest::ChildOptions,
            receive_requests: mpsc::UnboundedReceiver<ServerRequest>,
        },
        AddChildRealmFromDecl {
            name: String,
            decl: fdecl::Component,
            options: ftest::ChildOptions,
            receive_requests: mpsc::UnboundedReceiver<ServerRequest>,
        },
        GetComponentDecl {
            name: String,
        },
        ReplaceComponentDecl {
            name: String,
            component_decl: fdecl::Component,
        },
        UseNestedComponentManager {
            #[allow(unused)]
            component_manager_relative_url: String,
        },
        GetRealmDecl,
        ReplaceRealmDecl {
            component_decl: fdecl::Component,
        },
        AddRouteFromDictionary {
            capabilities: Vec<ftest::Capability>,
            from: fdecl::Ref,
            from_dictionary: String,
            to: Vec<fdecl::Ref>,
        },
        ReadOnlyDirectory {
            name: String,
            to: Vec<fdecl::Ref>,
        },
        AddStorage {
            name: String,
            to: Vec<fdecl::Ref>,
        },
        InitMutableConfigFromPackage {
            name: String,
        },
        InitMutableConfigToEmpty {
            name: String,
        },
        AddCapability {
            capability: fdecl::Capability,
        },
        AddCollection {
            collection: fdecl::Collection,
        },
        AddEnvironment {
            environment: fdecl::Environment,
        },
        SetConfigValue {
            name: String,
            key: String,
            value: fdecl::ConfigValueSpec,
        },
    }

    fn handle_realm_stream(
        mut stream: ftest::RealmRequestStream,
        mut report_requests: mpsc::UnboundedSender<ServerRequest>,
    ) -> BoxFuture<'static, ()> {
        async move {
            let mut child_realm_streams = vec![];
            while let Some(req) = stream.try_next().await.unwrap() {
                match req {
                    ftest::RealmRequest::AddChild { responder, name, url, options } => {
                        report_requests
                            .send(ServerRequest::AddChild { name, url, options })
                            .await
                            .unwrap();
                        responder.send(Ok(())).unwrap();
                    }
                    ftest::RealmRequest::AddChildFromDecl { responder, name, decl, options } => {
                        report_requests
                            .send(ServerRequest::AddChildFromDecl { name, decl, options })
                            .await
                            .unwrap();
                        responder.send(Ok(())).unwrap();
                    }
                    ftest::RealmRequest::AddLocalChild { responder, name, options } => {
                        report_requests
                            .send(ServerRequest::AddLocalChild { name, options })
                            .await
                            .unwrap();
                        responder.send(Ok(())).unwrap();
                    }
                    ftest::RealmRequest::AddChildRealm {
                        responder,
                        child_realm,
                        name,
                        options,
                    } => {
                        let (child_realm_report_requests, receive_requests) = mpsc::unbounded();

                        report_requests
                            .send(ServerRequest::AddChildRealm { name, options, receive_requests })
                            .await
                            .unwrap();

                        let child_realm_stream = child_realm.into_stream();
                        child_realm_streams.push(fasync::Task::spawn(async move {
                            handle_realm_stream(child_realm_stream, child_realm_report_requests)
                                .await
                        }));
                        responder.send(Ok(())).unwrap();
                    }
                    ftest::RealmRequest::AddChildRealmFromRelativeUrl {
                        responder,
                        child_realm,
                        name,
                        relative_url,
                        options,
                    } => {
                        let (child_realm_report_requests, receive_requests) = mpsc::unbounded();

                        report_requests
                            .send(ServerRequest::AddChildRealmFromRelativeUrl {
                                name,
                                relative_url,
                                options,
                                receive_requests,
                            })
                            .await
                            .unwrap();

                        let child_realm_stream = child_realm.into_stream();
                        child_realm_streams.push(fasync::Task::spawn(async move {
                            handle_realm_stream(child_realm_stream, child_realm_report_requests)
                                .await
                        }));
                        responder.send(Ok(())).unwrap();
                    }
                    ftest::RealmRequest::AddChildRealmFromDecl {
                        name,
                        decl,
                        options,
                        child_realm,
                        responder,
                    } => {
                        let (child_realm_report_requests, receive_requests) = mpsc::unbounded();

                        report_requests
                            .send(ServerRequest::AddChildRealmFromDecl {
                                name,
                                decl,
                                options,
                                receive_requests,
                            })
                            .await
                            .unwrap();

                        let child_realm_stream = child_realm.into_stream();
                        child_realm_streams.push(fasync::Task::spawn(async move {
                            handle_realm_stream(child_realm_stream, child_realm_report_requests)
                                .await
                        }));
                        responder.send(Ok(())).unwrap();
                    }
                    ftest::RealmRequest::GetComponentDecl { responder, name } => {
                        report_requests
                            .send(ServerRequest::GetComponentDecl { name })
                            .await
                            .unwrap();
                        responder.send(Ok(&fdecl::Component::default())).unwrap();
                    }
                    ftest::RealmRequest::UseNestedComponentManager {
                        responder,
                        component_manager_relative_url,
                    } => {
                        report_requests
                            .send(ServerRequest::UseNestedComponentManager {
                                component_manager_relative_url,
                            })
                            .await
                            .unwrap();
                        responder.send(Ok(())).unwrap();
                    }
                    ftest::RealmRequest::ReplaceComponentDecl {
                        responder,
                        name,
                        component_decl,
                    } => {
                        report_requests
                            .send(ServerRequest::ReplaceComponentDecl { name, component_decl })
                            .await
                            .unwrap();
                        responder.send(Ok(())).unwrap();
                    }
                    ftest::RealmRequest::GetRealmDecl { responder } => {
                        report_requests.send(ServerRequest::GetRealmDecl).await.unwrap();
                        responder.send(Ok(&fdecl::Component::default())).unwrap();
                    }
                    ftest::RealmRequest::ReplaceRealmDecl { responder, component_decl } => {
                        report_requests
                            .send(ServerRequest::ReplaceRealmDecl { component_decl })
                            .await
                            .unwrap();
                        responder.send(Ok(())).unwrap();
                    }
                    ftest::RealmRequest::AddRoute { .. } => {
                        panic!("AddRoute is deprecated");
                    }
                    ftest::RealmRequest::AddRouteFromDictionary {
                        responder,
                        capabilities,
                        from,
                        from_dictionary,
                        to,
                    } => {
                        report_requests
                            .send(ServerRequest::AddRouteFromDictionary {
                                capabilities,
                                from,
                                from_dictionary,
                                to,
                            })
                            .await
                            .unwrap();
                        responder.send(Ok(())).unwrap();
                    }
                    ftest::RealmRequest::ReadOnlyDirectory { responder, name, to, .. } => {
                        report_requests
                            .send(ServerRequest::ReadOnlyDirectory { name, to })
                            .await
                            .unwrap();
                        responder.send(Ok(())).unwrap();
                    }
                    ftest::RealmRequest::AddStorage { responder, name, to, .. } => {
                        report_requests.send(ServerRequest::AddStorage { name, to }).await.unwrap();
                        responder.send(Ok(())).unwrap();
                    }
                    ftest::RealmRequest::InitMutableConfigFromPackage { name, responder } => {
                        report_requests
                            .send(ServerRequest::InitMutableConfigFromPackage { name })
                            .await
                            .unwrap();
                        responder.send(Ok(())).unwrap();
                    }
                    ftest::RealmRequest::InitMutableConfigToEmpty { name, responder } => {
                        report_requests
                            .send(ServerRequest::InitMutableConfigToEmpty { name })
                            .await
                            .unwrap();
                        responder.send(Ok(())).unwrap();
                    }
                    ftest::RealmRequest::AddCapability { capability, responder } => {
                        report_requests
                            .send(ServerRequest::AddCapability { capability })
                            .await
                            .unwrap();
                        responder.send(Ok(())).unwrap();
                    }
                    ftest::RealmRequest::AddCollection { collection, responder } => {
                        report_requests
                            .send(ServerRequest::AddCollection { collection })
                            .await
                            .unwrap();
                        responder.send(Ok(())).unwrap();
                    }
                    ftest::RealmRequest::AddEnvironment { environment, responder } => {
                        report_requests
                            .send(ServerRequest::AddEnvironment { environment })
                            .await
                            .unwrap();
                        responder.send(Ok(())).unwrap();
                    }
                    ftest::RealmRequest::SetConfigValue { responder, name, key, value } => {
                        report_requests
                            .send(ServerRequest::SetConfigValue { name, key, value })
                            .await
                            .unwrap();
                        responder.send(Ok(())).unwrap();
                    }
                }
            }
        }
        .boxed()
    }

    fn new_realm_builder_and_server_task()
    -> (RealmBuilder, fasync::Task<()>, mpsc::UnboundedReceiver<ServerRequest>) {
        let (realm_proxy, realm_stream) = create_proxy_and_stream::<ftest::RealmMarker>();
        let (builder_proxy, mut builder_stream) = create_proxy_and_stream::<ftest::BuilderMarker>();

        let builder_task = fasync::Task::spawn(async move {
            while let Some(req) = builder_stream.try_next().await.unwrap() {
                match req {
                    ftest::BuilderRequest::Build { runner, responder } => {
                        drop(runner);
                        responder.send(Ok("test://hippo")).unwrap();
                    }
                }
            }
        });

        let (realm_report_requests, realm_receive_requests) = mpsc::unbounded();
        let server_task = fasync::Task::spawn(async move {
            let _builder_task = builder_task;
            handle_realm_stream(realm_stream, realm_report_requests).await
        });
        let id: u64 = rand::random();
        let realm_name = format!("auto-{:x}", id);
        let component_realm_proxy =
            fclient::connect_to_protocol::<fcomponent::RealmMarker>().unwrap();

        (
            RealmBuilder::build_struct(
                component_realm_proxy,
                realm_proxy,
                builder_proxy,
                crate::DEFAULT_COLLECTION_NAME.to_string(),
                false,
                realm_name,
            )
            .unwrap(),
            server_task,
            realm_receive_requests,
        )
    }

    // Checks that there are exactly `num` messages currently waiting in the `server_requests`
    // stream. Returns any mpsc receivers found in ServerRequest::AddChildRealm.
    fn confirm_num_server_requests(
        mut server_requests: mpsc::UnboundedReceiver<ServerRequest>,
        num: usize,
    ) -> Vec<mpsc::UnboundedReceiver<ServerRequest>> {
        let mut discovered_receivers = vec![];
        for i in 0..num {
            match server_requests.next().now_or_never() {
                Some(Some(ServerRequest::AddChildRealm { receive_requests, .. })) => {
                    discovered_receivers.push(receive_requests)
                }
                Some(Some(_)) => (),
                Some(None) => panic!("server_requests ended unexpectedly"),
                None => panic!("server_requests had less messages in it than we expected: {}", i),
            }
        }
        assert_matches!(server_requests.next().now_or_never(), None);
        discovered_receivers
    }

    fn assert_add_child_realm(
        receive_server_requests: &mut mpsc::UnboundedReceiver<ServerRequest>,
        expected_name: &str,
        expected_options: ftest::ChildOptions,
    ) -> mpsc::UnboundedReceiver<ServerRequest> {
        match receive_server_requests.next().now_or_never() {
            Some(Some(ServerRequest::AddChildRealm { name, options, receive_requests }))
                if &name == expected_name && options == expected_options =>
            {
                receive_requests
            }
            req => panic!("match failed, received unexpected server request: {:?}", req),
        }
    }

    fn assert_add_child_realm_from_relative_url(
        receive_server_requests: &mut mpsc::UnboundedReceiver<ServerRequest>,
        expected_name: &str,
        expected_relative_url: &str,
        expected_options: ftest::ChildOptions,
    ) -> mpsc::UnboundedReceiver<ServerRequest> {
        match receive_server_requests.next().now_or_never() {
            Some(Some(ServerRequest::AddChildRealmFromRelativeUrl {
                name,
                relative_url,
                options,
                receive_requests,
            })) if &name == expected_name
                && options == expected_options
                && relative_url == expected_relative_url =>
            {
                receive_requests
            }
            req => panic!("match failed, received unexpected server request: {:?}", req),
        }
    }

    fn assert_add_child_realm_from_decl(
        receive_server_requests: &mut mpsc::UnboundedReceiver<ServerRequest>,
        expected_name: &str,
        expected_decl: &fdecl::Component,
        expected_options: ftest::ChildOptions,
    ) -> mpsc::UnboundedReceiver<ServerRequest> {
        match receive_server_requests.next().now_or_never() {
            Some(Some(ServerRequest::AddChildRealmFromDecl {
                name,
                decl,
                options,
                receive_requests,
            })) if &name == expected_name
                && options == expected_options
                && decl == *expected_decl =>
            {
                receive_requests
            }
            req => panic!("match failed, received unexpected server request: {:?}", req),
        }
    }

    fn assert_read_only_directory(
        receive_server_requests: &mut mpsc::UnboundedReceiver<ServerRequest>,
        expected_directory_name: &str,
        expected_targets: Vec<impl Into<Ref>>,
    ) {
        let expected_targets = expected_targets
            .into_iter()
            .map(|t| {
                let t: Ref = t.into();
                t.into_fidl(RefContext::Target).0
            })
            .collect::<Vec<_>>();

        match receive_server_requests.next().now_or_never() {
            Some(Some(ServerRequest::ReadOnlyDirectory { name, to, .. }))
                if &name == expected_directory_name && to == expected_targets =>
            {
                return;
            }
            req => panic!("match failed, received unexpected server request: {:?}", req),
        }
    }

    #[fuchsia::test]
    async fn add_child() {
        let (builder, _server_task, mut receive_server_requests) =
            new_realm_builder_and_server_task();
        let _child_a = builder.add_child("a", "test://a", ChildOptions::new()).await.unwrap();
        assert_matches!(
            receive_server_requests.next().await,
            Some(ServerRequest::AddChild { name, url, options })
                if &name == "a" && &url == "test://a" && options == ChildOptions::new().into()
        );
        assert_matches!(receive_server_requests.next().now_or_never(), None);
    }

    #[fuchsia::test]
    async fn add_child_from_decl() {
        let (builder, _server_task, mut receive_server_requests) =
            new_realm_builder_and_server_task();
        let _child_a = builder
            .add_child_from_decl("a", cm_rust::ComponentDecl::default(), ChildOptions::new())
            .await
            .unwrap();
        assert_matches!(
            receive_server_requests.next().await,
            Some(ServerRequest::AddChildFromDecl { name, decl, options })
                if &name == "a"
                    && decl == fdecl::Component::default()
                    && options == ChildOptions::new().into()
        );
        assert_matches!(receive_server_requests.next().now_or_never(), None);
    }

    #[fuchsia::test]
    async fn add_local_child() {
        let (builder, _server_task, mut receive_server_requests) =
            new_realm_builder_and_server_task();
        let _child_a = builder
            .add_local_child("a", |_| async move { Ok(()) }.boxed(), ChildOptions::new())
            .await
            .unwrap();
        assert_matches!(
            receive_server_requests.next().await,
            Some(ServerRequest::AddLocalChild { name, options })
                if &name == "a" && options == ChildOptions::new().into()
        );
        assert_matches!(receive_server_requests.next().now_or_never(), None);
    }

    #[fuchsia::test]
    async fn add_child_realm() {
        let (builder, _server_task, mut receive_server_requests) =
            new_realm_builder_and_server_task();
        let child_realm_a = builder.add_child_realm("a", ChildOptions::new()).await.unwrap();
        let _child_b = child_realm_a.add_child("b", "test://b", ChildOptions::new()).await.unwrap();
        let child_realm_c = builder
            .add_child_realm_from_relative_url("c", "#c", ChildOptions::new())
            .await
            .unwrap();
        let _child_d = child_realm_c.add_child("d", "test://d", ChildOptions::new()).await.unwrap();
        let child_realm_e = builder
            .add_child_realm_from_decl("e", cm_rust::ComponentDecl::default(), ChildOptions::new())
            .await
            .unwrap();
        let _child_f = child_realm_e.add_child("f", "test://f", ChildOptions::new()).await.unwrap();

        let mut receive_sub_realm_requests =
            assert_add_child_realm(&mut receive_server_requests, "a", ChildOptions::new().into());
        assert_matches!(
            receive_sub_realm_requests.next().await,
            Some(ServerRequest::AddChild { name, url, options })
                if &name == "b" && &url == "test://b" && options == ChildOptions::new().into()
        );
        assert_matches!(receive_sub_realm_requests.next().now_or_never(), None);

        let mut receive_sub_realm_requests = assert_add_child_realm_from_relative_url(
            &mut receive_server_requests,
            "c",
            "#c",
            ChildOptions::new().into(),
        );
        assert_matches!(
            receive_sub_realm_requests.next().await,
            Some(ServerRequest::AddChild { name, url, options })
                if &name == "d" && &url == "test://d" && options == ChildOptions::new().into()
        );
        assert_matches!(receive_sub_realm_requests.next().now_or_never(), None);

        let mut receive_sub_realm_requests = assert_add_child_realm_from_decl(
            &mut receive_server_requests,
            "e",
            &fdecl::Component::default(),
            ChildOptions::new().into(),
        );
        assert_matches!(
            receive_sub_realm_requests.next().await,
            Some(ServerRequest::AddChild { name, url, options })
                if &name == "f" && &url == "test://f" && options == ChildOptions::new().into()
        );
        assert_matches!(receive_sub_realm_requests.next().now_or_never(), None);
        assert_matches!(receive_server_requests.next().now_or_never(), None);
    }

    #[fuchsia::test]
    async fn get_component_decl() {
        let (builder, _server_task, mut receive_server_requests) =
            new_realm_builder_and_server_task();
        let child_a = builder.add_child("a", "test://a", ChildOptions::new()).await.unwrap();
        let _decl = builder.get_component_decl(&child_a).await.unwrap();

        assert_matches!(
            receive_server_requests.next().await,
            Some(ServerRequest::AddChild { name, url, options })
                if &name == "a" && &url == "test://a" && options == ChildOptions::new().into()
        );
        assert_matches!(
            receive_server_requests.next().await,
            Some(ServerRequest::GetComponentDecl { name }) if &name == "a"
        );
        assert_matches!(receive_server_requests.next().now_or_never(), None);
    }

    #[fuchsia::test]
    async fn replace_component_decl() {
        let (builder, _server_task, mut receive_server_requests) =
            new_realm_builder_and_server_task();
        let child_a = builder.add_child("a", "test://a", ChildOptions::new()).await.unwrap();
        builder.replace_component_decl(&child_a, cm_rust::ComponentDecl::default()).await.unwrap();

        assert_matches!(
            receive_server_requests.next().await,
            Some(ServerRequest::AddChild { name, url, options })
                if &name == "a" && &url == "test://a" && options == ChildOptions::new().into()
        );
        assert_matches!(
            receive_server_requests.next().await,
            Some(ServerRequest::ReplaceComponentDecl { name, component_decl })
                if &name == "a" && component_decl == fdecl::Component::default()
        );
        assert_matches!(receive_server_requests.next().now_or_never(), None);
    }

    #[fuchsia::test]
    async fn get_realm_decl() {
        let (builder, _server_task, mut receive_server_requests) =
            new_realm_builder_and_server_task();
        let _decl = builder.get_realm_decl().await.unwrap();

        assert_matches!(receive_server_requests.next().await, Some(ServerRequest::GetRealmDecl));
        assert_matches!(receive_server_requests.next().now_or_never(), None);
    }

    #[fuchsia::test]
    async fn replace_realm_decl() {
        let (builder, _server_task, mut receive_server_requests) =
            new_realm_builder_and_server_task();
        builder.replace_realm_decl(cm_rust::ComponentDecl::default()).await.unwrap();

        assert_matches!(
            receive_server_requests.next().await,
            Some(ServerRequest::ReplaceRealmDecl { component_decl })
                if component_decl == fdecl::Component::default()
        );
        assert_matches!(receive_server_requests.next().now_or_never(), None);
    }

    #[fuchsia::test]
    async fn set_config_value() {
        let (builder, _server_task, mut receive_server_requests) =
            new_realm_builder_and_server_task();
        let child_a = builder.add_child("a", "test://a", ChildOptions::new()).await.unwrap();
        builder.init_mutable_config_from_package(&child_a).await.unwrap();
        builder.init_mutable_config_to_empty(&child_a).await.unwrap();
        builder.set_config_value(&child_a, "test_bool", false.into()).await.unwrap();
        builder.set_config_value(&child_a, "test_int16", (-2 as i16).into()).await.unwrap();
        builder.set_config_value(&child_a, "test_string", "test".to_string().into()).await.unwrap();
        builder
            .set_config_value(&child_a, "test_string_vector", vec!["hello", "fuchsia"].into())
            .await
            .unwrap();

        assert_matches!(
            receive_server_requests.next().await,
            Some(ServerRequest::AddChild { name, url, options })
                if &name == "a" && &url == "test://a" && options == ChildOptions::new().into()
        );

        assert_matches!(
            receive_server_requests.next().await,
            Some(ServerRequest::InitMutableConfigFromPackage { name }) if &name == "a"
        );

        assert_matches!(
            receive_server_requests.next().await,
            Some(ServerRequest::InitMutableConfigToEmpty { name }) if &name == "a"
        );

        assert_matches!(
            receive_server_requests.next().await,
            Some(ServerRequest::SetConfigValue { name, key, value: fdecl::ConfigValueSpec {
                value: Some(fdecl::ConfigValue::Single(fdecl::ConfigSingleValue::Bool(boolean))), ..
            }}) if &name == "a" && &key == "test_bool" && boolean == false
        );

        assert_matches!(
            receive_server_requests.next().await,
            Some(ServerRequest::SetConfigValue { name, key, value: fdecl::ConfigValueSpec {
                value: Some(fdecl::ConfigValue::Single(fdecl::ConfigSingleValue::Int16(int16))), ..
            }}) if &name == "a" && &key == "test_int16" && int16 == -2
        );

        assert_matches!(
            receive_server_requests.next().await,
            Some(ServerRequest::SetConfigValue { name, key, value: fdecl::ConfigValueSpec {
                value: Some(fdecl::ConfigValue::Single(fdecl::ConfigSingleValue::String(string))), ..
            }}) if &name == "a" && &key == "test_string" && &string == "test"
        );

        assert_matches!(
            receive_server_requests.next().await,
            Some(ServerRequest::SetConfigValue { name, key, value: fdecl::ConfigValueSpec {
                value: Some(fdecl::ConfigValue::Vector(fdecl::ConfigVectorValue::StringVector(string_vector))), ..
            }}) if &name == "a" && &key == "test_string_vector" && string_vector == vec!["hello", "fuchsia"]
        );

        assert_matches!(receive_server_requests.next().now_or_never(), None);
    }

    #[fuchsia::test]
    async fn add_route() {
        let (builder, _server_task, mut receive_server_requests) =
            new_realm_builder_and_server_task();
        let child_a = builder.add_child("a", "test://a", ChildOptions::new()).await.unwrap();
        builder
            .add_route(
                Route::new()
                    .capability(Capability::protocol_by_name("test"))
                    .capability(Capability::directory("test2"))
                    .capability(Capability::service_by_name("test3"))
                    .capability(Capability::configuration("test4"))
                    .capability(Capability::dictionary("test5"))
                    .from(&child_a)
                    .to(Ref::parent()),
            )
            .await
            .unwrap();

        assert_matches!(
            receive_server_requests.next().await,
            Some(ServerRequest::AddChild { name, url, options })
                if &name == "a" && &url == "test://a" && options == ChildOptions::new().into()
        );
        assert_matches!(
            receive_server_requests.next().await,
            Some(ServerRequest::AddRouteFromDictionary { capabilities, from, to, from_dictionary })
                if capabilities == vec![
                    Capability::protocol_by_name("test").into(),
                    Capability::directory("test2").into(),
                    Capability::service_by_name("test3").into(),
                    Capability::configuration("test4").into(),
                    Capability::dictionary("test5").into(),
                ]
                    && from == Ref::child("a").into_fidl(RefContext::Source).0
                    && to == vec![Ref::parent().into_fidl(RefContext::Target).0]
                    && from_dictionary == "."
        );
        assert_matches!(receive_server_requests.next().now_or_never(), None);
    }

    #[fuchsia::test]
    async fn add_route_to_dictionary() {
        let (builder, _server_task, mut receive_server_requests) =
            new_realm_builder_and_server_task();
        let child_a = builder.add_child("a", "test://a", ChildOptions::new()).await.unwrap();
        builder
            .add_capability(cm_rust::CapabilityDecl::Dictionary(cm_rust::DictionaryDecl {
                name: "my_dict".parse().unwrap(),
                source_path: None,
            }))
            .await
            .unwrap();
        builder
            .add_route(
                Route::new()
                    .capability(Capability::protocol_by_name("test"))
                    .capability(Capability::directory("test2"))
                    .capability(Capability::service_by_name("test3"))
                    .capability(Capability::dictionary("test4"))
                    .from(&child_a)
                    .to(Ref::dictionary(Ref::self_(), "my_dict")),
            )
            .await
            .unwrap();

        assert_matches!(
            receive_server_requests.next().await,
            Some(ServerRequest::AddChild { name, url, options })
                if &name == "a" && &url == "test://a" && options == ChildOptions::new().into()
        );
        assert_matches!(
            receive_server_requests.next().await,
            Some(ServerRequest::AddCapability { .. })
        );
        assert_matches!(
            receive_server_requests.next().await,
            Some(ServerRequest::AddRouteFromDictionary { capabilities, from, to, from_dictionary })
                if capabilities == vec![
                    Capability::protocol_by_name("test").into(),
                    Capability::directory("test2").into(),
                    Capability::service_by_name("test3").into(),
                    Capability::dictionary("test4").into(),
                ]
                    && from == Ref::child("a").into_fidl(RefContext::Source).0
                    && to == vec![Ref::dictionary(Ref::self_(), "my_dict").into_fidl(RefContext::Target).0]
                    && from_dictionary == "."
        );
        assert_matches!(receive_server_requests.next().now_or_never(), None);
    }

    #[fuchsia::test]
    async fn add_route_from_dictionary() {
        let (builder, _server_task, mut receive_server_requests) =
            new_realm_builder_and_server_task();
        let child_a = builder.add_child("a", "test://a", ChildOptions::new()).await.unwrap();
        builder
            .add_route(
                Route::new()
                    .capability(Capability::protocol_by_name("test"))
                    .capability(Capability::directory("test2"))
                    .capability(Capability::service_by_name("test3"))
                    .capability(Capability::dictionary("test4"))
                    .from(Ref::dictionary(&child_a, "source/dict"))
                    .to(Ref::parent()),
            )
            .await
            .unwrap();

        assert_matches!(
            receive_server_requests.next().await,
            Some(ServerRequest::AddChild { name, url, options })
                if &name == "a" && &url == "test://a" && options == ChildOptions::new().into()
        );
        assert_matches!(
            receive_server_requests.next().await,
            Some(ServerRequest::AddRouteFromDictionary { capabilities, from, to, from_dictionary })
                if capabilities == vec![
                    Capability::protocol_by_name("test").into(),
                    Capability::directory("test2").into(),
                    Capability::service_by_name("test3").into(),
                    Capability::dictionary("test4").into(),
                ]
                    && from == Ref::child("a").into_fidl(RefContext::Source).0
                    && to == vec![Ref::parent().into_fidl(RefContext::Target).0]
                    && from_dictionary == "source/dict"
        );
        assert_matches!(receive_server_requests.next().now_or_never(), None);
    }

    #[fuchsia::test]
    async fn add_child_to_sub_realm() {
        let (builder, _server_task, mut receive_server_requests) =
            new_realm_builder_and_server_task();
        let child_realm = builder.add_child_realm("1", ChildOptions::new()).await.unwrap();
        let _child_a = child_realm.add_child("a", "test://a", ChildOptions::new()).await.unwrap();
        let mut receive_sub_realm_requests =
            assert_add_child_realm(&mut receive_server_requests, "1", ChildOptions::new().into());
        assert_matches!(
            receive_sub_realm_requests.next().await,
            Some(ServerRequest::AddChild { name, url, options })
                if &name == "a" && &url == "test://a" && options == ChildOptions::new().into()
        );
        assert_matches!(receive_sub_realm_requests.next().now_or_never(), None);
        assert_matches!(receive_server_requests.next().now_or_never(), None);
    }

    #[fuchsia::test]
    async fn add_child_from_decl_to_sub_realm() {
        let (builder, _server_task, mut receive_server_requests) =
            new_realm_builder_and_server_task();
        let child_realm = builder.add_child_realm("1", ChildOptions::new()).await.unwrap();
        let _child_a = child_realm
            .add_child_from_decl("a", cm_rust::ComponentDecl::default(), ChildOptions::new())
            .await
            .unwrap();
        let mut receive_sub_realm_requests =
            assert_add_child_realm(&mut receive_server_requests, "1", ChildOptions::new().into());
        assert_matches!(
            receive_sub_realm_requests.next().await,
            Some(ServerRequest::AddChildFromDecl { name, decl, options })
                if &name == "a"
                    && decl == fdecl::Component::default()
                    && options == ChildOptions::new().into()
        );
        assert_matches!(receive_sub_realm_requests.next().now_or_never(), None);
        assert_matches!(receive_server_requests.next().now_or_never(), None);
    }

    #[fuchsia::test]
    async fn add_local_child_to_sub_realm() {
        let (builder, _server_task, mut receive_server_requests) =
            new_realm_builder_and_server_task();
        let child_realm = builder.add_child_realm("1", ChildOptions::new()).await.unwrap();
        let _child_a = child_realm
            .add_local_child("a", |_| async move { Ok(()) }.boxed(), ChildOptions::new())
            .await
            .unwrap();
        let mut receive_sub_realm_requests =
            assert_add_child_realm(&mut receive_server_requests, "1", ChildOptions::new().into());
        assert_matches!(
            receive_sub_realm_requests.next().await,
            Some(ServerRequest::AddLocalChild { name, options })
                if &name == "a" && options == ChildOptions::new().into()
        );
        assert_matches!(receive_sub_realm_requests.next().now_or_never(), None);
        assert_matches!(receive_server_requests.next().now_or_never(), None);
    }

    #[fuchsia::test]
    async fn add_child_realm_to_child_realm() {
        let (builder, _server_task, mut receive_server_requests) =
            new_realm_builder_and_server_task();
        let child_realm = builder.add_child_realm("1", ChildOptions::new()).await.unwrap();
        let child_realm_a = child_realm.add_child_realm("a", ChildOptions::new()).await.unwrap();
        let _child_b = child_realm_a.add_child("b", "test://b", ChildOptions::new()).await.unwrap();

        let mut receive_sub_realm_requests =
            assert_add_child_realm(&mut receive_server_requests, "1", ChildOptions::new().into());
        let mut receive_sub_sub_realm_requests = assert_add_child_realm(
            &mut receive_sub_realm_requests,
            "a",
            ChildOptions::new().into(),
        );
        assert_matches!(
            receive_sub_sub_realm_requests.next().await,
            Some(ServerRequest::AddChild { name, url, options })
                if &name == "b" && &url == "test://b" && options == ChildOptions::new().into()
        );
        assert_matches!(receive_sub_sub_realm_requests.next().now_or_never(), None);
        assert_matches!(receive_sub_realm_requests.next().now_or_never(), None);
        assert_matches!(receive_server_requests.next().now_or_never(), None);
    }

    #[fuchsia::test]
    async fn get_component_decl_in_sub_realm() {
        let (builder, _server_task, mut receive_server_requests) =
            new_realm_builder_and_server_task();
        let child_realm = builder.add_child_realm("1", ChildOptions::new()).await.unwrap();
        let child_a = child_realm.add_child("a", "test://a", ChildOptions::new()).await.unwrap();
        let _decl = child_realm.get_component_decl(&child_a).await.unwrap();

        let mut receive_sub_realm_requests =
            assert_add_child_realm(&mut receive_server_requests, "1", ChildOptions::new().into());
        assert_matches!(
            receive_sub_realm_requests.next().await,
            Some(ServerRequest::AddChild { name, url, options })
                if &name == "a" && &url == "test://a" && options == ChildOptions::new().into()
        );
        assert_matches!(
            receive_sub_realm_requests.next().await,
            Some(ServerRequest::GetComponentDecl { name }) if &name == "a"
        );
        assert_matches!(receive_sub_realm_requests.next().now_or_never(), None);
        assert_matches!(receive_server_requests.next().now_or_never(), None);
    }

    #[fuchsia::test]
    async fn replace_component_decl_in_sub_realm() {
        let (builder, _server_task, mut receive_server_requests) =
            new_realm_builder_and_server_task();
        let child_realm = builder.add_child_realm("1", ChildOptions::new()).await.unwrap();
        let child_a = child_realm.add_child("a", "test://a", ChildOptions::new()).await.unwrap();
        child_realm
            .replace_component_decl(&child_a, cm_rust::ComponentDecl::default())
            .await
            .unwrap();

        let mut receive_sub_realm_requests =
            assert_add_child_realm(&mut receive_server_requests, "1", ChildOptions::new().into());
        assert_matches!(
            receive_sub_realm_requests.next().await,
            Some(ServerRequest::AddChild { name, url, options })
                if &name == "a" && &url == "test://a" && options == ChildOptions::new().into()
        );
        assert_matches!(
            receive_sub_realm_requests.next().await,
            Some(ServerRequest::ReplaceComponentDecl { name, component_decl })
                if &name == "a" && component_decl == fdecl::Component::default()
        );
        assert_matches!(receive_sub_realm_requests.next().now_or_never(), None);
        assert_matches!(receive_server_requests.next().now_or_never(), None);
    }

    #[fuchsia::test]
    async fn get_realm_decl_in_sub_realm() {
        let (builder, _server_task, mut receive_server_requests) =
            new_realm_builder_and_server_task();
        let child_realm = builder.add_child_realm("1", ChildOptions::new()).await.unwrap();
        let _decl = child_realm.get_realm_decl().await.unwrap();

        let mut receive_sub_realm_requests =
            assert_add_child_realm(&mut receive_server_requests, "1", ChildOptions::new().into());
        assert_matches!(receive_sub_realm_requests.next().await, Some(ServerRequest::GetRealmDecl));
        assert_matches!(receive_sub_realm_requests.next().now_or_never(), None);
        assert_matches!(receive_server_requests.next().now_or_never(), None);
    }

    #[fuchsia::test]
    async fn replace_realm_decl_in_sub_realm() {
        let (builder, _server_task, mut receive_server_requests) =
            new_realm_builder_and_server_task();
        let child_realm = builder.add_child_realm("1", ChildOptions::new()).await.unwrap();
        child_realm.replace_realm_decl(cm_rust::ComponentDecl::default()).await.unwrap();

        let mut receive_sub_realm_requests =
            assert_add_child_realm(&mut receive_server_requests, "1", ChildOptions::new().into());
        assert_matches!(
            receive_sub_realm_requests.next().await,
            Some(ServerRequest::ReplaceRealmDecl { component_decl })
                if component_decl == fdecl::Component::default()
        );
        assert_matches!(receive_sub_realm_requests.next().now_or_never(), None);
        assert_matches!(receive_server_requests.next().now_or_never(), None);
    }

    #[fuchsia::test]
    async fn add_route_in_sub_realm() {
        let (builder, _server_task, mut receive_server_requests) =
            new_realm_builder_and_server_task();
        let child_realm = builder.add_child_realm("1", ChildOptions::new()).await.unwrap();
        let child_a = child_realm.add_child("a", "test://a", ChildOptions::new()).await.unwrap();
        child_realm
            .add_route(
                Route::new()
                    .capability(Capability::protocol_by_name("test"))
                    .capability(Capability::directory("test2"))
                    .from(&child_a)
                    .to(Ref::parent()),
            )
            .await
            .unwrap();

        let mut receive_sub_realm_requests =
            assert_add_child_realm(&mut receive_server_requests, "1", ChildOptions::new().into());
        assert_matches!(
            receive_sub_realm_requests.next().await,
            Some(ServerRequest::AddChild { name, url, options })
                if &name == "a" && &url == "test://a" && options == ChildOptions::new().into()
        );
        assert_matches!(
            receive_sub_realm_requests.next().await,
            Some(ServerRequest::AddRouteFromDictionary { capabilities, from, to, from_dictionary })
                if capabilities == vec![
                    Capability::protocol_by_name("test").into(),
                    Capability::directory("test2").into(),
                ]
                    && from == Ref::child("a").into_fidl(RefContext::Source).0
                    && to == vec![Ref::parent().into_fidl(RefContext::Target).0]
                    && from_dictionary == "."
        );
        assert_matches!(receive_sub_realm_requests.next().now_or_never(), None);
        assert_matches!(receive_server_requests.next().now_or_never(), None);
    }

    #[fuchsia::test]
    async fn read_only_directory() {
        let (builder, _server_task, mut receive_server_requests) =
            new_realm_builder_and_server_task();
        let child_a = builder.add_child("a", "test://a", ChildOptions::new()).await.unwrap();
        builder
            .read_only_directory(
                "config",
                vec![&child_a],
                DirectoryContents::new().add_file("config.json", "{ \"hippos\": \"rule!\" }"),
            )
            .await
            .unwrap();

        assert_matches!(
            receive_server_requests.next().await,
            Some(ServerRequest::AddChild { name, url, options })
                if &name == "a" && &url == "test://a" && options == ChildOptions::new().into()
        );
        assert_read_only_directory(&mut receive_server_requests, "config", vec![&child_a]);
    }

    #[fuchsia::test]
    async fn storage() {
        let (builder, _server_task, mut receive_server_requests) =
            new_realm_builder_and_server_task();
        let child_a = builder.add_child("a", "test://a", ChildOptions::new()).await.unwrap();
        builder.add_storage("data", vec![&child_a], None).await.unwrap();

        assert_matches!(
            receive_server_requests.next().await,
            Some(ServerRequest::AddChild { name, url, options })
                if &name == "a" && &url == "test://a" && options == ChildOptions::new().into()
        );
        assert_matches!(
            receive_server_requests.next().await,
            Some(ServerRequest::AddStorage { name, to })
                if &name == "data" && &to == &vec![fdecl::Ref::Child(fdecl::ChildRef { name: "a".to_string(), collection: None })]
        );
    }

    #[test]
    fn realm_builder_works_with_send() {
        // This test exercises realm builder on a multi-threaded executor, so that we can guarantee
        // that the library works in this situation.
        let mut executor = fasync::SendExecutorBuilder::new().num_threads(2).build();
        executor.run(async {
            let (builder, _server_task, _receive_server_requests) =
                new_realm_builder_and_server_task();
            let child_realm_a = builder.add_child_realm("a", ChildOptions::new()).await.unwrap();
            let child_b = builder
                .add_local_child("b", |_handles| pending().boxed(), ChildOptions::new())
                .await
                .unwrap();
            let child_c = builder.add_child("c", "test://c", ChildOptions::new()).await.unwrap();
            let child_e = builder
                .add_child_from_decl("e", cm_rust::ComponentDecl::default(), ChildOptions::new())
                .await
                .unwrap();

            let decl_for_e = builder.get_component_decl(&child_e).await.unwrap();
            builder.replace_component_decl(&child_e, decl_for_e).await.unwrap();
            let realm_decl = builder.get_realm_decl().await.unwrap();
            builder.replace_realm_decl(realm_decl).await.unwrap();
            builder
                .add_route(
                    Route::new()
                        .capability(Capability::protocol::<fcomponent::RealmMarker>())
                        .from(&child_e)
                        .to(&child_c)
                        .to(&child_b)
                        .to(&child_realm_a)
                        .to(Ref::parent()),
                )
                .await
                .unwrap();
            builder
                .read_only_directory(
                    "config",
                    vec![&child_e],
                    DirectoryContents::new().add_file("config.json", "{ \"hippos\": \"rule!\" }"),
                )
                .await
                .unwrap();
        });
    }

    #[fuchsia::test]
    async fn add_configurations() {
        let (builder, _server_task, mut receive_server_requests) =
            new_realm_builder_and_server_task();
        _ = builder.add_child("a", "test://a", ChildOptions::new()).await.unwrap();
        _ = receive_server_requests.next().now_or_never();

        builder
            .add_capability(cm_rust::CapabilityDecl::Config(cm_rust::ConfigurationDecl {
                name: "my-config".to_string().fidl_into_native(),
                value: cm_rust::ConfigValue::Single(cm_rust::ConfigSingleValue::Bool(true)),
            }))
            .await
            .unwrap();
        match receive_server_requests.next().now_or_never() {
            Some(Some(ServerRequest::AddCapability { capability, .. })) => {
                let configuration = assert_matches!(capability, fdecl::Capability::Config(c) => c);
                assert_eq!(configuration.name, Some("my-config".to_string()));
                assert_eq!(
                    configuration.value,
                    Some(fdecl::ConfigValue::Single(fdecl::ConfigSingleValue::Bool(true)))
                );
            }
            req => panic!("match failed, received unexpected server request: {:?}", req),
        };
    }

    #[fuchsia::test]
    async fn add_environment_and_collection() {
        let (builder, _server_task, mut receive_server_requests) =
            new_realm_builder_and_server_task();
        _ = builder.add_child("a", "test://a", ChildOptions::new()).await.unwrap();
        _ = receive_server_requests.next().now_or_never();

        builder
            .add_environment(cm_rust::EnvironmentDecl {
                name: "driver-host-env".parse().unwrap(),
                extends: fdecl::EnvironmentExtends::Realm,
                runners: Box::from([]),
                resolvers: Box::from([cm_rust::ResolverRegistration {
                    resolver: "boot-resolver".parse().unwrap(),
                    source: cm_rust::RegistrationSource::Child("fake-resolver".to_string()),
                    scheme: "fuchsia-boot".to_string(),
                }]),
                debug_capabilities: Box::from([]),
                stop_timeout_ms: Some(20000),
            })
            .await
            .unwrap();
        match receive_server_requests.next().now_or_never() {
            Some(Some(ServerRequest::AddEnvironment { environment, .. })) => {
                assert_eq!(environment.name, Some("driver-host-env".to_string()));
            }
            req => panic!("match failed, received unexpected server request: {:?}", req),
        };
        builder
            .add_collection(cm_rust::CollectionDecl {
                name: "driver-hosts".parse().unwrap(),
                durability: fdecl::Durability::SingleRun,
                environment: Some("driver-host-env".parse().unwrap()),
                allowed_offers: Default::default(),
                allow_long_names: Default::default(),
                persistent_storage: None,
            })
            .await
            .unwrap();
        match receive_server_requests.next().now_or_never() {
            Some(Some(ServerRequest::AddCollection { collection, .. })) => {
                assert_eq!(collection.name, Some("driver-hosts".to_string()));
            }
            req => panic!("match failed, received unexpected server request: {:?}", req),
        };
    }
}
