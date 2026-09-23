// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use ::routing::error::{ComponentInstanceError, RoutingError};
use ::routing::policy::PolicyError;
use ::routing::resolving::ResolverError;
use anyhow::Error;
use clonable_error::ClonableError;
use cm_config::CompatibilityCheckError;
use cm_rust::UseDecl;
use cm_types::{Name, Url};
use component_id_index::InstanceId;
use fidl_fuchsia_component as fcomponent;
use fidl_fuchsia_sys2 as fsys;
use fuchsia_fs::directory::WatcherCreateError;
use moniker::{ChildName, ExtendedMoniker, Moniker, MonikerError};
use router_error::{Explain, RouterError};
use runtime_capabilities::ConversionError;
use serve_processargs::BuildNamespaceError;
use std::sync::Arc;
use thiserror::Error;

/// Errors produced by `Model`.
#[derive(Debug, Error, Clone)]
pub enum ModelError {
    #[error("bad path")]
    BadPath,
    #[error(transparent)]
    MonikerError {
        #[from]
        err: MonikerError,
    },
    #[error("expected a component instance moniker")]
    UnexpectedComponentManagerMoniker,
    #[error(transparent)]
    RoutingError {
        #[from]
        err: RoutingError,
    },
    #[error(
        "opening path `{path}`, in storage directory for `{moniker}` backed by `{source_moniker}`: {err}"
    )]
    OpenStorageFailed {
        source_moniker: ExtendedMoniker,
        moniker: Moniker,
        path: String,
        #[source]
        err: zx::Status,
    },
    #[error(transparent)]
    StorageError {
        #[from]
        err: StorageError,
    },
    #[error(transparent)]
    ComponentInstanceError {
        #[from]
        err: ComponentInstanceError,
    },
    #[error("service dir VFS for component {moniker}:\n\t{err}")]
    ServiceDirError {
        moniker: Moniker,

        #[source]
        err: VfsError,
    },
    #[error("opening directory `{relative_path}` for component `{moniker}` failed")]
    OpenDirectoryError { moniker: Moniker, relative_path: String },
    #[error("events: {err}")]
    EventsError {
        #[from]
        err: EventsError,
    },
    #[error(transparent)]
    PolicyError {
        #[from]
        err: PolicyError,
    },
    #[error("component id index: {err}")]
    ComponentIdIndexError {
        #[from]
        err: component_id_index::IndexError,
    },
    #[error(transparent)]
    ActionError {
        #[from]
        err: ActionError,
    },
    #[error("resolve: {err}")]
    ResolveActionError {
        #[from]
        err: ResolveActionError,
    },
    #[error("start: {err}")]
    StartActionError {
        #[from]
        err: StartActionError,
    },
    #[error("open outgoing dir: {err}")]
    OpenOutgoingDirError {
        #[from]
        err: OpenOutgoingDirError,
    },
    #[error("router: {err}")]
    RouterError {
        #[from]
        err: RouterError,
    },
    #[error("capability provider: {err}")]
    CapabilityProviderError {
        #[from]
        err: CapabilityProviderError,
    },
    #[error("open: {err}")]
    OpenError {
        #[from]
        err: OpenError,
    },
}

impl ModelError {
    pub fn instance_not_found(moniker: Moniker) -> ModelError {
        ModelError::from(ComponentInstanceError::instance_not_found(moniker))
    }

    pub fn open_directory_error(moniker: Moniker, relative_path: impl Into<String>) -> ModelError {
        ModelError::OpenDirectoryError { moniker, relative_path: relative_path.into() }
    }
}

impl ModelError {
    fn as_zx_status(&self) -> zx::Status {
        match self {
            ModelError::RoutingError { err } => err.as_zx_status(),
            ModelError::PolicyError { err } => err.as_zx_status(),
            ModelError::StartActionError { err } => err.as_zx_status(),
            ModelError::ComponentInstanceError { err } => err.as_zx_status(),
            ModelError::OpenOutgoingDirError { err } => err.as_zx_status(),
            ModelError::RouterError { err } => err.as_zx_status(),
            ModelError::CapabilityProviderError { err } => err.as_zx_status(),
            // Any other type of error is not expected.
            _ => zx::Status::INTERNAL,
        }
    }
}

