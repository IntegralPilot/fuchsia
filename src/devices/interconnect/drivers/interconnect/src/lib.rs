// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::graph::{NodeGraph, NodeId, Path, PathId};
use fdf_component::inspect_publisher::ContentPublisher;
use fdf_component::{
    Driver, DriverContext, DriverError, Node, NodeBuilder, ServiceOffer, driver_register,
};
use fidl_fuchsia_driver_framework::NodeControllerProxy;
use fidl_next::{Client, ServerEnd};
use fuchsia_component::server::ServiceFs;
use fuchsia_inspect::Inspector;
use futures::channel::{mpsc, oneshot};
use futures::{SinkExt, StreamExt};
use log::{debug, error, info};
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::rc::Rc;
use zx::Status;

use fidl_next_fuchsia_hardware_interconnect::{self as icc, Device};
use fuchsia_async as fasync;
use fuchsia_trace as ftrace;

mod graph;

struct InterconnectDriver {
    #[expect(unused)]
    node: Node,
    scope: fasync::Scope,
}

driver_register!(InterconnectDriver);

struct Child {
    /// List of nodes following directed path from start of path to end of path.
    path: Path,
    tag: Option<u32>,
    #[expect(unused)]
    controller: NodeControllerProxy,
    device: fidl_next::Client<icc::Device>,
    average_bandwidth_bps: Cell<u64>,
    peak_bandwidth_bps: Cell<u64>,
}

impl Child {
    async fn set_bandwidth(
        &self,
        graph: &RefCell<NodeGraph>,
        synced: bool,
        average_bandwidth_bps: Option<u64>,
        peak_bandwidth_bps: Option<u64>,
        tag: Option<u32>,
    ) -> Result<(), Status> {
        let average_bandwidth_bps = average_bandwidth_bps.ok_or(Status::INVALID_ARGS)?;
        let peak_bandwidth_bps = peak_bandwidth_bps.ok_or(Status::INVALID_ARGS)?;
        let tag = tag.or(self.tag);

        self.average_bandwidth_bps.set(average_bandwidth_bps);
        self.peak_bandwidth_bps.set(peak_bandwidth_bps);

        ftrace::duration!(c"interconnect", c"set_bandwidth",
            "path" => self.path.name(),
            "average_bandwidth_bps" => average_bandwidth_bps,
            "peak_bandwidth_bps" => peak_bandwidth_bps);

        // If we've not hit sync_state yet, update the graph and return right away.
        if !synced {
            let mut graph = graph.borrow_mut();
            graph.update_path(&self.path, average_bandwidth_bps, peak_bandwidth_bps, tag);
            return Ok(());
        }

        let requests = {
            let mut graph = graph.borrow_mut();
            graph.update_path(&self.path, average_bandwidth_bps, peak_bandwidth_bps, tag);
            graph.make_bandwidth_requests(&self.path)
        };

        let result = self.device.set_nodes_bandwidth(&requests).await.map_err(|err| {
            error!("Failed to set bandwidth with {err}");
            Status::INTERNAL
        })?;

        let response = match result {
            Ok(response) => response,
            Err(err) => return Err(err.err().unwrap_or(Status::INTERNAL)),
        };

        graph.borrow_mut().update_stats(response.aggregated_bandwidth);

        // TODO(b/405206028): On failure, try to set old values?

        Ok(())
    }

    fn record_inspect(&self, node: &fuchsia_inspect::Node) {
        node.record_child(format!("{}-{}", self.path.name(), self.path.id()), |child| {
            child.record_uint("average_bandwidth_bps", self.average_bandwidth_bps.get());
            child.record_uint("peak_bandwidth_bps", self.peak_bandwidth_bps.get());
            self.path.record_inspect(&child);
        });
    }
}

struct ChildHandler<'a> {
    child: &'a Child,
    graph: &'a RefCell<NodeGraph>,
    sync_state: &'a Cell<bool>,
}

