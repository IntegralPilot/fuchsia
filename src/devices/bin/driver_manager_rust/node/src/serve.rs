// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::node::{Node, NodeComponent};
use driver_manager_shutdown::RemovalSet;
use driver_manager_types::{ShutdownState, StartRequestReceiver};
use fidl::endpoints::{ControlHandle as _, ServerEnd};
use fidl_fuchsia_component as fcomponent;
use fidl_fuchsia_component_runner as frunner;
use fidl_fuchsia_driver_framework as fdf;
use fidl_fuchsia_driver_host as fdh;
use fuchsia_async::{self as fasync};
use futures::channel::oneshot;
use futures::{StreamExt, TryStreamExt};
use log::{debug, warn};
use std::rc::Rc;

#[derive(Debug)]
pub struct NodeServerBinding {
    pub node_ref: fdf::NodeControlHandle,
    server_task: Option<fasync::Task<()>>,
    close_listener_task: Option<fasync::Task<()>>,
}

impl NodeServerBinding {
    pub fn new(
        node_ref: fdf::NodeControlHandle,
        server_task: Option<fasync::Task<()>>,
        close_listener_task: Option<fasync::Task<()>>,
    ) -> Self {
        NodeServerBinding { node_ref, server_task, close_listener_task }
    }

    pub fn close(self) {
        self.node_ref.shutdown_with_epitaph(Ok(()));
    }
}

impl Drop for NodeServerBinding {
    fn drop(&mut self) {
        if let Some(t) = self.close_listener_task.take() {
            drop(t.abort());
        }

        if let Some(t) = self.server_task.take() {
            drop(t.abort());
        }
    }
}

#[derive(Debug)]
pub struct NodeControllerServerBinding {
    pub node_controller_ref: fdf::NodeControllerControlHandle,
    server_task: Option<fasync::Task<()>>,
    close_listener_task: Option<fasync::Task<()>>,
}

impl NodeControllerServerBinding {
    pub fn new(
        node_controller_ref: fdf::NodeControllerControlHandle,
        server_task: Option<fasync::Task<()>>,
        close_listener_task: Option<fasync::Task<()>>,
    ) -> Self {
        NodeControllerServerBinding { node_controller_ref, server_task, close_listener_task }
    }

    pub fn close(self) {
        self.node_controller_ref.shutdown_with_epitaph(Ok(()));
    }
}

impl Drop for NodeControllerServerBinding {
    fn drop(&mut self) {
        if let Some(t) = self.close_listener_task.take() {
            drop(t.abort());
        }

        if let Some(t) = self.server_task.take() {
            drop(t.abort());
        }
    }
}

#[derive(Debug)]
pub struct ComponentRunnerComponentControllerServerBinding {
    pub control_handle: frunner::ComponentControllerControlHandle,
    server_task: Option<fasync::Task<()>>,
    close_listener_task: Option<fasync::Task<()>>,
}

impl ComponentRunnerComponentControllerServerBinding {
    pub fn new(
        control_handle: frunner::ComponentControllerControlHandle,
        server_task: Option<fasync::Task<()>>,
        close_listener_task: Option<fasync::Task<()>>,
    ) -> Self {
        ComponentRunnerComponentControllerServerBinding {
            control_handle,
            server_task,
            close_listener_task,
        }
    }
}

impl Drop for ComponentRunnerComponentControllerServerBinding {
    fn drop(&mut self) {
        if let Some(t) = self.close_listener_task.take() {
            drop(t.abort());
        }

        if let Some(t) = self.server_task.take() {
            drop(t.abort());
        }
    }
}

pub struct ComponentControllerClientBinding {
    pub component_controller_proxy: fcomponent::ControllerProxy,
    close_listener_task: Option<fasync::Task<()>>,
}

impl ComponentControllerClientBinding {
    pub fn new(
        component_controller_proxy: fcomponent::ControllerProxy,
        close_listener_task: Option<fasync::Task<()>>,
    ) -> Self {
        ComponentControllerClientBinding { component_controller_proxy, close_listener_task }
    }
}

