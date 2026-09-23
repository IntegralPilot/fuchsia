// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::parent_set_collector::ParentSetCollector;
use driver_manager_node::{Node, NodeManager, NodeProperty, NodePropertyValue};
use fidl_fuchsia_driver_development as fdd;
use fidl_fuchsia_driver_framework as fdf;
use flyweights::FlyStr;
use futures::channel::oneshot;
use std::rc::{Rc, Weak};

#[derive(Copy, Clone)]
pub enum Condition {
    Unknown = 0,
    Accept = 1,
    Reject = 2,
}

impl std::convert::From<fdf::Condition> for Condition {
    fn from(source: fdf::Condition) -> Self {
        match source {
            fdf::Condition::Unknown => Self::Unknown,
            fdf::Condition::Accept => Self::Accept,
            fdf::Condition::Reject => Self::Reject,
        }
    }
}

impl std::convert::From<Condition> for fdf::Condition {
    fn from(source: Condition) -> fdf::Condition {
        match source {
            Condition::Unknown => fdf::Condition::Unknown,
            Condition::Accept => fdf::Condition::Accept,
            Condition::Reject => fdf::Condition::Reject,
        }
    }
}

#[derive(Clone)]
pub struct BindRule {
    pub key: FlyStr,
    pub condition: Condition,
    pub values: Vec<NodePropertyValue>,
}

impl std::convert::From<fdf::BindRule2> for BindRule {
    fn from(source: fdf::BindRule2) -> Self {
        Self {
            key: FlyStr::new(source.key),
            condition: source.condition.into(),
            values: source.values.into_iter().map(|v| v.into()).collect(),
        }
    }
}

impl std::convert::From<BindRule> for fdf::BindRule2 {
    fn from(source: BindRule) -> fdf::BindRule2 {
        fdf::BindRule2 {
            key: source.key.to_string(),
            condition: source.condition.into(),
            values: source.values.into_iter().map(|v| v.into()).collect(),
        }
    }
}

#[derive(Clone)]
pub struct ParentSpec {
    pub bind_rules: Vec<BindRule>,
    pub properties: Vec<NodeProperty>,
}

impl std::convert::From<fdf::ParentSpec2> for ParentSpec {
    fn from(source: fdf::ParentSpec2) -> Self {
        Self {
            bind_rules: source.bind_rules.into_iter().map(|b| b.into()).collect(),
            properties: source.properties.into_iter().map(|p| p.into()).collect(),
        }
    }
}

impl std::convert::From<ParentSpec> for fdf::ParentSpec2 {
    fn from(source: ParentSpec) -> fdf::ParentSpec2 {
        fdf::ParentSpec2 {
            bind_rules: source.bind_rules.into_iter().map(|b| b.into()).collect(),
            properties: source.properties.into_iter().map(|p| p.into()).collect(),
        }
    }
}

#[derive(Clone)]
pub struct NodeSpec {
    pub name: String,
    pub parents: Vec<ParentSpec>,
    pub driver_host: Option<String>,
}

impl std::convert::From<fdf::CompositeNodeSpec> for NodeSpec {
    fn from(source: fdf::CompositeNodeSpec) -> Self {
        Self {
            name: source.name.unwrap(),
            parents: source.parents2.unwrap().into_iter().map(|p| p.into()).collect(),
            driver_host: source.driver_host,
        }
    }
}

#[derive(Clone)]
pub struct DriverInfo {
    pub url: String,
    pub name: Option<String>,
    pub colocate: bool,
    pub package_type: fdf::DriverPackageType,
    pub is_fallback: bool,
    pub device_categories: Vec<fdf::DeviceCategory>,
    pub bind_rules_bytecode: Vec<u8>,
    pub driver_framework_version: u8,
    pub is_disabled: bool,
}

impl std::convert::From<fdf::DriverInfo> for DriverInfo {
    fn from(source: fdf::DriverInfo) -> Self {
        Self {
            url: source.url.unwrap(),
            name: source.name,
            colocate: source.colocate.unwrap(),
            package_type: source.package_type.unwrap(),
            is_fallback: source.is_fallback.unwrap(),
            device_categories: source.device_categories.unwrap(),
            bind_rules_bytecode: source.bind_rules_bytecode.unwrap_or_default(),
            driver_framework_version: source.driver_framework_version.unwrap(),
            is_disabled: source.is_disabled.unwrap(),
        }
    }
}

