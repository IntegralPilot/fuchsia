// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::model::component::manager::ComponentManagerInstance;
use ::routing::component_instance::ComponentInstanceInterface;
use ::routing::error::{ComponentInstanceError, RoutingError};
use anyhow::{Context, format_err};
use clonable_error::ClonableError;
use cm_types::Name;
use fidl::endpoints::{DiscoverableProtocolMarker, create_proxy};
use fidl_fuchsia_component_runtime::RouteRequest;
use fidl_fuchsia_update_verify as fupdate;
use fuchsia_inspect::ArrayProperty;
use futures::TryStreamExt;
use moniker::{ExtendedMoniker, Moniker};
use router_error::RouterError;
use runtime_capabilities::Capability;
use std::sync::{Arc, Weak};

#[derive(Debug, Clone)]
struct OtaHealthVerificationErrors {
    router: Vec<String>,
    unhealthy: Vec<String>,
    fidl: Vec<String>,
}

impl OtaHealthVerificationErrors {
    fn new() -> Self {
        Self { router: vec![], unhealthy: vec![], fidl: vec![] }
    }

    fn is_empty(&self) -> bool {
        self.router.is_empty() && self.unhealthy.is_empty() && self.fidl.is_empty()
    }
}

#[derive(Debug)]
pub struct OtaHealthVerification {
    health_checks: Vec<Moniker>,
    root: Weak<ComponentManagerInstance>,
    node: fuchsia_inspect::Node,
}

impl OtaHealthVerification {
    pub fn new(
        health_checks: Vec<String>,
        root: Weak<ComponentManagerInstance>,
        node: fuchsia_inspect::Node,
    ) -> Arc<Self> {
        Arc::new(Self {
            health_checks: health_checks
                .into_iter()
                .map(|s| s.as_str().try_into().unwrap())
                .collect(),
            root,
            node,
        })
    }

    pub async fn serve(
        self: Arc<Self>,
        mut stream: fupdate::HealthVerificationRequestStream,
    ) -> Result<(), anyhow::Error> {
        while let Some(request) = stream.try_next().await? {
            self.handle_request(request)
                .await
                .with_context(|| format!("OtaHealthVerification server failed"))?;
        }
        Ok(())
    }

    async fn handle_request(
        &self,
        request: fupdate::HealthVerificationRequest,
    ) -> Result<(), anyhow::Error> {
        match request {
            fupdate::HealthVerificationRequest::QueryHealthChecks { responder } => {
                let mut errors = OtaHealthVerificationErrors::new();
                for moniker in &self.health_checks {
                    let health_check_status = match open_protocol(moniker, self.root.clone()).await
                    {
                        Ok(health_check_status) => health_check_status,
                        Err(e) => {
                            log::error!(
                                "Error trying to connect to ComponentOtaHealthCheckProxy: {:?}",
                                e
                            );
                            errors.router.push(moniker.to_string());
                            continue;
                        }
                    };
                    let res = health_check_status.get_health_status().await;
                    match res {
                        Ok(fupdate::HealthStatus::Unhealthy) => {
                            log::error!("{:?} reported unhealthy status", moniker);
                            errors.unhealthy.push(moniker.to_string());
                        }
                        Ok(fupdate::HealthStatus::Healthy) => {}
                        Err(_e) => {
                            errors.fidl.push(moniker.to_string());
                        }
                    }
                }
                write_to_inspect(&self.node, &errors);

                if errors.is_empty() {
                    // All health checks responded with healthy!
                    return Ok(responder.send(zx::sys::ZX_OK)?);
                }
                return Ok(responder.send(zx::sys::ZX_ERR_BAD_STATE)?);
            }
        }
    }
}

