// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::node::{Node, NodePropertyEntry};
use crate::node_manager::NodeManager;
use crate::shutdown::NodeBridge;
use crate::types::{NodeDictionary, NodeState, NodeTypeVariant};
use driver_manager_devfs::DevfsDevice;
use driver_manager_shutdown::{NodeShutdownCoordinator, ShutdownIntent};
use driver_manager_types::{Collection, NodeOffer, OfferTransport};
use futures::channel::oneshot;
use std::cell::RefCell;
use std::rc::{Rc, Weak};
use {
    fidl_fuchsia_component_decl as fdecl, fidl_fuchsia_driver_framework as fdf,
    fuchsia_async as fasync,
};

impl Node {
    pub fn new_composite(
        name: &str,
        parents: Vec<Weak<Node>>,
        parents_names: Vec<String>,
        primary_index: u32,
        node_manager: Box<dyn NodeManager>,
    ) -> Rc<Self> {
        let initial_host = parents[primary_index as usize].upgrade().and_then(|p| p.driver_host());

        Rc::new_cyclic(|weak_self| {
            let bridge = Box::new(NodeBridge::new(weak_self.clone()));
            let enable_test_shutdown_delays = node_manager.is_test_shutdown_delay_enabled();
            let shutdown_test_rng = node_manager.get_shutdown_test_rng();
            Self {
                name: name.to_string(),
                node_manager,
                core: RefCell::new(crate::node::NodeCore {
                    collection: Collection::None,
                    driver_package_type: fdf::DriverPackageType::Base,
                    node_type: NodeTypeVariant::Composite {
                        parents,
                        parents_names,
                        primary_index,
                    },
                    children: Vec::new(),
                    properties: Vec::new(),
                    symbols: Vec::new(),
                    offers: Vec::new(),
                    bus_info: None,
                    dictionary: NodeDictionary::None,
                }),
                state: RefCell::new(NodeState::Unbound),
                devfs: RefCell::new(crate::node::NodeDevfs {
                    device: DevfsDevice::new(),
                    protocol_connector: None,
                    controller_allowlist_passthrough: None,
                }),
                shutdown: RefCell::new(crate::node::NodeShutdown {
                    remove_complete_callback: None,
                    unbinding_children_completers: Vec::new(),
                    should_destroy_driver_component: false,
                }),
                binding: RefCell::new(crate::node::NodeBinding {
                    node_controller: None,
                    pending_bind_completer: None,
                    bind_error: None,
                    wait_for_driver_completer: None,
                    restart_driver_url_suffix: None,
                    composite_rebind_completer: None,
                }),
                component: RefCell::new(None),
                driver_host: RefCell::new(crate::node::NodeDriverHost {
                    host: initial_host,
                    name_for_colocation: String::new(),
                    restart_on_crash: false,
                }),
                node_shutdown_coordinator: RefCell::new(NodeShutdownCoordinator::new(
                    bridge,
                    enable_test_shutdown_delays,
                    shutdown_test_rng,
                )),
                can_multibind_composites: true,
                weak_self: weak_self.clone(),
                scope: fasync::Scope::new_with_name(format!("node:{name}")),
            }
        })
    }

    pub fn create_composite_offer(
        offer: &NodeOffer,
        parents_name: &str,
        primary_parent: bool,
    ) -> NodeOffer {
        let is_default_offer = |name: &str| name == "default";

        let mut new_instance_count = offer.renamed_instances.len();
        if primary_parent {
            new_instance_count += offer
                .renamed_instances
                .iter()
                .filter(|instance| is_default_offer(&instance.target_name))
                .count();
        }

        let mut new_mappings = Vec::with_capacity(new_instance_count);
        for instance in &offer.renamed_instances {
            let target_name = &instance.target_name;
            if !is_default_offer(target_name) {
                new_mappings.push(instance.clone());
                continue;
            }

            if primary_parent {
                new_mappings.push(instance.clone());
            }

            new_mappings.push(fdecl::NameMapping {
                source_name: instance.source_name.clone(),
                target_name: parents_name.to_string(),
            });
        }

        let mut new_filter_count = offer.source_instance_filter.len();
        if primary_parent {
            new_filter_count += offer
                .source_instance_filter
                .iter()
                .filter(|filter| is_default_offer(filter))
                .count();
        }

        let mut new_filters = Vec::with_capacity(new_filter_count);
        for filter in &offer.source_instance_filter {
            if !is_default_offer(filter) {
                new_filters.push(filter.clone());
                continue;
            }

            if primary_parent {
                new_filters.push("default".to_string());
            }

            new_filters.push(parents_name.to_string());
        }

        NodeOffer {
            source_name: offer.source_name.clone(),
            source_collection: offer.source_collection,
            transport: offer.transport.clone(),
            service_name: offer.service_name.clone(),
            source_instance_filter: new_filters,
            renamed_instances: new_mappings,
            dir_connector: offer.dir_connector.clone(),
        }
    }