impl icc::PathLocalServerHandler for ChildHandler<'_> {
    async fn set_bandwidth(
        &mut self,
        request: fidl_next::Request<icc::path::SetBandwidth>,
        responder: fidl_next::Responder<icc::path::SetBandwidth>,
    ) {
        let payload = request.payload();
        let res = self
            .child
            .set_bandwidth(
                self.graph,
                self.sync_state.get(),
                payload.average_bandwidth_bps,
                payload.peak_bandwidth_bps,
                payload.tag,
            )
            .await;
        match res {
            Ok(()) => responder.respond(()).await.unwrap(),
            Err(err) => responder.respond_err(err).await.unwrap(),
        }
    }
}

struct PathService {
    scope: fasync::ScopeHandle,
    name: String,
    sender: mpsc::UnboundedSender<(fidl_next::ServerEnd<icc::Path>, String)>,
}

impl icc::PathServiceHandler for PathService {
    fn path(&self, server_end: fidl_next::ServerEnd<icc::Path>) {
        let name = self.name.clone();
        let mut sender = self.sender.clone();
        self.scope.spawn(async move {
            sender.send((server_end, name)).await.ok();
        });
    }
}

impl InterconnectDriver {
    async fn run_graph_service(
        self,
        mut inspect_publisher: ContentPublisher,
        device: Client<Device>,
        children: BTreeMap<String, Child>,
        graph: NodeGraph,
        sync_complete_rx: oneshot::Receiver<()>,
        conn_rx: mpsc::UnboundedReceiver<(ServerEnd<icc::Path>, String)>,
    ) -> Result<Self, DriverError> {
        let graph = Rc::new(RefCell::new(graph));
        let children = Rc::new(children);
        let sync_state = Rc::new(Cell::new(false));

        let graph_clone = graph.clone();
        let sync_state_clone = sync_state.clone();
        let children_clone = children.clone();
        self.scope.spawn_local(async move {
            while let Some(responder) = inspect_publisher.next().await {
                let inspector = Inspector::default();
                let root = inspector.root();
                let sync_state_child = root.create_child("sync_state");
                sync_state_child.record_bool("sync_state", sync_state_clone.get());
                root.record(sync_state_child);

                let nodes_child = root.create_child("nodes");
                graph_clone.borrow().record_inspect(&nodes_child);
                root.record(nodes_child);

                let paths_child = root.create_child("paths");
                for child in children_clone.values() {
                    child.record_inspect(&paths_child);
                }
                root.record(paths_child);

                responder.send(inspector).ok();
            }
        });

        let sync_graph = graph.clone();
        let sync_state_clone = sync_state.clone();
        self.scope.spawn_local(async move {
            sync_complete_rx.await.unwrap();
            sync_state_clone.set(true);

            info!("Sending initial votes");
            // Vote for all nodes with all received votes thus far.
            let requests = { sync_graph.borrow().make_inital_bandwidth_requests() };

            match device.set_nodes_bandwidth(&requests).await {
                Err(err) => {
                    error!("Failed to set bandwidth with {err}");
                }
                Ok(Ok(result)) => {
                    sync_graph.borrow_mut().update_stats(result.aggregated_bandwidth);
                }
                Ok(Err(err)) => {
                    error!("Failed to set bandwidth with {err:?}");
                }
            };
        });

        self.scope.spawn_local(async move {
            let children = Rc::new(children);
            conn_rx
                .for_each_concurrent(None, move |(request, child_name)| {
                    let children = children.clone();
                    let graph = graph.clone();
                    let sync_state = sync_state.clone();
                    async move {
                        if let Some(child) = children.get(&child_name) {
                            let child_handler =
                                ChildHandler { child, sync_state: &sync_state, graph: &graph };
                            let dispatcher = fidl_next::ServerDispatcher::new(request);
                            dispatcher
                                .run_local(child_handler)
                                .await
                                .inspect_err(|err| {
                                    error!("Error in child dispatch loop for {child_name}: {err}");
                                })
                                .ok();
                        } else {
                            error!("Failed to find child {child_name}");
                        }
                    }
                })
                .await;
        });

        Ok(self)
    }
}

impl Driver for InterconnectDriver {
    const NAME: &str = "interconnect";

