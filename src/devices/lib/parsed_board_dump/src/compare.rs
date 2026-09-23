// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Semantic comparison and mathematical parity calculation between board dumps.

use crate::ast::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParityReport {
    pub total_nodes: usize,
    pub matched_nodes: usize,
    pub missing_nodes: Vec<String>,
    pub extra_nodes: Vec<String>,
    pub node_differences: Vec<String>,

    pub total_specs: usize,
    pub matched_specs: usize,
    pub missing_specs: Vec<String>,
    pub extra_specs: Vec<String>,
    pub spec_differences: Vec<String>,

    pub total_board_children: usize,
    pub matched_board_children: usize,
    pub missing_board_children: Vec<String>,
    pub extra_board_children: Vec<String>,
    pub board_children_differences: Vec<String>,

    pub total_iommus: usize,
    pub matched_iommus: usize,
    pub missing_iommus: Vec<u32>,
    pub extra_iommus: Vec<u32>,
    pub iommu_differences: Vec<String>,

    pub score_percentage: f64,
}

impl ParityReport {
    pub fn is_pass(&self) -> bool {
        self.missing_nodes.is_empty()
            && self.extra_nodes.is_empty()
            && self.node_differences.is_empty()
            && self.missing_specs.is_empty()
            && self.extra_specs.is_empty()
            && self.spec_differences.is_empty()
            && self.missing_board_children.is_empty()
            && self.extra_board_children.is_empty()
            && self.board_children_differences.is_empty()
            && self.missing_iommus.is_empty()
            && self.extra_iommus.is_empty()
            && self.iommu_differences.is_empty()
    }

    pub fn format_summary(&self, show_diff: bool) -> String {
        let mut out = String::new();
        out.push_str("\n=======================================================\n");
        out.push_str("               BOARD DML PARITY REPORT                \n");
        out.push_str("=======================================================\n");
        out.push_str(&format!(
            "Platform Bus Nodes   : {} / {} matched ({} missing, {} extra)\n",
            self.matched_nodes,
            self.total_nodes,
            self.missing_nodes.len(),
            self.extra_nodes.len()
        ));
        out.push_str(&format!(
            "Composite Node Specs : {} / {} matched ({} missing, {} extra)\n",
            self.matched_specs,
            self.total_specs,
            self.missing_specs.len(),
            self.extra_specs.len()
        ));
        if self.total_board_children > 0 || !self.extra_board_children.is_empty() {
            out.push_str(&format!(
                "Board Child Nodes    : {} / {} matched ({} missing, {} extra)\n",
                self.matched_board_children,
                self.total_board_children,
                self.missing_board_children.len(),
                self.extra_board_children.len()
            ));
        }
        if self.total_iommus > 0 || !self.extra_iommus.is_empty() {
            out.push_str(&format!(
                "IOMMUs               : {} / {} matched ({} missing, {} extra)\n",
                self.matched_iommus,
                self.total_iommus,
                self.missing_iommus.len(),
                self.extra_iommus.len()
            ));
        }
        out.push_str("-------------------------------------------------------\n");
        out.push_str(&format!("Overall Parity Score : {:.1}%\n", self.score_percentage));
        out.push_str("=======================================================\n");

        if show_diff {
            if !self.missing_nodes.is_empty() {
                out.push_str("\n[MISSING PLATFORM NODES]\n");
                for n in &self.missing_nodes {
                    out.push_str(&format!("  - {}\n", n));
                }
            }
            if !self.extra_nodes.is_empty() {
                out.push_str("\n[EXTRA PLATFORM NODES]\n");
                for n in &self.extra_nodes {
                    out.push_str(&format!("  + {}\n", n));
                }
            }
            if !self.node_differences.is_empty() {
                out.push_str("\n[PLATFORM NODE DIFFERENCES]\n");
                for diff in &self.node_differences {
                    out.push_str(&format!("{}\n", diff));
                }
            }
            if !self.missing_specs.is_empty() {
                out.push_str("\n[MISSING COMPOSITE NODE SPECS]\n");
                for s in &self.missing_specs {
                    out.push_str(&format!("  - {}\n", s));
                }
            }
            if !self.extra_specs.is_empty() {
                out.push_str("\n[EXTRA COMPOSITE NODE SPECS]\n");
                for s in &self.extra_specs {
                    out.push_str(&format!("  + {}\n", s));
                }
            }
            if !self.spec_differences.is_empty() {
                out.push_str("\n[COMPOSITE SPEC DIFFERENCES]\n");
                for diff in &self.spec_differences {
                    out.push_str(&format!("{}\n", diff));
                }
            }
            if !self.missing_board_children.is_empty() {
                out.push_str("\n[MISSING BOARD CHILDREN]\n");
                for c in &self.missing_board_children {
                    out.push_str(&format!("  - {}\n", c));
                }
            }
            if !self.extra_board_children.is_empty() {
                out.push_str("\n[EXTRA BOARD CHILDREN]\n");
                for c in &self.extra_board_children {
                    out.push_str(&format!("  + {}\n", c));
                }
            }
            if !self.board_children_differences.is_empty() {
                out.push_str("\n[BOARD CHILD DIFFERENCES]\n");
                for diff in &self.board_children_differences {
                    out.push_str(&format!("{}\n", diff));
                }
            }
            if !self.missing_iommus.is_empty() {
                out.push_str("\n[MISSING IOMMUS]\n");
                for i in &self.missing_iommus {
                    out.push_str(&format!("  - ID: {}\n", i));
                }
            }
            if !self.extra_iommus.is_empty() {
                out.push_str("\n[EXTRA IOMMUS]\n");
                for i in &self.extra_iommus {
                    out.push_str(&format!("  + ID: {}\n", i));
                }
            }
            if !self.iommu_differences.is_empty() {
                out.push_str("\n[IOMMU DIFFERENCES]\n");
                for diff in &self.iommu_differences {
                    out.push_str(&format!("{}\n", diff));
                }
            }
        }
        out
    }

