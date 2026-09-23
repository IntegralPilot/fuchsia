// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use driver_manager_node::Node;
use futures::channel::mpsc;
use log::warn;
use std::collections::HashMap;
use std::rc::Weak;

pub(crate) struct BindNodeSet {
    orphaned_nodes: HashMap<String, Weak<Node>>,
    multibind_nodes: HashMap<String, Weak<Node>>,
    new_orphaned_nodes: HashMap<String, Weak<Node>>,
    new_multibind_nodes: HashMap<String, Weak<Node>>,
    on_bind_state_changed: Option<mpsc::UnboundedSender<()>>,
    is_bind_ongoing: bool,
}

impl Default for BindNodeSet {
    fn default() -> Self {
        Self::new()
    }
}

impl BindNodeSet {
    pub(crate) fn new() -> Self {
        Self {
            orphaned_nodes: HashMap::new(),
            multibind_nodes: HashMap::new(),
            new_orphaned_nodes: HashMap::new(),
            new_multibind_nodes: HashMap::new(),
            on_bind_state_changed: None,
            is_bind_ongoing: false,
        }
    }

    pub(crate) fn set_on_bind_state_changed(&mut self, sender: mpsc::UnboundedSender<()>) {
        self.on_bind_state_changed = Some(sender);
    }

    pub(crate) fn start_next_bind_process(&mut self) {
        if self.is_bind_ongoing {
            self.complete_ongoing_bind();
        }
        self.new_orphaned_nodes = self.orphaned_nodes.clone();
        self.is_bind_ongoing = true;
        self.notify_bind_state();
    }

    pub(crate) fn end_bind_process(&mut self) {
        assert!(self.is_bind_ongoing);
        self.complete_ongoing_bind();
        self.is_bind_ongoing = false;
        self.notify_bind_state();
    }

    fn complete_ongoing_bind(&mut self) {
        assert!(self.is_bind_ongoing);
        self.orphaned_nodes = std::mem::take(&mut self.new_orphaned_nodes);
        self.multibind_nodes.extend(std::mem::take(&mut self.new_multibind_nodes));
    }

    fn notify_bind_state(&self) {
        if let Some(sender) = &self.on_bind_state_changed
            && let Err(e) = sender.unbounded_send(())
        {
            warn!("Failed to send bind state changed notification: {}", e);
        }
    }

    pub(crate) fn add_orphaned_node(&mut self, node: &Node) {
        let moniker = node.make_component_moniker();
        assert!(!self.multibind_contains(&moniker));
        if self.is_bind_ongoing {
            self.new_orphaned_nodes.insert(moniker, node.weak_from_this());
        } else {
            self.orphaned_nodes.insert(moniker, node.weak_from_this());
        }
    }

    pub(crate) fn remove_orphaned_node(&mut self, node_moniker: &str) {
        if self.is_bind_ongoing {
            self.new_orphaned_nodes.remove(node_moniker);
        } else {
            self.orphaned_nodes.remove(node_moniker);
        }
    }

    pub(crate) fn add_or_move_multibind_node(&mut self, node: &Node) {
        let moniker = node.make_component_moniker();
        self.remove_orphaned_node(&moniker);
        if self.is_bind_ongoing {
            self.new_multibind_nodes.insert(moniker, node.weak_from_this());
        } else {
            self.multibind_nodes.insert(moniker, node.weak_from_this());
        }
    }

    pub(crate) fn multibind_contains(&self, node_moniker: &str) -> bool {
        self.multibind_nodes.contains_key(node_moniker)
            || self.new_multibind_nodes.contains_key(node_moniker)
    }

    pub(crate) fn is_bind_ongoing(&self) -> bool {
        self.is_bind_ongoing
    }

    pub(crate) fn num_of_available_nodes(&self) -> usize {
        self.orphaned_nodes.len() + self.multibind_nodes.len()
    }

    pub(crate) fn current_orphaned_nodes(&self) -> HashMap<String, Weak<Node>> {
        self.orphaned_nodes.clone()
    }

    pub(crate) fn current_multibind_nodes(&self) -> HashMap<String, Weak<Node>> {
        self.multibind_nodes.clone()
    }
}
