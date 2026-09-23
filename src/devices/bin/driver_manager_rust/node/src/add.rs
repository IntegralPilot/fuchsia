// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::node::{Node, NodePropertyEntry};
use crate::types::{NodeDictionary, NodeState};
use driver_manager_types::{Collection, NodeOffer, OfferTransport, to_property2};
use fidl::endpoints::ServerEnd;
use fidl_fuchsia_component_decl as fdecl;
use fidl_fuchsia_device_fs as fdevfs;
use fidl_fuchsia_driver_framework as fdf;
use futures::channel::oneshot;
use log::{error, warn};
use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

impl Node {
    pub async fn add_child(
        self: &Rc<Self>,
        mut args: fdf::NodeAddArgs,
        controller: Option<ServerEnd<fdf::NodeControllerMarker>>,
        node: Option<ServerEnd<fdf::NodeMarker>>,
    ) -> Result<Rc<Node>, fdf::NodeError> {
        let name = args.name.ok_or(fdf::NodeError::NameMissing)?;

        if args.properties.is_some() && args.properties2.is_some() {
            return Err(fdf::NodeError::UnsupportedArgs);
        }

        self.wait_for_child_to_exit(&name).await?;

        let child = Node::new(&name, self.weak_self.clone(), self.node_manager.clone_box());

        let mut properties = if let Some(props) = args.properties2 {
            props
        } else if let Some(props) = args.properties {
            props.into_iter().map(|prop| to_property2(&prop)).collect()
        } else {
            vec![]
        };

        let has_manual_service_property =
            properties.iter().any(|prop| prop.key == bind_fuchsia::SERVICE);

        let mut has_dictionary_offer = false;

        if let Some(offers) = args.offers2 {
            let mut source_node = Some(self.clone());
            while source_node.is_some()
                && source_node.as_ref().unwrap().collection() == Collection::None
            {
                let current_node = source_node.unwrap();
                source_node = current_node.get_primary_parent();
            }

            let (source_name, source_collection) = if let Some(source_node) = source_node {
                (source_node.make_component_moniker(), source_node.collection())
            } else {
                (self.make_component_moniker(), self.collection())
            };

            child.reserve_offers(offers.len());

            for offer in offers {
                if matches!(offer, fdf::Offer::DictionaryOffer(_)) {
                    has_dictionary_offer = true;
                }

                match Self::process_node_offer_with_transport_property(
                    &offer,
                    source_collection,
                    &source_name,
                    !has_manual_service_property,
                ) {
                    Ok((processed_offer, property)) => {
                        child.push_offer(processed_offer);
                        if let Some(property) = property {
                            properties.push(property);
                        }
                    }
                    Err(e) => return Err(e),
                }
            }
        }

        child.set_non_composite_properties(properties);

        if let Some(driver_host) = args.driver_host {
            child.set_driver_host_name_for_colocation(&driver_host);
        }

        if let Some(symbols) = args.symbols {
            let mut names = HashSet::new();
            for symbol in &symbols {
                if symbol.name.is_none() {
                    return Err(fdf::NodeError::SymbolNameMissing);
                }
                if symbol.address.is_none() {
                    return Err(fdf::NodeError::SymbolAddressMissing);
                }
                if !names.insert(symbol.name.as_ref().unwrap()) {
                    return Err(fdf::NodeError::SymbolAlreadyExists);
                }
            }
            child.set_symbols(symbols);
        }

        if let Some(bus_info) = args.bus_info {
            child.set_bus_info(bus_info);
        }

        // Copy the subtree dictionary of a parent node down to the child.
        if let NodeDictionary::Subtree(d) = self.dictionary() {
            if has_dictionary_offer {
                panic!("Cannot use dictionary offers on node");
            }

            child.set_dictionary(NodeDictionary::Subtree(d));
        }

        let devfs_class_path = args.devfs_args.as_ref().and_then(|args| args.class_name.clone());

        let devfs_connector = if let Some(ref mut devfs_args) = args.devfs_args {
            let allow_controller = match devfs_args.connector_supports {
                Some(supports) => supports.contains(fdevfs::ConnectionType::CONTROLLER),
                _ => false,
            };
            let class_name = match (allow_controller, &devfs_args.class_name) {
                (_, Some(class_name)) => class_name.clone(),
                (true, None) => format!("No_class_name_but_driver_url_is_{}", self.driver_url()),
                (_, _) => "Unknown_Class_name".to_string(),
            };

            child.create_devfs_passthrough(
                devfs_args.connector.take(),
                devfs_args.controller_connector.take(),
                allow_controller,
                class_name,
            )
        } else {
            child.create_devfs_passthrough(None, None, false, "Unknown_Class_name".to_string())
        };

        let devfs_device = {
            let device = self.device();
            let topological = device.topological.as_ref().unwrap_or_else(|| {
                panic!("Missing topological devfs node: {}", self.make_topological_path(false))
            });

            topological
                .add_child(child.name(), devfs_class_path.as_deref(), devfs_connector)
                .unwrap_or_else(|_| {
                    panic!("Failed to export {}", child.make_topological_path(false))
                })
        };
        assert!(devfs_device.topological.is_some());
        child.set_device(devfs_device);

        if let Some(controller) = controller {
            let control_handle = child.serve_node_controller(controller);
            child.set_node_controller(control_handle);
        }

        if has_dictionary_offer && args.offers_dictionary.is_none() {
            warn!("cannot have dictionary type offers without supplying the offers_dictionary");
            return Err(fdf::NodeError::UnsupportedArgs);
        }

        if !has_dictionary_offer && args.offers_dictionary.is_some() {
            warn!("supplied offers_dictionary but no offers have Dictionary type.");
            return Err(fdf::NodeError::UnsupportedArgs);
        }

        if let Some(offers_dictionary) = args.offers_dictionary {
            let dictionary_util = self.node_manager.get_dictionary_util().map_err(|e| {
                error!("failed to get dictionary util: {}", e);
                fdf::NodeError::Internal
            })?;

            let dictionary_id =
                dictionary_util.import_dictionary(offers_dictionary).await.map_err(|e| {
                    error!("failed to import dictionary: {}", e);
                    fdf::NodeError::Internal
                })?;

            let dictionary_offer_services = child
                .offers()
                .iter()
                .filter(|offer| matches!(offer.transport, OfferTransport::Dictionary))
                .map(|offer| offer.service_name.clone())
                .collect::<Vec<_>>();

            for dictionary_offer_service in dictionary_offer_services {
                let dir_connector = dictionary_util
                    .dictionary_dir_connector_route(dictionary_id, &dictionary_offer_service)
                    .await
                    .map_err(|e| {
                        error!("failed to route dictionary: {}", e);
                        fdf::NodeError::Internal
                    })?;

                let mut offers = child.offers();
                offers
                    .iter_mut()
                    .find(|offer| {
                        matches!(offer.transport, OfferTransport::Dictionary)
                            && offer.service_name == dictionary_offer_service
                    })
                    .unwrap()
                    .dir_connector = Rc::new(RefCell::new(Some(dir_connector)));
                child.set_offers(offers);
            }
        }

        if let Some(node) = node {
            let node_server_binding = child.serve_node(node);
            child.set_state(NodeState::OwnedByParent {
                node_server_binding: Some(node_server_binding),
            });
        } else {
            // Use a silent bind tracker to avoid tracking binds.
            let tracker = child.create_bind_result_tracker(true);
            self.node_manager.bind(&child, tracker);
        }

        child.add_to_parents();

        Ok(child)
    }

