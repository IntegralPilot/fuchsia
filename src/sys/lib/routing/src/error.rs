// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::policy::PolicyError;
use crate::rights::Rights;
use async_trait::async_trait;
use clonable_error::ClonableError;
use cm_rust::offer::OfferDeclCommon;
use cm_rust::{CapabilityTypeName, ExposeDeclCommon, SourceName, UseDeclCommon};
use cm_types::{Availability, LongName, Name, RelativePath};
use fidl_fuchsia_component as fcomponent;
use fidl_fuchsia_component_decl as fdecl;
use itertools::Itertools;
use moniker::{ChildName, ExtendedMoniker, Moniker};
use router_error::{Explain, RouterError};
use std::sync::Arc;
use thiserror::Error;
use zx_status as zx;

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// Errors produced by `ComponentInstanceInterface`.
#[cfg_attr(feature = "serde", derive(Deserialize, Serialize), serde(rename_all = "snake_case"))]
#[derive(Debug, Error, Clone)]
pub enum ComponentInstanceError {
    #[error("could not find `{moniker}`")]
    InstanceNotFound { moniker: Moniker },
    #[error("component is not executable `{moniker}`")]
    InstanceNotExecutable { moniker: Moniker },
    #[error("component manager instance unavailable")]
    ComponentManagerInstanceUnavailable {},
    #[error("expected a component instance, but got component manager's instance")]
    ComponentManagerInstanceUnexpected {},
    #[error("malformed url `{url}` for `{moniker}`")]
    MalformedUrl { url: String, moniker: Moniker },
    #[error("url `{url}` for `{moniker}` does not resolve to an absolute url")]
    NoAbsoluteUrl { url: String, moniker: Moniker },
    // The capability routing static analyzer never produces this error subtype, so we don't need
    // to serialize it.
    #[cfg_attr(feature = "serde", serde(skip))]
    #[error("failed to resolve `{moniker}`:\n\t{err}")]
    ResolveFailed {
        moniker: Moniker,
        #[source]
        err: ClonableError,
    },
    // The capability routing static analyzer never produces this error subtype, so we don't need
    // to serialize it.
    #[cfg_attr(feature = "serde", serde(skip))]
    #[error("failed to start `{moniker}`:\n\t{err_msg}")]
    StartFailed {
        moniker: Moniker,
        // This error always comes from a StartActionError in
        // //src/sys/component_manager/lib/errors, but we can't directly use the error value here
        // because that library already depends on us.
        err_msg: String,
        err_as_zx: zx::Status,
    },
    #[error("failed to create storage for `{moniker}`:\n\t{err_msg}")]
    FailedToCreateStorage { moniker: Moniker, err_msg: String },
}

impl ComponentInstanceError {
    pub fn as_zx_status(&self) -> zx::Status {
        match self {
            ComponentInstanceError::ResolveFailed { .. }
            | ComponentInstanceError::InstanceNotFound { .. }
            | ComponentInstanceError::ComponentManagerInstanceUnavailable {}
            | ComponentInstanceError::InstanceNotExecutable { .. }
            | ComponentInstanceError::NoAbsoluteUrl { .. }
            | ComponentInstanceError::FailedToCreateStorage { .. } => zx::Status::NOT_FOUND,
            ComponentInstanceError::StartFailed { err_as_zx, .. } => *err_as_zx,
            ComponentInstanceError::MalformedUrl { .. }
            | ComponentInstanceError::ComponentManagerInstanceUnexpected { .. } => {
                zx::Status::INTERNAL
            }
        }
    }

    pub fn instance_not_found(moniker: Moniker) -> ComponentInstanceError {
        ComponentInstanceError::InstanceNotFound { moniker }
    }

    pub fn cm_instance_unavailable() -> ComponentInstanceError {
        ComponentInstanceError::ComponentManagerInstanceUnavailable {}
    }

    pub fn resolve_failed(moniker: Moniker, err: impl Into<anyhow::Error>) -> Self {
        Self::ResolveFailed { moniker, err: err.into().into() }
    }
}

impl Explain for ComponentInstanceError {
    fn as_zx_status(&self) -> zx::Status {
        self.as_zx_status()
    }
}

