// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::info_iterator::{CompositeInfoIterator, DeviceInfoIterator, DriverHostInfoIterator};
use driver_manager_core::DriverRunner;
use driver_manager_node::Node;
use driver_manager_shutdown::RemovalSet;
use driver_manager_types::to_deprecated_property;
use fdd::ManagerRequest::*;
use fidl::endpoints::{DiscoverableProtocolMarker, Responder, ServerEnd};
use fidl_fuchsia_component_decl as fdecl;
use fidl_fuchsia_driver_development as fdd;
use fidl_fuchsia_driver_framework as fdf;
use fidl_fuchsia_driver_index as fdi;
use fuchsia_async as fasync;
use fuchsia_component::client::connect_to_protocol;
use fuchsia_component::server::{ServiceFs, ServiceObjLocal};
use futures::prelude::*;
use log::{error, warn};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::rc::{Rc, Weak};

pub struct DriverDevelopmentService {
    driver_runner: Rc<DriverRunner>,
    test_nodes: RefCell<HashMap<String, Weak<Node>>>,
    scope: fasync::Scope,
}

impl DriverDevelopmentService {
    pub fn new(driver_runner: Rc<DriverRunner>) -> Self {
        let scope = fasync::Scope::new_with_name("driver_development_service");
        Self { driver_runner, test_nodes: RefCell::new(HashMap::new()), scope }
    }

    pub fn publish(self: &Rc<Self>, fs: &mut ServiceFs<ServiceObjLocal<'_, ()>>) {
        let this = self.clone();
        fs.dir("svc").add_fidl_service(move |stream: fdd::ManagerRequestStream| {
            let this_clone = this.clone();
            this.scope.spawn_local(async move {
                if let Err(e) = this_clone.serve(stream).await {
                    warn!("Failed to serve DriverDevelopmentService: {}", e);
                }
            });
        });
    }

    pub async fn serve(
        self: Rc<Self>,
        mut stream: fdd::ManagerRequestStream,
    ) -> Result<(), fidl::Error> {
        while let Some(request) = stream.try_next().await? {
            match request {
                GetNodeInfo { node_filter, exact_match, iterator, .. } => {
                    self.get_node_info(node_filter, exact_match, iterator).await;
                }
                GetCompositeInfo { iterator, .. } => {
                    self.get_composite_info(iterator);
                }
                GetDriverInfo { driver_filter, iterator, .. } => {
                    self.get_driver_info(driver_filter, iterator);
                }
                GetCompositeNodeSpecs { name_filter, iterator, .. } => {
                    self.get_composite_node_specs(name_filter, iterator);
                }
                AddTestNode { args, responder } => {
                    self.add_test_node(args, responder).await;
                }
                RemoveTestNode { name, responder } => {
                    self.remove_test_node(name, responder);
                }
                BindAllUnboundNodes { responder } => {
                    self.bind_all_unbound_nodes(responder).await;
                }
                BindAllUnboundNodes2 { responder } => {
                    self.bind_all_unbound_nodes2(responder).await;
                }
                WaitForBootup { responder } => {
                    self.wait_for_bootup(responder).await;
                }
                GetDriverHostInfo { iterator, .. } => {
                    self.get_driver_host_info(iterator).await;
                }
                RestartDriverHosts { driver_url, rematch_flags, responder } => {
                    self.restart_driver_hosts(driver_url, rematch_flags, responder).await;
                }
                DisableDriver { driver_url, package_hash, responder } => {
                    self.disable_driver(driver_url, package_hash, responder).await;
                }
                EnableDriver { driver_url, package_hash, responder } => {
                    self.enable_driver(driver_url, package_hash, responder).await;
                }
                RebindCompositesWithDriver { driver_url, responder } => {
                    self.rebind_composites_with_driver(driver_url, responder).await;
                }
                RestartWithDictionary { moniker, dictionary, responder } => {
                    self.restart_with_dictionary(moniker, dictionary, responder).await;
                }
                _ => {}
            }
        }
        Ok(())
    }

    async fn get_node_info(
        &self,
        node_filter: Vec<String>,
        exact_match: bool,
        iterator: ServerEnd<fdd::NodeInfoIteratorMarker>,
    ) {
        let mut device_infos = vec![];
        let mut unique_nodes = HashSet::new();
        let mut remaining_nodes = VecDeque::new();
        remaining_nodes.push_back(self.driver_runner.root_node());

        while let Some(node) = remaining_nodes.pop_front() {
            let node_ptr: *const _ = Rc::as_ptr(&node);
            if !unique_nodes.insert(node_ptr) {
                continue;
            }

            for child in node.children() {
                remaining_nodes.push_back(child);
            }

            let moniker = node.make_component_moniker();
            if !node_filter.is_empty() {
                let found = node_filter.iter().any(|filter| {
                    if exact_match { &moniker == filter } else { moniker.contains(filter) }
                });
                if !found {
                    continue;
                }
            }

            match create_device_info(&node).await {
                Ok(info) => device_infos.push(info),
                Err(_) => return, // Error already logged
            }
        }

        let iterator_stream = iterator.into_stream();
        let device_info_iterator = DeviceInfoIterator::new(device_infos);
        self.scope.spawn_local(async move {
            if let Err(e) = device_info_iterator.serve(iterator_stream).await {
                warn!("DeviceInfoIterator server failed: {}", e);
            }
        });
    }

