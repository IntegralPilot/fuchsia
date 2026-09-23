// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::broker::{Lease, LeaseID};
use crate::topology::{Dependency, Element, OnRequiredElementRemoval, Topology};
use crate::{ElementID, IndexedPowerLevel};
use either::Either;
use fidl_fuchsia_power_broker as fpb;
use fuchsia_inspect as inspect;
use fuchsia_inspect::{ArrayProperty, InspectTypeReparentable, Property};
use fuchsia_inspect_contrib::graph as igraph;
use fuchsia_inspect_contrib::graph::{Digraph, DigraphOpts};
use fuchsia_inspect_contrib::nodes::BoundedListNode;
use fuchsia_sync::Mutex;
use futures::FutureExt;
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use std::sync::Arc;

const ADD_ELEMENT_EVENT: &str = "add_element";
const ADD_DEPENDENCY_EVENT: &str = "add_dep";
const REMOVE_DEP_EVENT: &str = "rm_dep";
const UPDATE_LEVEL_EVENT: &str = "update_level";
const REMOVE_ELEMENT_EVENT: &str = "rm_element";
const CREATE_LEASE_EVENT: &str = "create_lease";
const REMOVE_LEASE_EVENT: &str = "rm_lease";
const UPDATE_LEASE_STATUS_EVENT: &str = "update_lease";
const UNSET: &str = "unset";

const ELEMENT_ID: &str = "element_id";
const ELEMENT_NAME: &str = "name";
const LEVEL: &str = "level";
const LEASE_ID: &str = "lease_id";
const STATUS: &str = "status";
const TIME: &str = "@time";
const EDGE_ID: &str = "edge_id";
const DEP_ELEMENT: &str = "dependent_element";
const REQ_ELEMENT: &str = "required_element";
const DEPENDENT_LEVEL: &str = "dependent_level";
const CURRENT_LEVEL: &str = "current_level";
const REQUIRED_LEVEL: &str = "required_level";
const VALID_LEVELS: &str = "valid_levels";
const REMOVE_WITH_REQUIRED_ELEMENT: &str = "remove_with_required_element";

#[derive(Debug)]
pub struct TopologyInspect {
    graph: Digraph,
    events: Option<RefCell<BoundedListNode>>,
    _node: inspect::Node,
    shadow: Arc<Mutex<ShadowBuffer>>,
}

#[derive(Debug)]
pub struct ElementData {
    current_level: Option<Either<inspect::UintProperty, inspect::StringProperty>>,
    required_level: Option<Either<inspect::UintProperty, inspect::StringProperty>>,
    // { LeaseId  => { Level => LeaseStatus } }
    leases: HashMap<LeaseID, LeaseData>,
    name: inspect::StringProperty,
    valid_levels: inspect::UintArrayProperty,
    _node: inspect::Node,
    leases_node: inspect::Node,
}

#[derive(Debug)]
struct LeaseData {
    node: inspect::Node,
    level: inspect::UintProperty,
    status: Option<inspect::UintProperty>,
}

impl igraph::VertexMetadata for ElementData {
    type Id = ElementID;
    type EdgeMeta = DependencyData;
}

impl ElementData {
    const CURRENT_LEVEL: &str = "current_level";
    const REQUIRED_LEVEL: &str = "required_level";
    const LEASES: &str = "leases";
    const SYNTHETIC: &str = "synthetic";

    fn new(
        node: inspect::Node,
        name: &str,
        valid_levels: &[IndexedPowerLevel],
        current_level: Option<fpb::PowerLevel>,
        required_level: Option<fpb::PowerLevel>,
    ) -> Self {
        let name = node.create_string(ELEMENT_NAME, name);
        let valid_levels = Self::create_valid_levels(&node, valid_levels);
        let leases_node = node.create_child(Self::LEASES);
        Self {
            current_level: current_level
                .map(|level| Either::Left(node.create_uint(Self::CURRENT_LEVEL, level as u64)))
                .or_else(|| Some(Either::Right(node.create_string(Self::CURRENT_LEVEL, UNSET)))),
            required_level: required_level
                .map(|level| Either::Left(node.create_uint(Self::REQUIRED_LEVEL, level as u64)))
                .or_else(|| Some(Either::Right(node.create_string(Self::REQUIRED_LEVEL, UNSET)))),
            leases: Default::default(),
            leases_node,
            name,
            valid_levels,
            _node: node,
        }
    }

