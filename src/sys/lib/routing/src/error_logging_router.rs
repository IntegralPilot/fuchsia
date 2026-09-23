// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::error::{ErrorReporter, RouteRequestErrorInfo};
use async_trait::async_trait;
use capability_source::CapabilitySource;
use fidl_fuchsia_component_runtime::RouteRequest;
use router_error::RouterError;
use runtime_capabilities::{
    Capability, CapabilityBound, Connector, Data, Dictionary, DirConnector, Routable, Router,
    WeakInstanceToken,
};
use std::sync::Arc;

pub struct ErrorLoggingRouter<R: ErrorReporter> {
    inner_router: Capability,
    route_request: RouteRequestErrorInfo,
    error_reporter: R,
    error_location: Arc<WeakInstanceToken>,
}

impl<R: ErrorReporter> ErrorLoggingRouter<R> {
    pub fn new(
        inner_router: Capability,
        route_request: impl Into<RouteRequestErrorInfo>,
        error_reporter: R,
        error_location: Arc<WeakInstanceToken>,
    ) -> Capability {
        match &inner_router {
            Capability::ConnectorRouter(_) => Router::<Connector>::new(Self {
                inner_router,
                route_request: route_request.into(),
                error_reporter,
                error_location,
            })
            .into(),
            Capability::DirConnectorRouter(_) => Router::<DirConnector>::new(Self {
                inner_router,
                route_request: route_request.into(),
                error_reporter,
                error_location,
            })
            .into(),
            Capability::DictionaryRouter(_) => Router::<Dictionary>::new(Self {
                inner_router,
                route_request: route_request.into(),
                error_reporter,
                error_location,
            })
            .into(),
            Capability::DataRouter(_) => Router::<Data>::new(Self {
                inner_router,
                route_request: route_request.into(),
                error_reporter,
                error_location,
            })
            .into(),
            _ => panic!("non-router type passed to ErrorLoggingRouter"),
        }
    }
}

#[async_trait]
impl<T: CapabilityBound, R: ErrorReporter> Routable<T> for ErrorLoggingRouter<R>
where
    Arc<Router<T>>: TryFrom<Capability>,
{
    async fn route(
        &self,
        request: RouteRequest,
        target: Arc<WeakInstanceToken>,
    ) -> Result<Option<Arc<T>>, RouterError> {
        let inner_router: Arc<Router<T>> =
            self.inner_router.clone().try_into().ok().expect("type mismatch");
        match inner_router.route(request, target).await {
            Ok(res) => Ok(res),
            Err(err) => {
                self.error_reporter
                    .report(&self.route_request, &err, self.error_location.clone())
                    .await;
                Err(err)
            }
        }
    }

    async fn route_debug(
        &self,
        request: RouteRequest,
        target: Arc<WeakInstanceToken>,
    ) -> Result<CapabilitySource, RouterError> {
        let inner_router: Arc<Router<T>> =
            self.inner_router.clone().try_into().ok().expect("type mismatch");
        match inner_router.route_debug(request, target).await {
            Ok(res) => Ok(res),
            Err(err) => {
                self.error_reporter
                    .report(&self.route_request, &err, self.error_location.clone())
                    .await;
                Err(err)
            }
        }
    }

    fn error_info(&self) -> Option<runtime_capabilities::RouterErrorInfo> {
        Some((&self.route_request).into())
    }
}