impl Drop for ComponentControllerClientBinding {
    fn drop(&mut self) {
        if let Some(t) = self.close_listener_task.take() {
            drop(t.abort());
        }
    }
}

#[derive(Debug)]
pub struct DriverHostClientBinding {
    pub driver_host_proxy: fdh::DriverProxy,
    close_listener_task: Option<fasync::Task<()>>,
}

impl DriverHostClientBinding {
    pub fn new(
        driver_host_proxy: fdh::DriverProxy,
        close_listener_task: Option<fasync::Task<()>>,
    ) -> Self {
        DriverHostClientBinding { driver_host_proxy, close_listener_task }
    }
}

impl Drop for DriverHostClientBinding {
    fn drop(&mut self) {
        if let Some(t) = self.close_listener_task.take() {
            drop(t.abort());
        }
    }
}

impl Node {
    pub async fn set_created_info(
        self: &Rc<Self>,
        proxy: fcomponent::ControllerProxy,
        handle_info: fidl_fuchsia_process::HandleInfo,
        receiver: StartRequestReceiver,
    ) {
        let self_clone = self.clone();

        let (sender, local_receiver) = oneshot::channel();
        self.scope.spawn_local(async move {
            let stream = proxy.take_event_stream();
            let weak_node = self_clone.weak_from_this();
            let close_listener_task = Some(fasync::Task::local(async move {
                // There is no specific event in this stream, we just want to know when it is closed.
                stream.for_each(|_| async {}).await;
                if let Some(node) = weak_node.upgrade() {
                    node.take_component();
                    node.on_component_controller_closed();
                }
            }));
            self_clone.set_component(NodeComponent {
                controller: ComponentControllerClientBinding::new(proxy, close_listener_task),
                start_handles: Some(vec![handle_info]),
                start_request_receiver: Some(receiver),
            });
            sender.send(()).unwrap();
        });
        local_receiver.await.unwrap();
    }

    pub(crate) fn serve_node(
        self: &Rc<Self>,
        node: ServerEnd<fdf::NodeMarker>,
    ) -> NodeServerBinding {
        let (mut stream, control_handle) = node.into_stream_and_control_handle();
        let control_handle_clone = control_handle.clone();
        let weak_self = Rc::downgrade(self);

        let close_listener_task = Some(fasync::Task::local(async move {
            let result = control_handle_clone.on_closed().await;
            if let Some(this) = weak_self.upgrade() {
                this.on_node_closed(result);
            }
        }));

        let weak_self = Rc::downgrade(self);
        let server_task = Some(fasync::Task::local(async move {
            while let Some(Ok(msg)) = stream.next().await {
                let Some(this) = weak_self.upgrade() else {
                    break;
                };
                match msg {
                    fdf::NodeRequest::AddChild { args, controller, node, responder } => {
                        let _ = responder
                            .send(this.add_child(args, Some(controller), node).await.map(|_| ()));
                    }
                    _ => {
                        log::warn!("received unknown method.");
                    }
                };
            }
        }));

        NodeServerBinding::new(control_handle, server_task, close_listener_task)
    }