    pub fn create_composite_node(
        node_name: &str,
        parents: Vec<Weak<Node>>,
        parents_names: Vec<String>,
        parent_properties: &[NodePropertyEntry],
        node_manager: Box<dyn NodeManager>,
        driver_host_name_for_colocation: String,
        primary_index: u32,
    ) -> Result<Rc<Self>, zx::Status> {
        if parents.is_empty() {
            return Err(zx::Status::INVALID_ARGS);
        }
        if parents.len() != parent_properties.len() {
            return Err(zx::Status::INVALID_ARGS);
        }
        if primary_index as usize >= parents.len() {
            return Err(zx::Status::OUT_OF_RANGE);
        }

        let mut has_dictionary_offer = false;

        let mut offers = vec![];
        for (i, parent) in parents.iter().enumerate() {
            let parent = parent.upgrade().expect("parent should be alive");
            for offer in parent.offers().iter() {
                let new_offer = Self::create_composite_offer(
                    offer,
                    &parents_names[i],
                    i == primary_index as usize,
                );

                if matches!(new_offer.transport, OfferTransport::Dictionary) {
                    has_dictionary_offer = true;
                }

                offers.push(new_offer);
            }
        }

        let composite =
            Self::new_composite(node_name, parents, parents_names, primary_index, node_manager);

        composite.set_driver_host_name_for_colocation(&driver_host_name_for_colocation);

        Self::set_composite_parent_properties(&composite, parent_properties);

        let primary_parent = composite.get_primary_parent().expect("primary parent should exist");
        let symbols = primary_parent.symbols();
        composite.set_symbols(symbols);

        // Copy the subtree dictionary of the primary parent node down to the composite.
        if let NodeDictionary::Subtree(d) = primary_parent.dictionary() {
            if has_dictionary_offer {
                panic!("Cannot use dictionary offers on node");
            }

            composite.set_subtree_dictionary(d);
        }

        composite.set_offers(offers);

        composite.add_to_parents();

        let primary_parent_device = primary_parent.device();
        let topological = primary_parent_device.topological.as_ref().unwrap_or_else(|| {
            panic!(
                "Missing topological devfs node for primary parent: {}",
                composite.make_topological_path(false)
            )
        });

        // TODO(https://fxbug.dev/331779666): disable controller access for composite nodes
        let devfs_device = topological.add_child(
            &composite.name,
            None,
            composite.create_devfs_passthrough(None, None, true, "".to_string()),
        )?;
        composite.set_device(devfs_device);

        Ok(composite)
    }

    pub fn set_composite_parent_properties(&self, parent_properties: &[NodePropertyEntry]) {
        self.clear_properties();
        self.set_properties(parent_properties.to_vec());
        let primary_index = if let NodeTypeVariant::Composite { primary_index, .. } = &*self.node_type() {
            Some(*primary_index)
        } else {
            None
        };
        if let Some(primary_index) = primary_index {
            let default_properties = &parent_properties[primary_index as usize].properties;
            self.push_property(NodePropertyEntry {
                name: "default".to_string(),
                properties: default_properties.to_vec(),
            });
        }
    }

    pub fn remove_composite_node_for_rebind(
        self: &Rc<Self>,
        completer: oneshot::Sender<Result<(), zx::Status>>,
    ) {
        if let Err(completer) = self.set_composite_rebind_completer(completer) {
            let _ = completer.send(Err(zx::Status::ALREADY_EXISTS));
            return;
        }

        self.get_shutdown_coordinator().set_shutdown_intent(ShutdownIntent::RebindComposite);
        self.remove(driver_manager_shutdown::RemovalSet::All, None);
    }
}
