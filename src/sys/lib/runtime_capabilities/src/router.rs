// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{CapabilityBound, WeakInstanceToken};
use async_trait::async_trait;
use capability_source::CapabilitySource;
use cm_rust::{Availability, CapabilityTypeName};
use cm_types::Name;
use fidl_fuchsia_component_runtime::RouteRequest;
use router_error::RouterError;
use std::fmt;
use std::sync::Arc;

/// Types that implement [`Routable`] let the holder asynchronously request capabilities
/// from them.
#[async_trait]
pub trait Routable<T>: Send + Sync
where
    T: CapabilityBound,
{
    async fn route(
        &self,
        request: RouteRequest,
        // A reference to the requesting component.
        target: Arc<WeakInstanceToken>,
    ) -> Result<Option<Arc<T>>, RouterError>;

    /// Performs the same operation as `route`, but returns a
    /// `fidl_fuchsia_internal::CapabilitySource` persisted into bytes.
    async fn route_debug(
        &self,
        request: RouteRequest,
        // A reference to the requesting component.
        target: Arc<WeakInstanceToken>,
    ) -> Result<CapabilitySource, RouterError>;

    /// Returns diagnostic data about the capability being routed.
    fn error_info(&self) -> Option<RouterErrorInfo> {
        None
    }
}

/// Diagnostic data derived from the capability decl used to instantiate the router.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RouterErrorInfo {
    pub capability_type: CapabilityTypeName,
    /// The capability name. The semantics of this name depends on the type of the
    /// decl:
    ///   - Expose -> `target_name`
    ///   - Offer -> `target_name`
    ///   - Capability -> `name`
    ///   - Use -> `source_name`
    pub name: Name,
    pub availability: Availability,
}

/// A [`Router`] is a capability that lets the holder obtain other capabilities
/// asynchronously. [`Router`] is the object capability representation of
/// [`Routable`].
///
/// During routing, a request usually traverses through the component topology,
/// passing through several routers, ending up at some router that will fulfill
/// the request instead of forwarding it upstream.
pub struct Router<T: CapabilityBound> {
    routable: Box<dyn Routable<T>>,
}

impl CapabilityBound for Router<crate::Connector> {
    fn debug_typename() -> &'static str {
        "ConnectorRouter"
    }

    #[cfg(target_os = "fuchsia")]
    fn try_into_directory_entry(
        self: Arc<Self>,
        scope: vfs::execution_scope::ExecutionScope,
        token: Arc<crate::WeakInstanceToken>,
    ) -> Result<Arc<dyn vfs::directory::entry::DirectoryEntry>, crate::ConversionError> {
        Ok(self.into_directory_entry(fidl_fuchsia_io::DirentType::Service, scope, token))
    }
}
impl CapabilityBound for Router<crate::Data> {
    fn debug_typename() -> &'static str {
        "DataRouter"
    }

    #[cfg(target_os = "fuchsia")]
    fn try_into_directory_entry(
        self: Arc<Self>,
        scope: vfs::execution_scope::ExecutionScope,
        token: Arc<crate::WeakInstanceToken>,
    ) -> Result<Arc<dyn vfs::directory::entry::DirectoryEntry>, crate::ConversionError> {
        Ok(self.into_directory_entry(fidl_fuchsia_io::DirentType::Service, scope, token))
    }
}
impl CapabilityBound for Router<crate::Dictionary> {
    fn debug_typename() -> &'static str {
        "DictionaryRouter"
    }

    #[cfg(target_os = "fuchsia")]
    fn try_into_directory_entry(
        self: Arc<Self>,
        scope: vfs::execution_scope::ExecutionScope,
        token: Arc<crate::WeakInstanceToken>,
    ) -> Result<Arc<dyn vfs::directory::entry::DirectoryEntry>, crate::ConversionError> {
        Ok(self.into_directory_entry(fidl_fuchsia_io::DirentType::Service, scope, token))
    }
}

impl CapabilityBound for Router<crate::DirConnector> {
    fn debug_typename() -> &'static str {
        "DirConnectorRouter"
    }

    #[cfg(target_os = "fuchsia")]
    fn try_into_directory_entry(
        self: Arc<Self>,
        scope: vfs::execution_scope::ExecutionScope,
        token: Arc<crate::WeakInstanceToken>,
    ) -> Result<Arc<dyn vfs::directory::entry::DirectoryEntry>, crate::ConversionError> {
        Ok(self.into_directory_entry(fidl_fuchsia_io::DirentType::Service, scope, token))
    }
}