    pub(crate) fn serve_node_controller(
        self: &Rc<Self>,
        node_controller: ServerEnd<fdf::NodeControllerMarker>,
    ) -> NodeControllerServerBinding {
        let (mut stream, control_handle) = node_controller.into_stream_and_control_handle();

        let weak_self = Rc::downgrade(self);
        let control_handle_clone = control_handle.clone();
        let close_listener_task = Some(fasync::Task::local(async move {
            let _ = control_handle_clone.on_closed().await;
            if let Some(this) = weak_self.upgrade() {
                this.take_node_controller();
            }
        }));

        let weak_self = Rc::downgrade(self);
        let server_task = Some(fasync::Task::local(async move {
            while let Some(Ok(msg)) = stream.next().await {
                let Some(this) = weak_self.upgrade() else {
                    break;
                };
                match msg {
                    fdf::NodeControllerRequest::RequestBind { payload, responder } => {
                        let result = this
                            .bind_helper(
                                payload.force_rebind.unwrap_or(false),
                                payload.driver_url_suffix,
                            )
                            .await;
                        let _ = responder.send(result.map_err(zx::Status::into_raw));
                    }
                    fdf::NodeControllerRequest::Remove { .. } => {
                        this.set_should_destroy_driver_component(true);
                        this.remove(RemovalSet::All, None);
                    }
                    fdf::NodeControllerRequest::WaitForDriver { responder } => {
                        let (tx, rx) = oneshot::channel();
                        this.wait_for_driver(tx);
                        this.scope.spawn_local(async move {
                            match rx.await {
                                Ok(result) => {
                                    let _ = responder.send(result.map_err(zx::Status::into_raw));
                                }
                                Err(_) => {
                                    let _ = responder.send(Err(zx::Status::CANCELED.into_raw()));
                                }
                            }
                        });
                    }
                    _ => {
                        log::warn!("received unknown method.");
                    }
                };
            }
        }));

        NodeControllerServerBinding::new(control_handle, server_task, close_listener_task)
    }

    pub(crate) fn serve_runner_component_controller(
        self: &Rc<Self>,
        runner_component_controller: ServerEnd<frunner::ComponentControllerMarker>,
    ) -> ComponentRunnerComponentControllerServerBinding {
        let (controller_stream, control_handle) =
            runner_component_controller.into_stream_and_control_handle();

        let weak_self = Rc::downgrade(self);
        let control_handle_clone = control_handle.clone();
        let close_listener_task = Some(fasync::Task::local(async move {
            let _ = control_handle_clone.on_closed().await;
            if let Some(this) = weak_self.upgrade() {
                this.take_node_controller();
                this.on_runner_component_controller_closed();
            }
        }));

        let weak_self = self.weak_from_this();
        let server_task = Some(fasync::Task::local(async move {
            let mut stream: frunner::ComponentControllerRequestStream = controller_stream;
            while let Ok(Some(s)) = stream.try_next().await {
                match s {
                    frunner::ComponentControllerRequest::Stop { .. } => {
                        if let Some(node) = weak_self.upgrade() {
                            debug!(
                                "Node: '{}' received stop from component framework",
                                node.make_component_moniker()
                            );
                            node.remove(RemovalSet::All, None);
                        }
                    }
                    frunner::ComponentControllerRequest::Kill { .. } => {
                        if let Some(node) = weak_self.upgrade() {
                            debug!(
                                "Node: '{}' received kill from component framework",
                                node.make_component_moniker()
                            );
                            node.remove(RemovalSet::All, None);
                        }
                    }
                    _ => {}
                }
            }
        }));

        ComponentRunnerComponentControllerServerBinding::new(
            control_handle,
            server_task,
            close_listener_task,
        )
    }