impl From<ComponentInstanceError> for ExtendedMoniker {
    fn from(err: ComponentInstanceError) -> ExtendedMoniker {
        match err {
            ComponentInstanceError::InstanceNotFound { moniker }
            | ComponentInstanceError::MalformedUrl { moniker, .. }
            | ComponentInstanceError::NoAbsoluteUrl { moniker, .. }
            | ComponentInstanceError::InstanceNotExecutable { moniker }
            | ComponentInstanceError::ResolveFailed { moniker, .. }
            | ComponentInstanceError::StartFailed { moniker, .. }
            | ComponentInstanceError::FailedToCreateStorage { moniker, .. } => {
                ExtendedMoniker::ComponentInstance(moniker)
            }
            ComponentInstanceError::ComponentManagerInstanceUnavailable {}
            | ComponentInstanceError::ComponentManagerInstanceUnexpected {} => {
                ExtendedMoniker::ComponentManager
            }
        }
    }
}

// Custom implementation of PartialEq in which two ComponentInstanceError::ResolveFailed errors are
// never equal.
impl PartialEq for ComponentInstanceError {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Self::InstanceNotFound { moniker: self_moniker },
                Self::InstanceNotFound { moniker: other_moniker },
            ) => self_moniker.eq(other_moniker),
            (
                Self::ComponentManagerInstanceUnavailable {},
                Self::ComponentManagerInstanceUnavailable {},
            ) => true,
            (Self::ResolveFailed { .. }, Self::ResolveFailed { .. }) => false,
            _ => false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(Deserialize, Serialize), serde(rename_all = "snake_case"))]
pub enum RouteVerb {
    Use,
    Offer,
    Expose,
    Declare,
    Contain,
    Register,
    IncludeInAggregate,
}

impl std::fmt::Display for RouteVerb {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RouteVerb::Use => write!(f, "use"),
            RouteVerb::Offer => write!(f, "offer"),
            RouteVerb::Expose => write!(f, "expose"),
            RouteVerb::Declare => write!(f, "declare"),
            RouteVerb::Contain => write!(f, "contain"),
            RouteVerb::Register => write!(f, "register in environment"),
            RouteVerb::IncludeInAggregate => write!(f, "include in aggregate"),
        }
    }
}

impl From<&cm_rust::ExposeDecl> for RouteVerb {
    fn from(_decl: &cm_rust::ExposeDecl) -> Self {
        Self::Expose
    }
}

impl From<&cm_rust::OfferDecl> for RouteVerb {
    fn from(_decl: &cm_rust::OfferDecl) -> Self {
        Self::Offer
    }
}

impl From<&cm_rust::UseDecl> for RouteVerb {
    fn from(_decl: &cm_rust::UseDecl) -> Self {
        Self::Use
    }
}

impl From<&cm_rust::DebugRegistration> for RouteVerb {
    fn from(_decl: &cm_rust::DebugRegistration) -> Self {
        Self::Register
    }
}

impl From<&cm_rust::RunnerRegistration> for RouteVerb {
    fn from(_decl: &cm_rust::RunnerRegistration) -> Self {
        Self::Register
    }
}

impl From<&cm_rust::ResolverRegistration> for RouteVerb {
    fn from(_decl: &cm_rust::ResolverRegistration) -> Self {
        Self::Register
    }
}

#[derive(Clone, PartialEq, Debug, Error)]
#[cfg_attr(feature = "serde", derive(Deserialize, Serialize), serde(rename_all = "snake_case"))]
pub enum PrettyPrintRef {
    Parent,
    Self_,
    Child(Name),
    ChildInCollection(LongName, Name),
    Collection(Name),
    Framework,
    Capability(Name),
    Debug,
    Void,
    Environment,
}

impl std::fmt::Display for PrettyPrintRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PrettyPrintRef::Parent => write!(f, "parent"),
            PrettyPrintRef::Self_ => write!(f, "self"),
            PrettyPrintRef::Child(name) => write!(f, "child {name}"),
            PrettyPrintRef::ChildInCollection(name, collection) => {
                write!(f, "child {name} in collection {collection}")
            }
            PrettyPrintRef::Collection(name) => write!(f, "collection {name}"),
            PrettyPrintRef::Framework => write!(f, "framework"),
            PrettyPrintRef::Capability(name) => write!(f, "capability {name}"),
            PrettyPrintRef::Debug => write!(f, "debug"),
            PrettyPrintRef::Void => write!(f, "void"),
            PrettyPrintRef::Environment => write!(f, "environment"),
        }
    }
}