    fn synthetic(node: inspect::Node, name: &str, valid_levels: &[IndexedPowerLevel]) -> Self {
        let name = node.create_string(ELEMENT_NAME, name);
        node.record_bool(Self::SYNTHETIC, true);
        let valid_levels = Self::create_valid_levels(&node, valid_levels);
        let leases_node = node.create_child(Self::LEASES);
        Self {
            current_level: None,
            required_level: None,
            _node: node,
            name,
            valid_levels,
            leases: Default::default(),
            leases_node,
        }
    }

    fn update_level(&self, update_level: &UpdateLevelInfo) {
        if let Some(level) = update_level.current {
            self.inner_update_level(&self.current_level, level);
        }
        if let Some(level) = update_level.required {
            self.inner_update_level(&self.required_level, level)
        }
    }

    fn inner_update_level(
        &self,
        property: &Option<Either<inspect::UintProperty, inspect::StringProperty>>,
        level: fpb::PowerLevel,
    ) {
        match property {
            None => {}
            Some(Either::Left(property)) => property.set(level as u64),
            Some(Either::Right(_)) => {
                // Ignore update level requests for the unsatifiable element.
            }
        }
    }

    fn create_lease(&mut self, lease_id: LeaseID, level: fpb::PowerLevel) {
        match self.leases.get_mut(&lease_id) {
            Some(_) => unreachable!("We can't call into create lease twice"),
            None => {
                let lease_node = self.leases_node.create_child(format!("{lease_id}"));
                let level = lease_node.create_uint(LEVEL, level as u64);
                self.leases.insert(
                    lease_id.to_owned(),
                    LeaseData { node: lease_node, level, status: None },
                );
            }
        }
    }

    fn set_lease_status(&mut self, lease_id: LeaseID, status: fpb::LeaseStatus) {
        let status = status.into_primitive() as u64;
        match self.leases.get_mut(&lease_id) {
            None => unreachable!("we must have a lease created if we got here"),
            Some(LeaseData { status: Some(status_property), .. }) => {
                status_property.set(status);
            }
            Some(lease) => {
                lease.status = Some(lease.node.create_uint(STATUS, status));
            }
        }
    }

    fn remove_lease(&mut self, lease_id: LeaseID) -> Option<LeaseData> {
        self.leases.remove(&lease_id)
    }

    fn create_valid_levels(
        node: &inspect::Node,
        valid_levels: &[IndexedPowerLevel],
    ) -> inspect::UintArrayProperty {
        let prop = node.create_uint_array(VALID_LEVELS, valid_levels.len());
        for (idx, v) in valid_levels.iter().enumerate() {
            prop.set(idx, v.level);
        }
        prop
    }
}

#[derive(Debug)]
pub struct DependencyData {
    // Map from dp_level => (rq_level, opportunistic)
    levels: HashMap<fpb::PowerLevel, inspect::Node>,
    node: inspect::Node,
}

impl DependencyData {
    fn new(
        meta_node: inspect::Node,
        dp_level: fpb::PowerLevel,
        rq_level: fpb::PowerLevel,
        on_required_element_removal: OnRequiredElementRemoval,
    ) -> Self {
        let mut levels = HashMap::new();
        levels.insert(
            dp_level,
            Self::dep_node(&meta_node, dp_level, rq_level, on_required_element_removal),
        );
        Self { levels, node: meta_node }
    }

