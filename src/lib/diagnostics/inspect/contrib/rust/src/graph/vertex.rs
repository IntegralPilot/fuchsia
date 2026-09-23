// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::edge::WeakEdgeRef;
use super::{Edge, EdgeMetadata, VertexId};
use fuchsia_inspect as inspect;
use std::collections::BTreeMap;

/// A vertex of the graph. When this is dropped, all the outgoing edges and metadata fields will
/// removed from Inspect.
#[derive(Debug)]
pub struct Vertex<M: VertexMetadata> {
    _node: inspect::Node,
    id: M::Id,
    metadata: M,
    incoming_edges: BTreeMap<u64, WeakEdgeRef<M::EdgeMeta>>,
    outgoing_edges: BTreeMap<u64, WeakEdgeRef<M::EdgeMeta>>,
    pub(crate) outgoing_edges_node: inspect::Node,
}

/// Trait implemented by types that hold a vertex metadata.
pub trait VertexMetadata {
    type Id: VertexId;
    type EdgeMeta: EdgeMetadata;
}

impl<M: VertexMetadata> Vertex<M>
where
    <M as VertexMetadata>::Id: VertexId,
{
    pub(crate) fn new(
        id: M::Id,
        parent: &inspect::Node,
        init_metadata: impl FnOnce(inspect::Node) -> M,
    ) -> Self {
        parent.atomic_update(|parent| {
            let id_str = id.get_id();
            let node = parent.create_child(id_str.as_ref());
            let outgoing_edges_node = node.create_child("relationships");
            let metadata = init_metadata(node.create_child("meta"));
            Vertex {
                id,
                _node: node,
                outgoing_edges_node,
                metadata,
                incoming_edges: BTreeMap::new(),
                outgoing_edges: BTreeMap::new(),
            }
        })
    }

    /// Add a new edge to the graph originating at this vertex and going to the vertex `to` with the
    /// given metadata.
    pub fn add_edge(
        &mut self,
        to: &mut Vertex<M>,
        init_metadata: impl FnOnce(inspect::Node) -> M::EdgeMeta,
    ) -> Edge<M::EdgeMeta> {
        let edge = Edge::new(self, to, init_metadata);

        let weak_ref = edge.weak_ref();
        let edge_id = edge.id();

        self.outgoing_edges.retain(|_, n| n.is_valid());
        to.incoming_edges.retain(|_, n| n.is_valid());

        to.incoming_edges.insert(edge_id, weak_ref.clone());
        self.outgoing_edges.insert(edge_id, weak_ref);
        edge
    }

    /// Get an exclusive reference to the metadata to modify it.
    pub fn meta(&mut self) -> &mut M {
        &mut self.metadata
    }

    pub(crate) fn id(&self) -> &M::Id {
        &self.id
    }
}

impl<M> Drop for Vertex<M>
where
    M: VertexMetadata,
{
    fn drop(&mut self) {
        self.outgoing_edges.iter().for_each(|(_, n)| n.mark_as_gone());
        self.incoming_edges.iter().for_each(|(_, n)| n.mark_as_gone());
    }
}