impl From<fdecl::Ref> for PrettyPrintRef {
    fn from(ref_: fdecl::Ref) -> Self {
        match ref_ {
            fdecl::Ref::Parent(_) => PrettyPrintRef::Parent,
            fdecl::Ref::Self_(_) => PrettyPrintRef::Self_,
            fdecl::Ref::Child(child_ref) if child_ref.collection.is_none() => {
                PrettyPrintRef::Child(Name::new(child_ref.name).unwrap())
            }
            fdecl::Ref::Child(child_ref) => PrettyPrintRef::ChildInCollection(
                LongName::new(child_ref.name).unwrap(),
                Name::new(child_ref.collection.unwrap()).unwrap(),
            ),
            fdecl::Ref::Collection(collection) => {
                PrettyPrintRef::Collection(Name::new(collection.name).unwrap())
            }
            fdecl::Ref::Framework(_) => PrettyPrintRef::Framework,
            fdecl::Ref::Capability(capability) => {
                PrettyPrintRef::Capability(Name::new(capability.name).unwrap())
            }
            fdecl::Ref::Debug(_) => PrettyPrintRef::Debug,
            fdecl::Ref::VoidType(_) => PrettyPrintRef::Void,
            fdecl::Ref::Environment(_) => PrettyPrintRef::Environment,
            _ => panic!("unexpected fdecl::Ref variant found"),
        }
    }
}

impl From<ChildName> for PrettyPrintRef {
    fn from(child_name: ChildName) -> Self {
        if let Some(collection_name) = child_name.collection() {
            Self::ChildInCollection(child_name.name().to_owned(), collection_name.to_owned())
        } else {
            Self::Child(Name::new(child_name.name()).unwrap())
        }
    }
}