impl std::convert::From<DriverInfo> for fdf::DriverInfo {
    fn from(source: DriverInfo) -> fdf::DriverInfo {
        fdf::DriverInfo {
            url: Some(source.url),
            name: source.name,
            colocate: Some(source.colocate),
            package_type: Some(source.package_type),
            is_fallback: Some(source.is_fallback),
            device_categories: Some(source.device_categories),
            bind_rules_bytecode: Some(source.bind_rules_bytecode),
            driver_framework_version: Some(source.driver_framework_version),
            is_disabled: Some(source.is_disabled),
            ..Default::default()
        }
    }
}

#[derive(Clone)]
pub struct CompositeDriverInfo {
    pub composite_name: String,
    pub driver_info: DriverInfo,
}

impl std::convert::From<fdf::CompositeDriverInfo> for CompositeDriverInfo {
    fn from(source: fdf::CompositeDriverInfo) -> Self {
        Self {
            composite_name: source.composite_name.unwrap(),
            driver_info: source.driver_info.unwrap().into(),
        }
    }
}

#[derive(Clone)]
pub struct CompositeDriverMatch {
    pub composite_driver: CompositeDriverInfo,
    pub parent_names: Vec<String>,
    pub primary_parent_index: u32,
}

impl std::convert::From<fdf::CompositeDriverMatch> for CompositeDriverMatch {
    fn from(source: fdf::CompositeDriverMatch) -> Self {
        Self {
            composite_driver: source.composite_driver.unwrap().into(),
            parent_names: source.parent_names.unwrap(),
            primary_parent_index: source.primary_parent_index.unwrap_or(0),
        }
    }
}

#[derive(Clone)]
pub struct CompositeInfo {
    pub spec: NodeSpec,
    pub matched_driver: CompositeDriverMatch,
}

impl std::convert::From<fdf::CompositeInfo> for CompositeInfo {
    fn from(source: fdf::CompositeInfo) -> Self {
        Self {
            spec: source.spec.unwrap().into(),
            matched_driver: source.matched_driver.unwrap().into(),
        }
    }
}

pub struct CompositeNodeSpec {
    #[allow(unused)]
    name: String,
    parent_specs: Vec<ParentSpec>,
    parent_nodes: Vec<Option<Weak<Node>>>,
    parent_set_collector: Option<ParentSetCollector>,
    driver_url: String,
    node_manager: Box<dyn NodeManager>,
    composite_info: Option<CompositeInfo>,
    driver_host_name_for_colocation: String,
}

impl CompositeNodeSpec {
    pub fn new(
        name: String,
        parent_specs: Vec<fdf::ParentSpec2>,
        node_manager: Box<dyn NodeManager>,
        driver_host_name_for_colocation: String,
    ) -> Self {
        let parent_nodes = vec![None; parent_specs.len()];
        Self {
            name,
            parent_specs: parent_specs.into_iter().map(|p| p.into()).collect(),
            parent_nodes,
            parent_set_collector: None,
            driver_url: String::new(),
            node_manager,
            composite_info: None,
            driver_host_name_for_colocation,
        }
    }

