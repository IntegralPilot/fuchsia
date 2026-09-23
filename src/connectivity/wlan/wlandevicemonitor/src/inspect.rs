// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fuchsia_inspect::{InspectType, Inspector};
use fuchsia_inspect_contrib::nodes::BoundedListNode;
use fuchsia_sync::Mutex;
use futures::FutureExt;
use log::error;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

const MAX_CACHED_IFACES: usize = 3;
pub const VMO_SIZE_BYTES: usize = 1000 * 1024;

/// Limit was chosen arbitrary. 20 events seem enough to log multiple PHY/iface create or
/// destroy events.
const DEVICE_EVENTS_LIMIT: usize = 20;

pub struct WlanMonitorTree {
    /// Root of the tree
    pub _inspector: Inspector,
    /// "device_events" subtree
    pub device_events: Mutex<BoundedListNode>,
}

impl WlanMonitorTree {
    pub fn new(inspector: Inspector) -> Self {
        let device_events = inspector.root().create_child("device_events");
        Self {
            _inspector: inspector,
            device_events: Mutex::new(BoundedListNode::new(device_events, DEVICE_EVENTS_LIMIT)),
        }
    }
}

pub struct IfacesTree(Mutex<IfacesTreeInternal>);

struct IfacesTreeInternal {
    inspector: Inspector,
    parent_node: fuchsia_inspect::Node,
    cache_node: Option<fuchsia_inspect::Node>,
    iface_nodes: HashMap<u16, Box<dyn InspectType>>,
    destroyed_nodes: VecDeque<Box<dyn InspectType>>,
}

impl IfacesTree {
    pub fn new(inspector: Inspector) -> Self {
        let parent_node = inspector.root().create_child("ifaces");
        let destroyed_nodes = VecDeque::with_capacity(MAX_CACHED_IFACES);
        Self(Mutex::new(IfacesTreeInternal {
            inspector,
            parent_node,
            cache_node: None,
            iface_nodes: HashMap::new(),
            destroyed_nodes,
        }))
    }

    pub fn add_iface(&self, iface_id: u16, vmo: fidl::Vmo) {
        let mut tree = self.0.lock();
        if tree.iface_nodes.contains_key(&iface_id) {
            error!("add_iface called for already existing iface {}. Skipping.", iface_id);
            return;
        }

        let vmo = Arc::new(vmo);
        let iface_node_name = format!("{iface_id}");
        let iface_node = tree.parent_node.create_lazy_child(iface_node_name, move || {
            let inspect_vmo_inner_clone = Arc::clone(&vmo);
            async move {
                let iface_inspector = Inspector::new(
                    fuchsia_inspect::InspectorConfig::default()
                        .vmo(inspect_vmo_inner_clone.duplicate_handle(zx::Rights::SAME_RIGHTS)?),
                );
                Ok(iface_inspector)
            }
            .boxed()
        });
        tree.iface_nodes.insert(iface_id, Box::new(iface_node));
    }

    pub fn record_destroyed_iface(&self, iface_id: u16, vmo: Option<fidl::Vmo>) {
        let mut tree = self.0.lock();
        if tree.iface_nodes.remove(&iface_id).is_none() {
            error!("record_destroyed_iface called with missing iface_id {}. Skipping.", iface_id);
            return;
        }
        let destroyed_node_name = format!("{iface_id}");

        if tree.cache_node.is_none() {
            let cache_node = tree.inspector.root().create_child("destroyed_ifaces");
            tree.cache_node.replace(cache_node);
        }
        let cache_node = tree.cache_node.as_ref().unwrap();
        let destroyed_iface_node = match vmo {
            Some(vmo) => {
                let vmo = Arc::new(vmo);
                Box::new(cache_node.create_lazy_child(destroyed_node_name, move || {
                    let inner_clone = Arc::clone(&vmo);
                    async move {
                        let iface_inspector = Inspector::new(
                            fuchsia_inspect::InspectorConfig::default()
                                .vmo(inner_clone.duplicate_handle(zx::Rights::SAME_RIGHTS)?),
                        );
                        Ok(iface_inspector)
                    }
                    .boxed()
                })) as Box<dyn InspectType>
            }
            None => {
                Box::new(cache_node.create_string(destroyed_node_name, "No cached inspect data"))
                    as Box<dyn InspectType>
            }
        };
        while tree.destroyed_nodes.len() > MAX_CACHED_IFACES - 1 {
            tree.destroyed_nodes.pop_front();
        }
        tree.destroyed_nodes.push_back(destroyed_iface_node);
    }

