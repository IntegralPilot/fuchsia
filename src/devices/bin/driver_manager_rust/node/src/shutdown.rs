// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::node::Node;
use async_trait::async_trait;
use driver_manager_driver_host::DriverHost;
use driver_manager_shutdown::{
    NodeInfo, NodeShutdownBridge, NodeShutdownCoordinator, ShutdownIntent, ShutdownNode,
};
use driver_manager_types::{Collection, ShutdownState};
use log::{debug, error, warn};
use std::rc::{Rc, Weak};

pub struct NodeBridge(Weak<Node>);

impl NodeBridge {
    pub fn new(node: Weak<Node>) -> Self {
        Self(node)
    }
}

#[async_trait(?Send)]
impl ShutdownNode for Node {
    fn get_shutdown_coordinator(&self) -> std::cell::RefMut<'_, NodeShutdownCoordinator> {
        self.node_shutdown_coordinator.borrow_mut()
    }
    fn name(&self) -> &str {
        &self.name
    }
    async fn finish_shutdown(&self) {
        if let Some(this) = self.weak_self.upgrade() {
            this.finish_shutdown().await;
        }
    }
    fn schedule_post_shutdown(&self, intent: ShutdownIntent) {
        if let Some(this) = self.weak_self.upgrade() {
            this.schedule_post_shutdown(intent);
        }
    }
    fn set_should_destroy_driver_component(&self, val: bool) {
        self.set_should_destroy_driver_component(val);
    }
}

#[async_trait(?Send)]
impl NodeShutdownBridge for NodeBridge {
    fn get_removal_tracker_info(&self, shutdown_state: ShutdownState) -> NodeInfo {
        if let Some(node) = self.0.upgrade() {
            NodeInfo {
                name: node.make_component_moniker(),
                driver_url: node.driver_url(),
                collection: node.collection(),
                state: shutdown_state,
                host: node.driver_host(),
            }
        } else {
            // Should not happen
            NodeInfo {
                name: "".to_string(),
                driver_url: "".to_string(),
                collection: Collection::None,
                state: ShutdownState::Stopped,
                host: None,
            }
        }
    }
    fn stop_driver(&self) {
        if let Some(node) = self.0.upgrade() {
            node.stop_driver();
        }
    }
    fn stop_driver_component(&self) {
        if let Some(node) = self.0.upgrade() {
            node.stop_driver_component();
        }
    }
    fn is_pending_bind(&self) -> bool {
        if let Some(node) = self.0.upgrade() { node.is_pending_bind() } else { false }
    }
    fn has_children(&self) -> bool {
        if let Some(node) = self.0.upgrade() { !node.children().is_empty() } else { false }
    }
    fn has_driver(&self) -> bool {
        if let Some(node) = self.0.upgrade() { node.has_driver() } else { false }
    }
    fn has_driver_component(&self) -> bool {
        if let Some(node) = self.0.upgrade() { node.has_driver_component() } else { false }
    }
    fn has_driver_component_controller(&self) -> bool {
        if let Some(node) = self.0.upgrade() {
            node.has_component_controller_proxy()
        } else {
            false
        }
    }
    fn maybe_destroy_driver_component(&self, intent: ShutdownIntent) -> bool {
        if let Some(node) = self.0.upgrade() {
            node.maybe_destroy_driver_component(intent)
        } else {
            false
        }
    }
    fn get_driver_host(&self) -> Option<Rc<dyn DriverHost>> {
        let node = self.0.upgrade()?;
        node.driver_host()
    }
    fn collection(&self) -> Collection {
        if let Some(node) = self.0.upgrade() { node.collection() } else { Collection::None }
    }
    fn children(&self) -> Vec<Rc<dyn ShutdownNode>> {
        if let Some(node) = self.0.upgrade() {
            node.children().into_iter().map(|c| c as Rc<dyn ShutdownNode>).collect()
        } else {
            vec![]
        }
    }
    fn get_weak_node(&self) -> Weak<dyn ShutdownNode> {
        self.0.clone() as Weak<dyn ShutdownNode>
    }
}

impl Node {
    fn remove_child(&self, child: &Rc<Node>) {
        log::debug!("RemoveChild {} from parent {}", child.name(), self.name());
        if self.remove_child_from_children(child) {
            self.node_shutdown_coordinator.borrow_mut().check_node_state();
        }
    }

