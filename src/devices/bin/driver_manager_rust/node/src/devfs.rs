// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::node::Node;
use driver_manager_devfs::{Connector, ConnectorMsg, TopologicalDevnode};
use fidl::endpoints::{ClientEnd, ServerEnd};
use futures::channel::mpsc;
use futures::{StreamExt, TryStreamExt};
use log::{error, warn};
use phf::{Map, Set, phf_map, phf_set};
use std::rc::{Rc, Weak};
use {fidl_fuchsia_device as fdevice, fidl_fuchsia_device_fs as fdevfs, fuchsia_async as fasync};

const ALLOW_ALL_USES: &str = "Allow_all_uses";

// LINT.IfChange
static CONTROLLER_ALLOWLISTS: Map<&'static str, Set<&'static str>> = phf_map! {
    "ConnectToController" => phf_set! {
        "block",
        "No_class_name_but_driver_url_is_fuchsia-boot:///dtr#meta/sample_driver.cm",
        "driver_runner_test",
    },
    "ConnectToDeviceFidl" => phf_set! {
        "block",
        "driver_runner_test",
        "nand",
        "skip-block",
        "No_class_name_but_driver_url_is_fuchsia-boot:///dtr#meta/fvm.cm",
        "No_class_name_but_driver_url_is_fuchsia-boot:///gpt#meta/gpt.cm",
        "No_class_name_but_driver_url_is_fuchsia-boot:///dtr#meta/gpt.cm",
        "No_class_name_but_driver_url_is_fuchsia-boot:///dtr#meta/nand-broker.cm",
        "No_class_name_but_driver_url_is_fuchsia-boot:///dtr#meta/sample_driver.cm",
    },
    "Bind" => phf_set! {
        "block",
        "No_class_name_but_driver_url_is_fuchsia-boot:///dtr#meta/test.cm",
        "driver_runner_test",
    },
    "Rebind" => phf_set! {
        "block",
        "driver_runner_test",
        "No_class_name_but_driver_url_is_owned by parent",
        "nand",
        "No_class_name_but_driver_url_is_fuchsia-boot:///dtr#meta/ddk-lifecycle-test.cm",
    },
    "UnbindChildren" => phf_set! {
        "block",
        "driver_runner_test",
    },
    "ScheduleUnbind" => phf_set! {
        "bt-emulator",
        "driver_runner_test",
        "No_class_name_but_driver_url_is_fuchsia-boot:///dtr#meta/ddk-lifecycle-test.cm",
        "No_class_name_but_driver_url_is_fuchsia-boot:///dtr#meta/fvm.cm",
        "No_class_name_but_driver_url_is_fuchsia-boot:///fvm#meta/fvm.cm",
        "No_class_name_but_driver_url_is_owned by parent",
    },
    "GetTopologicalPath" => phf_set! {
        "Allow_all_uses",
    },
};
// LINT.ThenChange(//src/devices/bin/driver_manager/controller_allowlist_passthrough.cc)

pub struct ControllerAllowlistPassthrough {
    node: Weak<Node>,
    class_name: String,
    compat_client: Option<fdevice::ControllerProxy>,
    scope: fasync::Scope,
}

impl ControllerAllowlistPassthrough {
    pub fn new(
        node: Weak<Node>,
        class_name: String,
        controller_connector: Option<ClientEnd<fdevfs::ConnectorMarker>>,
        parent_scope: &fasync::ScopeHandle,
    ) -> Rc<Self> {
        let compat_client = controller_connector.and_then(|connector| {
            let (client, server) = fidl::endpoints::create_proxy::<fdevice::ControllerMarker>();
            let connector_proxy = connector.into_proxy();
            connector_proxy.connect(server.into()).ok()?;
            Some(client)
        });

        let scope = parent_scope.new_child_with_name("controller passthrough");
        Rc::new(Self { node, class_name, compat_client, scope })
    }

    fn check_allowlist(&self, function_name: &str) {
        let allowlist = CONTROLLER_ALLOWLISTS.get(function_name).unwrap();
        if allowlist.contains(ALLOW_ALL_USES) {
            return;
        }
        assert!(
            allowlist.contains(self.class_name.as_str()),
            "\nUndeclared DEVFS_USAGE detected: {} is using {}.\n",
            self.class_name,
            function_name
        );
    }

    pub fn serve(
        self: &Rc<Self>,
        server_end: ServerEnd<fdevice::ControllerMarker>,
    ) -> Result<(), zx::Status> {
        let mut stream = server_end.into_stream();
        let this = self.clone();
        self.scope.spawn_local(async move {
            while let Ok(Some(request)) = stream.try_next().await {
                this.handle_request(request).await.unwrap_or_else(|e| {
                    error!("Error handling controller request: {}", e);
                });
            }
        });
        Ok(())
    }