    fn remove(&mut self, dp_level: fpb::PowerLevel) {
        self.levels.remove(&dp_level);
    }

    fn add_new_dep(
        &mut self,
        dp_level: fpb::PowerLevel,
        rq_level: fpb::PowerLevel,
        on_required_element_removal: OnRequiredElementRemoval,
    ) {
        match self.levels.get_mut(&dp_level) {
            Some(_data) => unreachable!("we never update a dep level"),
            None => {
                self.levels.insert(
                    dp_level,
                    Self::dep_node(&self.node, dp_level, rq_level, on_required_element_removal),
                );
            }
        }
    }

    fn dep_node(
        parent: &inspect::Node,
        dp_level: fpb::PowerLevel,
        rq_level: fpb::PowerLevel,
        on_required_element_removal: OnRequiredElementRemoval,
    ) -> inspect::Node {
        let node = parent.create_child(format!("{dp_level}"));
        node.record_uint(REQUIRED_LEVEL, rq_level as u64);
        if on_required_element_removal == OnRequiredElementRemoval::RemoveWithRequiredElement {
            node.record_bool(REMOVE_WITH_REQUIRED_ELEMENT, true);
        }
        node
    }
}

impl igraph::EdgeMetadata for DependencyData {}

impl TopologyInspect {
    pub fn new(node: inspect::Node, max_events: usize) -> Self {
        let graph = igraph::Digraph::new(&node, DigraphOpts::default());
        let shadow = Arc::new(Mutex::new(ShadowBuffer::new(max_events)));
        let mut events = None;
        if max_events != 0 {
            events =
                Some(RefCell::new(BoundedListNode::new(node.create_child("events"), max_events)));
            let shadow_weak = Arc::downgrade(&shadow);
            node.record_lazy_child("stats", move || {
                let shadow_weak = shadow_weak.clone();
                async move {
                    let inspector = inspect::Inspector::default();
                    if let Some(shadow) = shadow_weak.upgrade() {
                        let shadow_ref = shadow.lock();
                        let root = inspector.root();
                        root.record_uint("event_capacity", max_events as u64);
                        let duration = shadow_ref.history_duration().into_seconds();
                        root.record_int("history_duration_seconds", duration);
                        if shadow_ref.at_capacity() {
                            root.record_int("at_capacity_history_duration_seconds", duration);
                        }
                    }
                    Ok(inspector)
                }
                .boxed()
            });
        }
        Self { graph, events, shadow, _node: node }
    }

    fn create_element_vertex(
        &self,
        element_id: ElementID,
        name: &str,
        synthetic: bool,
        valid_levels: &[IndexedPowerLevel],
        current_level: Option<fpb::PowerLevel>,
        required_level: Option<fpb::PowerLevel>,
    ) -> igraph::Vertex<ElementData> {
        if synthetic {
            self.graph.add_vertex(element_id, |meta_node| {
                ElementData::synthetic(meta_node, name, &valid_levels)
            })
        } else {
            self.graph.add_vertex(element_id.clone(), |meta_node| {
                ElementData::new(meta_node, name, &valid_levels, current_level, required_level)
            })
        }
    }

