// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::types::VertexId;
use super::{Vertex, VertexMetadata};
use fuchsia_inspect as inspect;
use fuchsia_sync::RwLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Weak};

/// An Edge in the graph.
#[derive(Debug)]
pub struct Edge<EM> {
    state: Arc<RwLock<EdgeState<EM>>>,
    id: u64,
}

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

pub trait EdgeMetadata {}

impl<EM> Edge<EM> {
    pub(crate) fn new<VM>(
        from: &Vertex<VM>,
        to: &mut Vertex<VM>,
        init_metadata: impl FnOnce(inspect::Node) -> EM,
    ) -> Self
    where
        VM: VertexMetadata<EdgeMeta = EM>,
    {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let to_id = to.id().get_id();
        let (node, metadata) = from.outgoing_edges_node.atomic_update(|parent| {
            let node = parent.create_child(to_id.as_ref());
            node.record_uint("edge_id", id);
            let metadata = init_metadata(node.create_child("meta"));
            (node, metadata)
        });
        let state = Arc::new(RwLock::new(EdgeState::Active { metadata, _node: node }));
        Self { state, id }
    }

    pub fn maybe_update_meta(&self, cb: impl FnOnce(&mut EM)) {
        let mut lock = self.state.write();
        if let Some(meta) = lock.metadata_mut() {
            cb(meta);
        }
    }

    pub fn id(&self) -> u64 {
        self.id
    }

    pub(crate) fn weak_ref(&self) -> WeakEdgeRef<EM> {
        WeakEdgeRef(Arc::downgrade(&self.state))
    }
}

#[derive(Debug)]
enum EdgeState<EM> {
    Active { metadata: EM, _node: inspect::Node },
    Gone,
}

impl<EM> EdgeState<EM> {
    pub fn mark_as_gone(&mut self) {
        match self {
            Self::Active { .. } => *self = Self::Gone,
            Self::Gone { .. } => {}
        }
    }

    fn metadata_mut(&mut self) -> Option<&mut EM> {
        match self {
            EdgeState::Active { metadata, .. } => Some(metadata),
            EdgeState::Gone => None,
        }
    }
}

#[derive(Debug)]
pub(crate) struct WeakEdgeRef<EM>(Weak<RwLock<EdgeState<EM>>>);

impl<EM> Clone for WeakEdgeRef<EM> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<EM> WeakEdgeRef<EM> {
    pub fn mark_as_gone(&self) {
        if let Some(value) = self.0.upgrade() {
            value.write().mark_as_gone();
        }
    }

    pub fn is_valid(&self) -> bool {
        self.0.upgrade().map(|v| matches!(*v.read(), EdgeState::Active { .. })).unwrap_or(false)
    }
}