/// Errors produced during routing.
#[cfg_attr(feature = "serde", derive(Deserialize, Serialize), serde(rename_all = "snake_case"))]
#[derive(Debug, Error, Clone, PartialEq)]
pub enum RoutingError {
    #[error(
        "cannot {verb} {capability_type} {capability_name} from {source} at {moniker} because {source} does not {counter_verb} the capability"
    )]
    RouteSourceNotFound {
        moniker: Moniker,
        verb: RouteVerb,
        counter_verb: RouteVerb,
        source: PrettyPrintRef,
        capability_type: CapabilityTypeName,
        capability_name: RelativePath,
    },

    #[error("route request received by `{moniker}` is unexpectedly unset")]
    RouteRequestUnset { moniker: ExtendedMoniker },

    #[error(
        "route request received by `{moniker}` has been rejected because the following field is unset: `{missing_field}`"
    )]
    RouteRequestMissingField { moniker: ExtendedMoniker, missing_field: String },

    #[error(
        "failed to parse field `{field}` in route request received by `{moniker}`: {parse_error}"
    )]
    RouteRequestFailedToParseField { moniker: ExtendedMoniker, field: String, parse_error: String },

    #[error(
        "`{target_name:?}` tried to use a storage capability from `{source_moniker}` but it is \
        not in the component id index. https://fuchsia.dev/go/components/instance-id"
    )]
    ComponentNotInIdIndex { source_moniker: Moniker, target_name: Option<ChildName> },

    #[error(
        "`{moniker}` tried to use {capability_type} `{capability_name}` from the root environment"
    )]
    UseFromRootEnvironmentNotAllowed {
        moniker: Moniker,
        capability_type: String,
        capability_name: Name,
    },

    #[error(
        "`{moniker}` tried to expose `{capability_id}` from the framework, but no such framework capability was found"
    )]
    ExposeFromFrameworkNotFound { moniker: Moniker, capability_id: String },

    #[error("`{capability_id}` was not exposed from `/`")]
    UseFromRootExposeNotFound { capability_id: String },

    #[error("routing a capability from an unsupported source type `{source_type}` at `{moniker}`")]
    UnsupportedRouteSource { source_type: String, moniker: ExtendedMoniker },

    #[error("routing a capability of an unsupported type `{type_name}` at `{moniker}`")]
    UnsupportedCapabilityType { type_name: CapabilityTypeName, moniker: ExtendedMoniker },

    #[error("dynamic dictionaries are not allowed at component `{moniker}`")]
    DynamicDictionariesNotAllowed { moniker: Moniker },

    #[error("item `{name}` is not present in dictionary at component `{moniker}`")]
    BedrockNotPresentInDictionary { name: String, moniker: ExtendedMoniker },

    #[error(
        "routed capability was the wrong type at component `{moniker}`. Was: {actual}, expected: {expected}"
    )]
    BedrockWrongCapabilityType { actual: String, expected: String, moniker: ExtendedMoniker },

    #[error("failed to send message for capability `{capability_id}` from component `{moniker}`")]
    BedrockFailedToSend { moniker: ExtendedMoniker, capability_id: String },

    #[error(
        "the source of capability `{capability_id}` at component `{moniker}` is unknown because we did not perform any routing tasks to find it"
    )]
    SourceUnknown { moniker: ExtendedMoniker, capability_id: String },

    #[error(
        "failed to route capability because the route source has been shutdown and possibly destroyed"
    )]
    RouteSourceShutdown { moniker: Moniker },

    #[error(transparent)]
    ComponentInstanceError(#[from] ComponentInstanceError),

    #[error(transparent)]
    EventsRoutingError(#[from] EventsRoutingError),

    #[error(transparent)]
    RightsRoutingError(#[from] RightsRoutingError),

    #[error(transparent)]
    AvailabilityRoutingError(#[from] AvailabilityRoutingError),

    #[error(transparent)]
    PolicyError(#[from] PolicyError),

    #[error(
        "source capability at component {moniker} is void. \
        If the offer/expose declaration has `source_availability` set to `unknown`, \
        the source component instance likely isn't defined in the component declaration"
    )]
    SourceCapabilityIsVoid { moniker: ExtendedMoniker },

    #[error(
        "routes that do not set the `debug` flag are unsupported in the current configuration (at `{moniker}`)."
    )]
    NonDebugRoutesUnsupported { moniker: ExtendedMoniker },

    #[error("debug routes are unsupported for external routers (at `{moniker}`).")]
    DebugRoutesUnsupported { moniker: ExtendedMoniker },

    #[error("{type_name} router unexpectedly returned unavailable for target {moniker}")]
    RouteUnexpectedUnavailable { type_name: CapabilityTypeName, moniker: ExtendedMoniker },

    #[error("path at `{moniker}` was too long for `{keyword}`: {path}")]
    PathTooLong { moniker: ExtendedMoniker, path: String, keyword: String },

    #[error(
        "conflicting dictionary entries detected component `{moniker}`: {}",
        conflicting_names.iter().map(|n| format!("{}", n)).join(", ")
    )]
    ConflictingDictionaryEntries { moniker: ExtendedMoniker, conflicting_names: Vec<Name> },

    #[error("FIDL error encountered while talking to a router implemented by component {moniker}")]
    RemoteFIDLError { moniker: Moniker },

    // We store the raw value of a zx::Status here because zx::Status does not implement Serialize
    #[error("error returned by a router implemented by component {moniker}")]
    RemoteRouterError { moniker: Moniker, error_code: i32 },
}

impl Explain for RoutingError {
    /// Convert this error into its approximate `zx::Status` equivalent.
    fn as_zx_status(&self) -> zx::Status {
        match self {
            RoutingError::UseFromRootEnvironmentNotAllowed { .. }
            | RoutingError::DynamicDictionariesNotAllowed { .. } => zx::Status::ACCESS_DENIED,
            RoutingError::RouteRequestMissingField { .. }
            | RoutingError::RouteRequestFailedToParseField { .. }
            | RoutingError::RouteRequestUnset { .. }
            | RoutingError::ComponentNotInIdIndex { .. }
            | RoutingError::ConflictingDictionaryEntries { .. }
            | RoutingError::ExposeFromFrameworkNotFound { .. }
            | RoutingError::UseFromRootExposeNotFound { .. }
            | RoutingError::UnsupportedRouteSource { .. }
            | RoutingError::UnsupportedCapabilityType { .. }
            | RoutingError::EventsRoutingError(_)
            | RoutingError::BedrockNotPresentInDictionary { .. }
            | RoutingError::BedrockFailedToSend { .. }
            | RoutingError::SourceUnknown { .. }
            | RoutingError::RouteSourceShutdown { .. }
            | RoutingError::BedrockWrongCapabilityType { .. }
            | RoutingError::SourceCapabilityIsVoid { .. }
            | RoutingError::AvailabilityRoutingError(_)
            | RoutingError::RouteSourceNotFound { .. }
            | RoutingError::PathTooLong { .. } => zx::Status::NOT_FOUND,
            RoutingError::NonDebugRoutesUnsupported { .. }
            | RoutingError::DebugRoutesUnsupported { .. } => zx::Status::NOT_SUPPORTED,
            RoutingError::ComponentInstanceError(err) => err.as_zx_status(),
            RoutingError::RightsRoutingError(err) => err.as_zx_status(),
            RoutingError::PolicyError(err) => err.as_zx_status(),
            RoutingError::RouteUnexpectedUnavailable { .. } => zx::Status::INTERNAL,
            RoutingError::RemoteFIDLError { .. } => zx::Status::PEER_CLOSED,
            RoutingError::RemoteRouterError { error_code, .. } => {
                zx::Status::err_from_raw(*error_code)
            }
        }
    }
}