    fn emit_add_element_event(
        &self,
        element_id: ElementID,
        current_level: Option<fpb::PowerLevel>,
        required_level: Option<fpb::PowerLevel>,
        dependencies: &[(Dependency, OnRequiredElementRemoval)],
    ) {
        let Some(ref events) = self.events else {
            return;
        };
        let instant = zx::BootInstant::get();
        events.borrow_mut().add_entry(|node| {
            node.record_int("@time", instant.into_nanos());
            node.record_child(ADD_ELEMENT_EVENT, |node| {
                node.record_uint(ELEMENT_ID, *element_id as u64);
                if let Some(level) = current_level {
                    node.record_uint(CURRENT_LEVEL, level as u64);
                } else {
                    node.record_string(CURRENT_LEVEL, UNSET);
                }
                if let Some(level) = required_level {
                    node.record_uint(REQUIRED_LEVEL, level as u64);
                } else {
                    node.record_string(REQUIRED_LEVEL, UNSET);
                }
                if !dependencies.is_empty() {
                    let deps_node = node.create_child("dependencies");
                    for (i, (dep, on_required_element_removal)) in dependencies.iter().enumerate() {
                        let dep_node = deps_node.create_child(format!("{i}"));
                        // No need to record the dependent_element event since that must be the
                        // element we are adding.
                        // dep_node.record_uint(DEP_ELEMENT, *dep.dependent.element_id);
                        dep_node.record_uint(REQ_ELEMENT, *dep.requires.element_id as u64);
                        dep_node.record_uint(DEPENDENT_LEVEL, dep.dependent.level.level as u64);
                        dep_node.record_uint(REQUIRED_LEVEL, dep.requires.level.level as u64);
                        if *on_required_element_removal
                            == OnRequiredElementRemoval::RemoveWithRequiredElement
                        {
                            dep_node.record_bool(REMOVE_WITH_REQUIRED_ELEMENT, true);
                        }
                        deps_node.record(dep_node);
                    }
                    node.record(deps_node);
                }
            });
        });
    }

    fn on_add_dependency(
        &self,
        elements: &HashMap<ElementID, Element>,
        dep: &Dependency,
        on_required_element_removal: OnRequiredElementRemoval,
        write_inspect_event: bool,
    ) {
        let (dp_id, rq_id) = (dep.dependent.element_id, dep.requires.element_id);
        let (Some(dp), Some(rq)) = (elements.get(&dp_id), elements.get(&rq_id)) else {
            // elements[dp_id] and elements[rq_id] guaranteed by prior validation
            log::error!(dep:?; "Failed to add inspect for dependency.");
            return;
        };
        let (dp_level, rq_level) = (dep.dependent.level, dep.requires.level);
        let mut inspect_edges = dp.inspect_edges.borrow_mut();
        match inspect_edges.get_mut(&rq_id) {
            None => {
                let mut dp_vertex = dp.inspect_vertex.as_ref().unwrap().borrow_mut();
                let mut rq_vertex = rq.inspect_vertex.as_ref().unwrap().borrow_mut();
                let edge = dp_vertex.add_edge(&mut rq_vertex, |meta_node| {
                    DependencyData::new(
                        meta_node,
                        dp_level.level,
                        rq_level.level,
                        on_required_element_removal,
                    )
                });
                inspect_edges.insert(rq_id, edge);
            }
            Some(edge) => {
                edge.maybe_update_meta(|meta| {
                    meta.add_new_dep(dp_level.level, rq_level.level, on_required_element_removal);
                });
            }
        }
        if write_inspect_event {
            self.maybe_record_event(dp, ADD_DEPENDENCY_EVENT, |node| {
                node.record_uint(DEP_ELEMENT, *dp_id as u64);
                node.record_uint(REQ_ELEMENT, *rq_id as u64);
                node.record_uint(DEPENDENT_LEVEL, dp_level.level as u64);
                node.record_uint(REQUIRED_LEVEL, rq_level.level as u64);
                if on_required_element_removal
                    == OnRequiredElementRemoval::RemoveWithRequiredElement
                {
                    node.record_bool(REMOVE_WITH_REQUIRED_ELEMENT, true);
                }
            });
        }
    }

    pub fn on_remove_dependency(&self, elements: &HashMap<ElementID, Element>, dep: &Dependency) {
        // elements[dp_id] and elements[rq_id] guaranteed by prior validation
        let (dp_id, rq_id) = (&dep.dependent.element_id, &dep.requires.element_id);
        let Some(dp) = elements.get(dp_id) else {
            log::error!(dp_id:?; "Missing element for removal");
            return;
        };
        let mut dp_edges = dp.inspect_edges.borrow_mut();
        let Some(edge) = dp_edges.get_mut(rq_id) else {
            return;
        };
        let dp_level = dep.dependent.level;
        edge.maybe_update_meta(|meta| {
            meta.remove(dp_level.level);
        });
        self.maybe_record_event(dp, REMOVE_DEP_EVENT, |node| {
            node.record_uint(EDGE_ID, edge.id());
            node.record_uint(DEPENDENT_LEVEL, dp_level.level as u64);
        });
    }