async fn open_protocol(
    moniker: &Moniker,
    top_instance: Weak<ComponentManagerInstance>,
) -> Result<fupdate::ComponentOtaHealthCheckProxy, RouterError> {
    let instance = top_instance
        .upgrade()
        .ok_or(RouterError::Internal)?
        .root()
        .find(moniker)
        .await
        .ok_or_else(|| {
            RoutingError::from(ComponentInstanceError::InstanceNotFound {
                moniker: moniker.clone(),
            })
        })?;

    let instance_output = instance
        .lock_resolved_state()
        .await
        .map_err(|e| {
            RoutingError::from(ComponentInstanceError::ResolveFailed {
                moniker: Moniker::root(),
                err: ClonableError::from(format_err!("{:?}", e)),
            })
        })?
        .sandbox
        .component_output
        .clone();
    let capability = instance_output
        .framework()
        .get(&Name::new(fupdate::ComponentOtaHealthCheckMarker::PROTOCOL_NAME).unwrap())
        .ok_or_else(|| {
            RoutingError::expose_from_framework_not_found(
                &moniker,
                fupdate::ComponentOtaHealthCheckMarker::PROTOCOL_NAME,
            )
        })?;
    let router = match capability {
        Capability::ConnectorRouter(r) => r,
        other_type => {
            return Err(RoutingError::BedrockWrongCapabilityType {
                actual: format!("{:?}", other_type),
                expected: "ConnectorRouter".to_string(),
                moniker: ExtendedMoniker::from(moniker.clone()),
            }
            .into());
        }
    };
    let connector =
        match router.route(RouteRequest::default(), instance.clone().as_weak().into()).await? {
            Some(cap) => cap,
            None => {
                return Err(RoutingError::RouteUnexpectedUnavailable {
                    type_name: cm_rust::CapabilityTypeName::Protocol,
                    moniker: ExtendedMoniker::from(moniker.clone()),
                }
                .into());
            }
        };

    let (proxy, server_end) = create_proxy::<fupdate::ComponentOtaHealthCheckMarker>();
    connector.send(server_end.into_channel()).map_err(|_| RoutingError::BedrockFailedToSend {
        moniker: ExtendedMoniker::from(moniker.clone()),
        capability_id: fupdate::ComponentOtaHealthCheckMarker::PROTOCOL_NAME.to_string(),
    })?;
    Ok(proxy)
}

fn write_to_inspect(node: &fuchsia_inspect::Node, errors: &OtaHealthVerificationErrors) {
    if errors.is_empty() {
        node.record_string("result", "success");
        return;
    }

    node.record_string("result", "failure");
    let errors_node = node.create_child("errors");

    if !errors.router.is_empty() {
        let router_errors_node =
            errors_node.create_string_array("router_errors", errors.router.len());
        errors.router.iter().enumerate().for_each(|(i, moniker)| {
            router_errors_node.set(i, moniker);
        });
        errors_node.record(router_errors_node);
    }

    if !errors.fidl.is_empty() {
        let fidl_errors_node = errors_node.create_string_array("fidl_errors", errors.fidl.len());
        errors.fidl.iter().enumerate().for_each(|(i, moniker)| {
            fidl_errors_node.set(i, moniker);
        });
        errors_node.record(fidl_errors_node);
    }

    if !errors.unhealthy.is_empty() {
        let unhealthy_errors_node =
            errors_node.create_string_array("unhealthy_errors", errors.unhealthy.len());
        errors.unhealthy.iter().enumerate().for_each(|(i, moniker)| {
            unhealthy_errors_node.set(i, moniker);
        });
        errors_node.record(unhealthy_errors_node);
    }

    node.record(errors_node);
}

#[cfg(all(test, not(feature = "src_model_tests")))]
mod tests {
    use super::*;
    use diagnostics_assertions::assert_json_diff;
    use fuchsia_inspect::Inspector;

    #[fuchsia::test]
    async fn success() {
        let inspector = Inspector::default();
        let errors = OtaHealthVerificationErrors::new();

        let () = write_to_inspect(inspector.root(), &errors);

        assert_json_diff! {
            inspector,
            root: {
                "result": "success"
            }
        }
    }

    #[fuchsia::test]
    async fn fail_router() {
        let inspector = Inspector::default();
        let mut errors = OtaHealthVerificationErrors::new();
        errors.router.push("/bad/route".to_string());

        let () = write_to_inspect(inspector.root(), &errors);
        let expected: Vec<String> = vec!["/bad/route".into()];

        assert_json_diff! {
            inspector,
            root: {
                "result": "failure",
                "errors" : {
                    "router_errors": expected
                }
            }
        }
    }

    #[fuchsia::test]
    async fn fail_all_types() {
        let inspector = Inspector::default();
        let mut errors = OtaHealthVerificationErrors::new();
        errors.router.push("/bad/route".to_string());
        errors.fidl.push("/bad/server".to_string());
        errors.unhealthy.push("/ill".to_string());
        errors.unhealthy.push("unwell/component".to_string());

        let () = write_to_inspect(inspector.root(), &errors);
        let expected_router = vec!["/bad/route".to_string()];
        let expected_fidl = vec!["/bad/server".to_string()];
        let expected_unhealthy = vec!["/ill".to_string(), "unwell/component".to_string()];

        assert_json_diff! {
            inspector,
            root: {
                "result": "failure",
                "errors" : {
                    "router_errors": expected_router,
                    "fidl_errors": expected_fidl,
                    "unhealthy_errors": expected_unhealthy,
                }
            }
        }
    }
}
