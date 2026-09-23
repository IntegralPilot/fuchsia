// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::devfs::ControllerAllowlistPassthrough;
use crate::node_manager::NodeManager;
use crate::serve::{ComponentControllerClientBinding, NodeControllerServerBinding};
use crate::shutdown::NodeBridge;
use crate::types::{DriverState, NodeDictionary, NodeState, NodeTypeVariant};
use driver_manager_devfs::DevfsDevice;
use driver_manager_driver_host::DriverHost;
use driver_manager_shutdown::{NodeRemovalTracker, NodeShutdownCoordinator, RemovalSet};
use driver_manager_types::{
    Collection, NodeOffer, OfferTransport, ShutdownState, StartRequestReceiver,
};
use fidl_fuchsia_component_sandbox::AggregateSource;
use fidl_fuchsia_device_fs as fdevfs;
use fidl_fuchsia_driver_development as fdd;
use fidl_fuchsia_driver_framework as fdf;
use fidl_fuchsia_driver_host as fdh;
use flyweights::FlyStr;
use fuchsia_async::{self as fasync};
use futures::channel::oneshot;
use log::{debug, warn};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};

#[derive(Clone)]
pub enum NodePropertyValue {
    IntValue(u32),
    StringValue(FlyStr),
    BoolValue(bool),
    EnumValue(FlyStr),
}

impl std::convert::From<fdf::NodePropertyValue> for NodePropertyValue {
    fn from(source: fdf::NodePropertyValue) -> Self {
        match source {
            fdf::NodePropertyValue::IntValue(i) => Self::IntValue(i),
            fdf::NodePropertyValue::StringValue(s) => Self::StringValue(FlyStr::new(s)),
            fdf::NodePropertyValue::BoolValue(b) => Self::BoolValue(b),
            fdf::NodePropertyValue::EnumValue(e) => Self::EnumValue(FlyStr::new(e)),
            _ => unimplemented!(),
        }
    }
}

impl std::convert::From<NodePropertyValue> for fdf::NodePropertyValue {
    fn from(source: NodePropertyValue) -> fdf::NodePropertyValue {
        match source {
            NodePropertyValue::IntValue(i) => fdf::NodePropertyValue::IntValue(i),
            NodePropertyValue::StringValue(s) => fdf::NodePropertyValue::StringValue(s.to_string()),
            NodePropertyValue::BoolValue(b) => fdf::NodePropertyValue::BoolValue(b),
            NodePropertyValue::EnumValue(e) => fdf::NodePropertyValue::EnumValue(e.to_string()),
        }
    }
}

#[derive(Clone)]
pub struct NodeProperty {
    pub key: FlyStr,
    pub value: fdf::NodePropertyValue,
}

impl std::convert::From<fdf::NodeProperty2> for NodeProperty {
    fn from(source: fdf::NodeProperty2) -> Self {
        Self { key: FlyStr::new(source.key), value: source.value }
    }
}

impl std::convert::From<NodeProperty> for fdf::NodeProperty2 {
    fn from(source: NodeProperty) -> fdf::NodeProperty2 {
        fdf::NodeProperty2 { key: source.key.to_string(), value: source.value }
    }
}

#[derive(Clone)]
pub struct NodePropertyEntry {
    pub name: String,
    pub properties: Vec<NodeProperty>,
}

impl std::convert::From<fdf::NodePropertyEntry2> for NodePropertyEntry {
    fn from(source: fdf::NodePropertyEntry2) -> Self {
        Self {
            name: source.name,
            properties: source.properties.into_iter().map(|p| p.into()).collect(),
        }
    }
}

impl std::convert::From<NodePropertyEntry> for fdf::NodePropertyEntry2 {
    fn from(source: NodePropertyEntry) -> fdf::NodePropertyEntry2 {
        fdf::NodePropertyEntry2 {
            name: source.name,
            properties: source.properties.into_iter().map(|p| p.into()).collect(),
        }
    }
}

pub(crate) struct NodeDevfs {
    pub(crate) device: DevfsDevice,
    pub(crate) protocol_connector: Option<fdevfs::ConnectorProxy>,
    pub(crate) controller_allowlist_passthrough: Option<Rc<ControllerAllowlistPassthrough>>,
}

impl NodeDevfs {
    pub(crate) fn set_protocol_connector(&mut self, connector: fdevfs::ConnectorProxy) {
        self.protocol_connector = Some(connector);
    }

    pub(crate) fn set_controller_allowlist_passthrough(
        &mut self,
        passthrough: Rc<ControllerAllowlistPassthrough>,
    ) {
        self.controller_allowlist_passthrough = Some(passthrough);
    }

    pub(crate) fn set_device(&mut self, device: DevfsDevice) {
        self.device = device;
    }

    pub(crate) fn device(&self) -> &DevfsDevice {
        &self.device
    }
}

pub(crate) struct NodeShutdown {
    pub(crate) remove_complete_callback: Option<oneshot::Sender<()>>,
    pub(crate) unbinding_children_completers: Vec<oneshot::Sender<Result<(), zx::Status>>>,
    pub(crate) should_destroy_driver_component: bool,
}