    async fn wait_for_child_to_exit(&self, name: &str) -> Result<(), fdf::NodeError> {
        let (sender, receiver) = oneshot::channel();
        {
            let child = self.children().into_iter().find(|c| c.name() == name);
            if let Some(child) = child {
                if !child.node_shutdown_coordinator.borrow().is_shutting_down() {
                    return Err(fdf::NodeError::NameAlreadyExists);
                }
                child.set_remove_complete_callback(sender);
                child.node_shutdown_coordinator.borrow_mut().check_node_state();
            } else {
                return Ok(());
            }
        }

        // Wait for channel
        receiver.await.map_err(|_| fdf::NodeError::Internal)
    }

    fn process_node_offer_with_transport_property(
        add_offer: &fdf::Offer,
        source_collection: Collection,
        source_name: &str,
        generate_service_property: bool,
    ) -> Result<(NodeOffer, Option<fdf::NodeProperty2>), fdf::NodeError> {
        let processed_offer = Self::process_node_offer(add_offer, source_collection, source_name)?;
        let name = &processed_offer.service_name;
        let property = if generate_service_property && !should_exclude_service(name) {
            Some(fdf::NodeProperty2 {
                key: bind_fuchsia::SERVICE.to_string(),
                value: fdf::NodePropertyValue::StringValue(name.clone()),
            })
        } else {
            None
        };
        Ok((processed_offer, property))
    }