    fn get_composite_info(&self, iterator: ServerEnd<fdd::CompositeInfoIteratorMarker>) {
        let list = self.driver_runner.get_composite_list_info();
        let iterator_stream = iterator.into_stream();
        let composite_info_iterator = CompositeInfoIterator::new(list);
        self.scope.spawn_local(async move {
            if let Err(e) = composite_info_iterator.serve(iterator_stream).await {
                warn!("CompositeInfoIterator server failed: {}", e);
            }
        });
    }

    fn get_driver_info(
        &self,
        driver_filter: Vec<String>,
        iterator: ServerEnd<fdd::DriverInfoIteratorMarker>,
    ) {
        let driver_index_client = match connect_to_protocol::<fdi::DevelopmentManagerMarker>() {
            Ok(proxy) => proxy,
            Err(e) => {
                error!(
                    "Failed to connect to service '{}': {}",
                    fdi::DevelopmentManagerMarker::PROTOCOL_NAME,
                    e
                );
                iterator.close_with_epitaph(zx::Status::UNAVAILABLE).ok();
                return;
            }
        };

        if let Err(e) = driver_index_client.get_driver_info(&driver_filter, iterator) {
            error!("Failed to call DriverIndex::GetDriverInfo: {}", e);
        }
    }

    fn get_composite_node_specs(
        &self,
        name_filter: Option<String>,
        iterator: ServerEnd<fdd::CompositeNodeSpecIteratorMarker>,
    ) {
        let driver_index_client = match connect_to_protocol::<fdi::DevelopmentManagerMarker>() {
            Ok(proxy) => proxy,
            Err(e) => {
                error!(
                    "Failed to connect to service '{}': {}",
                    fdi::DevelopmentManagerMarker::PROTOCOL_NAME,
                    e
                );
                iterator.close_with_epitaph(zx::Status::UNAVAILABLE).ok();
                return;
            }
        };

        if let Err(e) =
            driver_index_client.get_composite_node_specs(name_filter.as_deref(), iterator)
        {
            error!("Failed to call DriverIndex::GetCompositeNodeSpecs: {}", e);
        }
    }

    async fn add_test_node(
        &self,
        args: fdd::TestNodeAddArgs,
        responder: fdd::ManagerAddTestNodeResponder,
    ) {
        let name = if let Some(name) = args.name {
            name
        } else {
            let _ = responder.send(Err(fdf::NodeError::NameMissing));
            return;
        };

        let add_args = fdf::NodeAddArgs {
            name: Some(name.clone()),
            properties: args.properties,
            ..Default::default()
        };

        let result = self.driver_runner.root_node().add_child(add_args, None, None).await;

        match result {
            Ok(node) => {
                self.test_nodes.borrow_mut().insert(name, Rc::downgrade(&node));
                let _ = responder.send(Ok(()));
            }
            Err(e) => {
                let _ = responder.send(Err(e));
            }
        }
    }

    fn remove_test_node(&self, name: String, responder: fdd::ManagerRemoveTestNodeResponder) {
        let mut test_nodes = self.test_nodes.borrow_mut();
        if !test_nodes.contains_key(&name) {
            let _ = responder.send(Err(zx::Status::NOT_FOUND.into_raw()));
            return;
        }

        if let Some(node_weak) = test_nodes.get(&name)
            && let Some(node) = node_weak.upgrade()
        {
            node.remove(RemovalSet::All, None);
        }

        test_nodes.remove(&name);
        let _ = responder.send(Ok(()));
    }

    async fn bind_all_unbound_nodes(&self, responder: fdd::ManagerBindAllUnboundNodesResponder) {
        let result = self.driver_runner.bind_manager.try_bind_all_available().await;
        let _ = responder.send(Ok(&result));
    }

    async fn bind_all_unbound_nodes2(&self, responder: fdd::ManagerBindAllUnboundNodes2Responder) {
        let result = self.driver_runner.bind_manager.try_bind_all_available().await;
        let _ = responder.send(Ok(&result));
    }

    async fn wait_for_bootup(&self, responder: fdd::ManagerWaitForBootupResponder) {
        self.driver_runner.bootup_tracker.wait_for_bootup().await;
        let _ = responder.send();
    }

