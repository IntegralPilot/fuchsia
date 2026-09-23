// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use driver_manager_node::Node;
use driver_manager_types::Collection;
use fidl::endpoints::ClientEnd;
use futures::channel::{mpsc, oneshot};
use log::warn;
use std::collections::{HashSet, VecDeque};
use std::rc::{Rc, Weak};
use {fidl_fuchsia_driver_framework as fdf, fidl_fuchsia_ldsvc as fldsvc};

mod bootup_tracker;
mod driver_host_runner;
mod driver_runner;
mod memory_attribution;
mod offer_injection;
mod runner;
#[cfg(test)]
pub(crate) mod testing;
mod trait_impls;

pub use driver_runner::*;
pub use offer_injection::*;

// A helper struct to implement the bridge traits. It holds a weak reference to the DriverRunner
// to avoid reference cycles.
pub(crate) struct DriverRunnerBridge(Weak<DriverRunner>);

pub(crate) type LoaderServiceFactory =
    mpsc::UnboundedSender<oneshot::Sender<Result<ClientEnd<fldsvc::LoaderMarker>, zx::Status>>>;

pub(crate) async fn perform_bfs<F>(starting_node: Rc<Node>, mut visitor: F)
where
    F: AsyncFnMut(&Rc<Node>) -> bool,
{
    let mut visited: HashSet<*const Node> = HashSet::new();
    let mut node_queue = VecDeque::new();

    visited.insert(Rc::as_ptr(&starting_node));
    node_queue.push_back(starting_node);

    while let Some(current) = node_queue.pop_front() {
        let visit_children = visitor(&current).await;
        if !visit_children {
            continue;
        }

        for child in current.children() {
            let Some(primary_parent) = child.get_primary_parent() else {
                continue;
            };
            if !Rc::ptr_eq(&primary_parent, &current) {
                continue;
            }

            if visited.insert(Rc::as_ptr(&child)) {
                node_queue.push_back(child);
            }
        }
    }
}

pub(crate) fn to_collection(node: &Node, package_type: fdf::DriverPackageType) -> Collection {
    let collection = to_collection_internal(package_type);
    get_highest_ranking_collection(node, collection)
}

fn to_collection_internal(package_type: fdf::DriverPackageType) -> Collection {
    match package_type {
        fdf::DriverPackageType::Boot => Collection::Boot,
        fdf::DriverPackageType::Base => Collection::Package,
        fdf::DriverPackageType::Cached | fdf::DriverPackageType::Universe => {
            Collection::FullPackage
        }
        _ => Collection::None,
    }
}

fn get_highest_ranking_collection(node: &Node, mut collection: Collection) -> Collection {
    let mut ancestors = std::collections::VecDeque::new();
    for parent in node.parents() {
        ancestors.push_back(parent);
    }

    while let Some(ancestor_weak) = ancestors.pop_front() {
        if let Some(ancestor) = ancestor_weak.upgrade() {
            let ancestor_collection = ancestor.collection();
            if ancestor_collection == Collection::None {
                for parent in ancestor.parents() {
                    ancestors.push_back(parent);
                }
            } else if ancestor_collection > collection {
                collection = ancestor_collection;
            }
        } else {
            warn!("Ancestor node released");
        }
    }
    collection
}
