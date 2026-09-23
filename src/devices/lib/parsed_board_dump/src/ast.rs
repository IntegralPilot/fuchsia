// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Core data structures and Abstract Syntax Tree (AST) definitions for `parsed_board_dump`.
//!
//! Represents the normalized, ground-truth hardware topology of a Fuchsia board,
//! covering platform bus nodes, direct board children, composite node specifications,
//! and global IOMMUs.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PropertyValue {
    IntValue(u64),
    StringValue(String),
    BoolValue(bool),
}

impl Default for PropertyValue {
    fn default() -> Self {
        PropertyValue::StringValue(String::new())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum BindCondition {
    Accept,
    Reject,
}

impl Default for BindCondition {
    fn default() -> Self {
        BindCondition::Accept
    }
}

impl std::fmt::Display for BindCondition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BindCondition::Accept => write!(f, "Accept"),
            BindCondition::Reject => write!(f, "Reject"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Default)]
pub struct NodeProperty {
    pub key: String,
    pub value: PropertyValue,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Default)]
pub struct BindRule {
    pub key: String,
    pub condition: BindCondition,
    pub values: Vec<PropertyValue>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
pub struct ParentSpec {
    #[serde(default)]
    pub bind_rules: Vec<BindRule>,
    #[serde(default)]
    pub properties: Vec<NodeProperty>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
pub struct CompositeNodeSpec {
    pub name: String,
    #[serde(default)]
    pub parents: Vec<ParentSpec>,
    #[serde(default)]
    pub driver_host: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
pub struct MmioResource {
    #[serde(default)]
    pub base: Option<u64>,
    #[serde(default)]
    pub length: Option<u64>,
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
pub struct IrqResource {
    #[serde(default)]
    pub number: Option<u32>,
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub wake_vector: Option<bool>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub properties: Vec<NodeProperty>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
pub struct BtiResource {
    #[serde(default)]
    pub iommu_id: Option<u32>,
    #[serde(default)]
    pub bti_id: Option<u32>,
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
pub struct SmcResource {
    #[serde(default)]
    pub base: Option<u32>,
    #[serde(default)]
    pub count: Option<u32>,
    #[serde(default)]
    pub exclusive: Option<bool>,
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
pub struct BootMetadataResource {
    #[serde(default)]
    pub zbi_type: Option<u32>,
    #[serde(default)]
    pub zbi_extra: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
pub struct MetadataEntry {
    pub id: String,
    #[serde(default)]
    pub data_size: Option<usize>,
    #[serde(default)]
    pub decoded_dictionary: Option<BTreeMap<String, String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
pub struct PlatformBusNode {
    pub name: String,
    #[serde(default)]
    pub vid: Option<u32>,
    #[serde(default)]
    pub pid: Option<u32>,
    #[serde(default)]
    pub did: Option<u32>,
    #[serde(default)]
    pub instance_id: Option<u32>,
    #[serde(default)]
    pub interrupt_controller_id: Option<u32>,
    #[serde(default)]
    pub mmios: Vec<MmioResource>,
    #[serde(default)]
    pub irqs: Vec<IrqResource>,
    #[serde(default)]
    pub btis: Vec<BtiResource>,
    #[serde(default)]
    pub smcs: Vec<SmcResource>,
    #[serde(default)]
    pub boot_metadata: Vec<BootMetadataResource>,
    #[serde(default)]
    pub metadata: Vec<MetadataEntry>,
    #[serde(default)]
    pub properties: Vec<NodeProperty>,
    #[serde(default)]
    pub driver_host: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
pub struct BoardChildNode {
    pub name: String,
    #[serde(default)]
    pub driver_host: Option<String>,
    #[serde(default)]
    pub bus_info: Option<String>,
    #[serde(default)]
    pub properties: Vec<NodeProperty>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
pub struct IommuEntry {
    pub id: u32,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
pub struct ParsedBoardDump {
    #[serde(default)]
    pub platform_bus_nodes: Vec<PlatformBusNode>,
    #[serde(default)]
    pub board_child_nodes: Vec<BoardChildNode>,
    #[serde(default)]
    pub composite_node_specs: Vec<CompositeNodeSpec>,
    #[serde(default)]
    pub iommus: Vec<IommuEntry>,
}

// Backward-compatibility type aliases
pub type PbusNode = PlatformBusNode;
pub type PbusMmio = MmioResource;
pub type PbusIrq = IrqResource;
pub type PbusBti = BtiResource;
pub type PbusSmc = SmcResource;
pub type PbusBootMetadata = BootMetadataResource;
pub type InterceptedBoardDump = ParsedBoardDump;