impl<T: CapabilityBound> fmt::Debug for Router<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // TODO(https://fxbug.dev/329680070): Require `Debug` on `Routable` trait.
        f.debug_struct("Router").field("routable", &"[some routable object]").finish()
    }
}

#[async_trait]
impl<T: CapabilityBound> Routable<T> for Router<T> {
    async fn route(
        &self,
        request: RouteRequest,
        target: Arc<WeakInstanceToken>,
    ) -> Result<Option<Arc<T>>, RouterError> {
        Router::route(self, request, target).await
    }

    async fn route_debug(
        &self,
        request: RouteRequest,
        target: Arc<WeakInstanceToken>,
    ) -> Result<CapabilitySource, RouterError> {
        Router::route_debug(self, request, target).await
    }

    fn error_info(&self) -> Option<RouterErrorInfo> {
        self.routable.error_info()
    }
}

impl<T: CapabilityBound> Router<T> {
    /// Package a [`Routable`] object into a [`Router`].
    pub fn new(routable: impl Routable<T> + 'static) -> Arc<Self> {
        Arc::new(Self { routable: Box::new(routable) })
    }

    /// Creates a router that will always fail a request with the provided error.
    pub fn new_error(error: impl Into<RouterError>) -> Arc<Self> {
        let v: RouterError = error.into();
        Self::new(ErrRouter { v })
    }

    /// Creates a router that will always return the given debug info.
    pub fn new_debug(source: CapabilitySource) -> Arc<Self> {
        Self::new(DebugRouter { source })
    }

    /// Obtain a capability from this router, following the description in `request`.
    pub async fn route(
        &self,
        request: RouteRequest,
        target: Arc<WeakInstanceToken>,
    ) -> Result<Option<Arc<T>>, RouterError> {
        self.routable.route(request, target).await
    }

    /// Obtain a CapabilitySource from this router, following the description in `request`.
    pub async fn route_debug(
        &self,
        request: RouteRequest,
        target: Arc<WeakInstanceToken>,
    ) -> Result<CapabilitySource, RouterError> {
        self.routable.route_debug(request, target).await
    }

    /// Returns diagnostic data about the capability being routed.
    pub fn error_info(&self) -> Option<RouterErrorInfo> {
        self.routable.error_info()
    }
}

impl<T: CapabilityBound> Router<T> {
    /// Creates a router that will always resolve with the provided capability.
    // TODO: Should this require debug info?
    pub fn new_ok(c: impl Into<Arc<T>>) -> Arc<Self> {
        let v: Arc<T> = c.into();
        Self::new(OkRouter { v })
    }
}

#[derive(Clone)]
struct OkRouter<T: CapabilityBound> {
    v: Arc<T>,
}

#[async_trait]
impl<T: CapabilityBound> Routable<T> for OkRouter<T> {
    async fn route(
        &self,
        _request: RouteRequest,
        _target: Arc<WeakInstanceToken>,
    ) -> Result<Option<Arc<T>>, RouterError> {
        Ok(Some(self.v.clone()))
    }

    async fn route_debug(
        &self,
        _request: RouteRequest,
        _target: Arc<WeakInstanceToken>,
    ) -> Result<CapabilitySource, RouterError> {
        panic!("OkRouter does not handle debug routes");
    }
}

#[derive(Clone)]
struct DebugRouter {
    source: CapabilitySource,
}

#[async_trait]
impl<T: CapabilityBound> Routable<T> for DebugRouter {
    async fn route(
        &self,
        _request: RouteRequest,
        _target: Arc<WeakInstanceToken>,
    ) -> Result<Option<Arc<T>>, RouterError> {
        panic!("DebugRouter does not handle non-debug routes");
    }

    async fn route_debug(
        &self,
        _request: RouteRequest,
        _target: Arc<WeakInstanceToken>,
    ) -> Result<CapabilitySource, RouterError> {
        Ok(self.source.clone())
    }
}

#[derive(Clone)]
struct ErrRouter {
    v: RouterError,
}

#[async_trait]
impl<T: CapabilityBound> Routable<T> for ErrRouter {
    async fn route(
        &self,
        _request: RouteRequest,
        _target: Arc<WeakInstanceToken>,
    ) -> Result<Option<Arc<T>>, RouterError> {
        Err(self.v.clone())
    }

    async fn route_debug(
        &self,
        _request: RouteRequest,
        _target: Arc<WeakInstanceToken>,
    ) -> Result<CapabilitySource, RouterError> {
        Err(self.v.clone())
    }
}
