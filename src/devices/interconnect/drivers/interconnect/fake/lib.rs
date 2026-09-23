// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fdf_component::{
    Driver, DriverContext, DriverError, Node, NodeBuilder, ServiceOffer, driver_register,
};
use fidl::endpoints::ClientEnd;
use fidl_fuchsia_driver_framework::NodeControllerMarker;
use fidl_fuchsia_hardware_interconnect as icc;
use fidl_fuchsia_interconnect_test as ft;
use fuchsia_component::server::ServiceFs;
use futures::{StreamExt, TryStreamExt};
use log::{info, warn};
use zx::Status;

driver_register!(FakeInterconnectDriver);

#[allow(unused)]
struct FakeInterconnectDriver {
    node: Node,
    child_controller: ClientEnd<NodeControllerMarker>,
    scope: fuchsia_async::Scope,
}

impl Driver for FakeInterconnectDriver {
    const NAME: &str = "fake_interconnect";

    async fn start(mut context: DriverContext) -> Result<Self, DriverError> {
        let node = context.take_node()?;

        let mut outgoing = ServiceFs::new();

        let offer = ServiceOffer::new()
            .add_default_named(&mut outgoing, Self::NAME, move |req| {
                let icc::ServiceRequest::Device(service) = req;
                service
            })
            .build_zircon_offer();

        let node_args = NodeBuilder::new(Self::NAME).add_offer(offer).build();
        let child_controller = node.add_child(node_args).await?;
        context.serve_outgoing(&mut outgoing)?;

        let waiter: ft::WaiterProxy = context.incoming.connect_protocol().unwrap();
        let scope = fuchsia_async::Scope::new_with_name("outgoing_directory");
        scope.spawn_local(async move {
            outgoing
                .for_each_concurrent(None, move |request| {
                    let waiter = waiter.clone();
                    async move {
                        let mut connection = Connection { waiter };
                        connection.run_device_server(request).await;
                    }
                })
                .await;
        });

        Ok(Self { node, child_controller, scope })
    }

    async fn stop(&self) {}
}

struct Connection {
    waiter: ft::WaiterProxy,
}

impl Connection {
    fn set_nodes_bandwidth(
        &self,
        _nodes: Vec<icc::NodeBandwidth>,
    ) -> Result<&[icc::AggregatedBandwidth], Status> {
        info!("set_nodes_bandwidth called");
        self.waiter.ack().unwrap();
        Ok(&[])
    }

    fn get_node_graph(&self) -> (Vec<icc::Node>, Vec<icc::Edge>) {
        info!("get_node_graph called");
        let nodes = vec![
            icc::Node {
                id: Some(0),
                name: Some("zero".to_string()),
                interconnect_name: None,
                ..Default::default()
            },
            icc::Node {
                id: Some(1),
                name: Some("one".to_string()),
                interconnect_name: None,
                ..Default::default()
            },
            icc::Node {
                id: Some(2),
                name: Some("two".to_string()),
                interconnect_name: None,
                ..Default::default()
            },
            icc::Node {
                id: Some(3),
                name: Some("three".to_string()),
                interconnect_name: None,
                ..Default::default()
            },
        ];
        let edges = vec![
            icc::Edge {
                src_node_id: Some(0),
                dst_node_id: Some(1),
                weight: Some(1),
                ..Default::default()
            },
            icc::Edge {
                src_node_id: Some(1),
                dst_node_id: Some(2),
                weight: Some(1),
                ..Default::default()
            },
            icc::Edge {
                src_node_id: Some(1),
                dst_node_id: Some(3),
                weight: Some(1),
                ..Default::default()
            },
            icc::Edge {
                src_node_id: Some(2),
                dst_node_id: Some(1),
                weight: Some(1),
                ..Default::default()
            },
        ];
        (nodes, edges)
    }

    fn get_path_endpoints(&self) -> Vec<icc::PathEndpoints> {
        info!("get_path_endpoints called");
        vec![
            icc::PathEndpoints {
                name: Some("path_a".to_string()),
                id: Some(0),
                src_node_id: Some(0),
                dst_node_id: Some(2),
                ..Default::default()
            },
            icc::PathEndpoints {
                name: Some("path_b".to_string()),
                id: Some(1),
                src_node_id: Some(0),
                dst_node_id: Some(3),
                ..Default::default()
            },
            icc::PathEndpoints {
                name: Some("path_c".to_string()),
                id: Some(2),
                src_node_id: Some(2),
                dst_node_id: Some(3),
                ..Default::default()
            },
        ]
    }

    async fn run_device_server(&mut self, mut service: icc::DeviceRequestStream) {
        use icc::DeviceRequest::*;
        while let Some(req) = service.try_next().await.unwrap() {
            match req {
                SetNodesBandwidth { nodes, responder, .. } => {
                    responder.send(self.set_nodes_bandwidth(nodes).map_err(Status::into_raw))
                }
                GetNodeGraph { responder, .. } => {
                    let (nodes, edges) = self.get_node_graph();
                    responder.send(&nodes, &edges)
                }
                GetPathEndpoints { responder, .. } => responder.send(&self.get_path_endpoints()),
                // Ignore unknown requests.
                _ => {
                    warn!("Received unknown path request");
                    Ok(())
                }
            }
            .unwrap();
        }
    }
}