    async fn get_driver_host_info(&self, iterator: ServerEnd<fdd::DriverHostInfoIteratorMarker>) {
        let mut driver_host_to_drivers: HashMap<zx::Koid, HashSet<String>> = HashMap::new();
        let mut unique_nodes = HashSet::new();
        let mut remaining_nodes = VecDeque::new();
        remaining_nodes.push_back(self.driver_runner.root_node());

        while let Some(node) = remaining_nodes.pop_front() {
            let node_ptr: *const _ = Rc::as_ptr(&node);
            if !unique_nodes.insert(node_ptr) {
                continue;
            }

            for child in node.children() {
                remaining_nodes.push_back(child);
            }

            if node.is_bound()
                && let Some(host) = node.driver_host()
                && let Ok(koid) = host.get_process_koid().await
            {
                driver_host_to_drivers.entry(koid).or_default().insert(node.driver_url());
            }
        }

        let mut infos = vec![];
        for host in self.driver_runner.driver_hosts() {
            let process_info = match host.get_process_info_internal().await {
                Ok(info) => info,
                Err(_) => continue,
            };

            let threads = process_info
                .threads
                .into_iter()
                .map(|t| fdd::ThreadInfo {
                    koid: Some(t.koid),
                    name: Some(t.name),
                    scheduler_role: Some(t.scheduler_role),
                    ..Default::default()
                })
                .collect();

            let dispatchers = process_info
                .dispatchers
                .into_iter()
                .map(|d| fdd::DispatcherInfo {
                    driver: Some(d.driver),
                    name: Some(d.name),
                    options: Some(d.options),
                    scheduler_role: Some(d.scheduler_role),
                    ..Default::default()
                })
                .collect();

            let mut drivers = vec![];
            if let Some(d) = driver_host_to_drivers.get(&process_info.process_koid) {
                drivers = d.iter().cloned().collect();
                drivers.sort();
            }

            infos.push(fdd::DriverHostInfo {
                process_koid: Some(process_info.process_koid.raw_koid()),
                name: Some(host.name_for_colocation().to_string()),
                threads: Some(threads),
                dispatchers: Some(dispatchers),
                drivers: Some(drivers),
                ..Default::default()
            });
        }

        let iterator_stream = iterator.into_stream();
        let driver_host_info_iterator = DriverHostInfoIterator::new(infos);
        self.scope.spawn_local(async move {
            if let Err(e) = driver_host_info_iterator.serve(iterator_stream).await {
                warn!("DriverHostInfoIterator server failed: {}", e);
            }
        });
    }

    async fn restart_driver_hosts(
        &self,
        driver_url: String,
        rematch_flags: fdd::RestartRematchFlags,
        responder: fdd::ManagerRestartDriverHostsResponder,
    ) {
        let result = self
            .driver_runner
            .restart_nodes_colocated_with_driver_url(&driver_url, rematch_flags)
            .await;
        match result {
            Ok(count) => {
                let _ = responder.send(Ok(count));
            }
            Err(status) => {
                let _ = responder.send(Err(status.into_raw()));
            }
        }
    }

    async fn disable_driver(
        &self,
        driver_url: String,
        package_hash: Option<String>,
        responder: fdd::ManagerDisableDriverResponder,
    ) {
        let driver_index_client = match connect_to_protocol::<fdi::DevelopmentManagerMarker>() {
            Ok(proxy) => proxy,
            Err(e) => {
                error!(
                    "Failed to connect to service '{}': {}",
                    fdi::DevelopmentManagerMarker::PROTOCOL_NAME,
                    e
                );
                responder.control_handle().shutdown_with_epitaph(zx::Status::UNAVAILABLE);
                return;
            }
        };

        match driver_index_client.disable_driver(&driver_url, package_hash.as_deref()).await {
            Ok(result) => {
                let _ = responder.send(result);
            }
            Err(e) => {
                error!("Failed to call DriverIndex::DisableDriver: {}", e);
                let epitaph = match e {
                    fidl::Error::ClientChannelClosed { epitaph, .. } => epitaph,
                    _ => zx::Status::INTERNAL.into(),
                };
                responder.control_handle().shutdown_with_epitaph(epitaph);
            }
        }
    }