impl From<RoutingError> for ExtendedMoniker {
    fn from(err: RoutingError) -> ExtendedMoniker {
        match err {
            RoutingError::ComponentNotInIdIndex { source_moniker: moniker, .. }
            | RoutingError::ExposeFromFrameworkNotFound { moniker, .. }
            | RoutingError::UseFromRootEnvironmentNotAllowed { moniker, .. }
            | RoutingError::DynamicDictionariesNotAllowed { moniker, .. }
            | RoutingError::RouteSourceShutdown { moniker }
            | RoutingError::RemoteFIDLError { moniker }
            | RoutingError::RouteSourceNotFound { moniker, .. }
            | RoutingError::RemoteRouterError { moniker, .. } => moniker.into(),
            RoutingError::PathTooLong { moniker, .. } => moniker,

            RoutingError::BedrockNotPresentInDictionary { moniker, .. }
            | RoutingError::BedrockFailedToSend { moniker, .. }
            | RoutingError::SourceUnknown { moniker, .. }
            | RoutingError::BedrockWrongCapabilityType { moniker, .. }
            | RoutingError::RouteRequestMissingField { moniker, .. }
            | RoutingError::RouteRequestFailedToParseField { moniker, .. }
            | RoutingError::RouteRequestUnset { moniker, .. }
            | RoutingError::SourceCapabilityIsVoid { moniker, .. }
            | RoutingError::ConflictingDictionaryEntries { moniker, .. }
            | RoutingError::NonDebugRoutesUnsupported { moniker }
            | RoutingError::DebugRoutesUnsupported { moniker }
            | RoutingError::RouteUnexpectedUnavailable { moniker, .. }
            | RoutingError::UnsupportedCapabilityType { moniker, .. }
            | RoutingError::UnsupportedRouteSource { moniker, .. } => moniker,
            RoutingError::AvailabilityRoutingError(err) => err.into(),
            RoutingError::ComponentInstanceError(err) => err.into(),
            RoutingError::EventsRoutingError(err) => err.into(),
            RoutingError::PolicyError(err) => err.into(),
            RoutingError::RightsRoutingError(err) => err.into(),

            RoutingError::UseFromRootExposeNotFound { .. } => ExtendedMoniker::ComponentManager,
        }
    }
}

impl From<RoutingError> for RouterError {
    fn from(value: RoutingError) -> Self {
        Self::NotFound(Arc::new(value))
    }
}

impl TryFrom<RouterError> for RoutingError {
    type Error = RouterError;

    fn try_from(value: RouterError) -> Result<Self, Self::Error> {
        match value {
            RouterError::NotFound(arc_dyn_explain) => {
                match arc_dyn_explain.as_any().downcast_ref::<Self>() {
                    Some(routing_error) => Ok(routing_error.clone()),
                    None => Err(RouterError::NotFound(arc_dyn_explain)),
                }
            }
            err => Err(err),
        }
    }
}

impl RoutingError {
    /// Convert this error into its approximate `fuchsia.component.Error` equivalent.
    pub fn as_fidl_error(&self) -> fcomponent::Error {
        fcomponent::Error::ResourceUnavailable
    }

    pub fn expose_from_framework_not_found(
        moniker: &Moniker,
        capability_id: impl Into<String>,
    ) -> Self {
        Self::ExposeFromFrameworkNotFound {
            moniker: moniker.clone(),
            capability_id: capability_id.into(),
        }
    }