    fn on_update_level(&self, element: &Element, info: UpdateLevelInfo) {
        let Some(ref inspect_vertex) = element.inspect_vertex else {
            return;
        };
        let mut vertex = inspect_vertex.borrow_mut();
        vertex.meta().update_level(&info);
        self.maybe_record_event(element, UPDATE_LEVEL_EVENT, |node| {
            node.record_uint(ELEMENT_ID, *element.id as u64);
            if let Some(level) = info.current {
                node.record_uint(CURRENT_LEVEL, level.into());
            }
            if let Some(level) = info.required {
                node.record_uint(REQUIRED_LEVEL, level.into());
            }
        });
    }

    pub fn on_remove_element(
        &self,
        element: Element,
        current_level: Option<fpb::PowerLevel>,
        required_level: Option<fpb::PowerLevel>,
    ) {
        let mut vertex = element.inspect_vertex.as_ref().unwrap().borrow_mut();

        // Ensure we don't drop the properties when dropping the vertex since we'll be reparenting
        // them and leaving them in the event. So we swap them for a no-op property.
        let mut name_property = inspect::StringProperty::default();
        std::mem::swap(&mut vertex.meta().name, &mut name_property);
        let mut valid_levels_prop = inspect::UintArrayProperty::default();
        std::mem::swap(&mut vertex.meta().valid_levels, &mut valid_levels_prop);

        self.maybe_record_event(&element, REMOVE_ELEMENT_EVENT, |node| {
            node.record_uint(ELEMENT_ID, *element.id as u64);
            let _ = name_property.reparent(&node);
            node.record(name_property);
            let _ = valid_levels_prop.reparent(&node);
            node.record(valid_levels_prop);
            if let Some(level) = current_level {
                node.record_uint(CURRENT_LEVEL, level.into());
            }
            if let Some(level) = required_level {
                node.record_uint(REQUIRED_LEVEL, level.into());
            }
        });
    }

    pub fn on_create_lease_and_claims(
        &self,
        element: &Element,
        lease_id: LeaseID,
        level: fpb::PowerLevel,
    ) {
        let Some(ref vertex) = element.inspect_vertex else {
            return;
        };
        vertex.borrow_mut().meta().create_lease(lease_id, level);
        self.maybe_record_event(element, CREATE_LEASE_EVENT, |node| {
            node.record_uint(ELEMENT_ID, *element.id as u64);
            node.record_uint(LEASE_ID, *lease_id);
            node.record_uint(LEVEL, level as u64);
        });
    }

    pub fn on_update_lease_status(
        &self,
        element: &Element,
        lease: &Lease,
        status: &fpb::LeaseStatus,
    ) {
        let Some(ref inspect_vertex) = element.inspect_vertex else {
            return;
        };
        inspect_vertex.borrow_mut().meta().set_lease_status(lease.id, *status);
        self.maybe_record_event(element, UPDATE_LEASE_STATUS_EVENT, |node| {
            node.record_uint(ELEMENT_ID, *element.id as u64);
            node.record_uint(LEASE_ID, *lease.id);
            node.record_uint(STATUS, status.into_primitive() as u64);
        });
    }

