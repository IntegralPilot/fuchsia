// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_next_fuchsia_hardware_interconnect as icc;
use fuchsia_inspect::ArrayProperty;
use fuchsia_trace as ftrace;
use log::error;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use zx::Status;

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct NodeId(pub u32);

#[derive(Debug, PartialEq, Eq)]
struct IncomingEdge {
    /// Weight to use for finding optimal path.
    weight: u32,
}

#[derive(Debug, PartialEq, Eq)]
struct BandwidthRequest {
    average_bandwidth_bps: u64,
    peak_bandwidth_bps: u64,
    tag: Option<u32>,
}

#[derive(Debug, PartialEq, Eq)]
struct InterconnectNode {
    name: String,
    id: NodeId,
    /// List of nodes can send to this node.
    incoming_edges: BTreeMap<NodeId, IncomingEdge>,
    /// List of paths with bandwidth requests.
    path_bandwidth_requests: BTreeMap<PathId, BandwidthRequest>,
    initial_avg_bandwidth_bps: u64,
    initial_peak_bandwidth_bps: u64,
    current_avg_bandwidth_bps: u64,
    current_peak_bandwidth_bps: u64,
    trace_id: ftrace::Id,
}

#[cfg(test)]
impl Default for InterconnectNode {
    fn default() -> Self {
        Self {
            name: "".to_string(),
            id: NodeId(0),
            incoming_edges: Default::default(),
            path_bandwidth_requests: Default::default(),
            initial_avg_bandwidth_bps: 0,
            initial_peak_bandwidth_bps: 0,
            current_avg_bandwidth_bps: 0,
            current_peak_bandwidth_bps: 0,
            trace_id: ftrace::Id::from(0),
        }
    }
}