    async fn handle_request(
        self: &Rc<Self>,
        request: fdevice::ControllerRequest,
    ) -> Result<(), fidl::Error> {
        match request {
            fdevice::ControllerRequest::ConnectToDeviceFidl { server, .. } => {
                self.check_allowlist("ConnectToDeviceFidl");
                if let Some(client) = &self.compat_client {
                    client.connect_to_device_fidl(server)?;
                } else if let Some(node) = self.node.upgrade() {
                    node.connect_to_device_fidl(server);
                }
            }
            fdevice::ControllerRequest::ConnectToController { server, .. } => {
                self.check_allowlist("ConnectToController");
                self.serve(server).unwrap();
            }
            fdevice::ControllerRequest::Bind { driver, responder } => {
                self.check_allowlist("Bind");
                if let Some(client) = &self.compat_client {
                    let result = client.bind(driver.as_str()).await?;
                    responder.send(result)?;
                } else if let Some(node) = self.node.upgrade() {
                    responder.send(node.bind(driver).await.map_err(zx::Status::into_raw))?;
                } else {
                    responder.send(Err(zx::Status::INTERNAL.into_raw()))?;
                }
            }
            fdevice::ControllerRequest::Rebind { driver, responder } => {
                self.check_allowlist("Rebind");
                if let Some(client) = &self.compat_client {
                    let result = client.rebind(driver.as_ref()).await?;
                    responder.send(result)?;
                } else if let Some(node) = self.node.upgrade() {
                    responder
                        .send(node.rebind(Some(driver)).await.map_err(zx::Status::into_raw))?;
                } else {
                    responder.send(Err(zx::Status::INTERNAL.into_raw()))?;
                }
            }
            fdevice::ControllerRequest::UnbindChildren { responder } => {
                self.check_allowlist("UnbindChildren");
                if let Some(client) = &self.compat_client {
                    let result = client.unbind_children().await?;
                    responder.send(result)?;
                } else if let Some(node) = self.node.upgrade() {
                    let result = node.unbind_children().await.map_err(zx::Status::into_raw);
                    responder.send(result)?;
                } else {
                    responder.send(Err(zx::Status::INTERNAL.into_raw()))?;
                }
            }
            fdevice::ControllerRequest::ScheduleUnbind { responder } => {
                self.check_allowlist("ScheduleUnbind");
                if let Some(client) = &self.compat_client {
                    let result = client.schedule_unbind().await?;
                    responder.send(result)?;
                } else if let Some(node) = self.node.upgrade() {
                    node.schedule_unbind();
                    responder.send(Ok(()))?;
                } else {
                    responder.send(Err(zx::Status::INTERNAL.into_raw()))?;
                }
            }
            fdevice::ControllerRequest::GetTopologicalPath { responder } => {
                self.check_allowlist("GetTopologicalPath");
                if let Some(node) = self.node.upgrade() {
                    let path = format!("/{}", node.make_topological_path(false));
                    responder.send(Ok(&path))?;
                } else {
                    responder.send(Err(zx::Status::INTERNAL.into_raw()))?;
                }
            }
        }
        Ok(())
    }
}

impl Node {
    pub fn setup_devfs_for_root_node(&self, root: TopologicalDevnode) {
        let mut device = self.device();
        device.topological = Some(root);
        self.set_device(device);
    }

    pub(crate) fn connect_to_device_fidl(&self, server: zx::Channel) {
        if let Some(connector) = self.protocol_connector()
            && let Err(e) = connector.connect(server)
        {
            error!("Failed to connect to device fidl: {}", e);
        }
    }

    pub(crate) fn connect_to_controller(&self, server_end: ServerEnd<fdevice::ControllerMarker>) {
        if let Some(passthrough) = self.controller_allowlist_passthrough() {
            let _ = passthrough.serve(server_end);
        } else {
            warn!(
                concat!(
                    "Connection to {} controller interface failed, as that node ",
                    "did not include controller support in its DevAddArgs"
                ),
                self.name()
            );
        }
    }

    pub(crate) fn create_devfs_passthrough(
        self: &Rc<Self>,
        protocol_connector: Option<ClientEnd<fdevfs::ConnectorMarker>>,
        controller_connector: Option<ClientEnd<fdevfs::ConnectorMarker>>,
        allow_controller_connection: bool,
        class_name: String,
    ) -> Connector {
        if allow_controller_connection {
            let passthrough = ControllerAllowlistPassthrough::new(
                self.weak_self.clone(),
                class_name,
                controller_connector,
                self.scope.as_handle(),
            );
            self.set_controller_allowlist_passthrough(passthrough);
        }
        if let Some(c) = protocol_connector {
            self.set_protocol_connector(c.into_proxy());
        }
        let weak_node = self.weak_self.clone();
        let node_name = self.name().to_string();
        let (tx, mut rx) = mpsc::unbounded::<ConnectorMsg>();
        self.scope.spawn_local(async move {
            while let Some(msg) = rx.next().await {
                if let Some(node) = weak_node.upgrade() {
                    match msg {
                        ConnectorMsg::Controller(server_end) => {
                            node.connect_to_controller(server_end);
                        }
                        ConnectorMsg::Protocol(server_end) => {
                            node.connect_to_device_fidl(server_end);
                        }
                    }
                } else {
                    error!("Node was freed before it was used for {}.", node_name);
                }
            }
        });
        tx
    }
}