    pub(crate) fn serve_driver_host_client(
        &self,
        driver_host_proxy: fdh::DriverProxy,
    ) -> DriverHostClientBinding {
        let weak_self = self.weak_from_this();
        let mut driver_event_stream = driver_host_proxy.take_event_stream();
        let close_listener_task = Some(fasync::Task::local(async move {
            if let Some(event) = driver_event_stream.next().await {
                // The only valid way a driver host should shut down the Driver channel
                // is with the ZX_OK epitaph.
                // TODO(b/322235974): Increase the log severity to ERROR once we resolve the
                // component shutdown order in DriverTestRealm.
                let Err(e) = event;
                if let fidl::Error::ClientChannelClosed { epitaph, .. } = e
                    && epitaph.is_ok()
                {
                } else {
                    warn!("Node: driver channel shutdown with: {e}");
                }
            }

            let Some(this) = weak_self.upgrade() else {
                return;
            };
            this.clear_driver_host();

            let moniker = this.make_component_moniker();

            let shutdown_state = this.shutdown_state();
            if shutdown_state == ShutdownState::WaitingOnDriver {
                debug!("Node: {moniker}: driver channel had expected shutdown.");
                this.node_shutdown_coordinator.borrow_mut().check_node_state();
                return;
            }

            if this.host_restart_on_crash() {
                warn!("Restarting node {moniker} because of unexpected driver channel shutdown.");
                let host_name = this.driver_host_name_for_colocation();
                if let Err(e) = this.node_manager.destroy_driver_host(host_name).await {
                    log::error!("Failed to destroy old driver host during restart: {:?}", e);
                }
                if let Some(this) = weak_self.upgrade() {
                    this.restart_node();
                }
                return;
            }

            // If the driver fails to bind to the node, don't remove the node.
            if this.is_pending_bind() {
                debug!("Node: {moniker}: driver channel closed during binding.");
                return;
            }

            warn!("Removing node {moniker} because of unexpected driver channel shutdown.");
            this.remove(RemovalSet::All, None);
        }));

        DriverHostClientBinding::new(driver_host_proxy, close_listener_task)
    }

    fn on_node_closed(self: &Rc<Self>, result: Result<zx::Signals, fidl::Status>) {
        // If the unbind is initiated from us, we don't need to do anything to handle
        // the closure.
        if self.node_shutdown_coordinator.borrow().is_shutting_down() {
            return;
        }

        // If the driver fails to bind to the node, don't remove the node.
        if self.is_pending_bind() {
            warn!("The driver for node {} failed to bind.", self.name());
            return;
        }

        if self.shutdown_state() == ShutdownState::Running {
            // If the node is running but this node closure has happened, then we want to restart
            // the node if it has the host_restart_on_crash_ enabled on it.
            if self.host_restart_on_crash() {
                warn!("Restarting node {} due to node closure while running.", self.name());
                let host_name = self.driver_host_name_for_colocation();
                let node_manager = self.node_manager.clone_box();
                let weak_self = Rc::downgrade(self);
                self.scope.spawn_local(async move {
                    if let Err(e) = node_manager.destroy_driver_host(host_name).await {
                        log::error!("Failed to destroy old driver host during restart: {:?}", e);
                    }
                    if let Some(this) = weak_self.upgrade() {
                        this.restart_node();
                    }
                });
                return;
            }

            warn!(
                "fdf::Node binding for node {} closed while the node was running: {:?}",
                self.name(),
                result
            );
        }

        self.remove(RemovalSet::All, None);
    }

    fn on_component_controller_closed(self: &Rc<Self>) {
        if self.node_shutdown_coordinator.borrow().node_state() == &ShutdownState::WaitingOnDestroy
        {
            debug!(
                "Node '{}': component controller channel had expected shutdown.",
                self.make_component_moniker()
            );
            self.node_shutdown_coordinator.borrow_mut().check_node_state();
            return;
        }

        warn!(
            "Node '{}': unexpected component controller channel shutdown. in state {:?}",
            self.make_component_moniker(),
            self.node_shutdown_coordinator.borrow().node_state()
        );
    }

    fn on_runner_component_controller_closed(self: &Rc<Self>) {
        let node_state = self.shutdown_state();
        if self.has_driver_component() {
            if node_state == ShutdownState::WaitingOnDriverComponent {
                debug!(
                    "Node '{}': runner component controller channel had expected close",
                    self.make_component_moniker()
                );
                self.set_driver_stopped();
                self.node_shutdown_coordinator.borrow_mut().check_node_state();
            } else {
                warn!(
                    "Node '{}': runner component controller channel had unexpected close",
                    self.make_component_moniker()
                );
                self.set_driver_stopped();
                self.remove(RemovalSet::All, None);
            }
        }
    }
}