    pub fn on_drop_lease(&self, element: &Element, lease: &Lease) {
        let Some(ref inspect_vertex) = element.inspect_vertex else {
            return;
        };
        let Some(lease_data) = inspect_vertex.borrow_mut().meta().remove_lease(lease.id) else {
            return;
        };
        self.maybe_record_event(element, REMOVE_LEASE_EVENT, |node| {
            node.record_uint(ELEMENT_ID, *element.id as u64);
            node.record_uint(LEASE_ID, *lease.id);

            let _ = lease_data.level.reparent(&node);
            node.record(lease_data.level);

            if let Some(status) = lease_data.status {
                let _ = status.reparent(&node);
                node.record(status);
            }
        });
    }

    fn maybe_record_event(
        &self,
        _element: &Element,
        event_name: &str,
        callback: impl FnOnce(&inspect::Node),
    ) {
        if let Some(events) = &self.events {
            let instant = zx::BootInstant::get();
            events.borrow_mut().add_entry(|node| {
                node.record_int(TIME, instant.into_nanos());
                node.record_child(event_name, callback);
            });
            self.shadow.lock().add_entry(ShadowEvent { time: instant })
        }
    }
}

pub trait InspectUpdateLevel {
    fn update_current_level(
        &mut self,
        topology: &Topology,
        element_id: ElementID,
        level: fpb::PowerLevel,
    ) -> &mut Self;

    fn update_required_level(
        &mut self,
        topology: &Topology,
        element_id: ElementID,
        level: fpb::PowerLevel,
    ) -> &mut Self;
}

pub trait InspectAddDependency {
    fn add_dependency(
        &mut self,
        _topology: &Topology,
        dependency: &Dependency,
        on_required_element_removal: OnRequiredElementRemoval,
    );
}

#[derive(Default)]
pub struct UpdateLevelInspectWriter {
    events: HashMap<ElementID, UpdateLevelInfo>,
}

struct UpdateLevelInfo {
    timestamp: zx::BootInstant,
    required: Option<fpb::PowerLevel>,
    current: Option<fpb::PowerLevel>,
}

impl InspectUpdateLevel for UpdateLevelInspectWriter {
    fn update_current_level(
        &mut self,
        _topology: &Topology,
        element_id: ElementID,
        level: fpb::PowerLevel,
    ) -> &mut Self {
        let entry = self.events.entry(element_id).or_insert_with(|| UpdateLevelInfo {
            timestamp: zx::BootInstant::get(),
            required: None,
            current: None,
        });
        entry.current = Some(level);
        self
    }

    fn update_required_level(
        &mut self,
        _topology: &Topology,
        element_id: ElementID,
        level: fpb::PowerLevel,
    ) -> &mut Self {
        let entry = self.events.entry(element_id).or_insert_with(|| UpdateLevelInfo {
            timestamp: zx::BootInstant::get(),
            required: None,
            current: None,
        });
        entry.required = Some(level);
        self
    }
}

impl UpdateLevelInspectWriter {
    pub fn commit(self, topology: &Topology) {
        let mut events = self.events.into_iter().collect::<Vec<_>>();
        events.sort_by(|(_, a), (_, b)| a.timestamp.cmp(&b.timestamp));
        for (element_id, data) in events {
            if let Some(element) = topology.get_element(&element_id) {
                topology.inspect().on_update_level(element, data);
            }
        }
    }
}

pub struct EagerInspectWriter;

impl InspectUpdateLevel for EagerInspectWriter {
    fn update_current_level(
        &mut self,
        topology: &Topology,
        element_id: ElementID,
        level: fpb::PowerLevel,
    ) -> &mut Self {
        let mut writer = UpdateLevelInspectWriter::default();
        writer.update_current_level(topology, element_id, level);
        writer.commit(topology);
        self
    }

    fn update_required_level(
        &mut self,
        topology: &Topology,
        element_id: ElementID,
        level: fpb::PowerLevel,
    ) -> &mut Self {
        let mut writer = UpdateLevelInspectWriter::default();
        writer.update_required_level(topology, element_id, level);
        writer.commit(topology);
        self
    }
}