    pub fn unsupported_route_source(
        moniker: impl Into<ExtendedMoniker>,
        source: impl Into<String>,
    ) -> Self {
        Self::UnsupportedRouteSource { source_type: source.into(), moniker: moniker.into() }
    }

    pub fn unsupported_capability_type(
        moniker: impl Into<ExtendedMoniker>,
        type_name: impl Into<CapabilityTypeName>,
    ) -> Self {
        Self::UnsupportedCapabilityType { type_name: type_name.into(), moniker: moniker.into() }
    }
}

/// Errors produced during routing specific to events.
#[cfg_attr(feature = "serde", derive(Deserialize, Serialize), serde(rename_all = "snake_case"))]
#[derive(Error, Debug, Clone, PartialEq)]
pub enum EventsRoutingError {
    #[error("filter is not a subset at `{moniker}`")]
    InvalidFilter { moniker: ExtendedMoniker },

    #[error("event routes must end at source with a filter declaration at `{moniker}`")]
    MissingFilter { moniker: ExtendedMoniker },
}

impl From<EventsRoutingError> for ExtendedMoniker {
    fn from(err: EventsRoutingError) -> ExtendedMoniker {
        match err {
            EventsRoutingError::InvalidFilter { moniker }
            | EventsRoutingError::MissingFilter { moniker } => moniker,
        }
    }
}

#[cfg_attr(feature = "serde", derive(Deserialize, Serialize), serde(rename_all = "snake_case"))]
#[derive(Debug, Error, Clone, PartialEq)]
pub enum RightsRoutingError {
    #[error(
        "requested rights ({requested}) greater than provided rights ({provided}) at \"{moniker}\""
    )]
    Invalid { moniker: ExtendedMoniker, requested: Rights, provided: Rights },

    #[error(
        "directory routes must end at source with a rights declaration, it's missing at \"{moniker}\""
    )]
    MissingRightsSource { moniker: ExtendedMoniker },
}

impl RightsRoutingError {
    /// Convert this error into its approximate `zx::Status` equivalent.
    pub fn as_zx_status(&self) -> zx::Status {
        match self {
            RightsRoutingError::Invalid { .. } => zx::Status::ACCESS_DENIED,
            RightsRoutingError::MissingRightsSource { .. } => zx::Status::NOT_FOUND,
        }
    }
}

impl From<RightsRoutingError> for ExtendedMoniker {
    fn from(err: RightsRoutingError) -> ExtendedMoniker {
        match err {
            RightsRoutingError::Invalid { moniker, .. }
            | RightsRoutingError::MissingRightsSource { moniker } => moniker,
        }
    }
}

#[cfg_attr(feature = "serde", derive(Deserialize, Serialize), serde(rename_all = "snake_case"))]
#[derive(Debug, Error, Clone, PartialEq)]
pub enum AvailabilityRoutingError {
    #[error(
        "availability requested by the target has stronger guarantees than what \
    is being provided at the source at `{moniker}`"
    )]
    TargetHasStrongerAvailability { moniker: ExtendedMoniker },

    #[error("offer uses void source, but target requires the capability at `{moniker}`")]
    OfferFromVoidToRequiredTarget { moniker: ExtendedMoniker },

    #[error("expose uses void source, but target requires the capability at `{moniker}`")]
    ExposeFromVoidToRequiredTarget { moniker: ExtendedMoniker },
}

impl From<availability::TargetHasStrongerAvailability> for AvailabilityRoutingError {
    fn from(value: availability::TargetHasStrongerAvailability) -> Self {
        let availability::TargetHasStrongerAvailability { moniker } = value;
        AvailabilityRoutingError::TargetHasStrongerAvailability { moniker }
    }
}

impl From<AvailabilityRoutingError> for ExtendedMoniker {
    fn from(err: AvailabilityRoutingError) -> ExtendedMoniker {
        match err {
            AvailabilityRoutingError::ExposeFromVoidToRequiredTarget { moniker }
            | AvailabilityRoutingError::OfferFromVoidToRequiredTarget { moniker }
            | AvailabilityRoutingError::TargetHasStrongerAvailability { moniker } => moniker,
        }
    }
}

// Implements error reporting upon routing failure. For example, component
// manager logs the error.
#[async_trait]
pub trait ErrorReporter: Clone + Send + Sync + 'static {
    async fn report(
        &self,
        request: &RouteRequestErrorInfo,
        err: &RouterError,
        route_target: Arc<runtime_capabilities::WeakInstanceToken>,
    );
}

