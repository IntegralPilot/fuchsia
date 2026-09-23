// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::future_help::{Observable, Observer};
use crate::labels::NodeId;
use anyhow::{bail, format_err, Error};
use fidl::Channel;
use futures::lock::Mutex;
use std::collections::{btree_map, BTreeMap};

/// A type that can be converted into a super::Peer
#[derive(Debug, Clone, PartialEq)]
pub struct ListablePeer {
    /// Node ID of this peer.
    pub node_id: NodeId,
    /// Whether this peer is the node producing the list.
    pub is_self: bool,
    /// Services offered by this peer.
    pub services: Vec<String>,
}

struct ListablePeerSet {
    listable_peers: Vec<ListablePeer>,
    peers_with_client_connection: BTreeMap<NodeId, usize>,
}

impl ListablePeerSet {
    fn publish(&self) -> Vec<ListablePeer> {
        let peers_with_client_connection = &self.peers_with_client_connection;
        self.listable_peers
            .iter()
            .filter(move |p| p.is_self || peers_with_client_connection.contains_key(&p.node_id))
            .cloned()
            .collect()
    }
}

pub struct ServiceMapInner {
    local_services: BTreeMap<String, Box<dyn Fn(fidl::Channel) -> Result<(), Error> + Send>>,
    local_service_list: Observable<Vec<String>>,
    list_peers: Observable<Vec<ListablePeer>>,
    listable_peer_set: ListablePeerSet,
}

pub struct ServiceMap {
    inner: Mutex<ServiceMapInner>,
    local_node_id: NodeId,
}

impl ServiceMap {
    pub fn new(local_node_id: NodeId) -> ServiceMap {
        let listable_peers =
            vec![ListablePeer { node_id: local_node_id, is_self: true, services: vec![] }];
        ServiceMap {
            local_node_id,
            inner: Mutex::new(ServiceMapInner {
                local_services: BTreeMap::new(),
                local_service_list: Observable::new(Vec::new()),
                list_peers: Observable::new(listable_peers.clone()),
                listable_peer_set: ListablePeerSet {
                    listable_peers,
                    peers_with_client_connection: std::iter::once((local_node_id, 1)).collect(),
                },
            }),
        }
    }

    pub async fn connect(&self, service_name: &str, chan: Channel) -> Result<(), Error> {
        (self
            .inner
            .lock()
            .await
            .local_services
            .get(service_name)
            .ok_or_else(|| format_err!("Service not found: {}", service_name))?)(chan)?;
        Ok(())
    }

    pub async fn register_service(
        &self,
        service_name: String,
        provider: impl Fn(fidl::Channel) -> Result<(), Error> + Send + 'static,
    ) {
        log::trace!("Request register_service '{}'", service_name);
        let mut inner = self.inner.lock().await;
        if inner.local_services.insert(service_name.clone(), Box::new(provider)).is_none() {
            log::trace!("Publish new service '{}'", service_name);
            let services: Vec<String> = inner.local_services.keys().cloned().collect();
            inner.local_service_list.maybe_push(services).await;
        }
    }

    pub async fn update_node(&self, node_id: NodeId, services: Vec<String>) -> Result<(), Error> {
        if node_id == self.local_node_id {
            bail!("Attempt to set local services list");
        }
        self.update_list_peers(ListablePeer { node_id, is_self: false, services }).await;
        Ok(())
    }

    async fn update_list_peers(&self, update_peer: ListablePeer) {
        let mut inner = self.inner.lock().await;
        let peers = &mut inner.listable_peer_set.listable_peers;
        for existing_peer in peers.iter_mut() {
            if existing_peer.node_id == update_peer.node_id {
                if *existing_peer == update_peer {
                    return;
                }
                *existing_peer = update_peer;
                inner.list_peers.maybe_push(inner.listable_peer_set.publish()).await;
                return;
            }
        }
        peers.push(update_peer);
        inner.list_peers.maybe_push(inner.listable_peer_set.publish()).await;
    }

    pub async fn add_client_connection(&self, peer_id: NodeId) {
        let mut inner = self.inner.lock().await;
        match inner.listable_peer_set.peers_with_client_connection.entry(peer_id) {
            btree_map::Entry::Occupied(o) => *o.into_mut() += 1,
            btree_map::Entry::Vacant(v) => {
                v.insert(1);
                inner.list_peers.maybe_push(inner.listable_peer_set.publish()).await;
            }
        }
    }

    pub async fn remove_client_connection(&self, peer_id: NodeId) {
        let mut inner = self.inner.lock().await;
        match inner.listable_peer_set.peers_with_client_connection.entry(peer_id) {
            btree_map::Entry::Occupied(mut o) => match *o.get() {
                0 => unreachable!(),
                1 => {
                    o.remove();
                    inner.list_peers.maybe_push(inner.listable_peer_set.publish()).await;
                }
                n => *o.get_mut() = n - 1,
            },
            btree_map::Entry::Vacant(_) => unreachable!(),
        }
    }

    pub async fn new_local_service_observer(&self) -> Observer<Vec<String>> {
        self.inner.lock().await.local_service_list.new_observer()
    }

    pub async fn new_list_peers_observer(&self) -> Observer<Vec<ListablePeer>> {
        self.inner.lock().await.list_peers.new_observer()
    }
}