impl InspectAddDependency for EagerInspectWriter {
    fn add_dependency(
        &mut self,
        topology: &Topology,
        dependency: &Dependency,
        on_required_element_removal: OnRequiredElementRemoval,
    ) {
        topology.inspect().on_add_dependency(
            &topology.elements,
            dependency,
            on_required_element_removal,
            true,
        );
    }
}

pub struct AddElementInspectWriter {
    element_id: ElementID,
    current_level: Option<fpb::PowerLevel>,
    required_level: Option<fpb::PowerLevel>,
    dependencies: Vec<(Dependency, OnRequiredElementRemoval)>,
    other_events: UpdateLevelInspectWriter,
}

impl AddElementInspectWriter {
    pub fn new(element_id: ElementID) -> Self {
        Self {
            element_id,
            current_level: None,
            required_level: None,
            other_events: UpdateLevelInspectWriter::default(),
            dependencies: Vec::new(),
        }
    }

    pub fn commit(self, topology: &mut Topology) {
        // First create the vertex since we'll need it when adding dependencies.
        let inspect_vertex = {
            // unwrap: safe, if we are here it means we are just adding an element so we can't have
            // removed it. Otherwise, it's a bug in the implementation.
            let element = topology.get_element(&self.element_id).unwrap();
            topology.inspect().create_element_vertex(
                element.id,
                &element.name,
                element.synthetic,
                &element.valid_levels,
                self.current_level,
                self.required_level,
            )
        };
        topology.get_element_mut(&self.element_id).unwrap().inspect_vertex =
            Some(Rc::new(RefCell::new(inspect_vertex)));

        for (dependency, on_required_element_removal) in &self.dependencies {
            topology.inspect().on_add_dependency(
                &topology.elements,
                dependency,
                *on_required_element_removal,
                false,
            );
        }

        topology.inspect().emit_add_element_event(
            self.element_id,
            self.current_level,
            self.required_level,
            &self.dependencies,
        );

        self.other_events.commit(topology);
    }
}

impl InspectUpdateLevel for AddElementInspectWriter {
    fn update_current_level(
        &mut self,
        topology: &Topology,
        element_id: ElementID,
        level: fpb::PowerLevel,
    ) -> &mut Self {
        if element_id == self.element_id {
            self.current_level = Some(level);
        } else {
            self.other_events.update_current_level(topology, element_id, level);
        }
        self
    }

    fn update_required_level(
        &mut self,
        topology: &Topology,
        element_id: ElementID,
        level: fpb::PowerLevel,
    ) -> &mut Self {
        if element_id == self.element_id {
            self.required_level = Some(level);
        } else {
            self.other_events.update_required_level(topology, element_id, level);
        }
        self
    }
}

impl InspectAddDependency for AddElementInspectWriter {
    fn add_dependency(
        &mut self,
        _topology: &Topology,
        dependency: &Dependency,
        on_required_element_removal: OnRequiredElementRemoval,
    ) {
        self.dependencies.push((dependency.clone(), on_required_element_removal))
    }
}

#[derive(Debug)]
struct ShadowEvent {
    time: zx::BootInstant,
}

#[derive(Debug)]
struct ShadowBuffer {
    buffer: VecDeque<ShadowEvent>,
    capacity: usize,
}

impl ShadowBuffer {
    fn new(capacity: usize) -> Self {
        Self { buffer: VecDeque::with_capacity(capacity), capacity }
    }

    fn add_entry(&mut self, shadow_event: ShadowEvent) {
        if self.buffer.len() >= self.capacity {
            self.buffer.pop_front();
        }
        self.buffer.push_back(shadow_event);
    }

    fn history_duration(&self) -> zx::BootDuration {
        if self.buffer.len() < 2 {
            return zx::BootDuration::ZERO;
        }
        self.buffer.back().unwrap().time - self.buffer.front().unwrap().time
    }

    fn at_capacity(&self) -> bool {
        self.buffer.len() == self.capacity
    }
}