/// What to print in an error if a route request fails.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RouteRequestErrorInfo {
    capability_type: cm_rust::CapabilityTypeName,
    name: cm_types::Name,
    availability: cm_rust::Availability,
}

impl RouteRequestErrorInfo {
    pub fn availability(&self) -> cm_rust::Availability {
        self.availability
    }

    pub fn name(&self) -> &Name {
        &self.name
    }

    pub fn type_name(&self) -> &CapabilityTypeName {
        &self.capability_type
    }

    pub fn for_builtin(capability_type: CapabilityTypeName, name: &Name) -> Self {
        Self { capability_type, name: name.clone(), availability: Availability::Required }
    }
}

impl From<&RouteRequestErrorInfo> for runtime_capabilities::RouterErrorInfo {
    fn from(value: &RouteRequestErrorInfo) -> Self {
        Self {
            capability_type: value.capability_type,
            name: value.name.clone(),
            availability: value.availability,
        }
    }
}

impl From<runtime_capabilities::RouterErrorInfo> for RouteRequestErrorInfo {
    fn from(value: runtime_capabilities::RouterErrorInfo) -> Self {
        Self {
            capability_type: value.capability_type,
            name: value.name,
            availability: value.availability,
        }
    }
}

impl From<&cm_rust::UseDecl> for RouteRequestErrorInfo {
    fn from(value: &cm_rust::UseDecl) -> Self {
        RouteRequestErrorInfo {
            capability_type: value.into(),
            name: value.source_name().clone(),
            availability: value.availability().clone(),
        }
    }
}

impl From<&cm_rust::UseConfigurationDecl> for RouteRequestErrorInfo {
    fn from(value: &cm_rust::UseConfigurationDecl) -> Self {
        RouteRequestErrorInfo {
            capability_type: CapabilityTypeName::Config,
            name: value.source_name().clone(),
            availability: value.availability().clone(),
        }
    }
}

impl From<&cm_rust::UseEventStreamDecl> for RouteRequestErrorInfo {
    fn from(value: &cm_rust::UseEventStreamDecl) -> Self {
        RouteRequestErrorInfo {
            capability_type: CapabilityTypeName::EventStream,
            name: value.source_name.clone(),
            availability: value.availability,
        }
    }
}

impl From<&cm_rust::ExposeDecl> for RouteRequestErrorInfo {
    fn from(value: &cm_rust::ExposeDecl) -> Self {
        RouteRequestErrorInfo {
            capability_type: value.into(),
            name: value.target_name().clone(),
            availability: value.availability().clone(),
        }
    }
}

impl From<&cm_rust::offer::OfferDecl> for RouteRequestErrorInfo {
    fn from(value: &cm_rust::offer::OfferDecl) -> Self {
        RouteRequestErrorInfo {
            capability_type: value.into(),
            name: value.target_name().clone(),
            availability: value.availability().clone(),
        }
    }
}

impl From<&cm_rust::ResolverRegistration> for RouteRequestErrorInfo {
    fn from(value: &cm_rust::ResolverRegistration) -> Self {
        RouteRequestErrorInfo {
            capability_type: CapabilityTypeName::Resolver,
            name: value.source_name().clone(),
            availability: Availability::Required,
        }
    }
}

impl From<&cm_rust::RunnerRegistration> for RouteRequestErrorInfo {
    fn from(value: &cm_rust::RunnerRegistration) -> Self {
        RouteRequestErrorInfo {
            capability_type: CapabilityTypeName::Runner,
            name: value.source_name().clone(),
            availability: Availability::Required,
        }
    }
}

impl From<&cm_rust::DebugRegistration> for RouteRequestErrorInfo {
    fn from(value: &cm_rust::DebugRegistration) -> Self {
        RouteRequestErrorInfo {
            capability_type: CapabilityTypeName::Protocol,
            name: value.source_name().clone(),
            availability: Availability::Required,
        }
    }
}

impl From<&cm_rust::CapabilityDecl> for RouteRequestErrorInfo {
    fn from(value: &cm_rust::CapabilityDecl) -> Self {
        RouteRequestErrorInfo {
            capability_type: value.into(),
            name: value.name().clone(),
            availability: Availability::Required,
        }
    }
}

impl std::fmt::Display for RouteRequestErrorInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} `{}`", self.capability_type, self.name)
    }
}
