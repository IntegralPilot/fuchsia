// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::component_instance::{ComponentInstanceInterface, WeakExtendedInstanceInterface};
use crate::error::{ComponentInstanceError, RoutingError};
use crate::policy::GlobalPolicyChecker;
use async_trait::async_trait;
use capability_source::CapabilitySource;
use fidl_fuchsia_component_runtime::RouteRequest;
use moniker::ExtendedMoniker;
use router_error::RouterError;
use runtime_capabilities::{CapabilityBound, Routable, Router, WeakInstanceToken};
use std::sync::Arc;

/// If the metadata for a route contains a Data::Uint64 value under this key with a value greater
/// than 0, then no policy checks will be performed. This behavior is limited to non-fuchsia
/// builds, and is exclusively used when performing routes from an offer declaration. This is
/// necessary because we don't know the ultimate target of the route, and thus routes that are
/// otherwise valid could fail due to policy checks.
///
/// Consider a policy that allows a component `/core/session_manager/session:session/my_cool_app`
/// to access `fuchsia.kernel.VmexResource`. If we attempt to validate that route from the offer
/// placed on `session_manager`, we'd have to fill in `session_manager` for the target of the route
/// in the route request and follow the route to its source from there. If this policy check were
/// applied on this route it would fail the route, as `session` manager is not allowed to access
/// `fuchsia.kernel.VmexResource`. The route is valid though, because the offer on
/// `session_manager` doesn't grant the session manager program access to the restricted
/// capability.
///
/// To be able to properly support this scenario, we need to selectively disable policy checks when
/// routing from offer declarations.
pub const SKIP_POLICY_CHECKS: &'static str = "skip_policy_checks";

pub trait WithPolicyCheck {
    /// Returns a router that ensures the capability request is allowed by the
    /// policy in [`GlobalPolicyChecker`].
    fn with_policy_check<C: ComponentInstanceInterface + 'static>(
        self,
        capability_source: CapabilitySource,
        policy_checker: GlobalPolicyChecker,
    ) -> Self;
}

impl<T: CapabilityBound> WithPolicyCheck for Arc<Router<T>> {
    fn with_policy_check<C: ComponentInstanceInterface + 'static>(
        self,
        capability_source: CapabilitySource,
        policy_checker: GlobalPolicyChecker,
    ) -> Self {
        Router::new(PolicyCheckRouter::<C, T>::new(capability_source, policy_checker, self))
    }
}

pub struct PolicyCheckRouter<C: ComponentInstanceInterface + 'static, T: CapabilityBound> {
    capability_source: CapabilitySource,
    policy_checker: GlobalPolicyChecker,
    router: Arc<Router<T>>,
    _phantom_data: std::marker::PhantomData<C>,
}

impl<C: ComponentInstanceInterface + 'static, T: CapabilityBound> PolicyCheckRouter<C, T> {
    pub fn new(
        capability_source: CapabilitySource,
        policy_checker: GlobalPolicyChecker,
        router: Arc<Router<T>>,
    ) -> Self {
        Self {
            capability_source,
            policy_checker,
            router,
            _phantom_data: std::marker::PhantomData::<C>,
        }
    }

    fn check_policy(
        &self,
        _request: &RouteRequest,
        target_token: Arc<WeakInstanceToken>,
    ) -> Result<(), RouterError> {
        #[cfg(not(target_os = "fuchsia"))]
        if _request.skip_policy_checks.unwrap_or(false) {
            return Ok(());
        }
        let target = target_token
            .inner
            .as_any()
            .downcast_ref::<WeakExtendedInstanceInterface<C>>()
            .ok_or(RouterError::Unknown)?;
        let ExtendedMoniker::ComponentInstance(moniker) = target.extended_moniker() else {
            return Err(RoutingError::from(
                ComponentInstanceError::ComponentManagerInstanceUnexpected {},
            )
            .into());
        };
        match self.policy_checker.can_route_capability(&self.capability_source, &moniker) {
            Ok(()) => Ok(()),
            Err(policy_error) => Err(RoutingError::PolicyError(policy_error).into()),
        }
    }
}

#[async_trait]
impl<C: ComponentInstanceInterface + 'static, T: CapabilityBound> Routable<T>
    for PolicyCheckRouter<C, T>
{
    async fn route(
        &self,
        request: RouteRequest,
        target_token: Arc<WeakInstanceToken>,
    ) -> Result<Option<Arc<T>>, RouterError> {
        self.check_policy(&request, target_token.clone())?;
        self.router.route(request, target_token).await
    }

    async fn route_debug(
        &self,
        request: RouteRequest,
        target_token: Arc<WeakInstanceToken>,
    ) -> Result<CapabilitySource, RouterError> {
        self.check_policy(&request, target_token.clone())?;
        self.router.route_debug(request, target_token).await
    }
}