    async fn finish_shutdown(self: &Rc<Self>) {
        log::debug!("Node: {} finishing shutdown", self.make_component_moniker());

        if let Some(koid) = self.token_koid()
            && let Some(attributor) = self.node_manager.memory_attributor()
        {
            attributor.remove_driver(koid.raw_koid());
        }

        if let Some(b) = self.take_node_controller() {
            b.close()
        }

        self.finish_shutdown_state();

        for parent in self.parents() {
            if let Some(p) = parent.upgrade() {
                p.remove_child(self);
            } else if !self.is_root_node() {
                warn!("Parent freed before child {} could be removed from it", self.name());
            }
        }

        self.reset_node_type();
    }

    fn schedule_post_shutdown(self: &Rc<Self>, intent: ShutdownIntent) {
        let self_clone = self.clone();
        self.scope.spawn_local(async move {
            self_clone.post_shutdown(intent).await;
        });
    }

    async fn post_shutdown(self: &Rc<Self>, intent: ShutdownIntent) {
        if let Some(koid) = self.token_koid()
            && let Some(attributor) = self.node_manager.memory_attributor()
        {
            attributor.remove_driver(koid.raw_koid());
        }

        if intent == ShutdownIntent::Restart {
            log::debug!("Node '{}': finishing restart", self.make_component_moniker());
            self.finish_restart().await;
            return;
        } else if intent == ShutdownIntent::Quarantine {
            log::debug!("Node '{}': finishing quarantine", self.make_component_moniker());
            self.finish_quarantine();
            return;
        }

        if let Some(cb) = self.take_remove_complete_callback() {
            let _ = cb.send(());
        }

        if intent == ShutdownIntent::RebindComposite
            && let Some(completer) = self.take_composite_rebind_completer()
        {
            let _ = completer.send(Ok(()));
        }
    }

    async fn finish_restart(self: &Rc<Self>) {
        self.get_shutdown_coordinator().reset_shutdown();
        // Store previous url before we reset the state_.
        let previous_url = self.driver_url();

        // Perform cleanups for previous driver before we try to start the next driver.
        self.finish_restart_state();

        if self.host_restart_on_crash() {
            self.take_host();
        }

        let suffix = self.take_restart_driver_url_suffix();
        if let Some(suffix) = suffix {
            let tracker = self.create_bind_result_tracker(false);
            self.node_manager.bind_to_url(self, &suffix, tracker);
            return;
        }

        let package_type = self.driver_package_type();
        if let Err(e) = self.node_manager.start_driver(self, &previous_url, package_type) {
            error!("Failed to start driver '{}': {}", self.name(), e);
        }
    }

    fn finish_quarantine(self: &Rc<Self>) {
        self.get_shutdown_coordinator().reset_shutdown();
        assert!(self.is_quarantined(), "Node::state_ was not set to Quarantined");
    }

    fn stop_driver(&self) {
        if self.is_pending_bind() {
            warn!(
                "Stopping driver for node '{}' while bind is in process",
                self.make_component_moniker()
            );
            return;
        }

        if let Some(driver) = self.driver_client_binding()
            && let Err(e) = driver.stop()
        {
            error!("Node: {} failed to stop driver: {}", self.name(), e);
            self.clear_driver_host();
        }
    }

    fn maybe_destroy_driver_component(self: &Rc<Self>, intent: ShutdownIntent) -> bool {
        if self.should_destroy_driver_component()
            || intent != ShutdownIntent::Removal
            || self.has_remove_complete_callback()
            || self.is_root_node()
        {
            let Some(proxy) = self.component_controller_proxy() else {
                return false;
            };

            let name = self.make_component_moniker();

            let self_rc = self.clone();
            self.scope.spawn_local(async move {
                let e = proxy.destroy().await;
                match e {
                    Ok(inner) => match inner {
                        Ok(()) => (),
                        Err(e) => {
                            error!("Node: '{}' destroy failed: {:?}", name, e);
                        }
                    },
                    Err(e) => {
                        error!("Node: '{}' failed to send destroy: {:?}", name, e);
                    }
                }

                self_rc.set_should_destroy_driver_component(false);
            });

            return true;
        }

        debug!("Node: '{}' not destroying", self.make_component_moniker());
        false
    }
}