    async fn enable_driver(
        &self,
        driver_url: String,
        package_hash: Option<String>,
        responder: fdd::ManagerEnableDriverResponder,
    ) {
        let driver_index_client = match connect_to_protocol::<fdi::DevelopmentManagerMarker>() {
            Ok(proxy) => proxy,
            Err(e) => {
                error!(
                    "Failed to connect to service '{}': {}",
                    fdi::DevelopmentManagerMarker::PROTOCOL_NAME,
                    e
                );
                responder.control_handle().shutdown_with_epitaph(zx::Status::UNAVAILABLE);
                return;
            }
        };

        match driver_index_client.enable_driver(&driver_url, package_hash.as_deref()).await {
            Ok(result) => {
                let _ = responder.send(result);
            }
            Err(e) => {
                error!("Failed to call DriverIndex::EnableDriver: {}", e);
                let epitaph = match e {
                    fidl::Error::ClientChannelClosed { epitaph, .. } => epitaph,
                    _ => zx::Status::INTERNAL.into(),
                };
                responder.control_handle().shutdown_with_epitaph(epitaph);
            }
        }
    }

    async fn rebind_composites_with_driver(
        &self,
        driver_url: String,
        responder: fdd::ManagerRebindCompositesWithDriverResponder,
    ) {
        let driver_index_client = match connect_to_protocol::<fdi::DevelopmentManagerMarker>() {
            Ok(proxy) => proxy,
            Err(e) => {
                error!(
                    "Failed to connect to service '{}': {}",
                    fdi::DevelopmentManagerMarker::PROTOCOL_NAME,
                    e
                );
                responder.control_handle().shutdown_with_epitaph(zx::Status::UNAVAILABLE);
                return;
            }
        };

        match driver_index_client.rebind_composites_with_driver(&driver_url).await {
            Ok(Ok(())) => {
                // success from driver_index, now rebind in driver_runner
                let count = self.driver_runner.rebind_composites_with_driver(&driver_url).await;
                let _ = responder.send(Ok(count));
            }
            Ok(Err(status)) => {
                error!(
                    "DriverIndex::RebindCompositesWithDriver failed: {}",
                    zx::Status::err_from_raw(status)
                );
                let _ = responder.send(Err(status));
            }
            Err(e) => {
                error!("Failed to call DriverIndex::RebindCompositesWithDriver: {}", e);
                let status = match e {
                    fidl::Error::ClientChannelClosed { epitaph, .. } => match epitaph.into() {
                        Err(s) => s,
                        Ok(()) => zx::Status::PEER_CLOSED,
                    },
                    _ => zx::Status::INTERNAL,
                };
                let _ = responder.send(Err(status.into_raw()));
            }
        }
    }

    async fn restart_with_dictionary(
        &self,
        moniker: String,
        dictionary: fidl_fuchsia_component_sandbox::DictionaryRef,
        responder: fdd::ManagerRestartWithDictionaryResponder,
    ) {
        let (endpoint0, endpoint1) = zx::EventPair::create();
        self.driver_runner.restart_with_dictionary(moniker, dictionary, endpoint1).await;
        let _ = responder.send(Ok(endpoint0));
    }
}

async fn create_device_info(node: &Rc<Node>) -> Result<fdd::NodeInfo, zx::Status> {
    let children = node.children();
    let child_ids: Vec<u64> = children.iter().map(|child| Rc::as_ptr(child) as u64).collect();

    let parents = node.parents();
    let parent_ids: Vec<u64> = parents
        .iter()
        .filter_map(|parent| parent.upgrade())
        .map(|parent| Rc::as_ptr(&parent) as u64)
        .collect();

    let driver_host_koid = if node.as_ref().is_bound() {
        match node.driver_host().as_ref() {
            Some(dh) => Some(dh.get_process_koid().await.unwrap().raw_koid()),
            None => None,
        }
    } else {
        None
    };

    let offers = node.offers();
    let offer_list: Vec<fdecl::Offer> = offers
        .iter()
        .map(|o| o.into())
        .filter_map(|offer| match offer {
            fdf::Offer::DriverTransport(d) => Some(d),
            fdf::Offer::ZirconTransport(z) => Some(z),
            fdf::Offer::DictionaryOffer(d) => Some(d),
            _ => None,
        })
        .collect();

    let node_property_list = if node.is_composite() {
        None
    } else {
        node.get_node_properties(None).and_then(|properties| {
            if properties.is_empty() {
                None
            } else {
                Some(properties.iter().map(to_deprecated_property).collect())
            }
        })
    };

    Ok(fdd::NodeInfo {
        id: Some(Rc::as_ptr(node) as u64),
        moniker: Some(node.make_component_moniker()),
        bound_driver_url: Some(node.driver_url()),
        quarantined: Some(node.is_quarantined()),
        child_ids: if child_ids.is_empty() { None } else { Some(child_ids) },
        parent_ids: if parent_ids.is_empty() { None } else { Some(parent_ids) },
        driver_host_koid,
        offer_list: if offer_list.is_empty() { None } else { Some(offer_list) },
        node_property_list,
        bus_topology: Some(node.get_bus_topology()),
        ..Default::default()
    })
}
