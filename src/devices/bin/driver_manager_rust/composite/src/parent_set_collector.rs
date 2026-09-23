// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use driver_manager_node::{Node, NodeManager, NodeProperty, NodePropertyEntry};
use log::info;
use std::rc::{Rc, Weak};

pub(crate) struct ParentSetCollector {
    composite_name: String,
    parents: Vec<Weak<Node>>,
    parent_names: Vec<String>,
    parent_properties: Vec<NodePropertyEntry>,
    primary_index: u32,
    completed_composite_node: Option<Weak<Node>>,
    driver_host_name_for_colocation: String,
}

impl ParentSetCollector {
    pub(crate) fn new(
        composite_name: String,
        parent_names: Vec<String>,
        primary_index: u32,
        driver_host_name_for_colocation: String,
    ) -> Self {
        let num_parents = parent_names.len();
        Self {
            composite_name,
            parents: {
                let mut v = Vec::with_capacity(num_parents);
                for _ in 0..num_parents {
                    v.push(Weak::new());
                }
                v
            },
            parent_names,
            parent_properties: vec![
                NodePropertyEntry { name: "".to_string(), properties: vec![] };
                num_parents
            ],
            primary_index,
            completed_composite_node: None,
            driver_host_name_for_colocation,
        }
    }

    pub(crate) fn add_node(
        &mut self,
        index: u32,
        node_properties: Vec<NodeProperty>,
        node: Weak<Node>,
    ) -> Result<(), zx::Status> {
        let index = index as usize;
        if index >= self.parents.len() {
            return Err(zx::Status::OUT_OF_RANGE);
        }
        if self.parents[index].upgrade().is_some() {
            return Err(zx::Status::ALREADY_BOUND);
        }
        self.parents[index] = node.clone();
        self.parent_properties[index] = NodePropertyEntry {
            name: self.parent_names[index].clone(),
            properties: node_properties,
        };

        if let Some(node_ptr) = node.upgrade() {
            node_ptr.mark_as_composite_parent();
        }

        Ok(())
    }

    pub(crate) fn release_nodes(&self) {
        for node in &self.parents {
            if let Some(node_ptr) = node.upgrade() {
                node_ptr.unmark_as_composite_parent();
            }
        }
    }

    pub(crate) fn try_to_assemble(
        &mut self,
        node_manager: Box<dyn NodeManager>,
    ) -> Result<Rc<Node>, zx::Status> {
        if let Some(node) = &self.completed_composite_node
            && node.upgrade().is_some()
        {
            return Err(zx::Status::ALREADY_EXISTS);
        }

        if self.parents.iter().any(|node| node.upgrade().is_none()) {
            return Err(zx::Status::SHOULD_WAIT);
        }

        let result = Node::create_composite_node(
            &self.composite_name,
            self.parents.clone(),
            self.parent_names.clone(),
            &self.parent_properties,
            node_manager,
            self.driver_host_name_for_colocation.clone(),
            self.primary_index,
        );

        match result {
            Ok(node) => {
                info!(
                    "Built composite node '{}' for completed composite node spec",
                    self.composite_name
                );
                self.completed_composite_node = Some(Rc::downgrade(&node));
                Ok(node)
            }
            Err(e) => Err(e),
        }
    }

    pub(crate) fn get_parent_topological_paths(&self) -> Vec<Option<String>> {
        self.parents
            .iter()
            .map(|node| node.upgrade().map(|n| n.make_topological_path(false)))
            .collect()
    }

    pub(crate) fn get_parent_monikers(&self) -> Vec<Option<String>> {
        self.parents.iter().map(|node| node.upgrade().map(|n| n.make_component_moniker())).collect()
    }

    pub(crate) fn completed_composite_node(&self) -> Option<Weak<Node>> {
        self.completed_composite_node.clone()
    }
}
