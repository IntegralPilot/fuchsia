// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::node::Node;
use crate::types::{DriverComponent, DriverState, NodeState};
use driver_manager_driver_host::{DriverLoadArgs, DriverStartArgs};
use driver_manager_types::{ShutdownState, StartRequestReceiver};
use fidl::endpoints::{ServerEnd, create_endpoints};
use fidl_fuchsia_component as fcomponent;
use fidl_fuchsia_component_runner as frunner;
use fidl_fuchsia_data as fdata;
use fidl_fuchsia_driver_framework as fdf;
use fidl_fuchsia_driver_host as fdh;
use futures::StreamExt;
use log::{debug, error, warn};
use std::rc::Rc;

impl Node {
    pub async fn send_start_request(&self) -> Result<(), zx::Status> {
        let (handles, proxy) = {
            let handles = self.take_start_handles_for_start();
            let proxy = self.component_controller_proxy().expect("component");
            (handles, proxy)
        };

        let start_child_args =
            fcomponent::StartChildArgs { numbered_handles: Some(handles), ..Default::default() };
        let (_, server_end) = fidl::endpoints::create_endpoints();

        proxy
            .start(start_child_args, server_end)
            .await
            .map_err(|e| {
                error!("Failed to start driver for node {}: {}", self.name(), e);
                zx::Status::INTERNAL
            })?
            .map_err(|e: fcomponent::Error| {
                error!("Failed to start driver for node {}: {:?}", self.name(), e);
                zx::Status::INTERNAL
            })?;
        Ok(())
    }

    pub async fn get_next_start_request(
        &self,
    ) -> Result<
        (frunner::ComponentStartInfo, ServerEnd<frunner::ComponentControllerMarker>),
        zx::Status,
    > {
        struct ReleaseStartRequestReceiverGuard<'a> {
            receiver: Option<StartRequestReceiver>,
            node: &'a Node,
        }

        impl<'a> Drop for ReleaseStartRequestReceiverGuard<'a> {
            fn drop(&mut self) {
                if let Some(rx) = self.receiver.take() {
                    self.node.set_start_request_receiver(rx);
                }
            }
        }

        let receiver = self.take_start_request_receiver().ok_or(zx::Status::BAD_STATE)?;

        let mut guard = ReleaseStartRequestReceiverGuard { receiver: Some(receiver), node: self };