#[derive(Debug, Error, Clone)]
pub enum StructuredConfigError {
    #[error("component has a config schema but resolver did not provide values")]
    ConfigValuesMissing,
    #[error("failed to resolve component's config:\n\t{_0}")]
    ConfigResolutionFailed(#[source] config_encoder::ResolutionError),
    #[error("couldn't create vmo: {_0}")]
    VmoCreateFailed(#[source] zx::Status),
    #[error("failed to match values for key `{key}`")]
    ValueMismatch { key: String },
    #[error("failed to find values for key `{key}`")]
    KeyNotFound { key: String },
    #[error("failed to route structured config values:\n\t{_0}")]
    RoutingError(#[from] router_error::RouterError),
}

#[derive(Clone, Debug, Error)]
pub enum VfsError {
    #[error("failed to add node `{name}`: {status}")]
    AddNodeError { name: String, status: zx::Status },
    #[error("failed to remove node `{name}`: {status}")]
    RemoveNodeError { name: String, status: zx::Status },
}

#[derive(Debug, Error)]
pub enum RebootError {
    #[error("failed to connect to admin protocol in root component's exposed dir:\n\t{0}")]
    ConnectToAdminFailed(#[source] anyhow::Error),
    #[error("StateControl Admin FIDL:\n\t{0}")]
    FidlError(#[from] fidl::Error),
    #[error("StateControl Admin: {0}")]
    AdminError(zx::Status),
    #[error("opening root component's exposed dir: {0}")]
    OpenRootExposedDirFailed(#[from] OpenExposedDirError),
}

#[derive(Debug, Error)]
pub enum OpenExposedDirError {
    #[error("instance is not resolved")]
    InstanceNotResolved,
    #[error("instance was destroyed")]
    InstanceDestroyed,
    #[error("open error: {0}")]
    Open(#[from] zx::Status),
}

impl Explain for OpenExposedDirError {
    fn as_zx_status(&self) -> zx::Status {
        match self {
            Self::InstanceNotResolved => zx::Status::NOT_FOUND,
            Self::InstanceDestroyed => zx::Status::NOT_FOUND,
            Self::Open(status) => *status,
        }
    }
}

impl From<OpenExposedDirError> for fsys::OpenError {
    fn from(value: OpenExposedDirError) -> Self {
        match value {
            OpenExposedDirError::InstanceNotResolved => fsys::OpenError::InstanceNotResolved,
            OpenExposedDirError::InstanceDestroyed => fsys::OpenError::InstanceDestroyed,
            OpenExposedDirError::Open(_) => fsys::OpenError::FidlError,
        }
    }
}

#[derive(Clone, Debug, Error)]
pub enum OpenOutgoingDirError {
    #[error("instance is not resolved")]
    InstanceNotResolved,
    #[error("instance is non-executable")]
    InstanceNonExecutable,
    #[error("failed to open: {0}")]
    Open(#[from] zx::Status),
    #[error("fidl IPC to protocol in outgoing directory:\n\t{0}")]
    Fidl(fidl::Error),
}

impl Explain for OpenOutgoingDirError {
    fn as_zx_status(&self) -> zx::Status {
        match self {
            Self::InstanceNotResolved => zx::Status::NOT_FOUND,
            Self::InstanceNonExecutable => zx::Status::NOT_FOUND,
            Self::Open(err) => *err,
            Self::Fidl(_) => zx::Status::NOT_FOUND,
        }
    }
}

impl From<OpenOutgoingDirError> for fsys::OpenError {
    fn from(value: OpenOutgoingDirError) -> Self {
        match value {
            OpenOutgoingDirError::InstanceNotResolved => fsys::OpenError::InstanceNotResolved,
            OpenOutgoingDirError::InstanceNonExecutable => fsys::OpenError::NoSuchDir,
            OpenOutgoingDirError::Open(_) => fsys::OpenError::FidlError,
            OpenOutgoingDirError::Fidl(_) => fsys::OpenError::FidlError,
        }
    }
}

impl From<OpenOutgoingDirError> for RouterError {
    fn from(value: OpenOutgoingDirError) -> Self {
        Self::NotFound(Arc::new(value))
    }
}

#[derive(Debug, Error, Clone)]
pub enum AddDynamicChildError {
    #[error("component collection not found with name `{name}`")]
    CollectionNotFound { name: String },
    #[error(
        "numbered handles can only be provided when adding components to a single-run collection"
    )]
    NumberedHandleNotInSingleRunCollection,
    #[error("name length is longer than the allowed max of {max_len}")]
    NameTooLong { max_len: usize },
    #[error("collection `{collection_name}` does not allow dynamic offers")]
    DynamicOffersNotAllowed { collection_name: String },
    #[error(transparent)]
    ActionError {
        #[from]
        err: ActionError,
    },
    #[error("invalid dictionary")]
    InvalidDictionary,
    #[error(
        "dictionary entry for capability `{capability_name}` conflicts with existing static route"
    )]
    StaticRouteConflict { capability_name: Name },
    #[error(transparent)]
    AddChildError {
        #[from]
        err: AddChildError,
    },
    #[error("cannot set both `dictionary` and `additional_inputs`")]
    DictionaryAndAdditionalInputsSet,
}

// This is implemented for fuchsia.component.Realm protocol
impl Into<fcomponent::Error> for AddDynamicChildError {
    fn into(self) -> fcomponent::Error {
        match self {
            AddDynamicChildError::CollectionNotFound { .. } => {
                fcomponent::Error::CollectionNotFound
            }
            AddDynamicChildError::NumberedHandleNotInSingleRunCollection => {
                fcomponent::Error::Unsupported
            }
            AddDynamicChildError::AddChildError {
                err: AddChildError::InstanceAlreadyExists { .. },
            } => fcomponent::Error::InstanceAlreadyExists,
            AddDynamicChildError::DynamicOffersNotAllowed { .. } => {
                fcomponent::Error::InvalidArguments
            }
            AddDynamicChildError::ActionError { err } => err.into(),
            AddDynamicChildError::InvalidDictionary { .. } => fcomponent::Error::InvalidArguments,
            AddDynamicChildError::StaticRouteConflict { .. } => fcomponent::Error::InvalidArguments,
            AddDynamicChildError::NameTooLong { .. } => fcomponent::Error::InvalidArguments,
            // TODO(https://fxbug.dev/297403341): This should become its own error in fidl once
            // we can introduce it without breaking compatibility.
            AddDynamicChildError::AddChildError {
                err:
                    AddChildError::DynamicCapabilityError { err: DynamicCapabilityError::Cycle { .. } },
            } => fcomponent::Error::DependencyCycle,
            AddDynamicChildError::AddChildError {
                err: AddChildError::DynamicCapabilityError { .. },
            } => fcomponent::Error::InvalidArguments,
            AddDynamicChildError::AddChildError { err: AddChildError::ChildNameInvalid { .. } } => {
                fcomponent::Error::InvalidArguments
            }
            AddDynamicChildError::DictionaryAndAdditionalInputsSet => {
                fcomponent::Error::InvalidArguments
            }
        }
    }
}

// This is implemented for fuchsia.sys2.LifecycleController protocol
impl Into<fsys::CreateError> for AddDynamicChildError {
    fn into(self) -> fsys::CreateError {
        match self {
            AddDynamicChildError::CollectionNotFound { .. } => {
                fsys::CreateError::CollectionNotFound
            }
            AddDynamicChildError::AddChildError {
                err: AddChildError::InstanceAlreadyExists { .. },
            } => fsys::CreateError::InstanceAlreadyExists,

            AddDynamicChildError::DynamicOffersNotAllowed { .. } => {
                fsys::CreateError::DynamicOffersForbidden
            }
            AddDynamicChildError::ActionError { .. } => fsys::CreateError::Internal,
            AddDynamicChildError::InvalidDictionary { .. } => fsys::CreateError::Internal,
            AddDynamicChildError::StaticRouteConflict { .. } => fsys::CreateError::Internal,
            AddDynamicChildError::NameTooLong { .. } => fsys::CreateError::BadChildDecl,
            AddDynamicChildError::AddChildError {
                err: AddChildError::DynamicCapabilityError { .. },
            } => fsys::CreateError::BadDynamicOffer,
            AddDynamicChildError::AddChildError { err: AddChildError::ChildNameInvalid { .. } } => {
                fsys::CreateError::BadMoniker
            }
            AddDynamicChildError::NumberedHandleNotInSingleRunCollection => {
                fsys::CreateError::NumberedHandlesForbidden
            }
            AddDynamicChildError::DictionaryAndAdditionalInputsSet => {
                fsys::CreateError::DictionaryAndAdditionalInputsSet
            }
        }
    }
}

#[derive(Debug, Error, Clone)]
pub enum AddChildError {
    #[error("component instance `{child}` in realm `{moniker}` already exists")]
    InstanceAlreadyExists { moniker: Moniker, child: ChildName },
    #[error(transparent)]
    DynamicCapabilityError {
        #[from]
        err: DynamicCapabilityError,
    },
    #[error("invalid child name: {err}")]
    ChildNameInvalid {
        #[from]
        err: MonikerError,
    },
}

#[derive(Debug, Error, Clone, PartialEq)]
pub enum DynamicCapabilityError {
    #[error("a dynamic capability was not valid:\n\t{err}")]
    Invalid {
        #[source]
        err: Box<cm_fidl_validator::error::ErrorList>,
    },
    #[error("dynamic offers are not allowed for {typename}")]
    UnsupportedType { typename: &'static str },
    #[error("dynamic offer would create a cycle:\n\t{err}")]
    Cycle {
        #[source]
        err: Box<cm_fidl_validator::error::ErrorList>,
    },
    #[error("source for dynamic offer not found:\n\t{:?}", offer)]
    SourceNotFound { offer: cm_rust::offer::OfferDecl },
    #[error("unknown offer type in dynamic offers")]
    UnknownOfferType,
}

#[derive(Debug, Clone, Error)]
#[error(transparent)]
pub struct ActionError {
    kind: Box<ActionErrorKind>,
}

impl ActionError {
    pub fn kind(&self) -> &ActionErrorKind {
        &self.kind
    }
}

impl From<ActionErrorKind> for ActionError {
    fn from(err: ActionErrorKind) -> Self {
        ActionError { kind: Box::new(err) }
    }
}

macro_rules! impl_from_action_error_kind {
    ($($ty:ty,)+) => {
        $(
            impl From<$ty> for ActionError {
                fn from(err: $ty) -> Self {
                    ActionError::from(ActionErrorKind::from(err))
                }
            }
        )+
    }
}

impl_from_action_error_kind! {
    DiscoverActionError,
    ResolveActionError,
    UnresolveActionError,
    StartActionError,
    StopActionError,
    DestroyActionError,
    ShutdownActionError,
}

#[derive(Debug, Clone, Error)]
pub enum ActionErrorKind {
    #[error("discover: {err}")]
    DiscoverError {
        #[from]
        err: DiscoverActionError,
    },

    #[error("resolve: {err}")]
    ResolveError {
        #[from]
        err: ResolveActionError,
    },

    #[error("unresolve: {err}")]
    UnresolveError {
        #[from]
        err: UnresolveActionError,
    },

    #[error("start: {err}")]
    StartError {
        #[from]
        err: StartActionError,
    },

    #[error("stop: {err}")]
    StopError {
        #[from]
        err: StopActionError,
    },

    #[error("destroy: {err}")]
    DestroyError {
        #[from]
        err: DestroyActionError,
    },

    #[error("shutdown: {err}")]
    ShutdownError {
        #[from]
        err: ShutdownActionError,
    },
}

impl Explain for ActionError {
    fn as_zx_status(&self) -> zx::Status {
        match &*self.kind {
            ActionErrorKind::DiscoverError { .. } => zx::Status::INTERNAL,
            ActionErrorKind::ResolveError { err } => err.as_zx_status(),
            ActionErrorKind::UnresolveError { .. } => zx::Status::INTERNAL,
            ActionErrorKind::StartError { err } => err.as_zx_status(),
            ActionErrorKind::StopError { .. } => zx::Status::INTERNAL,
            ActionErrorKind::DestroyError { .. } => zx::Status::INTERNAL,
            ActionErrorKind::ShutdownError { .. } => zx::Status::INTERNAL,
        }
    }
}

impl From<ActionError> for fcomponent::Error {
    fn from(err: ActionError) -> Self {
        match *err.kind {
            ActionErrorKind::DiscoverError { .. } => fcomponent::Error::Internal,
            ActionErrorKind::ResolveError { .. } => fcomponent::Error::Internal,
            ActionErrorKind::UnresolveError { .. } => fcomponent::Error::Internal,
            ActionErrorKind::StartError { err } => err.into(),
            ActionErrorKind::StopError { err } => err.into(),
            ActionErrorKind::DestroyError { err } => err.into(),
            ActionErrorKind::ShutdownError { .. } => fcomponent::Error::Internal,
        }
    }
}

impl From<ActionError> for fsys::ResolveError {
    fn from(err: ActionError) -> Self {
        match *err.kind {
            ActionErrorKind::ResolveError { err } => err.into(),
            _ => fsys::ResolveError::Internal,
        }
    }
}

impl From<ActionError> for fsys::UnresolveError {
    fn from(err: ActionError) -> Self {
        match *err.kind {
            ActionErrorKind::UnresolveError { err } => err.into(),
            _ => fsys::UnresolveError::Internal,
        }
    }
}

impl From<ActionError> for fsys::StartError {
    fn from(err: ActionError) -> Self {
        match *err.kind {
            ActionErrorKind::StartError { err } => err.into(),
            _ => fsys::StartError::Internal,
        }
    }
}

impl From<ActionError> for fsys::StopError {
    fn from(err: ActionError) -> Self {
        match *err.kind {
            ActionErrorKind::StopError { err } => err.into(),
            _ => fsys::StopError::Internal,
        }
    }
}

impl From<ActionError> for fsys::DestroyError {
    fn from(err: ActionError) -> Self {
        match *err.kind {
            ActionErrorKind::DestroyError { err } => err.into(),
            _ => fsys::DestroyError::Internal,
        }
    }
}

#[derive(Debug, Clone, Error)]
pub enum DiscoverActionError {
    #[error("`{moniker}` was destroyed")]
    InstanceDestroyed { moniker: Moniker },
}

#[derive(Debug, Clone, Error)]
pub enum ResolveActionError {
    #[error("discover during resolve: {err}")]
    DiscoverActionError {
        #[from]
        err: DiscoverActionError,
    },
    #[error("`{moniker}` was shut down")]
    InstanceShutDown { moniker: Moniker },
    #[error("`{moniker}` was destroyed")]
    InstanceDestroyed { moniker: Moniker },
    #[error("could not parse component address for `{url}` at `{moniker}`:\n\t{err}")]
    ComponentAddressParseError {
        url: Url,
        moniker: Moniker,
        #[source]
        err: Box<ResolverError>,
    },
    #[error("resolve failed for `{url}`:\n\t{err}")]
    ResolverError {
        url: Url,
        #[source]
        err: Box<ResolverError>,
    },
    #[error("expose dir for `{moniker}`:\n\t{err}")]
    // TODO(https://fxbug.dev/42071713): Determine whether this is expected to fail.
    ExposeDirError {
        moniker: Moniker,

        #[source]
        err: VfsError,
    },
    #[error("adding static child `{child_name}`:\n\t{err}")]
    AddStaticChildError {
        child_name: String,
        #[source]
        err: AddChildError,
    },
    #[error("structured config: {err}")]
    StructuredConfigError {
        #[from]
        err: StructuredConfigError,
    },
    #[error("creating package dir proxy: {err}")]
    PackageDirProxyCreateError {
        #[source]
        err: fidl::Error,
    },
    #[error("ABI compatibility check for `{url}`: {err}")]
    AbiCompatibilityError {
        url: Url,
        #[source]
        err: Box<CompatibilityCheckError>,
    },
    #[error(transparent)]
    Policy(#[from] PolicyError),
    #[error("`{moniker}` was interrupted")]
    Aborted { moniker: Moniker },
}

impl ResolveActionError {
    fn as_zx_status(&self) -> zx::Status {
        match self {
            ResolveActionError::DiscoverActionError { .. }
            | ResolveActionError::InstanceShutDown { .. }
            | ResolveActionError::InstanceDestroyed { .. }
            | ResolveActionError::ComponentAddressParseError { .. }
            | ResolveActionError::AbiCompatibilityError { .. } => zx::Status::NOT_FOUND,
            ResolveActionError::ExposeDirError { .. }
            | ResolveActionError::AddStaticChildError { .. }
            | ResolveActionError::StructuredConfigError { .. }
            | ResolveActionError::Aborted { .. }
            | ResolveActionError::PackageDirProxyCreateError { .. } => zx::Status::INTERNAL,
            ResolveActionError::ResolverError { err, .. } => err.as_zx_status(),
            ResolveActionError::Policy(err) => err.as_zx_status(),
        }
    }
}

// This is implemented for fuchsia.sys2.LifecycleController protocol
impl Into<fsys::ResolveError> for ResolveActionError {
    fn into(self) -> fsys::ResolveError {
        match self {
            ResolveActionError::ResolverError { err, .. } => match *err {
                ResolverError::PackageNotFound(_) => fsys::ResolveError::PackageNotFound,
                ResolverError::ManifestNotFound(_) => fsys::ResolveError::ManifestNotFound,
                _ => fsys::ResolveError::Internal,
            },
            ResolveActionError::InstanceShutDown { .. }
            | ResolveActionError::InstanceDestroyed { .. } => fsys::ResolveError::InstanceNotFound,
            ResolveActionError::ExposeDirError { .. }
            | ResolveActionError::StructuredConfigError { .. }
            | ResolveActionError::ComponentAddressParseError { .. }
            | ResolveActionError::AddStaticChildError { .. }
            | ResolveActionError::DiscoverActionError { .. }
            | ResolveActionError::AbiCompatibilityError { .. }
            | ResolveActionError::Aborted { .. }
            | ResolveActionError::PackageDirProxyCreateError { .. } => fsys::ResolveError::Internal,
            ResolveActionError::Policy(_) => fsys::ResolveError::PolicyError,
        }
    }
}

// This is implemented for fuchsia.sys2.LifecycleController protocol.
// Starting a component instance also causes a resolve.
impl Into<fsys::StartError> for ResolveActionError {
    fn into(self) -> fsys::StartError {
        match self {
            ResolveActionError::ResolverError { err, .. } => match *err {
                ResolverError::PackageNotFound(_) => fsys::StartError::PackageNotFound,
                ResolverError::ManifestNotFound(_) => fsys::StartError::ManifestNotFound,
                _ => fsys::StartError::Internal,
            },
            ResolveActionError::InstanceShutDown { .. }
            | ResolveActionError::InstanceDestroyed { .. } => fsys::StartError::InstanceNotFound,
            ResolveActionError::ExposeDirError { .. }
            | ResolveActionError::StructuredConfigError { .. }
            | ResolveActionError::ComponentAddressParseError { .. }
            | ResolveActionError::AddStaticChildError { .. }
            | ResolveActionError::DiscoverActionError { .. }
            | ResolveActionError::AbiCompatibilityError { .. }
            | ResolveActionError::Aborted { .. }
            | ResolveActionError::PackageDirProxyCreateError { .. } => fsys::StartError::Internal,
            ResolveActionError::Policy(_) => fsys::StartError::PolicyError,
        }
    }
}

#[derive(Debug, Clone, Error)]
pub enum PkgDirError {
    #[error("no pkg dir found for component")]
    NoPkgDir,
    #[error("opening pkg dir failed: {err}")]
    OpenFailed {
        #[from]
        err: zx::Status,
    },
}

impl PkgDirError {
    fn as_zx_status(&self) -> zx::Status {
        match self {
            Self::NoPkgDir => zx::Status::NOT_FOUND,
            Self::OpenFailed { err } => *err,
        }
    }
}

#[derive(Debug, Clone, Error)]
pub enum ComponentProviderError {
    #[error("starting source instance:\n\t{err}")]
    SourceStartError {
        #[from]
        err: ActionError,
    },
    #[error("opening source instance's outgoing dir:\n\t{err}")]
    OpenOutgoingDirError {
        #[from]
        err: OpenOutgoingDirError,
    },
}

impl ComponentProviderError {
    pub fn as_zx_status(&self) -> zx::Status {
        match self {
            Self::SourceStartError { err } => err.as_zx_status(),
            Self::OpenOutgoingDirError { err } => err.as_zx_status(),
        }
    }
}

#[derive(Debug, Clone, Error)]
pub enum CapabilityProviderError {
    #[error("bad path")]
    BadPath,
    #[error(transparent)]
    ComponentInstanceError {
        #[from]
        err: ComponentInstanceError,
    },
    #[error(transparent)]
    PkgDirError {
        #[from]
        err: PkgDirError,
    },
    #[error("event source: {0}")]
    EventSourceError(#[from] EventSourceError),
    #[error(transparent)]
    ComponentProviderError {
        #[from]
        err: ComponentProviderError,
    },
    #[error("component_manager namespace: {err}")]
    CmNamespaceError {
        #[from]
        err: ClonableError,
    },
    #[error("router: {err}")]
    RouterError {
        #[from]
        err: RouterError,
    },
    #[error(transparent)]
    RoutingError(#[from] RoutingError),
    #[error("opening vfs failed: {0}")]
    VfsOpenError(#[source] zx::Status),
}

impl CapabilityProviderError {
    pub fn as_zx_status(&self) -> zx::Status {
        match self {
            Self::BadPath => zx::Status::INVALID_ARGS,
            Self::ComponentInstanceError { err } => err.as_zx_status(),
            Self::CmNamespaceError { .. } => zx::Status::INTERNAL,
            Self::PkgDirError { err } => err.as_zx_status(),
            Self::EventSourceError(err) => err.as_zx_status(),
            Self::ComponentProviderError { err } => err.as_zx_status(),
            Self::RouterError { err } => err.as_zx_status(),
            Self::RoutingError(err) => err.as_zx_status(),
            Self::VfsOpenError(err) => *err,
        }
    }
}

#[derive(Debug, Clone, Error)]
pub enum OpenError {
    #[error("failed to get default capability provider: {err}")]
    GetDefaultProviderError {
        // TODO(https://fxbug.dev/42068065): This will get fixed when we untangle ModelError
        #[source]
        err: Box<ModelError>,
    },
    #[error("no capability provider found")]
    CapabilityProviderNotFound,
    #[error("capability provider: {err}")]
    CapabilityProviderError {
        #[from]
        err: CapabilityProviderError,
    },
    #[error("opening storage capability: {err}")]
    OpenStorageError {
        // TODO(https://fxbug.dev/42068065): This will get fixed when we untangle ModelError
        #[source]
        err: Box<ModelError>,
    },
    #[error("timed out opening capability")]
    Timeout,
    #[error("invalid path found")]
    BadPath,
    #[error("capability does not support opening: {0}")]
    DoesNotSupportOpen(ConversionError),
    #[error("failed to create directory watcher: {err}")]
    WatcherCreateError {
        #[from]
        err: WatcherCreateError,
    },
}

impl Explain for OpenError {
    fn as_zx_status(&self) -> zx::Status {
        match self {
            Self::GetDefaultProviderError { err } => err.as_zx_status(),
            Self::OpenStorageError { err } => err.as_zx_status(),
            Self::CapabilityProviderError { err } => err.as_zx_status(),
            Self::CapabilityProviderNotFound => zx::Status::NOT_FOUND,
            Self::Timeout => zx::Status::TIMED_OUT,
            Self::BadPath => zx::Status::BAD_PATH,
            Self::DoesNotSupportOpen(_) => zx::Status::NOT_SUPPORTED,
            Self::WatcherCreateError { err: WatcherCreateError::SendWatchRequest(_err) } => {
                zx::Status::PEER_CLOSED
            }
            Self::WatcherCreateError { err: WatcherCreateError::WatchError(status) } => *status,
            Self::WatcherCreateError { err: WatcherCreateError::ChannelConversion(status) } => {
                *status
            }
        }
    }
}

impl From<OpenError> for RouterError {
    fn from(value: OpenError) -> Self {
        Self::NotFound(Arc::new(value))
    }
}

#[derive(Debug, Clone, Error)]
pub enum StartActionError {
    #[error("`{moniker}` was shut down")]
    InstanceShutDown { moniker: Moniker },
    #[error("`{moniker}` was destroyed")]
    InstanceDestroyed { moniker: Moniker },
    #[error("`{moniker}` couldn't resolve during start: {err}")]
    ResolveActionError {
        moniker: Moniker,
        #[source]
        err: Box<ActionError>,
    },
    #[error("runner for `{moniker}` `{runner}` couldn't resolve: {err}")]
    ResolveRunnerError {
        moniker: Moniker,
        runner: Name,
        #[source]
        err: Box<RouterError>,
    },
    #[error(
        "`{moniker}` uses `\"on_terminate\": \"reboot\"` but is disallowed by policy:\n\t{err}"
    )]
    RebootOnTerminateForbidden {
        moniker: Moniker,
        #[source]
        err: PolicyError,
    },
    #[error("creating program input dictionary for `{moniker}`")]
    InputDictionaryError { moniker: Moniker },
    #[error("creating namespace: {0}")]
    CreateNamespaceError(#[from] CreateNamespaceError),
    #[error("starting program for `{moniker}`: {err}")]
    StartProgramError {
        moniker: Moniker,
        #[source]
        err: StartError,
    },
    #[error("structured configuration for `{moniker}`: {err}")]
    StructuredConfigError {
        moniker: Moniker,
        #[source]
        err: StructuredConfigError,
    },
    #[error("starting eager child of `{moniker}`: {err}")]
    EagerStartError {
        moniker: Moniker,
        #[source]
        err: Box<ActionError>,
    },
    #[error("`{moniker}` was interrupted")]
    Aborted { moniker: Moniker },
}

impl StartActionError {
    fn as_zx_status(&self) -> zx::Status {
        match self {
            StartActionError::InstanceDestroyed { .. } | Self::InstanceShutDown { .. } => {
                zx::Status::NOT_FOUND
            }
            StartActionError::StartProgramError { .. }
            | StartActionError::StructuredConfigError { .. }
            | StartActionError::EagerStartError { .. } => zx::Status::INTERNAL,
            StartActionError::RebootOnTerminateForbidden { err, .. } => err.as_zx_status(),
            StartActionError::ResolveRunnerError { err, .. } => err.as_zx_status(),
            StartActionError::CreateNamespaceError(err) => err.as_zx_status(),
            StartActionError::InputDictionaryError { .. } => zx::Status::NOT_FOUND,
            StartActionError::ResolveActionError { err, .. } => err.as_zx_status(),
            StartActionError::Aborted { .. } => zx::Status::NOT_FOUND,
        }
    }
}

// This is implemented for fuchsia.sys2.LifecycleController protocol.
impl Into<fsys::StartError> for StartActionError {
    fn into(self) -> fsys::StartError {
        match self {
            StartActionError::ResolveActionError { err, .. } => (*err).into(),
            StartActionError::InstanceDestroyed { .. } => fsys::StartError::InstanceNotFound,
            StartActionError::InstanceShutDown { .. } => fsys::StartError::InstanceNotFound,
            _ => fsys::StartError::Internal,
        }
    }
}

// This is implemented for fuchsia.component.Realm protocol.
impl Into<fcomponent::Error> for StartActionError {
    fn into(self) -> fcomponent::Error {
        match self {
            StartActionError::ResolveActionError { .. } => fcomponent::Error::InstanceCannotResolve,
            StartActionError::RebootOnTerminateForbidden { .. } => fcomponent::Error::AccessDenied,
            StartActionError::InstanceShutDown { .. } => fcomponent::Error::InstanceDied,
            StartActionError::InstanceDestroyed { .. } => fcomponent::Error::InstanceDied,
            _ => fcomponent::Error::InstanceCannotStart,
        }
    }
}

#[derive(Debug, Clone, Error)]
pub enum StopActionError {
    #[error("stopping program: {0}")]
    ProgramStopError(#[source] StopError),
    #[error("failed to get top instance")]
    GetTopInstanceFailed,
    #[error("failed to get parent instance")]
    GetParentFailed,
    #[error("failed to destroy dynamic children: {err}")]
    DestroyDynamicChildrenFailed { err: Box<ActionError> },
    #[error("resolution during stop: {err}")]
    ResolveActionError {
        #[source]
        err: Box<ActionError>,
    },
    #[error("started while shutdown was ongoing")]
    ComponentStartedDuringShutdown,
}

// This is implemented for fuchsia.sys2.LifecycleController protocol.
impl Into<fsys::StopError> for StopActionError {
    fn into(self) -> fsys::StopError {
        fsys::StopError::Internal
    }
}

impl Into<fcomponent::Error> for StopActionError {
    fn into(self) -> fcomponent::Error {
        fcomponent::Error::Internal
    }
}

#[cfg(test)]
impl PartialEq for StopActionError {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (StopActionError::ProgramStopError(_), StopActionError::ProgramStopError(_)) => true,
            (StopActionError::GetTopInstanceFailed, StopActionError::GetTopInstanceFailed) => true,
            (StopActionError::GetParentFailed, StopActionError::GetParentFailed) => true,
            (
                StopActionError::DestroyDynamicChildrenFailed { .. },
                StopActionError::DestroyDynamicChildrenFailed { .. },
            ) => true,
            (
                StopActionError::ResolveActionError { .. },
                StopActionError::ResolveActionError { .. },
            ) => true,
            _ => false,
        }
    }
}

#[derive(Debug, Clone, Error)]
pub enum ShutdownActionError {
    #[error("child name invalid: {}", err)]
    InvalidChildName {
        #[from]
        err: MonikerError,
    },
    #[error("cycles detected in graph")]
    CyclesDetected {},
}

#[derive(Debug, Clone, Error)]
pub enum DestroyActionError {
    #[error("discover during destroy: {}", err)]
    DiscoverActionError {
        #[from]
        err: DiscoverActionError,
    },
    #[error("shutdown during destroy: {}", err)]
    ShutdownFailed {
        #[source]
        err: Box<ActionError>,
    },
    #[error("could not find `{moniker}`")]
    InstanceNotFound { moniker: Moniker },
    #[error("`{moniker}` is not resolved")]
    InstanceNotResolved { moniker: Moniker },
}

// This is implemented for fuchsia.component.Realm protocol.
impl Into<fcomponent::Error> for DestroyActionError {
    fn into(self) -> fcomponent::Error {
        match self {
            DestroyActionError::InstanceNotFound { .. } => fcomponent::Error::InstanceNotFound,
            _ => fcomponent::Error::Internal,
        }
    }
}

// This is implemented for fuchsia.sys2.LifecycleController protocol.
impl Into<fsys::DestroyError> for DestroyActionError {
    fn into(self) -> fsys::DestroyError {
        match self {
            DestroyActionError::InstanceNotFound { .. } => fsys::DestroyError::InstanceNotFound,
            DestroyActionError::InstanceNotResolved { .. } => {
                fsys::DestroyError::InstanceNotResolved
            }
            _ => fsys::DestroyError::Internal,
        }
    }
}

#[derive(Debug, Clone, Error)]
pub enum UnresolveActionError {
    #[error("shutdown during unresolve: {err}")]
    ShutdownFailed {
        #[from]
        err: StopActionError,
    },
    #[error("`{moniker}` cannot be unresolved while it is running")]
    InstanceRunning { moniker: Moniker },
    #[error("`{moniker}` was destroyed")]
    InstanceDestroyed { moniker: Moniker },
}

// This is implemented for fuchsia.sys2.LifecycleController protocol.
impl Into<fsys::UnresolveError> for UnresolveActionError {
    fn into(self) -> fsys::UnresolveError {
        match self {
            UnresolveActionError::InstanceDestroyed { .. } => {
                fsys::UnresolveError::InstanceNotFound
            }
            _ => fsys::UnresolveError::Internal,
        }
    }
}

#[derive(Debug, Clone, Error)]
pub enum CreateNamespaceError {
    #[error("failed to clone pkg dir for {moniker}: {err}")]
    ClonePkgDirFailed {
        moniker: Moniker,
        #[source]
        err: fuchsia_fs::node::CloneError,
    },

    #[error("use decl without path cannot be installed into the namespace for {moniker}: {decl:?}")]
    UseDeclWithoutPath { moniker: Moniker, decl: UseDecl },

    #[error("instance not in index: {0}")]
    InstanceNotInInstanceIdIndex(#[from] RoutingError),

    #[error("building namespace for {moniker}: {err}")]
    BuildNamespaceError {
        moniker: Moniker,
        #[source]
        err: serve_processargs::BuildNamespaceError,
    },

    #[error("failed to convert namespace into directory for {moniker}: {err}")]
    ConvertToDirectory {
        moniker: Moniker,
        #[source]
        err: ClonableError,
    },

    #[error(transparent)]
    ComponentInstanceError(#[from] ComponentInstanceError),
}

impl CreateNamespaceError {
    fn as_zx_status(&self) -> zx::Status {
        match self {
            Self::ClonePkgDirFailed { .. } => zx::Status::INTERNAL,
            Self::UseDeclWithoutPath { .. } => zx::Status::NOT_FOUND,
            Self::InstanceNotInInstanceIdIndex(err) => err.as_zx_status(),
            Self::BuildNamespaceError { .. } => zx::Status::NOT_FOUND,
            Self::ConvertToDirectory { .. } => zx::Status::INTERNAL,
            Self::ComponentInstanceError(err) => err.as_zx_status(),
        }
    }
}

#[derive(Debug, Clone, Error)]
pub enum EventSourceError {
    #[error(transparent)]
    ComponentInstance(#[from] ComponentInstanceError),
    #[error(transparent)]
    // TODO(https://fxbug.dev/42068065): This will get fixed when we untangle ModelError
    Model(#[from] Box<ModelError>),
    #[error("event stream already consumed")]
    AlreadyConsumed,
}

impl EventSourceError {
    fn as_zx_status(&self) -> zx::Status {
        match self {
            Self::ComponentInstance(err) => err.as_zx_status(),
            Self::Model(err) => err.as_zx_status(),
            Self::AlreadyConsumed => zx::Status::INTERNAL,
        }
    }
}

#[derive(Debug, Error, Clone)]
pub enum EventsError {
    #[error("capability_requested event streams cannot be taken twice")]
    CapabilityRequestedStreamTaken,

    #[error("model not available")]
    ModelNotAvailable,

    #[error("instance shut down")]
    InstanceShutdown,

    #[error("instance destroyed")]
    InstanceDestroyed,

    #[error("registry not found")]
    RegistryNotFound,

    #[error("event `{event_name}` appears more than once in a subscription request")]
    DuplicateEvent { event_name: Name },

    #[error("events not available for subscription: `{names:?}`")]
    NotAvailable { names: Vec<Name> },
}

impl EventsError {
    pub fn duplicate_event(event_name: Name) -> Self {
        Self::DuplicateEvent { event_name }
    }

    pub fn not_available(names: Vec<Name>) -> Self {
        Self::NotAvailable { names }
    }
}

/// Errors related to isolated storage.
#[derive(Debug, Error, Clone)]
pub enum StorageError {
    #[error("opening directory from `{dir_source_moniker:?}` at `{dir_source_path}`:\n\t{err}")]
    OpenRoot {
        dir_source_moniker: Option<Moniker>,
        dir_source_path: cm_types::Path,
        #[source]
        err: ClonableError,
    },
    #[error(
        "opening isolated storage from `{dir_source_moniker:?}`'s for `{moniker}` at `{dir_source_path}` (instance_id={instance_id:?}):\n\t{err}"
    )]
    Open {
        dir_source_moniker: Option<Moniker>,
        dir_source_path: cm_types::Path,
        moniker: Moniker,
        instance_id: Option<InstanceId>,
        #[source]
        err: ClonableError,
    },
    #[error(
        "opening isolated storage from `{dir_source_moniker:?}` at `{dir_source_path}` for {instance_id}:\n\t{err}"
    )]
    OpenById {
        dir_source_moniker: Option<Moniker>,
        dir_source_path: cm_types::Path,
        instance_id: InstanceId,
        #[source]
        err: ClonableError,
    },
    #[error(
        "removing isolated storage from {dir_source_moniker:?} at `{dir_source_path}` for `{moniker}` (instance_id={instance_id:?}):n\t{err} "
    )]
    Remove {
        dir_source_moniker: Option<Moniker>,
        dir_source_path: cm_types::Path,
        moniker: Moniker,
        instance_id: Option<InstanceId>,
        #[source]
        err: ClonableError,
    },
    #[error("storage path for `{moniker}` (instance_id={instance_id:?}) is invalid")]
    InvalidStoragePath { moniker: Moniker, instance_id: Option<InstanceId> },
}

impl StorageError {
    pub fn open_root(
        dir_source_moniker: Option<Moniker>,
        dir_source_path: cm_types::Path,
        err: impl Into<Error>,
    ) -> Self {
        Self::OpenRoot { dir_source_moniker, dir_source_path, err: err.into().into() }
    }

    pub fn open(
        dir_source_moniker: Option<Moniker>,
        dir_source_path: cm_types::Path,
        moniker: Moniker,
        instance_id: Option<InstanceId>,
        err: impl Into<Error>,
    ) -> Self {
        Self::Open {
            dir_source_moniker,
            dir_source_path,
            moniker,
            instance_id,
            err: err.into().into(),
        }
    }

    pub fn open_by_id(
        dir_source_moniker: Option<Moniker>,
        dir_source_path: cm_types::Path,
        instance_id: InstanceId,
        err: impl Into<Error>,
    ) -> Self {
        Self::OpenById { dir_source_moniker, dir_source_path, instance_id, err: err.into().into() }
    }

    pub fn remove(
        dir_source_moniker: Option<Moniker>,
        dir_source_path: cm_types::Path,
        moniker: Moniker,
        instance_id: Option<InstanceId>,
        err: impl Into<Error>,
    ) -> Self {
        Self::Remove {
            dir_source_moniker,
            dir_source_path,
            moniker,
            instance_id,
            err: err.into().into(),
        }
    }

    pub fn invalid_storage_path(moniker: Moniker, instance_id: Option<InstanceId>) -> Self {
        Self::InvalidStoragePath { moniker, instance_id }
    }
}

#[derive(Error, Debug, Clone)]
pub enum StartError {
    #[error("serving namespace: {0}")]
    ServeNamespace(BuildNamespaceError),
}

#[derive(Error, Debug, Clone)]
pub enum StopError {
    /// Internal errors are not meant to be meaningfully handled by the user.
    #[error("internal: {0}")]
    Internal(fidl::Error),
}