    pub fn print_summary(&self, show_diff: bool) {
        print!("{}", self.format_summary(show_diff));
    }
}

impl std::fmt::Display for ParityReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.format_summary(false))
    }
}

fn diff_arrays<T: PartialEq + std::fmt::Debug>(
    name: &str,
    cand: &[T],
    gold: &[T],
    diffs: &mut Vec<String>,
) {
    if cand != gold {
        diffs.push(format!(
            "    {} mismatch:\n      Got:  {:?}\n      Want: {:?}",
            name, cand, gold
        ));
    }
}

pub fn compare_board_dumps(candidate: &ParsedBoardDump, golden: &ParsedBoardDump) -> ParityReport {
    let mut cand_norm = candidate.clone();
    cand_norm.normalize();
    let mut gold_norm = golden.clone();
    gold_norm.normalize();

    let mut cand_nodes: BTreeMap<String, &PlatformBusNode> = BTreeMap::new();
    for n in &cand_norm.platform_bus_nodes {
        cand_nodes.insert(n.name.clone(), n);
    }
    let mut gold_nodes: BTreeMap<String, &PlatformBusNode> = BTreeMap::new();
    for n in &gold_norm.platform_bus_nodes {
        gold_nodes.insert(n.name.clone(), n);
    }

    let mut matched_nodes = 0;
    let mut missing_nodes = Vec::new();
    let mut extra_nodes = Vec::new();
    let mut node_differences = Vec::new();

    let all_node_names: BTreeSet<String> =
        cand_nodes.keys().chain(gold_nodes.keys()).cloned().collect();

    for name in &all_node_names {
        match (cand_nodes.get(name), gold_nodes.get(name)) {
            (Some(cand), Some(gold)) => {
                let mut diffs = Vec::new();
                if cand.vid != gold.vid {
                    diffs.push(format!("    vid: got {:?}, want {:?}", cand.vid, gold.vid));
                }
                if cand.pid != gold.pid {
                    diffs.push(format!("    pid: got {:?}, want {:?}", cand.pid, gold.pid));
                }
                if cand.did != gold.did {
                    diffs.push(format!("    did: got {:?}, want {:?}", cand.did, gold.did));
                }
                if cand.instance_id != gold.instance_id {
                    diffs.push(format!(
                        "    instance_id: got {:?}, want {:?}",
                        cand.instance_id, gold.instance_id
                    ));
                }
                if cand.interrupt_controller_id != gold.interrupt_controller_id {
                    diffs.push(format!(
                        "    interrupt_controller_id: got {:?}, want {:?}",
                        cand.interrupt_controller_id, gold.interrupt_controller_id
                    ));
                }
                diff_arrays("mmios", &cand.mmios, &gold.mmios, &mut diffs);
                diff_arrays("irqs", &cand.irqs, &gold.irqs, &mut diffs);
                diff_arrays("btis", &cand.btis, &gold.btis, &mut diffs);
                diff_arrays("smcs", &cand.smcs, &gold.smcs, &mut diffs);
                if cand.driver_host != gold.driver_host {
                    diffs.push(format!(
                        "    driver_host: got {:?}, want {:?}",
                        cand.driver_host, gold.driver_host
                    ));
                }
                diff_arrays("boot_metadata", &cand.boot_metadata, &gold.boot_metadata, &mut diffs);
                diff_arrays("metadata", &cand.metadata, &gold.metadata, &mut diffs);
                diff_arrays("properties", &cand.properties, &gold.properties, &mut diffs);

                if diffs.is_empty() {
                    matched_nodes += 1;
                } else {
                    node_differences.push(format!("Node \"{}\":\n{}", name, diffs.join("\n")));
                }
            }
            (None, Some(_)) => {
                missing_nodes.push(name.clone());
            }
            (Some(_), None) => {
                extra_nodes.push(name.clone());
            }
            (None, None) => {}
        }
    }

    // 2. Composite Node Specs
    let mut cand_specs: BTreeMap<String, &CompositeNodeSpec> = BTreeMap::new();
    for s in &cand_norm.composite_node_specs {
        cand_specs.insert(s.name.clone(), s);
    }
    let mut gold_specs: BTreeMap<String, &CompositeNodeSpec> = BTreeMap::new();
    for s in &gold_norm.composite_node_specs {
        gold_specs.insert(s.name.clone(), s);
    }

    let mut matched_specs = 0;
    let mut missing_specs = Vec::new();
    let mut extra_specs = Vec::new();
    let mut spec_differences = Vec::new();

    let all_spec_names: BTreeSet<String> =
        cand_specs.keys().chain(gold_specs.keys()).cloned().collect();

    for name in &all_spec_names {
        match (cand_specs.get(name), gold_specs.get(name)) {
            (Some(cand), Some(gold)) => {
                let mut diffs = Vec::new();
                if cand.driver_host != gold.driver_host {
                    diffs.push(format!(
                        "    driver_host: got {:?}, want {:?}",
                        cand.driver_host, gold.driver_host
                    ));
                }
                diff_arrays("parents", &cand.parents, &gold.parents, &mut diffs);

                if diffs.is_empty() {
                    matched_specs += 1;
                } else {
                    spec_differences.push(format!("Spec \"{}\":\n{}", name, diffs.join("\n")));
                }
            }
            (None, Some(_)) => {
                missing_specs.push(name.clone());
            }
            (Some(_), None) => {
                extra_specs.push(name.clone());
            }
            (None, None) => {}
        }
    }

    // 3. Board Child Nodes
    let mut cand_children: BTreeMap<String, &BoardChildNode> = BTreeMap::new();
    for c in &cand_norm.board_child_nodes {
        cand_children.insert(c.name.clone(), c);
    }
    let mut gold_children: BTreeMap<String, &BoardChildNode> = BTreeMap::new();
    for c in &gold_norm.board_child_nodes {
        gold_children.insert(c.name.clone(), c);
    }

    let mut matched_board_children = 0;
    let mut missing_board_children = Vec::new();
    let mut extra_board_children = Vec::new();
    let mut board_children_differences = Vec::new();

    let all_child_names: BTreeSet<String> =
        cand_children.keys().chain(gold_children.keys()).cloned().collect();

    for name in &all_child_names {
        match (cand_children.get(name), gold_children.get(name)) {
            (Some(cand), Some(gold)) => {
                let mut diffs = Vec::new();
                if cand.driver_host != gold.driver_host {
                    diffs.push(format!(
                        "    driver_host: got {:?}, want {:?}",
                        cand.driver_host, gold.driver_host
                    ));
                }
                if cand.bus_info != gold.bus_info {
                    diffs.push(format!(
                        "    bus_info: got {:?}, want {:?}",
                        cand.bus_info, gold.bus_info
                    ));
                }
                diff_arrays("properties", &cand.properties, &gold.properties, &mut diffs);

                if diffs.is_empty() {
                    matched_board_children += 1;
                } else {
                    board_children_differences.push(format!(
                        "Board Child \"{}\":\n{}",
                        name,
                        diffs.join("\n")
                    ));
                }
            }
            (None, Some(_)) => missing_board_children.push(name.clone()),
            (Some(_), None) => extra_board_children.push(name.clone()),
            (None, None) => {}
        }
    }

    // 4. IOMMUs
    let mut cand_iommus: BTreeMap<u32, &IommuEntry> = BTreeMap::new();
    for i in &cand_norm.iommus {
        cand_iommus.insert(i.id, i);
    }
    let mut gold_iommus: BTreeMap<u32, &IommuEntry> = BTreeMap::new();
    for i in &gold_norm.iommus {
        gold_iommus.insert(i.id, i);
    }

    let mut matched_iommus = 0;
    let mut missing_iommus = Vec::new();
    let mut extra_iommus = Vec::new();
    let mut iommu_differences = Vec::new();

    let all_iommu_ids: BTreeSet<u32> =
        cand_iommus.keys().chain(gold_iommus.keys()).cloned().collect();

    for id in &all_iommu_ids {
        match (cand_iommus.get(id), gold_iommus.get(id)) {
            (Some(cand), Some(gold)) => {
                if cand.description != gold.description {
                    iommu_differences.push(format!(
                        "IOMMU {}: got \"{}\", want \"{}\"",
                        id, cand.description, gold.description
                    ));
                } else {
                    matched_iommus += 1;
                }
            }
            (None, Some(_)) => missing_iommus.push(*id),
            (Some(_), None) => extra_iommus.push(*id),
            (None, None) => {}
        }
    }

    let total_items =
        all_node_names.len() + all_spec_names.len() + all_child_names.len() + all_iommu_ids.len();
    let matched_items = matched_nodes + matched_specs + matched_board_children + matched_iommus;

    let score =
        if total_items == 0 { 100.0 } else { (matched_items as f64 / total_items as f64) * 100.0 };

    ParityReport {
        total_nodes: gold_nodes.len(),
        matched_nodes,
        missing_nodes,
        extra_nodes,
        node_differences,

        total_specs: gold_specs.len(),
        matched_specs,
        missing_specs,
        extra_specs,
        spec_differences,

        total_board_children: gold_children.len(),
        matched_board_children,
        missing_board_children,
        extra_board_children,
        board_children_differences,

        total_iommus: gold_iommus.len(),
        matched_iommus,
        missing_iommus,
        extra_iommus,
        iommu_differences,

        score_percentage: score,
    }
}

