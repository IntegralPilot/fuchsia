// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! This module provides a mock for the driver framework provided Node and NodeController.

use anyhow::Result;
use fidl::endpoints::ServerEnd;
use fidl_fuchsia_device_fs as fdf_devfs;
use fidl_fuchsia_driver_framework as fdf_fidl;
use fuchsia_async as fasync;
use fuchsia_sync::Mutex;
use futures::TryStreamExt;
use std::collections::HashMap;
use std::sync::{Arc, Weak};
use zx;

/// This represents a node in the NodeManage.
pub type NodeId = usize;

/// The TestNode backs the driver framework provided protocols Node and NodeController.
struct TestNode {
    id: NodeId,
    context: Weak<NodeManager>,
    name: String,
    children: HashMap<String, NodeId>,
    properties: Vec<fdf_fidl::NodeProperty2>,
    parent: Option<NodeId>,
    devfs_connector_client: Option<fdf_devfs::ConnectorProxy>,
    scope: fasync::Scope,
}

impl TestNode {
    fn serve_node(&self, server_end: ServerEnd<fdf_fidl::NodeMarker>) {
        let mut stream = server_end.into_stream();
        let node_id = self.id;
        let context = self.context.clone();
        self.scope.spawn(async move {
            while let Some(request) = stream.try_next().await.unwrap() {
                if let fdf_fidl::NodeRequest::AddChild { args, controller, node, responder } =
                    request
                {
                    let name = args.name.as_ref().unwrap();
                    if let Some(manager) = context.upgrade() {
                        let _ = manager.new_node(
                            name,
                            Some(node_id),
                            args.properties2,
                            Some(controller),
                            node,
                            args.devfs_args,
                        );
                    }

                    let _ = responder.send(Ok(()));
                }
            }

            if let Some(manager) = context.upgrade() {
                manager.remove_node(&node_id);
            }
        });
    }

    fn serve_controller(&self, server_end: ServerEnd<fdf_fidl::NodeControllerMarker>) {
        let mut stream = server_end.into_stream();
        let node_id = self.id;
        let context = self.context.clone();
        self.scope.spawn(async move {
            while let Some(request) = stream.try_next().await.unwrap() {
                match request {
                    fdf_fidl::NodeControllerRequest::Remove { control_handle: _ } => {
                        if let Some(manager) = context.upgrade() {
                            manager.remove_node(&node_id);
                        }
                    }
                    fdf_fidl::NodeControllerRequest::RequestBind { payload: _, responder } => {
                        let _ = responder.send(Ok(()));
                    }
                    _ => (),
                }
            }
        });
    }
}

pub(crate) struct NodeManager {
    nodes: Mutex<HashMap<NodeId, TestNode>>,
    next_id: Mutex<NodeId>,
}

/// A handle to nodes running inside the test.
pub struct NodeHandle {
    manager: Weak<NodeManager>,
    id: NodeId,
}

impl NodeHandle {
    pub(crate) fn new(manager: Weak<NodeManager>, id: NodeId) -> Self {
        Self { manager, id }
    }

    /// Gets the name of the node.
    pub fn name(&self) -> String {
        self.manager.upgrade().expect("manager").name(&self.id)
    }

    /// Gets the children of the node.
    pub fn children(&self) -> HashMap<String, NodeHandle> {
        self.manager
            .upgrade()
            .expect("manager")
            .children(&self.id)
            .into_iter()
            .map(|(n, id)| (n, NodeHandle::new(self.manager.clone(), id)))
            .collect()
    }

    /// Gets the properties of the node.
    pub fn properties(&self) -> Vec<fdf_fidl::NodeProperty2> {
        self.manager.upgrade().expect("manager").properties(&self.id)
    }

    /// Gets the parent of the node.
    pub fn parent(&self) -> Option<NodeHandle> {
        self.manager
            .upgrade()
            .expect("manager")
            .parent(&self.id)
            .map(|id| NodeHandle::new(self.manager.clone(), id))
    }