    fn bind_parent_impl(
        &mut self,
        composite_parent: fdf::CompositeParent,
        node_ptr: Weak<Node>,
    ) -> Result<Option<Weak<Node>>, zx::Status> {
        if self.composite_info.is_none() {
            self.composite_info = composite_parent.composite.map(|c| c.into());
        }

        let composite_info = self.composite_info.as_ref().unwrap();
        let spec = &composite_info.spec;
        let matched_driver = &composite_info.matched_driver;
        let spec_name = &spec.name;
        let composite_driver = &matched_driver.composite_driver;
        let driver_info = &composite_driver.driver_info;
        let parent_names = &matched_driver.parent_names;
        let primary_index = matched_driver.primary_parent_index;
        let url = &driver_info.url;

        if self.parent_set_collector.is_none() {
            self.parent_set_collector = Some(ParentSetCollector::new(
                spec_name.clone(),
                parent_names.clone(),
                primary_index,
                self.driver_host_name_for_colocation.clone(),
            ));
            self.driver_url = url.clone();
        }

        let collector = self.parent_set_collector.as_mut().unwrap();
        let index = composite_parent.index.unwrap();
        let properties = self.parent_specs[index as usize].properties.clone();
        collector.add_node(index, properties, node_ptr)?;

        match collector.try_to_assemble(self.node_manager.clone_box()) {
            Ok(node) => Ok(Some(Rc::downgrade(&node))),
            Err(zx::Status::SHOULD_WAIT) => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub fn bind_parent(
        &mut self,
        composite_parent: fdf::CompositeParent,
        node_ptr: Weak<Node>,
    ) -> Result<Option<Weak<Node>>, zx::Status> {
        let node_index = composite_parent.index.unwrap() as usize;
        if node_index >= self.parent_nodes.len() {
            return Err(zx::Status::OUT_OF_RANGE);
        }

        if let Some(current_at_index) = &self.parent_nodes[node_index]
            && current_at_index.upgrade().is_some()
        {
            return Err(zx::Status::ALREADY_BOUND);
        }

        let result = self.bind_parent_impl(composite_parent, node_ptr.clone());
        if result.is_ok() {
            self.parent_nodes[node_index] = Some(node_ptr);
        }
        result
    }

    pub fn get_composite_info(&self) -> fdd::CompositeNodeInfo {
        let mut info = fdd::CompositeNodeInfo::default();

        let (spec, matched_driver) = if let Some(composite_info) = &self.composite_info {
            let spec = Some(fdf::CompositeNodeSpec {
                name: Some(composite_info.spec.name.clone()),
                parents2: Some(
                    composite_info.spec.parents.clone().into_iter().map(|p| p.into()).collect(),
                ),
                ..Default::default()
            });

            let md = &composite_info.matched_driver;
            let matched_driver = Some(fdf::CompositeDriverMatch {
                composite_driver: Some({
                    fdf::CompositeDriverInfo {
                        composite_name: Some(md.composite_driver.composite_name.clone()),
                        driver_info: Some(md.composite_driver.driver_info.clone().into()),
                        ..Default::default()
                    }
                }),
                parent_names: Some(md.parent_names.clone()),
                primary_parent_index: Some(md.primary_parent_index),
                ..Default::default()
            });
            (spec, matched_driver)
        } else {
            let spec = Some(fdf::CompositeNodeSpec {
                name: Some(self.name.clone()),
                parents2: Some(self.parent_specs.clone().into_iter().map(|p| p.into()).collect()),
                ..Default::default()
            });
            (spec, None)
        };

        info.composite = Some(fdd::CompositeInfo::Composite(fdf::CompositeInfo {
            spec,
            matched_driver,
            ..Default::default()
        }));

        if self.parent_set_collector.is_none() {
            info.parent_topological_paths = Some(vec![None; self.parent_nodes.len()]);
            info.parent_monikers = Some(vec![None; self.parent_nodes.len()]);
            return info;
        }

        let collector = self.parent_set_collector.as_ref().unwrap();
        info.parent_topological_paths = Some(collector.get_parent_topological_paths());
        info.parent_monikers = Some(collector.get_parent_monikers());

        if let Some(node_weak) = collector.completed_composite_node()
            && let Some(node) = node_weak.upgrade()
        {
            info.topological_path = Some(node.make_topological_path(false));
            info.moniker = Some(node.make_component_moniker());
        }
        info
    }

    pub fn remove(&mut self, callback: oneshot::Sender<Result<(), zx::Status>>) {
        self.parent_nodes = vec![None; self.parent_specs.len()];
        if self.parent_set_collector.is_none() {
            let _ = callback.send(Ok(()));
            return;
        }

        let collector = self.parent_set_collector.as_mut().unwrap();
        collector.release_nodes();

        if let Some(node_weak) = collector.completed_composite_node()
            && let Some(node) = node_weak.upgrade()
        {
            node.remove_composite_node_for_rebind(callback);
            self.parent_set_collector = None;
            self.driver_url.clear();
            self.composite_info = None;
            return;
        }

        self.parent_set_collector = None;
        self.driver_url.clear();
        self.composite_info = None;
        let _ = callback.send(Ok(()));
    }
}