    fn process_node_offer(
        add_offer: &fdf::Offer,
        source_collection: Collection,
        source_name: &str,
    ) -> Result<NodeOffer, fdf::NodeError> {
        let (fdecl_offer, transport) = match add_offer {
            fdf::Offer::ZirconTransport(offer) => (offer, OfferTransport::ZirconTransport),
            fdf::Offer::DriverTransport(offer) => (offer, OfferTransport::DriverTransport),
            fdf::Offer::DictionaryOffer(offer) => (offer, OfferTransport::Dictionary),
            _ => {
                error!("Unknown offer transport type");
                return Err(fdf::NodeError::Internal);
            }
        };

        let service_offer = match fdecl_offer {
            fdecl::Offer::Service(service) => service,
            _ => return Err(fdf::NodeError::UnsupportedArgs),
        };

        let source_name_from_offer =
            service_offer.source_name.as_ref().ok_or(fdf::NodeError::OfferSourceNameMissing)?;

        if let Some(target_name) = &service_offer.target_name
            && target_name != source_name_from_offer
        {
            return Err(fdf::NodeError::UnsupportedArgs);
        }

        if service_offer.source.is_some() || service_offer.target.is_some() {
            return Err(fdf::NodeError::OfferRefExists);
        }

        let source_instance_filter = service_offer
            .source_instance_filter
            .as_ref()
            .ok_or(fdf::NodeError::OfferSourceInstanceFilterMissing)?;

        let renamed_instances = service_offer
            .renamed_instances
            .as_ref()
            .ok_or(fdf::NodeError::OfferRenamedInstancesMissing)?;

        // Dictionary based offers don't go to the component framework, but developers can see
        // these fields in the node list output.
        let (source_name, source_collection) = match transport {
            OfferTransport::Dictionary => ("dictionary", Collection::None),
            _ => (source_name, source_collection),
        };

        Ok(NodeOffer {
            source_name: source_name.to_string(),
            source_collection,
            transport,
            service_name: source_name_from_offer.clone(),
            source_instance_filter: source_instance_filter.clone(),
            renamed_instances: renamed_instances.clone(),
            dir_connector: Rc::new(RefCell::new(None)),
        })
    }

    fn set_non_composite_properties(&self, properties: Vec<fdf::NodeProperty2>) {
        self.clear_properties();
        self.push_property(NodePropertyEntry {
            name: "default".to_string(),
            properties: properties.into_iter().map(|p| p.into()).collect(),
        });
    }
}

fn should_exclude_service(service_name: &str) -> bool {
    if service_name == "fuchsia.driver.compat.Service"
        || service_name == "fuchsia.hardware.power.PowerTokenService"
        || service_name == "fuchsia.hardware.interrupt.ControllerRegistryService"
        || service_name == "fuchsia.hardware.goldfish.ControlService"
    {
        return true;
    }
    if service_name.contains("Metadata") {
        return true;
    }
    false
}