    /// Connects to the node's devfs entry.
    pub async fn connect_to_device(&self) -> Result<zx::Channel, anyhow::Error> {
        self.manager.upgrade().expect("manager").connect_to_device(self.id).await
    }
}

impl NodeManager {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self { nodes: Mutex::new(HashMap::new()), next_id: Mutex::new(0) })
    }

    pub(crate) fn create_root_node(
        self: &Arc<Self>,
        node: ServerEnd<fdf_fidl::NodeMarker>,
    ) -> NodeId {
        self.new_node("root", None, None, None, Some(node), None)
    }

    fn new_node(
        self: &Arc<Self>,
        name: &str,
        parent: Option<NodeId>,
        properties: Option<Vec<fdf_fidl::NodeProperty2>>,
        controller: Option<ServerEnd<fdf_fidl::NodeControllerMarker>>,
        node: Option<ServerEnd<fdf_fidl::NodeMarker>>,
        devfs_args: Option<fdf_fidl::DevfsAddArgs>,
    ) -> NodeId {
        let mut next_id = self.next_id.lock();
        let child_id = *next_id;
        *next_id += 1;
        drop(next_id);

        let devfs_connector_client = {
            if let Some(fdf_fidl::DevfsAddArgs { connector: Some(client), .. }) = devfs_args {
                Some(client.into_proxy())
            } else {
                None
            }
        };

        let child_node = TestNode {
            id: child_id,
            context: Arc::downgrade(self),
            name: name.to_string(),
            children: HashMap::new(),
            properties: properties.unwrap_or_default(),
            parent,
            devfs_connector_client,
            scope: fasync::Scope::new(),
        };

        if let Some(parent_id) = parent {
            let mut nodes = self.nodes.lock();
            nodes.get_mut(&parent_id).expect("parent").children.insert(name.to_string(), child_id);
            log::info!("adding child {name} to parent {parent_id}");
        }

        if let Some(controller) = controller {
            child_node.serve_controller(controller);
        }

        if let Some(node) = node {
            child_node.serve_node(node);
        }

        self.nodes.lock().insert(child_id, child_node);
        child_id
    }

    async fn connect_to_device(&self, node_id: NodeId) -> Result<zx::Channel, anyhow::Error> {
        let (client_end, server_end) = zx::Channel::create();
        let nodes = self.nodes.lock();
        let node = nodes.get(&node_id).expect("node");
        if let Some(connector) = node.devfs_connector_client.as_ref() {
            connector.connect(server_end).unwrap();
            Ok(client_end)
        } else {
            Err(anyhow::anyhow!("Devfs connector not found"))
        }
    }

    fn children(self: &Arc<Self>, node_id: &NodeId) -> HashMap<String, NodeId> {
        let nodes = self.nodes.lock();
        nodes.get(node_id).expect("node").children.clone()
    }

    fn parent(&self, node_id: &NodeId) -> Option<NodeId> {
        let nodes = self.nodes.lock();
        nodes.get(node_id).expect("node").parent
    }

    fn name(&self, node_id: &NodeId) -> String {
        let nodes = self.nodes.lock();
        nodes.get(node_id).expect("node").name.clone()
    }

    fn properties(&self, node_id: &NodeId) -> Vec<fdf_fidl::NodeProperty2> {
        let nodes = self.nodes.lock();
        nodes.get(node_id).expect("node").properties.clone()
    }

    fn remove_node(self: &Arc<Self>, node_id: &NodeId) {
        let children = self.children(node_id);
        for child_id in children.values() {
            self.remove_node(child_id);
        }

        if let Some(parent_id) = self.parent(node_id) {
            let name = self.name(node_id);
            let mut nodes = self.nodes.lock();
            nodes.get_mut(&parent_id).expect("parent").children.remove(&name);
        }

        self.nodes.lock().remove(node_id);
    }
}