        let start_request =
            guard.receiver.as_mut().unwrap().next().await.ok_or(zx::Status::TIMED_OUT)??;
        let start_info = start_request.info;
        let controller = start_request.controller;
        Ok((start_info, controller))
    }

    pub async fn start_driver(
        self: &Rc<Self>,
        mut start_info: frunner::ComponentStartInfo,
        controller: ServerEnd<frunner::ComponentControllerMarker>,
    ) -> Result<(), zx::Status> {
        if self.shutdown_state() == ShutdownState::Stopped {
            self.node_shutdown_coordinator.borrow_mut().reset_shutdown();
        }
        let url = start_info.resolved_url.clone().ok_or(zx::Status::INVALID_ARGS)?;
        self.set_state(NodeState::Starting { driver_url: url.clone() });

        let program = start_info.program.as_ref();
        let get_prog_val = |key: &str| -> Option<String> {
            program.and_then(|p| p.entries.as_ref()).and_then(|e| {
                e.iter().find(|entry| entry.key == key).and_then(|entry| {
                    if let Some(fdata::DictionaryValue::Str(s)) =
                        entry.value.as_ref().map(|v| v.as_ref())
                    {
                        Some(s.clone())
                    } else {
                        None
                    }
                })
            })
        };

        let colocate = get_prog_val("colocate").as_deref() == Some("true");
        let use_next_vdso = get_prog_val("use_next_vdso").as_deref() == Some("true");
        let host_restart_on_crash =
            get_prog_val("host_restart_on_crash").as_deref() == Some("true");
        let use_dynamic_linker = get_prog_val("use_dynamic_linker").as_deref() == Some("true");

        if host_restart_on_crash && colocate {
            error!(
                "Failed to start driver '{}'. Both host_restart_on_crash and colocate cannot be true.",
                url
            );
            return Err(zx::Status::INVALID_ARGS);
        }
        self.set_restart_on_crash(host_restart_on_crash);

        let driver_host = self.host();
        let mut found_driver_host = colocate;
        let driver_host = if found_driver_host {
            match driver_host {
                Some(dh) => dh,
                None => {
                    error!(
                        "Failed to start driver '{}', driver is colocated but does not have a parent with a driver host",
                        url
                    );
                    return Err(zx::Status::INVALID_ARGS);
                }
            }
        } else {
            let driver_host_name = self.driver_host_name_for_colocation();
            match self.node_manager.get_driver_host(&driver_host_name) {
                Some(dh) => {
                    found_driver_host = true;
                    self.set_host(dh.clone());
                    dh
                }
                None => {
                    if use_dynamic_linker {
                        let driver_host = self
                            .node_manager
                            .create_driver_host_dynamic_linker(driver_host_name)
                            .await?;
                        self.set_host(driver_host.clone());
                        driver_host
                    } else {
                        debug!(
                            "Creating driver host for node '{}' driver url '{}'",
                            self.name(),
                            url
                        );
                        let driver_host = self
                            .node_manager
                            .create_driver_host(use_next_vdso, driver_host_name)
                            .await
                            .map_err(|e| {
                                error!("Failed to start driver '{url}': {e:?}");
                                zx::Status::INTERNAL
                            })?;
                        self.set_host(driver_host.clone());
                        driver_host
                    }
                }
            }
        };

        if found_driver_host {
            // Whether dynamic linking is enabled for a driver host is determined by the first driver in the
            // host. Otherwise for colocated drivers, we need to match what has been set for the driver
            // host.
            if use_dynamic_linker != driver_host.is_dynamic_linking_enabled() {
                error!(
                    concat!(
                        "Failed to start driver '{}', driver is colocated and set",
                        "use_dynamic_linker={} but its driver host is not configured for this"
                    ),
                    url,
                    if use_dynamic_linker { "true" } else { "false" }
                );
                return Err(zx::Status::INVALID_ARGS);
            }
        }

        let (node_client, node_server) = create_endpoints::<fdf::NodeMarker>();
        let (driver_client, driver_server) = create_endpoints::<fdh::DriverMarker>();

        let node_token = start_info.component_instance.take().unwrap_or_else(|| {
            warn!("Component instance not provided in start request");
            zx::Event::create()
        });

        let node_token_dup = node_token.duplicate_handle(zx::Rights::SAME_RIGHTS)?;

        let runner_component_controller = self.serve_runner_component_controller(controller);
        let node_server_binding = self.serve_node(node_server);

        let driver = driver_client.into_proxy();
        let driver_client_binding = self.serve_driver_host_client(driver);

        self.set_state(NodeState::DriverComponent(DriverComponent::new(
            url.clone(),
            node_token_dup,
            node_token.koid().unwrap(),
            Some(runner_component_controller),
            Some(node_server_binding),
            Some(driver_client_binding),
            DriverState::Binding,
        )));

        let symbols = if colocate { Some(self.symbols()) } else { None };
        let offers: Vec<fdf::Offer> = self.offers().iter().map(|f| f.into()).collect();
        let properties = self.get_node_property_dict();

        let load_args = if use_dynamic_linker {
            Some(DriverLoadArgs::new(&mut start_info).await?)
        } else {
            None
        };

        let start_args = DriverStartArgs {
            node: node_client,
            node_name: self.name.clone(),
            properties,
            symbols,
            offers,
            start_info,
            component_instance: node_token,
        };

        log::info!("Binding {} to {}", url, self.name);

        if use_dynamic_linker {
            driver_host
                .start_with_dynamic_linker(load_args.unwrap(), start_args, driver_server)
                .await
        } else {
            driver_host.start(start_args, driver_server).await
        }
        .inspect_err(|_| {
            error!("Failed to start driver host for {}", self.make_component_moniker())
        })?;

        Ok(())
    }
}