    #[cfg(test)]
    pub fn has_iface(&self, iface_id: u16) -> bool {
        self.0.lock().iface_nodes.contains_key(&iface_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use diagnostics_assertions::{TreeAssertion, assert_data_tree};

    fn create_inspect_vmo() -> fidl::Vmo {
        let inspector = Inspector::new(Default::default());
        inspector.root().record_string("foo", "bar");
        inspector.frozen_vmo_copy().expect("Failed to create frozen VMO copy.")
    }

    #[fuchsia::test]
    async fn test_add_iface() {
        let inspector = Inspector::new(Default::default());
        let ifaces_tree = IfacesTree::new(inspector.clone());

        ifaces_tree.add_iface(123, create_inspect_vmo());

        // verify inspect contents.
        #[rustfmt::skip]
        assert_data_tree!(inspector, root: {
            ifaces: {
                "123": {
                    foo: "bar",
                },
            }
        });
    }

    #[fuchsia::test]
    async fn test_add_duplicate_iface() {
        let inspector = Inspector::new(Default::default());
        let ifaces_tree = IfacesTree::new(inspector.clone());

        ifaces_tree.add_iface(123, create_inspect_vmo());
        ifaces_tree.add_iface(123, create_inspect_vmo());

        // verify inspect contents.
        #[rustfmt::skip]
        assert_data_tree!(inspector, root: {
            ifaces: {
                "123": {
                    foo: "bar",
                },
            },
        });
    }

    #[fuchsia::test]
    async fn test_destroy_iface() {
        let inspector = Inspector::new(Default::default());
        let ifaces_tree = IfacesTree::new(inspector.clone());

        ifaces_tree.add_iface(123, create_inspect_vmo());
        ifaces_tree.record_destroyed_iface(123, Some(create_inspect_vmo()));

        // verify inspect contents.
        #[rustfmt::skip]
        assert_data_tree!(inspector, root: {
            ifaces: {},
            destroyed_ifaces: {
                "123": {
                    foo: "bar",
                },
            },
        });
    }

    #[fuchsia::test]
    async fn test_destroy_missing_iface() {
        let inspector = Inspector::new(Default::default());
        let ifaces_tree = IfacesTree::new(inspector.clone());

        ifaces_tree.record_destroyed_iface(123, Some(create_inspect_vmo()));

        // verify inspect contents.
        #[rustfmt::skip]
        assert_data_tree!(inspector, root: {
            ifaces: {},
        });
    }

    #[fuchsia::test]
    async fn test_exceed_cached_ifaces() {
        let inspector = Inspector::new(Default::default());
        let ifaces_tree = IfacesTree::new(inspector.clone());

        for id in 0..=MAX_CACHED_IFACES as u16 {
            ifaces_tree.add_iface(id, create_inspect_vmo());
            ifaces_tree.record_destroyed_iface(id, Some(create_inspect_vmo()));
        }

        // Expect that iface 0 will be forced out by subsequent cached ifaces.
        let mut destroyed_ifaces_assertion = TreeAssertion::new("destroyed_ifaces", true);
        for id in 1..=MAX_CACHED_IFACES {
            destroyed_ifaces_assertion
                .add_child_assertion(TreeAssertion::new(&id.to_string(), false));
        }

        // verify inspect contents.
        #[rustfmt::skip]
        assert_data_tree!(inspector, root: {
            ifaces: {},
	    destroyed_ifaces_assertion,
        });
    }
}