impl NodeShutdown {
    pub(crate) fn set_remove_complete_callback(&mut self, callback: oneshot::Sender<()>) {
        self.remove_complete_callback = Some(callback);
    }

    pub(crate) fn push_unbinding_children_completer(
        &mut self,
        completer: oneshot::Sender<Result<(), zx::Status>>,
    ) {
        self.unbinding_children_completers.push(completer);
    }

    pub(crate) fn unbinding_children_completers_len(&self) -> usize {
        self.unbinding_children_completers.len()
    }

    pub(crate) fn has_remove_complete_callback(&self) -> bool {
        self.remove_complete_callback.is_some()
    }

    pub(crate) fn take_remove_complete_callback(&mut self) -> Option<oneshot::Sender<()>> {
        self.remove_complete_callback.take()
    }
}

pub(crate) struct NodeBinding {
    pub(crate) node_controller: Option<NodeControllerServerBinding>,
    pub(crate) pending_bind_completer: Option<oneshot::Sender<Result<(), zx::Status>>>,
    pub(crate) bind_error: Option<fdf::DriverResult>,
    pub(crate) wait_for_driver_completer:
        Option<oneshot::Sender<Result<fdf::DriverResult, zx::Status>>>,
    pub(crate) restart_driver_url_suffix: Option<String>,
    pub(crate) composite_rebind_completer: Option<oneshot::Sender<Result<(), zx::Status>>>,
}

impl NodeBinding {
    pub(crate) fn on_match_error(&mut self, error: zx::Status) {
        self.bind_error = Some(fdf::DriverResult::MatchError(error.into_raw()));
    }

    pub(crate) fn on_start_error(&mut self, error: zx::Status) {
        self.bind_error = Some(fdf::DriverResult::StartError(error.into_raw()));
    }

    pub(crate) fn bind_error(&self) -> Option<fdf::DriverResult> {
        match &self.bind_error {
            Some(fdf::DriverResult::MatchError(s)) => Some(fdf::DriverResult::MatchError(*s)),
            Some(fdf::DriverResult::StartError(s)) => Some(fdf::DriverResult::StartError(*s)),
            _ => None,
        }
    }

    pub(crate) fn has_pending_bind_completer(&self) -> bool {
        self.pending_bind_completer.is_some()
    }

    pub(crate) fn has_wait_for_driver_completer(&self) -> bool {
        self.wait_for_driver_completer.is_some()
    }

    pub(crate) fn node_controller_ref(&self) -> Option<fdf::NodeControllerControlHandle> {
        self.node_controller.as_ref().map(|c| c.node_controller_ref.clone())
    }
}

pub(crate) struct NodeComponent {
    pub(crate) controller: ComponentControllerClientBinding,
    pub(crate) start_request_receiver: Option<StartRequestReceiver>,
    pub(crate) start_handles: Option<Vec<fidl_fuchsia_process::HandleInfo>>,
}

impl NodeComponent {
    pub(crate) fn set_start_request_receiver(&mut self, receiver: StartRequestReceiver) {
        self.start_request_receiver = Some(receiver);
    }

    pub(crate) fn take_start_request_receiver(&mut self) -> Option<StartRequestReceiver> {
        self.start_request_receiver.take()
    }
}

pub(crate) struct NodeDriverHost {
    pub(crate) host: Option<Rc<dyn DriverHost>>,
    pub(crate) name_for_colocation: String,
    pub(crate) restart_on_crash: bool,
}

impl NodeDriverHost {
    pub(crate) fn set_name_for_colocation(&mut self, name: &str) {
        self.name_for_colocation = name.to_string();
    }

    pub(crate) fn set_host(&mut self, host: Rc<dyn DriverHost>) {
        self.host = Some(host);
    }

    pub(crate) fn host(&self) -> Option<Rc<dyn DriverHost>> {
        self.host.clone()
    }

    pub(crate) fn set_restart_on_crash(&mut self, value: bool) {
        self.restart_on_crash = value;
    }
}

pub(crate) struct NodeCore {
    pub(crate) collection: Collection,
    pub(crate) driver_package_type: fdf::DriverPackageType,
    pub(crate) node_type: NodeTypeVariant,
    pub(crate) children: Vec<Rc<Node>>,
    pub(crate) properties: Vec<NodePropertyEntry>,
    pub(crate) symbols: Vec<fdf::NodeSymbol>,
    pub(crate) offers: Vec<NodeOffer>,
    pub(crate) bus_info: Option<fdf::BusInfo>,
    pub(crate) dictionary: NodeDictionary,
}

impl NodeCore {
    pub(crate) fn set_subtree_dictionary(
        &mut self,
        dictionary: fidl_fuchsia_component_sandbox::CapabilityId,
    ) {
        if matches!(self.dictionary, NodeDictionary::Standard(_)) {
            panic!("Cannot set subtree dictionary on nodes with standard dictionaries");
        }

        self.dictionary = NodeDictionary::Subtree(dictionary);
    }

    pub(crate) fn remove_subtree_dictionary(&mut self) {
        self.dictionary = NodeDictionary::None;
    }