    async fn start(mut context: DriverContext) -> Result<Self, DriverError> {
        let node = context.take_node()?;

        let inspect_publisher = context.inspect_content_publisher()?;

        let device_service: fdf_component::ServiceInstance<icc::Service> =
            context.incoming.service().connect_next()?;
        let (device_client, device_server) = fidl_next::fuchsia::create_channel();
        device_service.device(device_server).inspect_err(|err| {
            error!("Error connecting to interconnect device at driver startup: {err}");
        })?;
        let device = device_client.spawn();

        let node_graph = device.get_node_graph().await.inspect_err(|err| {
            error!("Failed to get node graph with {err}");
        })?;
        let mut graph = NodeGraph::new(node_graph.nodes, node_graph.edges)?;

        let path_endpoints = device.get_path_endpoints().await.inspect_err(|err| {
            error!("Failed to get path endpoints with {err}");
        })?;
        let paths: Vec<_> = Result::from_iter(path_endpoints.paths.into_iter().map(|path| {
            let path_id = PathId(path.id.ok_or(Status::INVALID_ARGS)?);
            let path_name = path.name.ok_or(Status::INVALID_ARGS)?;
            let src_node_id = NodeId(path.src_node_id.ok_or(Status::INVALID_ARGS)?);
            let dst_node_id = NodeId(path.dst_node_id.ok_or(Status::INVALID_ARGS)?);
            Ok::<_, Status>((
                graph.make_path(path_id, path_name, src_node_id, dst_node_id)?,
                path.tag,
            ))
        }))?;

        let mut outgoing = ServiceFs::new();

        let (conn_tx, conn_rx) = mpsc::unbounded();
        let scope = fasync::Scope::new_with_name("driver");
        let mut children = BTreeMap::new();
        let mut sync_state_completers = Vec::new();
        for (path, tag) in paths {
            let name = format!("{}-{}", path.name(), path.id());
            debug!("Adding child node {name}");
            let offer = ServiceOffer::new_marker_next(icc::PathService)
                .add_default_named_next(
                    &mut outgoing,
                    &name,
                    PathService {
                        scope: scope.clone(),
                        name: name.clone(),
                        sender: conn_tx.clone(),
                    },
                )
                .build_zircon_offer_next();

            let node_args = NodeBuilder::new(&name)
                .add_property(bind_fuchsia::ID, path.id().0)
                .add_offer(offer)
                .build();
            let controller = node.add_child(node_args).await?.into_proxy();
            let device = device.clone();
            let average_bandwidth_bps = Cell::new(0);
            let peak_bandwidth_bps = Cell::new(0);

            let controller_clone = controller.clone();
            let path_name = path.name().to_string();
            sync_state_completers.push(async move {
                debug!("Waiting for {path_name} to bind");
                match controller_clone.wait_for_driver().await {
                    Err(e) => {
                        error!("Failed to wait for driver to bind and start: {e:?}");
                    }
                    Ok(r) => {
                        debug!("Driver bound to {path_name} with result: {r:?}");
                    }
                }
            });

            children.insert(
                name.clone(),
                Child { path, controller, device, average_bandwidth_bps, peak_bandwidth_bps, tag },
            );
        }

        context.serve_outgoing(&mut outgoing)?;
        scope.spawn(outgoing.collect());

        // Once all child devices spawned have had drivers bind and run their start routines, we
        // can assume they have also cast their initial votes and inform our parent to act upon
        // these votes.
        let (sync_complete_tx, sync_complete_rx) = oneshot::channel();
        scope.spawn(async move {
            debug!("Waiting for all node bindings to complete");
            futures::future::join_all(sync_state_completers).await;
            info!("Sync state achieved.");
            sync_complete_tx.send(()).ok();
        });

        let driver = InterconnectDriver { node, scope };
        fasync::Task::spawn(driver.run_graph_service(
            inspect_publisher,
            device,
            children,
            graph,
            sync_complete_rx,
            conn_rx,
        ))
        .await
        .inspect(|_| debug!("Started graph service"))
        .inspect_err(|e| error!("Failed to start graph service: {e}"))
    }

    async fn stop(&self) {}
}