impl InterconnectNode {
    pub fn record_inspect(&self, inspect: &fuchsia_inspect::Node) {
        inspect.record_string("name", &self.name);
        let edges = self.incoming_edges.keys();
        let array = inspect.create_uint_array("incoming_edges", edges.len());
        for (i, edge) in edges.enumerate() {
            array.set(i, edge.0);
        }
        inspect.record(array);
        inspect.record_child("path_bandwidth_requests", |inspect| {
            for (path_id, request) in &self.path_bandwidth_requests {
                inspect.record_child(path_id.0.to_string(), |inspect| {
                    inspect.record_uint("average_bandwidth_bps", request.average_bandwidth_bps);
                    inspect.record_uint("peak_bandwidth_bps", request.peak_bandwidth_bps);
                });
            }
        });
        inspect.record_uint("initial_average_bandwidth_bps", self.initial_avg_bandwidth_bps);
        inspect.record_uint("initial_peak_bandwidth_bps", self.initial_peak_bandwidth_bps);
        inspect.record_uint("current_average_bandwidth_bps", self.current_avg_bandwidth_bps);
        inspect.record_uint("current_peak_bandwidth_bps", self.current_peak_bandwidth_bps);
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct PathId(pub u32);

impl std::fmt::Display for PathId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct Path {
    name: String,
    id: PathId,
    nodes: Vec<NodeId>,
}

impl Path {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn id(&self) -> PathId {
        self.id
    }

    pub fn record_inspect(&self, inspect: &fuchsia_inspect::Node) {
        inspect.record_uint("id", self.id.0.into());
        let array = inspect.create_uint_array("nodes", self.nodes.len());
        for i in 0..self.nodes.len() {
            array.set(i, self.nodes[i].0);
        }
        inspect.record(array);
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct NodeGraph {
    /// Flattened graph. Each node maintains a list of incoming edges
    /// from other nodes.
    nodes: BTreeMap<NodeId, InterconnectNode>,
}

impl NodeGraph {
    pub fn new(nodes: Vec<icc::Node>, edges: Vec<icc::Edge>) -> Result<Self, Status> {
        // Parse data out of tables into structs.
        struct ParsedNode {
            id: NodeId,
            name: String,
            #[allow(unused)]
            interconnect_name: Option<String>,
            initial_avg_bandwidth_bps: u64,
            initial_peak_bandwidth_bps: u64,
        }
        let nodes: Vec<_> = Result::from_iter(nodes.into_iter().map(|node| {
            Ok::<_, Status>(ParsedNode {
                id: NodeId(node.id.ok_or(Status::INVALID_ARGS)?),
                name: node.name.ok_or(Status::INVALID_ARGS)?,
                interconnect_name: node.interconnect_name,
                initial_avg_bandwidth_bps: node.initial_avg_bandwidth_bps.unwrap_or(0),
                initial_peak_bandwidth_bps: node.initial_peak_bandwidth_bps.unwrap_or(0),
            })
        }))?;

        struct ParsedEdge {
            src_node_id: NodeId,
            dst_node_id: NodeId,
            weight: u32,
        }
        let edges: Vec<_> = Result::from_iter(edges.into_iter().map(|edge| {
            Ok::<_, Status>(ParsedEdge {
                src_node_id: NodeId(edge.src_node_id.ok_or(Status::INVALID_ARGS)?),
                dst_node_id: NodeId(edge.dst_node_id.ok_or(Status::INVALID_ARGS)?),
                weight: edge.weight.unwrap_or(1),
            })
        }))?;

        // Validate nodes and edges
        let valid_node_ids: BTreeSet<_> = nodes.iter().map(|node| node.id).collect();
        if valid_node_ids.len() != nodes.len() {
            error!("Node list contains duplicate entries.");
            return Err(Status::INVALID_ARGS);
        }

        let edge_pairs: BTreeSet<_> =
            edges.iter().map(|edge| (edge.src_node_id, edge.dst_node_id)).collect();
        if edge_pairs.len() != edges.len() {
            error!("Edge list contains duplicate pairs of source to destination node ids.");
            return Err(Status::INVALID_ARGS);
        }

        for edge in edges.iter() {
            if !valid_node_ids.contains(&edge.src_node_id) {
                error!("Edge contains an invalid src node id {:?}.", edge.src_node_id);
                return Err(Status::INVALID_ARGS);
            }
            if !valid_node_ids.contains(&edge.dst_node_id) {
                error!("Edge contains an invalid dst node id {:?}.", edge.dst_node_id);
                return Err(Status::INVALID_ARGS);
            }
            if edge.src_node_id == edge.dst_node_id {
                error!("Edge cannot have src and dst node ids be the same.");
                return Err(Status::INVALID_ARGS);
            }
        }

        // Extract nodes
        let mut graph = BTreeMap::new();
        for node in nodes {
            // Note that it's possible to improve the time complexity of this algorithm by only
            // iterating over the set of edges once rather than n times, but that would complicate
            // the logic and the total number of edges is not anticipated to be large so it's not
            // necessarily worthwhile.
            let incoming_edges: BTreeMap<_, _> = edges
                .iter()
                .filter(|edge| edge.dst_node_id == node.id)
                .map(|edge| (edge.src_node_id, IncomingEdge { weight: edge.weight }))
                .collect();
            graph.insert(
                node.id,
                InterconnectNode {
                    name: node.name,
                    id: node.id,
                    incoming_edges,
                    path_bandwidth_requests: BTreeMap::new(),
                    initial_avg_bandwidth_bps: node.initial_avg_bandwidth_bps,
                    initial_peak_bandwidth_bps: node.initial_peak_bandwidth_bps,
                    current_avg_bandwidth_bps: 0,
                    current_peak_bandwidth_bps: 0,
                    trace_id: ftrace::Id::new(),
                },
            );
        }

        Ok(NodeGraph { nodes: graph })
    }

    pub fn record_inspect(&self, inspect: &fuchsia_inspect::Node) {
        for (id, node) in &self.nodes {
            inspect.record_child(id.0.to_string(), |inspect| {
                node.record_inspect(inspect);
            });
        }
    }

    pub fn update_stats(&mut self, nodes: Vec<icc::AggregatedBandwidth>) {
        for node_bw in nodes {
            if let Some(node_id) = node_bw.node_id {
                if let Some(node) = self.nodes.get_mut(&NodeId(node_id)) {
                    node.current_avg_bandwidth_bps = node_bw.average_bandwidth_bps.unwrap_or(0);
                    node.current_peak_bandwidth_bps = node_bw.peak_bandwidth_bps.unwrap_or(0);
                    ftrace::counter!(c"interconnect", &node.name, node.trace_id.into(),
                        "average_bandwidth_bps" => node.current_avg_bandwidth_bps,
                        "peak_bandwidth_bps" => node.current_peak_bandwidth_bps);
                }
            }
        }
    }

    /// Sets the edge from |dst_node_id| to |src_node_id| to the provided bandwidth values.
    pub fn update_path(
        &mut self,
        path: &Path,
        average_bandwidth_bps: u64,
        peak_bandwidth_bps: u64,
        tag: Option<u32>,
    ) {
        for node in &path.nodes {
            let node = self.nodes.get_mut(&node).expect("path is already validated");
            let request = node
                .path_bandwidth_requests
                .get_mut(&path.id)
                .expect("path was created from graph");
            request.average_bandwidth_bps = average_bandwidth_bps;
            request.peak_bandwidth_bps = peak_bandwidth_bps;
            request.tag = tag;
        }
    }

    /// Creates a bandwidth request for all nodes.
    pub fn make_inital_bandwidth_requests(&self) -> Vec<icc::NodeBandwidth> {
        self.nodes
            .iter()
            .map(|(node_id, node)| {
                let requests: Vec<_> = node
                    .path_bandwidth_requests
                    .iter()
                    .map(|(_, request)| icc::BandwidthRequest {
                        average_bandwidth_bps: Some(request.average_bandwidth_bps),
                        peak_bandwidth_bps: Some(request.peak_bandwidth_bps),
                        tag: request.tag,
                        ..Default::default()
                    })
                    .collect();
                icc::NodeBandwidth {
                    node_id: Some(node_id.0),
                    requests: Some(requests),
                    ..Default::default()
                }
            })
            .collect()
    }

    /// Creates a bandwidth request for the specified path where the bandwidth request includes
    /// an entry for each node that intersects with this path.
    pub fn make_bandwidth_requests(&self, path: &Path) -> Vec<icc::NodeBandwidth> {
        path.nodes
            .iter()
            .map(|node_id| {
                let node = self.nodes.get(&node_id).expect("path is already validated");
                let requests: Vec<_> = node
                    .path_bandwidth_requests
                    .iter()
                    .map(|(_, request)| icc::BandwidthRequest {
                        average_bandwidth_bps: Some(request.average_bandwidth_bps),
                        peak_bandwidth_bps: Some(request.peak_bandwidth_bps),
                        tag: request.tag,
                        ..Default::default()
                    })
                    .collect();
                icc::NodeBandwidth {
                    node_id: Some(node_id.0),
                    requests: Some(requests),
                    ..Default::default()
                }
            })
            .collect()
    }

    /// Returns a path that includes provides a path between src and dst.
    pub fn make_path(
        &mut self,
        path_id: PathId,
        path_name: impl Into<String>,
        src_node_id: NodeId,
        dst_node_id: NodeId,
    ) -> Result<Path, Status> {
        let mut queue = VecDeque::new();
        // The graph is not guaranteed to be acyclic, so we need to avoid revisiting nodes.
        let mut visited = BTreeSet::new();
        queue.push_back((dst_node_id, Vec::new()));
        // TODO(https://fxbug.dev/415837761): Update algorithm to take into account edge weights.
        while let Some((node_id, mut path)) = queue.pop_front() {
            if visited.contains(&node_id) {
                continue;
            }
            visited.insert(node_id);
            path.push(node_id);
            if node_id == src_node_id {
                path.reverse();
                // Ensure there is no duplicates before inserting.
                for node_id in path.iter() {
                    let node = self.nodes.get(&node_id).unwrap();
                    if node.path_bandwidth_requests.contains_key(&path_id) {
                        error!("Duplicate path id {path_id:?}");
                        return Err(Status::ALREADY_EXISTS);
                    }
                }
                for node_id in path.iter() {
                    let node = self.nodes.get_mut(&node_id).unwrap();
                    node.path_bandwidth_requests.insert(
                        path_id,
                        BandwidthRequest {
                            average_bandwidth_bps: node.initial_avg_bandwidth_bps,
                            peak_bandwidth_bps: node.initial_peak_bandwidth_bps,
                            tag: None,
                        },
                    );
                }
                return Ok(Path { name: path_name.into(), id: path_id, nodes: path });
            }
            // We've already validated that all node ids are valid.
            for (node_id, _) in &self.nodes.get(&node_id).unwrap().incoming_edges {
                queue.push_back((*node_id, path.clone()));
            }
        }
        error!("Path between {src_node_id:?} and {dst_node_id:?} not found");
        Err(Status::NOT_FOUND)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[fuchsia::test]
    fn test_node_graph_simple() {
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
        ];
        let edges = vec![icc::Edge {
            src_node_id: Some(0),
            dst_node_id: Some(1),
            weight: None,
            ..Default::default()
        }];
        let mut graph = NodeGraph::new(nodes, edges).unwrap();
        graph.nodes.iter_mut().for_each(|(_, node)| node.trace_id = ftrace::Id::from(0));
        assert_eq!(
            graph,
            NodeGraph {
                nodes: BTreeMap::from([
                    (
                        NodeId(0),
                        InterconnectNode {
                            name: "zero".to_string(),
                            id: NodeId(0),
                            ..Default::default()
                        }
                    ),
                    (
                        NodeId(1),
                        InterconnectNode {
                            name: "one".to_string(),
                            id: NodeId(1),
                            incoming_edges: BTreeMap::from([(
                                NodeId(0),
                                IncomingEdge { weight: 1 }
                            )]),
                            ..Default::default()
                        }
                    ),
                ]),
            }
        );
    }

    #[fuchsia::test]
    fn test_node_graph_missing_id() {
        let nodes = vec![
            icc::Node {
                id: None,
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
        ];
        let edges = vec![icc::Edge {
            src_node_id: Some(0),
            dst_node_id: Some(1),
            weight: Some(1),
            ..Default::default()
        }];
        assert_eq!(NodeGraph::new(nodes, edges), Err(Status::INVALID_ARGS));
    }

    #[fuchsia::test]
    fn test_node_graph_missing_name() {
        let nodes = vec![
            icc::Node { id: Some(0), name: None, interconnect_name: None, ..Default::default() },
            icc::Node {
                id: Some(1),
                name: Some("one".to_string()),
                interconnect_name: None,
                ..Default::default()
            },
        ];
        let edges = vec![icc::Edge {
            src_node_id: Some(0),
            dst_node_id: Some(1),
            weight: Some(1),
            ..Default::default()
        }];
        assert_eq!(NodeGraph::new(nodes, edges), Err(Status::INVALID_ARGS));
    }

    #[fuchsia::test]
    fn test_node_graph_missing_src() {
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
        ];
        let edges = vec![icc::Edge {
            src_node_id: None,
            dst_node_id: Some(1),
            weight: Some(1),
            ..Default::default()
        }];
        assert_eq!(NodeGraph::new(nodes, edges), Err(Status::INVALID_ARGS));
    }

    #[fuchsia::test]
    fn test_node_graph_missing_dst() {
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
        ];
        let edges = vec![icc::Edge {
            src_node_id: Some(0),
            dst_node_id: None,
            weight: Some(1),
            ..Default::default()
        }];
        assert_eq!(NodeGraph::new(nodes, edges), Err(Status::INVALID_ARGS));
    }

    #[fuchsia::test]
    fn test_node_graph_invalid_edge() {
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
        ];
        let edges = vec![icc::Edge {
            src_node_id: Some(0),
            dst_node_id: Some(0),
            weight: Some(1),
            ..Default::default()
        }];
        assert_eq!(NodeGraph::new(nodes, edges), Err(Status::INVALID_ARGS));
    }

    #[fuchsia::test]
    fn test_node_graph_missing_dst_node() {
        let nodes = vec![icc::Node {
            id: Some(0),
            name: Some("zero".to_string()),
            interconnect_name: None,
            ..Default::default()
        }];
        let edges = vec![icc::Edge {
            src_node_id: Some(0),
            dst_node_id: Some(1),
            weight: Some(1),
            ..Default::default()
        }];
        assert_eq!(NodeGraph::new(nodes, edges), Err(Status::INVALID_ARGS));
    }

    #[fuchsia::test]
    fn test_node_graph_missing_src_node() {
        let nodes = vec![icc::Node {
            id: Some(0),
            name: Some("zero".to_string()),
            interconnect_name: None,
            ..Default::default()
        }];
        let edges = vec![icc::Edge {
            src_node_id: Some(1),
            dst_node_id: Some(0),
            weight: Some(1),
            ..Default::default()
        }];
        assert_eq!(NodeGraph::new(nodes, edges), Err(Status::INVALID_ARGS));
    }

    #[fuchsia::test]
    fn test_node_graph_missing_duplicate_node() {
        let nodes = vec![
            icc::Node {
                id: Some(0),
                name: Some("zero".to_string()),
                interconnect_name: None,
                ..Default::default()
            },
            icc::Node {
                id: Some(0),
                name: Some("zero-2".to_string()),
                interconnect_name: None,
                ..Default::default()
            },
            icc::Node {
                id: Some(1),
                name: Some("one".to_string()),
                interconnect_name: None,
                ..Default::default()
            },
        ];
        let edges = vec![icc::Edge {
            src_node_id: Some(0),
            dst_node_id: Some(1),
            weight: Some(1),
            ..Default::default()
        }];
        assert_eq!(NodeGraph::new(nodes, edges), Err(Status::INVALID_ARGS));
    }

    #[fuchsia::test]
    fn test_node_graph_missing_duplicate_edge() {
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
        ];
        let edges = vec![
            icc::Edge {
                src_node_id: Some(0),
                dst_node_id: Some(1),
                weight: Some(1),
                ..Default::default()
            },
            icc::Edge {
                src_node_id: Some(0),
                dst_node_id: Some(1),
                weight: Some(4),
                ..Default::default()
            },
        ];
        assert_eq!(NodeGraph::new(nodes, edges), Err(Status::INVALID_ARGS));
    }

    #[fuchsia::test]
    fn test_node_graph_complex() {
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
        let mut graph = NodeGraph::new(nodes, edges).unwrap();
        graph.nodes.iter_mut().for_each(|(_, node)| node.trace_id = ftrace::Id::from(0));
        assert_eq!(
            graph,
            NodeGraph {
                nodes: BTreeMap::from([
                    (
                        NodeId(0),
                        InterconnectNode {
                            name: "zero".to_string(),
                            id: NodeId(0),
                            ..Default::default()
                        }
                    ),
                    (
                        NodeId(1),
                        InterconnectNode {
                            name: "one".to_string(),
                            id: NodeId(1),
                            incoming_edges: BTreeMap::from([
                                (NodeId(0), IncomingEdge { weight: 1 }),
                                (NodeId(2), IncomingEdge { weight: 1 }),
                            ]),
                            ..Default::default()
                        }
                    ),
                    (
                        NodeId(2),
                        InterconnectNode {
                            name: "two".to_string(),
                            id: NodeId(2),
                            incoming_edges: BTreeMap::from([(
                                NodeId(1),
                                IncomingEdge { weight: 1 }
                            )]),
                            ..Default::default()
                        }
                    ),
                    (
                        NodeId(3),
                        InterconnectNode {
                            name: "three".to_string(),
                            id: NodeId(3),
                            incoming_edges: BTreeMap::from([(
                                NodeId(1),
                                IncomingEdge { weight: 1 }
                            )]),
                            ..Default::default()
                        }
                    ),
                ]),
            }
        );
    }

    #[fuchsia::test]
    fn test_find_full_path() {
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
        let mut graph = NodeGraph::new(nodes, edges).unwrap();

        static PATH_NAME: &str = "";

        let path = graph.make_path(PathId(0), PATH_NAME, NodeId(0), NodeId(2)).unwrap();
        assert_eq!(
            path,
            Path {
                id: PathId(0),
                name: PATH_NAME.to_string(),
                nodes: vec![NodeId(0), NodeId(1), NodeId(2)]
            }
        );
        assert_eq!(
            graph.make_path(PathId(1), PATH_NAME, NodeId(0), NodeId(3)),
            Ok(Path {
                id: PathId(1),
                name: PATH_NAME.to_string(),
                nodes: vec![NodeId(0), NodeId(1), NodeId(3)],
            })
        );
        assert_eq!(
            graph.make_path(PathId(2), PATH_NAME, NodeId(2), NodeId(3)),
            Ok(Path {
                id: PathId(2),
                name: PATH_NAME.to_string(),
                nodes: vec![NodeId(2), NodeId(1), NodeId(3)]
            })
        );
        assert_eq!(
            graph.make_path(PathId(2), PATH_NAME, NodeId(2), NodeId(3)),
            Err(Status::ALREADY_EXISTS)
        );
        assert_eq!(
            graph.make_path(PathId(3), PATH_NAME, NodeId(1), NodeId(0)),
            Err(Status::NOT_FOUND)
        );
        assert_eq!(
            graph.make_path(PathId(4), PATH_NAME, NodeId(2), NodeId(0)),
            Err(Status::NOT_FOUND)
        );
        assert_eq!(
            graph.make_path(PathId(5), PATH_NAME, NodeId(3), NodeId(2)),
            Err(Status::NOT_FOUND)
        );

        graph.update_path(&path, 100, 500, None);

        assert_eq!(
            graph.make_bandwidth_requests(&path),
            vec![
                icc::NodeBandwidth {
                    node_id: Some(0),
                    requests: Some(vec![
                        icc::BandwidthRequest {
                            average_bandwidth_bps: Some(100),
                            peak_bandwidth_bps: Some(500),
                            ..Default::default()
                        },
                        icc::BandwidthRequest {
                            average_bandwidth_bps: Some(0),
                            peak_bandwidth_bps: Some(0),
                            ..Default::default()
                        },
                    ]),
                    ..Default::default()
                },
                icc::NodeBandwidth {
                    node_id: Some(1),
                    requests: Some(vec![
                        icc::BandwidthRequest {
                            average_bandwidth_bps: Some(100),
                            peak_bandwidth_bps: Some(500),
                            ..Default::default()
                        },
                        icc::BandwidthRequest {
                            average_bandwidth_bps: Some(0),
                            peak_bandwidth_bps: Some(0),
                            ..Default::default()
                        },
                        icc::BandwidthRequest {
                            average_bandwidth_bps: Some(0),
                            peak_bandwidth_bps: Some(0),
                            ..Default::default()
                        },
                    ]),
                    ..Default::default()
                },
                icc::NodeBandwidth {
                    node_id: Some(2),
                    requests: Some(vec![
                        icc::BandwidthRequest {
                            average_bandwidth_bps: Some(100),
                            peak_bandwidth_bps: Some(500),
                            ..Default::default()
                        },
                        icc::BandwidthRequest {
                            average_bandwidth_bps: Some(0),
                            peak_bandwidth_bps: Some(0),
                            ..Default::default()
                        },
                    ]),
                    ..Default::default()
                },
            ]
        );
    }
}