    pub(crate) fn has_subtree_dictionary(&self) -> bool {
        matches!(self.dictionary, NodeDictionary::Subtree(_))
    }

    pub(crate) fn set_collection(&mut self, collection: Collection) {
        self.collection = collection;
    }

    pub(crate) fn set_driver_package_type(&mut self, package_type: fdf::DriverPackageType) {
        self.driver_package_type = package_type;
    }

    pub(crate) fn is_composite(&self) -> bool {
        matches!(self.node_type, NodeTypeVariant::Composite { .. })
    }

    pub(crate) fn dictionary_to_export(
        &self,
    ) -> Option<fidl_fuchsia_component_sandbox::CapabilityId> {
        match self.dictionary {
            NodeDictionary::None => None,
            NodeDictionary::Standard(d) => Some(d),
            NodeDictionary::Subtree(d) => Some(d),
        }
    }

    pub(crate) fn take_dictionary_offer_sources(
        &mut self,
    ) -> HashMap<String, Vec<AggregateSource>> {
        let mut sources = HashMap::<String, Vec<AggregateSource>>::new();
        for dictionary_offer in self.offers.iter_mut() {
            if !matches!(dictionary_offer.transport, OfferTransport::Dictionary) {
                continue;
            }

            if let Some(connector) = dictionary_offer.dir_connector.take() {
                sources.entry(dictionary_offer.service_name.clone()).or_default().push(
                    AggregateSource {
                        dir_connector: Some(connector),
                        source_instance_filter: Some(
                            dictionary_offer.source_instance_filter.clone(),
                        ),
                        renamed_instances: Some(dictionary_offer.renamed_instances.clone()),
                        ..Default::default()
                    },
                );
            }
        }
        sources
    }

    pub(crate) fn set_standard_dictionary(
        &mut self,
        dictionary: fidl_fuchsia_component_sandbox::CapabilityId,
    ) {
        self.dictionary = NodeDictionary::Standard(dictionary);
    }

    pub(crate) fn parents(&self) -> Vec<Weak<Node>> {
        match &self.node_type {
            NodeTypeVariant::Normal { parent } => vec![parent.clone()],
            NodeTypeVariant::Composite { parents, .. } => parents.clone(),
        }
    }

    pub(crate) fn get_primary_parent(&self) -> Option<Rc<Node>> {
        match &self.node_type {
            NodeTypeVariant::Normal { parent } => parent.upgrade(),
            NodeTypeVariant::Composite { parents, primary_index, .. } => {
                parents.get(*primary_index as usize).and_then(|p| p.upgrade())
            }
        }
    }

    pub(crate) fn add_child(&mut self, child: Rc<Node>) {
        self.children.push(child);
    }

    pub(crate) fn get_node_properties(
        &self,
        parent_name: Option<&str>,
    ) -> Option<Vec<fdf::NodeProperty2>> {
        let parent_name = parent_name.unwrap_or("default");
        for entry in self.properties.iter() {
            if entry.name == parent_name {
                return Some(entry.properties.clone().into_iter().map(|p| p.into()).collect());
            }
        }
        None
    }

    pub(crate) fn set_symbols(&mut self, symbols: Vec<fdf::NodeSymbol>) {
        self.symbols = symbols;
    }

    pub(crate) fn symbols(&self) -> &Vec<fdf::NodeSymbol> {
        &self.symbols
    }

    pub(crate) fn offers(&self) -> &Vec<NodeOffer> {
        &self.offers
    }

    pub(crate) fn set_offers(&mut self, offers: Vec<NodeOffer>) {
        self.offers = offers;
    }

    pub(crate) fn reserve_offers(&mut self, additional: usize) {
        self.offers.reserve(additional);
    }

    pub(crate) fn push_offer(&mut self, offer: NodeOffer) {
        self.offers.push(offer);
    }

    pub(crate) fn set_bus_info(&mut self, bus_info: fdf::BusInfo) {
        self.bus_info = Some(bus_info);
    }

    pub(crate) fn set_properties(&mut self, properties: Vec<NodePropertyEntry>) {
        self.properties = properties;
    }

    pub(crate) fn clear_properties(&mut self) {
        self.properties.clear();
    }

    pub(crate) fn push_property(&mut self, property: NodePropertyEntry) {
        self.properties.push(property);
    }

    pub(crate) fn dictionary(&self) -> &NodeDictionary {
        &self.dictionary
    }

    pub(crate) fn set_dictionary(&mut self, dictionary: NodeDictionary) {
        self.dictionary = dictionary;
    }

    pub(crate) fn remove_child_from_children(&mut self, child: &Rc<Node>) -> bool {
        self.children.retain(|c| !Rc::ptr_eq(c, child));
        true
    }
}