impl ParsedBoardDump {
    /// Compare this candidate board dump against a reference golden dump.
    pub fn compare(&self, golden: &ParsedBoardDump) -> ParityReport {
        compare_board_dumps(self, golden)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compare_identical_dumps() {
        let dump = ParsedBoardDump {
            platform_bus_nodes: vec![PlatformBusNode {
                name: "gpio".to_string(),
                vid: Some(0),
                pid: None,
                did: Some(50),
                mmios: vec![MmioResource {
                    base: Some(0xff634000),
                    length: Some(0x1000),
                    name: Some("gpio".to_string()),
                }],
                ..Default::default()
            }],
            board_child_nodes: vec![],
            composite_node_specs: vec![CompositeNodeSpec {
                name: "sdmmc".to_string(),
                parents: vec![ParentSpec {
                    bind_rules: vec![BindRule {
                        key: "fuchsia.BIND_PROTOCOL".to_string(),
                        condition: BindCondition::Accept,
                        values: vec![PropertyValue::IntValue(85)],
                    }],
                    properties: vec![],
                }],
                driver_host: None,
            }],
            iommus: vec![IommuEntry { id: 1, description: "main-iommu".to_string() }],
        };

        let report = dump.compare(&dump);
        assert!(report.is_pass());
        assert_eq!(report.matched_nodes, 1);
        assert_eq!(report.matched_specs, 1);
        assert_eq!(report.matched_iommus, 1);
        assert_eq!(report.score_percentage, 100.0);
    }

    #[test]
    fn test_compare_discrepancies() {
        let golden = ParsedBoardDump {
            platform_bus_nodes: vec![
                PlatformBusNode {
                    name: "gpio".to_string(),
                    mmios: vec![MmioResource {
                        base: Some(0xff634000),
                        length: Some(0x1000),
                        name: None,
                    }],
                    ..Default::default()
                },
                PlatformBusNode { name: "i2c".to_string(), ..Default::default() },
            ],
            composite_node_specs: vec![],
            board_child_nodes: vec![],
            iommus: vec![],
        };

        let candidate = ParsedBoardDump {
            platform_bus_nodes: vec![
                PlatformBusNode {
                    name: "gpio".to_string(),
                    mmios: vec![MmioResource {
                        base: Some(0xff635000), // Mismatched base address
                        length: Some(0x1000),
                        name: None,
                    }],
                    ..Default::default()
                },
                PlatformBusNode {
                    name: "extra_device".to_string(), // Extra node
                    ..Default::default()
                },
            ],
            composite_node_specs: vec![],
            board_child_nodes: vec![],
            iommus: vec![],
        };

        let report = candidate.compare(&golden);
        assert!(!report.is_pass());
        assert_eq!(report.total_nodes, 2);
        assert_eq!(report.matched_nodes, 0);
        assert_eq!(report.missing_nodes, vec!["i2c".to_string()]);
        assert_eq!(report.extra_nodes, vec!["extra_device".to_string()]);
        assert_eq!(report.node_differences.len(), 1);
        assert!(report.node_differences[0].contains("mmios mismatch"));
    }
}
