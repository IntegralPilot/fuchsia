// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub mod aggregate_router;
pub mod service;

use crate::model::component::{ComponentInstance, WeakComponentInstance};
use crate::model::storage;
use ::routing::bedrock::dict_ext::DictExt;
use ::routing::error::{ErrorReporter, RouteRequestErrorInfo};
use async_trait::async_trait;
use capability_source::CapabilitySource;
use cm_types::{Availability, Path};
use errors::ModelError;
use fidl_fuchsia_component_runtime::RouteRequest;
use log::error;
use router_error::RouterError;
use routing::component_instance::ComponentInstanceInterface;
use runtime_capabilities::Capability;
use std::sync::Arc;

pub struct RoutedStorage {
    backing_dir_info: storage::BackingDirectoryInfo,
    target: WeakComponentInstance,
}

pub(super) async fn route_storage(
    storage_path: &Path,
    target: &Arc<ComponentInstance>,
) -> Result<RoutedStorage, ModelError> {
    let storage_router_capability = target
        .lock_resolved_state()
        .await?
        .sandbox
        .program_input
        .namespace()
        .get_capability(storage_path)
        .expect("namespace is missing used storage capability");
    let Capability::DirConnectorRouter(router) = storage_router_capability else {
        panic!("wrong type for used storage capability");
    };
    let storage_source: CapabilitySource =
        router.route_debug(RouteRequest::default(), target.as_weak().into()).await?;
    let backing_dir_info = storage::route_backing_directory(target, storage_source).await?;
    Ok(RoutedStorage { backing_dir_info, target: WeakComponentInstance::new(target) })
}

pub(super) async fn delete_storage(routed_storage: RoutedStorage) -> Result<(), ModelError> {
    let target = routed_storage.target.upgrade()?;

    // As of today, the storage component instance must contain the target. This is because
    // it is impossible to expose storage declarations up.
    let moniker = target
        .moniker()
        .strip_prefix(&routed_storage.backing_dir_info.storage_source_moniker)
        .unwrap();
    storage::delete_isolated_storage(routed_storage.backing_dir_info, moniker, target.instance_id())
        .await
}

/// ErrorReporter that calls report_routing_failure.
#[derive(Clone)]
pub struct RoutingFailureErrorReporter {}

impl RoutingFailureErrorReporter {
    pub fn new() -> Self {
        Self {}
    }
}

#[async_trait]
impl ErrorReporter for RoutingFailureErrorReporter {
    async fn report(
        &self,
        request: &RouteRequestErrorInfo,
        err: &RouterError,
        target: Arc<runtime_capabilities::WeakInstanceToken>,
    ) {
        let component_to_log_at = match WeakComponentInstance::try_from(target) {
            Ok(target) => target,
            Err(()) => {
                error!(
                    err:%;
                    "Failed to convert WeakInstanceToken to WeakComponentInstance while reporting \
                    routing error."
                );
                return;
            }
        };
        match component_to_log_at.upgrade() {
            Ok(target) => {
                report_routing_failure(request, Some(request.availability()), &target, err).await;
            }
            Err(upgrade_err) => {
                error!(upgrade_err:%, err:%;
                    "Failed to upgrade WeakComponentInstance while reporting routing error.")
            }
        }
    }
}

/// Logs a failure to route a capability. Formats `err` as a `String`, but
/// elides the type if the error is a `RoutingError`, the common case.
pub async fn report_routing_failure(
    capability_requested: impl std::fmt::Display,
    availability: Option<Availability>,
    target: &Arc<ComponentInstance>,
    err: impl std::error::Error,
) {
    let availability = availability.unwrap_or(Availability::Required);
    let moniker = &target.moniker;
    let child_moniker = moniker.leaf().map(|m| m.as_ref()).unwrap_or("");
    match availability {
        Availability::Required => {
            // TODO(https://fxbug.dev/42060474): consider changing this to `error!()`
            target.context.routing_errors().record(
                &target.moniker,
                &capability_requested.to_string(),
                &err.to_string(),
                Availability::Required,
            );
            target
                .log(
                    log::Level::Warn,
                    format!(
                        "{capability_requested} was not available for target `{child_moniker}`:\n\t\
                        {err}\n\tFor more, run `ffx component doctor`",
                    ),
                    &[],
                )
                .await;
        }
        Availability::Optional | Availability::SameAsTarget | Availability::Transitional => {
            // If the target declared the capability as optional, but
            // the capability could not be routed (such as if the source
            // component is not available) the component _should_
            // tolerate the missing optional capability. However, this
            // should be logged. Developers are encouraged to change how
            // they build and/or assemble different product
            // configurations so declared routes are always end-to-end
            // complete routes.
            // TODO(https://fxbug.dev/42060474): if we change the log for
            // `Required` capabilities to `error!()`, consider also
            // changing this log for `Optional` to `warn!()`.
            target.context.routing_errors().record(
                &target.moniker,
                &capability_requested.to_string(),
                &err.to_string(),
                availability,
            );
            target.log(
                log::Level::Info,
                format!(
                    "{availability} {capability_requested} was not available for target `{child_moniker}`:\n\t\
                    {err}\n\tFor more, run `ffx component doctor`"
                ),
                &[]
            ).await;
        }
    }
}
