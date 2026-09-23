// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_component_sandbox::DirConnector;
use std::cell::RefCell;
use std::fmt;
use std::rc::Rc;
use {fidl_fuchsia_component_decl as fdecl, fidl_fuchsia_driver_framework as fdf};

#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Copy, Debug)]
#[repr(u8)]
pub enum Collection {
    None,
    Boot,
    Package,
    FullPackage,
}

impl fmt::Display for Collection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Collection::None => "none",
                Collection::Boot => "boot-drivers",
                Collection::Package => "base-drivers",
                Collection::FullPackage => "full-drivers",
            },
        )
    }
}

#[derive(PartialEq, Eq, Debug)]
pub enum NodeType {
    Normal,
    Composite,
}

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub enum ShutdownState {
    Running,
    Prestop,
    WaitingOnDriverBind,
    WaitingOnChildren,
    WaitingOnDriver,
    WaitingOnDriverComponent,
    Stopped,
    WaitingOnDestroy,
    Destroyed,
}

impl fmt::Display for ShutdownState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            match self {
                ShutdownState::WaitingOnDriverBind => "waiting for driver to finish binding",
                ShutdownState::Running => "in normal running state",
                ShutdownState::Prestop => "in running state, but flagged for removal soon.",
                ShutdownState::WaitingOnChildren => "waiting for children to complete shutdown",
                // This message is load-bearing server-side as it's used to identify the hanging driver.
                // Please notify //src/developer/forensics/OWNERS upon changing.
                ShutdownState::WaitingOnDriver => {
                    "waiting for driver's Stop() function and destructor finish running"
                }
                ShutdownState::WaitingOnDriverComponent =>
                    "waiting for the driver component to stop",
                ShutdownState::Stopped => "node component instance stop is completed",
                ShutdownState::WaitingOnDestroy => "waiting for the component to be destroyed.",
                ShutdownState::Destroyed => "node shutdown is completed",
            },
        )
    }
}

#[derive(Clone, Debug)]
pub enum OfferTransport {
    DriverTransport,
    ZirconTransport,
    Dictionary,
}

#[derive(Clone, Debug)]
pub struct NodeOffer {
    pub source_name: String,
    pub source_collection: Collection,
    pub transport: OfferTransport,
    pub service_name: String,
    pub source_instance_filter: Vec<String>,
    pub renamed_instances: Vec<fdecl::NameMapping>,
    pub dir_connector: Rc<RefCell<Option<DirConnector>>>,
}

impl From<&NodeOffer> for fdf::Offer {
    fn from(offer: &NodeOffer) -> Self {
        let service_offer = fdecl::OfferService {
            source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                name: offer.source_name.clone(),
                collection: Some(offer.source_collection.to_string()),
            })),
            source_name: Some(offer.service_name.clone()),
            target_name: Some(offer.service_name.clone()),
            source_instance_filter: Some(offer.source_instance_filter.clone()),
            renamed_instances: Some(offer.renamed_instances.clone()),
            ..Default::default()
        };
        let inner_offer = fdecl::Offer::Service(service_offer);
        match offer.transport {
            crate::node_types::OfferTransport::ZirconTransport => {
                fdf::Offer::ZirconTransport(inner_offer)
            }
            crate::node_types::OfferTransport::DriverTransport => {
                fdf::Offer::DriverTransport(inner_offer)
            }
            crate::node_types::OfferTransport::Dictionary => {
                fdf::Offer::DictionaryOffer(inner_offer)
            }
        }
    }
}