pub struct Node {
    pub(crate) name: String,
    pub(crate) node_manager: Box<dyn NodeManager>,
    pub(crate) core: RefCell<NodeCore>,
    pub(crate) state: RefCell<NodeState>,
    pub(crate) devfs: RefCell<NodeDevfs>,
    pub(crate) shutdown: RefCell<NodeShutdown>,
    pub(crate) binding: RefCell<NodeBinding>,
    pub(crate) component: RefCell<Option<NodeComponent>>,
    pub(crate) driver_host: RefCell<NodeDriverHost>,
    pub(crate) node_shutdown_coordinator: RefCell<NodeShutdownCoordinator>,
    pub can_multibind_composites: bool,
    pub(crate) weak_self: Weak<Self>,
    pub(crate) scope: fasync::Scope,
}

impl Drop for Node {
    fn drop(&mut self) {
        debug!("Node: '{}' dropped", self.name());
    }
}

impl Node {
    pub fn new(name: &str, parent: Weak<Node>, node_manager: Box<dyn NodeManager>) -> Rc<Self> {
        let driver_host = parent.upgrade().and_then(|p| p.host());
        Rc::new_cyclic(|weak_self| {
            let bridge = Box::new(NodeBridge::new(weak_self.clone()));
            let enable_test_shutdown_delays = node_manager.is_test_shutdown_delay_enabled();
            let shutdown_test_rng = node_manager.get_shutdown_test_rng();
            Self {
                name: name.to_string(),
                node_manager,
                core: RefCell::new(NodeCore {
                    collection: Collection::None,
                    driver_package_type: fdf::DriverPackageType::Base,
                    node_type: NodeTypeVariant::Normal { parent },
                    children: Vec::new(),
                    properties: Vec::new(),
                    symbols: Vec::new(),
                    offers: Vec::new(),
                    bus_info: None,
                    dictionary: NodeDictionary::None,
                }),
                state: RefCell::new(NodeState::Unbound),
                devfs: RefCell::new(NodeDevfs {
                    device: DevfsDevice::new(),
                    protocol_connector: None,
                    controller_allowlist_passthrough: None,
                }),
                shutdown: RefCell::new(NodeShutdown {
                    remove_complete_callback: None,
                    unbinding_children_completers: Vec::new(),
                    should_destroy_driver_component: false,
                }),
                binding: RefCell::new(NodeBinding {
                    node_controller: None,
                    pending_bind_completer: None,
                    bind_error: None,
                    wait_for_driver_completer: None,
                    restart_driver_url_suffix: None,
                    composite_rebind_completer: None,
                }),
                component: RefCell::new(None),
                driver_host: RefCell::new(NodeDriverHost {
                    host: driver_host,
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

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn token_koid(&self) -> Option<zx::Koid> {
        match &*self.state.borrow() {
            NodeState::DriverComponent(driver_component) => Some(driver_component.instance_koid()),
            _ => None,
        }
    }

    pub fn make_topological_path(&self, deduplicate: bool) -> String {
        let mut names = std::collections::VecDeque::new();
        let mut current = Some(self.weak_self.upgrade().unwrap());
        let mut prev_name = String::new();
        while let Some(node) = current {
            let name = node.name.clone();
            if !deduplicate || name != prev_name {
                names.push_front(name.clone());
                prev_name = name;
            }
            current = node.get_primary_parent();
        }
        names.into_iter().collect::<Vec<_>>().join("/")
    }

    pub fn on_match_error(&self, error: zx::Status) {
        self.binding.borrow_mut().on_match_error(error);
    }

    pub fn on_start_error(&self, error: zx::Status) {
        self.binding.borrow_mut().on_start_error(error);
    }

    pub fn mark_as_composite_parent(&self) {
        *self.state.borrow_mut() = NodeState::CompositeParent;
    }

    pub fn unmark_as_composite_parent(&self) {
        *self.state.borrow_mut() = NodeState::Unbound;
    }

    pub(crate) fn is_pending_bind(&self) -> bool {
        match &*self.state.borrow() {
            NodeState::DriverComponent(component) => component.state == DriverState::Binding,
            _ => false,
        }
    }

    pub(crate) fn clear_driver_host(&self) {
        if let NodeState::DriverComponent(ref mut component) = *self.state.borrow_mut() {
            component.driver_client_binding.take();
        }
    }

    pub fn make_component_moniker(&self) -> String {
        let mut topo_path = self.make_topological_path(true);

        let k_prefix = "dev/sys/platform/pt/";
        let k_prefix2 = "dev/sys/platform/";

        if topo_path == "dev" {
            topo_path = "root".to_string();
        } else if topo_path == "dev/sys/platform/pt" {
            topo_path = "board".to_string();
        } else if topo_path.starts_with(k_prefix) {
            topo_path.replace_range(0..k_prefix.len(), "");
        } else if topo_path.starts_with(k_prefix2) {
            topo_path.replace_range(0..k_prefix2.len(), "");
        }

        topo_path.replace([':', '.'], "_").replace('/', ".")
    }

    pub fn children(&self) -> Vec<Rc<Node>> {
        self.core.borrow().children.clone()
    }

    pub fn parents(&self) -> Vec<Weak<Node>> {
        self.core.borrow().parents()
    }

    pub(crate) fn is_root_node(&self) -> bool {
        self.make_topological_path(false) == "dev"
    }

    pub fn driver_url(&self) -> String {
        match &*self.state.borrow() {
            NodeState::Starting { driver_url } => driver_url.clone(),
            NodeState::DriverComponent(c) => c.driver_url.clone(),
            NodeState::Quarantined { driver_url } => driver_url.clone(),
            NodeState::OwnedByParent { .. } => "owned by parent".to_string(),
            NodeState::CompositeParent => "owned by composite(s)".to_string(),
            NodeState::Unbound => "unbound".to_string(),
        }
    }

    pub fn is_quarantined(&self) -> bool {
        matches!(&*self.state.borrow(), NodeState::Quarantined { .. })
    }

    pub fn get_primary_parent(&self) -> Option<Rc<Node>> {
        self.core.borrow().get_primary_parent()
    }

    pub fn get_bus_topology(&self) -> Vec<fdf::BusInfo> {
        let mut segments = vec![];
        let mut current = self.weak_self.upgrade();
        while let Some(node) = current {
            if let Some(bus_info) = node.core.borrow().bus_info.as_ref() {
                segments.push(bus_info.clone());
            }
            current = node.get_primary_parent();
        }
        segments.reverse();
        segments
    }

    pub fn get_node_properties(
        &self,
        parent_name: Option<&str>,
    ) -> Option<Vec<fdf::NodeProperty2>> {
        self.core.borrow().get_node_properties(parent_name)
    }

    pub(crate) fn add_to_parents(&self) {
        let this_node = self.weak_self.upgrade().unwrap();
        for parent in self.parents() {
            if let Some(p) = parent.upgrade() {
                p.add_child_to_children(this_node.clone());
            } else {
                warn!("Parent freed before child {} could be added to it", self.name());
            }
        }
    }

    pub(crate) fn add_child_to_children(&self, child: Rc<Node>) {
        self.core.borrow_mut().add_child(child);
    }

    pub fn add_child_to_children_for_testing(&self, child: Rc<Node>) {
        self.add_child_to_children(child);
    }

    pub fn set_state_for_testing(&self, state: NodeState) {
        self.set_state(state);
    }

    pub fn driver_host(&self) -> Option<Rc<dyn DriverHost>> {
        self.driver_host.borrow().host()
    }

    pub fn is_composite(&self) -> bool {
        self.core.borrow().is_composite()
    }

    pub fn is_bound(&self) -> bool {
        matches!(&*self.state.borrow(), NodeState::DriverComponent { .. })
    }

    pub fn evaluate_rematch_flags(
        &self,
        rematch_flags: fdd::RestartRematchFlags,
        url: &str,
    ) -> bool {
        if self.core.borrow().is_composite()
            && !rematch_flags.contains(fdd::RestartRematchFlags::COMPOSITE_SPEC)
        {
            return false;
        }

        let driver_url = self.driver_url();
        if driver_url == url && !rematch_flags.contains(fdd::RestartRematchFlags::REQUESTED) {
            return false;
        }

        if driver_url != url && rematch_flags.contains(fdd::RestartRematchFlags::NON_REQUESTED) {
            return false;
        }

        true
    }

    pub fn set_subtree_dictionary(&self, dictionary: fidl_fuchsia_component_sandbox::CapabilityId) {
        self.core.borrow_mut().set_subtree_dictionary(dictionary);
    }

    pub fn remove_subtree_dictionary(&self) {
        self.core.borrow_mut().remove_subtree_dictionary();
    }

    pub fn has_subtree_dictionary(&self) -> bool {
        self.core.borrow().has_subtree_dictionary()
    }

    pub fn skip_injected_offers(&self) -> bool {
        self.has_subtree_dictionary()
    }

    pub async fn prepare_dictionary(
        &self,
    ) -> Option<fidl_fuchsia_component_sandbox::DictionaryRef> {
        let dictionary_util = self.node_manager.get_dictionary_util().ok()?;

        let to_export = self.core.borrow().dictionary_to_export();

        if let Some(d) = to_export {
            return dictionary_util.copy_export_dictionary(d).await.ok();
        }

        let sources = self.core.borrow_mut().take_dictionary_offer_sources();

        let aggregate_dictionary =
            dictionary_util.create_aggregate_dictionary(sources).await.ok()?;

        self.core.borrow_mut().set_standard_dictionary(aggregate_dictionary);

        dictionary_util.copy_export_dictionary(aggregate_dictionary).await.ok()
    }

    pub fn remove(
        self: &Rc<Self>,
        removal_set: RemovalSet,
        removal_tracker: Option<Weak<RefCell<NodeRemovalTracker>>>,
    ) {
        NodeShutdownCoordinator::remove(self.clone(), removal_set, removal_tracker);
    }

    pub(crate) fn get_shutdown_coordinator(
        &self,
    ) -> std::cell::RefMut<'_, NodeShutdownCoordinator> {
        self.node_shutdown_coordinator.borrow_mut()
    }

    pub fn weak_from_this(&self) -> Weak<Self> {
        self.weak_self.clone()
    }

    pub fn collection(&self) -> Collection {
        self.core.borrow().collection
    }

    pub fn set_collection(&self, collection: Collection) {
        self.core.borrow_mut().set_collection(collection);
    }

    pub fn set_driver_package_type(&self, package_type: fdf::DriverPackageType) {
        self.core.borrow_mut().set_driver_package_type(package_type);
    }

    pub fn set_driver_host_name_for_colocation(&self, name: &str) {
        self.driver_host.borrow_mut().set_name_for_colocation(name);
    }

    pub fn node_type(&self) -> std::cell::Ref<'_, NodeTypeVariant> {
        std::cell::Ref::map(self.core.borrow(), |core| &core.node_type)
    }

    pub(crate) fn set_symbols(&self, symbols: Vec<fdf::NodeSymbol>) {
        self.core.borrow_mut().set_symbols(symbols);
    }

    pub fn symbols(&self) -> Vec<fdf::NodeSymbol> {
        self.core.borrow().symbols().clone()
    }

    pub fn offers(&self) -> Vec<NodeOffer> {
        self.core.borrow().offers().clone()
    }

    pub(crate) fn set_offers(&self, offers: Vec<NodeOffer>) {
        self.core.borrow_mut().set_offers(offers);
    }

    pub(crate) fn reserve_offers(&self, additional: usize) {
        self.core.borrow_mut().reserve_offers(additional);
    }

    pub(crate) fn push_offer(&self, offer: NodeOffer) {
        self.core.borrow_mut().push_offer(offer);
    }

    pub(crate) fn set_bus_info(&self, bus_info: fdf::BusInfo) {
        self.core.borrow_mut().set_bus_info(bus_info);
    }

    pub(crate) fn set_properties(&self, properties: Vec<NodePropertyEntry>) {
        self.core.borrow_mut().set_properties(properties);
    }

    pub(crate) fn clear_properties(&self) {
        self.core.borrow_mut().clear_properties();
    }

    pub(crate) fn push_property(&self, property: NodePropertyEntry) {
        self.core.borrow_mut().push_property(property);
    }

    pub(crate) fn dictionary(&self) -> NodeDictionary {
        self.core.borrow().dictionary().clone()
    }

    pub(crate) fn set_dictionary(&self, dictionary: NodeDictionary) {
        self.core.borrow_mut().set_dictionary(dictionary);
    }

    pub(crate) fn set_node_controller(&self, controller: NodeControllerServerBinding) {
        self.binding.borrow_mut().node_controller = Some(controller);
    }

    pub(crate) fn set_pending_bind_completer(
        &self,
        completer: oneshot::Sender<Result<(), zx::Status>>,
    ) {
        self.binding.borrow_mut().pending_bind_completer = Some(completer);
    }

    pub(crate) fn take_pending_bind_completer(
        &self,
    ) -> Option<oneshot::Sender<Result<(), zx::Status>>> {
        self.binding.borrow_mut().pending_bind_completer.take()
    }

    pub(crate) fn set_wait_for_driver_completer(
        &self,
        completer: oneshot::Sender<Result<fdf::DriverResult, zx::Status>>,
    ) {
        self.binding.borrow_mut().wait_for_driver_completer = Some(completer);
    }

    pub(crate) fn take_wait_for_driver_completer(
        &self,
    ) -> Option<oneshot::Sender<Result<fdf::DriverResult, zx::Status>>> {
        self.binding.borrow_mut().wait_for_driver_completer.take()
    }

    pub(crate) fn set_restart_driver_url_suffix(&self, suffix: String) {
        self.binding.borrow_mut().restart_driver_url_suffix = Some(suffix);
    }

    pub fn set_host(&self, host: Rc<dyn DriverHost>) {
        self.driver_host.borrow_mut().set_host(host);
    }

    pub(crate) fn host(&self) -> Option<Rc<dyn DriverHost>> {
        self.driver_host.borrow().host()
    }

    pub(crate) fn set_restart_on_crash(&self, value: bool) {
        self.driver_host.borrow_mut().set_restart_on_crash(value);
    }

    pub(crate) fn set_remove_complete_callback(&self, callback: oneshot::Sender<()>) {
        self.shutdown.borrow_mut().set_remove_complete_callback(callback);
    }

    pub(crate) fn push_unbinding_children_completer(
        &self,
        completer: oneshot::Sender<Result<(), zx::Status>>,
    ) {
        self.shutdown.borrow_mut().push_unbinding_children_completer(completer);
    }

    pub(crate) fn unbinding_children_completers_len(&self) -> usize {
        self.shutdown.borrow().unbinding_children_completers_len()
    }

    pub(crate) fn set_protocol_connector(&self, connector: fdevfs::ConnectorProxy) {
        self.devfs.borrow_mut().set_protocol_connector(connector);
    }

    pub(crate) fn set_controller_allowlist_passthrough(
        &self,
        passthrough: Rc<ControllerAllowlistPassthrough>,
    ) {
        self.devfs.borrow_mut().set_controller_allowlist_passthrough(passthrough);
    }

    pub(crate) fn set_device(&self, device: DevfsDevice) {
        self.devfs.borrow_mut().set_device(device);
    }

    pub(crate) fn device(&self) -> DevfsDevice {
        self.devfs.borrow().device().clone()
    }

    pub(crate) fn set_start_request_receiver(&self, receiver: StartRequestReceiver) {
        if let Some(ref mut component) = *self.component.borrow_mut() {
            component.set_start_request_receiver(receiver);
        }
    }

    pub(crate) fn take_start_request_receiver(&self) -> Option<StartRequestReceiver> {
        if let Some(ref mut component) = *self.component.borrow_mut() {
            component.take_start_request_receiver()
        } else {
            None
        }
    }

    pub(crate) fn set_state(&self, state: NodeState) {
        *self.state.borrow_mut() = state;
    }

    pub(crate) fn is_unbound(&self) -> bool {
        matches!(&*self.state.borrow(), NodeState::Unbound)
    }

    pub(crate) fn is_running(&self) -> bool {
        if let NodeState::DriverComponent(ref mut driver_component) = *self.state.borrow_mut() {
            if driver_component.state == DriverState::Stopped {
                warn!("completed bind but the driver is already stopped");
                false
            } else {
                driver_component.state = DriverState::Running;
                true
            }
        } else {
            false
        }
    }

    pub fn token_handle(&self) -> Option<zx::Event> {
        if self.core.borrow().is_composite() {
            self.core.borrow().children.iter().find_map(|child| {
                if let NodeState::DriverComponent(driver_component) = &*child.state.borrow()
                    && driver_component.state == DriverState::Running
                {
                    Some(driver_component.duplicate_instance_handle())
                } else {
                    None
                }
            })
        } else if let NodeState::DriverComponent(driver_component) = &*self.state.borrow() {
            if driver_component.state == DriverState::Running {
                Some(driver_component.duplicate_instance_handle())
            } else {
                None
            }
        } else {
            None
        }
    }

    pub(crate) fn has_wait_for_driver_completer(&self) -> bool {
        self.binding.borrow().has_wait_for_driver_completer()
    }

    pub(crate) fn bind_error(&self) -> Option<fdf::DriverResult> {
        self.binding.borrow().bind_error()
    }

    pub(crate) fn has_pending_bind_completer(&self) -> bool {
        self.binding.borrow().has_pending_bind_completer()
    }

    pub(crate) fn quarantine_start(&self) -> bool {
        let mut state = self.state.borrow_mut();
        match *state {
            NodeState::DriverComponent(ref mut driver_component) => {
                driver_component.close_node();
                driver_component.driver_client_binding.take();
            }
            NodeState::Starting { .. } => {}
            _ => {
                return false;
            }
        }
        let driver_url = match &*state {
            NodeState::Starting { driver_url } => driver_url.clone(),
            NodeState::DriverComponent(c) => c.driver_url.clone(),
            _ => unreachable!(),
        };
        *state = NodeState::Quarantined { driver_url };
        true
    }

    pub(crate) fn node_controller_ref(&self) -> Option<fdf::NodeControllerControlHandle> {
        self.binding.borrow().node_controller_ref()
    }

    pub(crate) fn take_start_handles_for_start(&self) -> Vec<fidl_fuchsia_process::HandleInfo> {
        let mut component = self.component.borrow_mut();
        let component = component.as_mut().expect("component");
        component
            .start_handles
            .take()
            .expect("handles")
            .iter()
            .map(|h| fidl_fuchsia_process::HandleInfo {
                handle: h
                    .handle
                    .duplicate_handle(zx::Rights::SAME_RIGHTS)
                    .expect("duplicate handle"),
                id: h.id,
            })
            .collect::<Vec<_>>()
    }

    pub(crate) fn shutdown_state(&self) -> ShutdownState {
        *self.node_shutdown_coordinator.borrow().node_state()
    }

    pub(crate) fn driver_host_name_for_colocation(&self) -> String {
        self.driver_host.borrow().name_for_colocation.clone()
    }

    pub(crate) fn get_node_property_dict(&self) -> fdf::NodePropertyDictionary2 {
        let core = self.core.borrow();
        core.properties
            .iter()
            .map(|entry| fdf::NodePropertyEntry2 {
                name: entry.name.clone(),
                properties: entry.properties.clone().into_iter().map(|p| p.into()).collect(),
            })
            .collect()
    }

    pub(crate) fn driver_package_type(&self) -> fdf::DriverPackageType {
        self.core.borrow().driver_package_type
    }

    pub fn has_component_controller_proxy(&self) -> bool {
        self.component.borrow().is_some()
    }

    pub(crate) fn protocol_connector(&self) -> Option<fdevfs::ConnectorProxy> {
        self.devfs.borrow().protocol_connector.clone()
    }

    pub(crate) fn controller_allowlist_passthrough(
        &self,
    ) -> Option<Rc<ControllerAllowlistPassthrough>> {
        self.devfs.borrow().controller_allowlist_passthrough.clone()
    }

    pub(crate) fn set_should_destroy_driver_component(&self, val: bool) {
        self.shutdown.borrow_mut().should_destroy_driver_component = val;
    }

    pub(crate) fn take_component(&self) -> Option<crate::node::NodeComponent> {
        self.component.borrow_mut().take()
    }

    pub(crate) fn set_component(&self, component: crate::node::NodeComponent) {
        *self.component.borrow_mut() = Some(component);
    }

    pub(crate) fn take_node_controller(&self) -> Option<NodeControllerServerBinding> {
        self.binding.borrow_mut().node_controller.take()
    }

    pub(crate) fn finish_shutdown_state(&self) {
        match *self.state.borrow_mut() {
            NodeState::DriverComponent(ref mut driver_component) => {
                driver_component.close_node();
                driver_component.driver_client_binding.take();
            }
            NodeState::OwnedByParent { ref mut node_server_binding } => {
                if let Some(binding) = node_server_binding.take() {
                    binding.close()
                }
            }
            _ => {}
        }

        self.devfs.borrow_mut().device.topological.take();
        self.devfs.borrow_mut().device.protocol.take();
    }

    pub(crate) fn reset_node_type(&self) {
        *self.state.borrow_mut() = NodeState::Unbound;

        let mut core = self.core.borrow_mut();
        match core.node_type {
            NodeTypeVariant::Normal { .. } => {
                core.node_type = NodeTypeVariant::Normal { parent: Weak::new() };
            }
            NodeTypeVariant::Composite { .. } => {
                core.node_type = NodeTypeVariant::Composite {
                    parents: vec![],
                    parents_names: vec![],
                    primary_index: 0,
                };
            }
        }
    }

    pub(crate) fn take_remove_complete_callback(&self) -> Option<oneshot::Sender<()>> {
        self.shutdown.borrow_mut().take_remove_complete_callback()
    }

    pub(crate) fn take_composite_rebind_completer(
        &self,
    ) -> Option<oneshot::Sender<Result<(), zx::Status>>> {
        self.binding.borrow_mut().composite_rebind_completer.take()
    }

    pub(crate) fn finish_restart_state(&self) {
        match *self.state.borrow_mut() {
            NodeState::DriverComponent(ref mut driver_component) => {
                driver_component.close_node();
                driver_component.driver_client_binding.take();
            }
            NodeState::OwnedByParent { ref mut node_server_binding } => {
                if let Some(binding) = node_server_binding.take() {
                    binding.close()
                }
            }
            _ => {}
        }
        *self.state.borrow_mut() = NodeState::Unbound;
    }

    pub(crate) fn host_restart_on_crash(&self) -> bool {
        self.driver_host.borrow().restart_on_crash
    }

    pub(crate) fn take_host(&self) -> Option<Rc<dyn DriverHost>> {
        self.driver_host.borrow_mut().host.take()
    }

    pub(crate) fn take_restart_driver_url_suffix(&self) -> Option<String> {
        self.binding.borrow_mut().restart_driver_url_suffix.take()
    }

    pub fn has_driver(&self) -> bool {
        match &*self.state.borrow() {
            NodeState::DriverComponent(component) => component.driver_client_binding.is_some(),
            _ => false,
        }
    }

    pub fn has_driver_component(&self) -> bool {
        match &*self.state.borrow() {
            NodeState::DriverComponent(component) => component.state != DriverState::Stopped,
            _ => false,
        }
    }

    pub(crate) fn stop_driver_component(&self) {
        if self.has_driver_component() {
            debug!(
                "Node '{}' sending stop through component runner",
                self.make_component_moniker()
            );
            self.send_on_stop();
        }
    }

    pub(crate) fn remove_child_from_children(&self, child: &Rc<Node>) -> bool {
        self.core.borrow_mut().remove_child_from_children(child)
    }

    pub(crate) fn driver_client_binding(&self) -> Option<fdh::DriverProxy> {
        match &*self.state.borrow() {
            NodeState::DriverComponent(component) => {
                component.driver_client_binding.as_ref().map(|b| b.driver_host_proxy.clone())
            }
            _ => None,
        }
    }

    pub(crate) fn send_on_stop(&self) {
        if let NodeState::DriverComponent(ref driver_component) = *self.state.borrow() {
            driver_component.send_on_stop();
        }
    }

    pub(crate) fn should_destroy_driver_component(&self) -> bool {
        self.shutdown.borrow().should_destroy_driver_component
    }

    pub(crate) fn has_remove_complete_callback(&self) -> bool {
        self.shutdown.borrow().has_remove_complete_callback()
    }

    pub(crate) fn component_controller_proxy(
        &self,
    ) -> Option<fidl_fuchsia_component::ControllerProxy> {
        self.component.borrow().as_ref().map(|c| c.controller.component_controller_proxy.clone())
    }

    pub(crate) fn set_driver_stopped(&self) {
        let mut state = self.state.borrow_mut();
        if let NodeState::DriverComponent(ref mut driver_component) = *state {
            driver_component.state = DriverState::Stopped;
        }
    }

    pub(crate) fn set_composite_rebind_completer(
        &self,
        completer: oneshot::Sender<Result<(), zx::Status>>,
    ) -> Result<(), oneshot::Sender<Result<(), zx::Status>>> {
        let mut binding = self.binding.borrow_mut();
        if binding.composite_rebind_completer.is_some() {
            return Err(completer);
        }
        binding.composite_rebind_completer = Some(completer);
        Ok(())
    }
}
