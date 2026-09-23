// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::node::Node;
use crate::serve::{
    ComponentRunnerComponentControllerServerBinding, DriverHostClientBinding, NodeServerBinding,
};
use driver_manager_types::NodeType;
use fidl_fuchsia_component_sandbox::CapabilityId;
use std::rc::Weak;

#[derive(Clone, Debug, PartialEq)]
pub enum DriverState {
    Binding,
    Running,
    Stopped,
}

pub enum NodeTypeVariant {
    Normal {
        parent: Weak<Node>,
    },
    Composite {
        parents: Vec<Weak<Node>>,
        #[allow(unused)]
        parents_names: Vec<String>,
        primary_index: u32,
    },
}

impl PartialEq<NodeType> for NodeTypeVariant {
    fn eq(&self, other: &NodeType) -> bool {
        match self {
            NodeTypeVariant::Normal { .. } => *other == NodeType::Normal,
            NodeTypeVariant::Composite { .. } => *other == NodeType::Composite,
        }
    }
}

#[derive(Debug)]
pub struct DriverComponent {
    pub driver_url: String,
    component_instance: zx::Event,
    component_instance_koid: zx::Koid,
    runner_component_controller: Option<ComponentRunnerComponentControllerServerBinding>,
    node_server_binding: Option<NodeServerBinding>,
    pub driver_client_binding: Option<DriverHostClientBinding>,
    pub state: DriverState,
}

impl DriverComponent {
    pub fn new(
        driver_url: String,
        component_instance: zx::Event,
        component_instance_koid: zx::Koid,
        runner_component_controller: Option<ComponentRunnerComponentControllerServerBinding>,
        node_server_binding: Option<NodeServerBinding>,
        driver_client_binding: Option<DriverHostClientBinding>,
        state: DriverState,
    ) -> Self {
        Self {
            driver_url,
            component_instance,
            component_instance_koid,
            runner_component_controller,
            node_server_binding,
            driver_client_binding,
            state,
        }
    }

    pub fn instance_koid(&self) -> zx::Koid {
        self.component_instance_koid
    }

    pub fn duplicate_instance_handle(&self) -> zx::Event {
        self.component_instance.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap()
    }

    pub fn close_node(&mut self) {
        if let Some(binding) = self.node_server_binding.take() {
            binding.close()
        }
    }

    pub fn send_on_stop(&self) {
        if let Err(e) =
            self.runner_component_controller.as_ref().unwrap().control_handle.send_on_stop(
                fidl_fuchsia_component_runner::ComponentStopInfo { ..Default::default() },
            )
        {
            log::error!("Failed to stop driver component: {}", e);
        }
    }
}

#[derive(Debug)]
pub enum NodeState {
    Unbound,
    Starting { driver_url: String },
    OwnedByParent { node_server_binding: Option<NodeServerBinding> },
    CompositeParent,
    DriverComponent(DriverComponent),
    Quarantined { driver_url: String },
}

#[derive(Clone)]
pub enum NodeDictionary {
    None,

    // Node specific dictionary that contains offers that the node provides that are type
    // |DictionaryOffer|
    Standard(CapabilityId),

    // Passed down the node tree to children, as it contains non-driver protocol
    // capabilities for testing (the ones injected in offer_injection). This is only used for
    // system testing at the moment through |restart_with_dictionary|.
    Subtree(CapabilityId),
}
