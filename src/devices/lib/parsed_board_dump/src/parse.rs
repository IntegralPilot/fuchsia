// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Normalization and serialization/deserialization utilities for `ParsedBoardDump`.

use crate::ast::*;
use anyhow::Context;

/// Parse a JSON5 or JSON string into a normalized `ParsedBoardDump`.
pub fn parse_board_dump_json5(content: &str) -> Result<ParsedBoardDump, anyhow::Error> {
    ParsedBoardDump::from_json5(content)
}

/// Parse a JSON string into a normalized `ParsedBoardDump`.
pub fn parse_board_dump_json(content: &str) -> Result<ParsedBoardDump, anyhow::Error> {
    ParsedBoardDump::from_json(content)
}

/// Serialize a `ParsedBoardDump` into formatted JSON string (valid JSON5 subset).
pub fn serialize_board_dump_json5(dump: &ParsedBoardDump) -> Result<String, anyhow::Error> {
    dump.to_json5()
}

/// Serialize a `ParsedBoardDump` into formatted JSON string.
pub fn serialize_board_dump_json(dump: &ParsedBoardDump) -> Result<String, anyhow::Error> {
    dump.to_json()
}

impl ParsedBoardDump {
    /// Canonically sort top-level node collections and commutative sets (e.g. properties,
    /// bind rules) while preserving positional order for index-dependent hardware resources
    /// (such as MMIOs, IRQs, BTIs, SMCs, and composite spec parents).
    pub fn normalize(&mut self) {
        // Sort CompositeNodeSpecs inner commutative sets
        for spec in &mut self.composite_node_specs {
            for parent in &mut spec.parents {
                for rule in &mut parent.bind_rules {
                    rule.values.sort();
                }
                parent.bind_rules.sort();
                parent.properties.sort();
            }
        }

        // Sort BoardChildNodes properties
        for child in &mut self.board_child_nodes {
            child.properties.sort();
        }

        // Sort PlatformBusNodes properties and IRQ properties (preserving hardware resource order)
        for node in &mut self.platform_bus_nodes {
            for irq in &mut node.irqs {
                irq.properties.sort();
            }
            node.properties.sort();
        }

        // Sort top-level arrays
        self.platform_bus_nodes.sort();
        self.board_child_nodes.sort();
        self.composite_node_specs.sort();
        self.iommus.sort();
    }

    /// Parse a JSON5 string into a normalized `ParsedBoardDump`.
    pub fn from_json5(content: &str) -> Result<Self, anyhow::Error> {
        let mut dump: Self =
            serde_json5::from_str(content).context("Failed to parse JSON5 board dump")?;
        dump.normalize();
        Ok(dump)
    }

    /// Parse a standard JSON string into a normalized `ParsedBoardDump`.
    pub fn from_json(content: &str) -> Result<Self, anyhow::Error> {
        let mut dump: Self =
            serde_json::from_str(content).context("Failed to parse JSON board dump")?;
        dump.normalize();
        Ok(dump)
    }

    /// Serialize this `ParsedBoardDump` into formatted JSON string (valid JSON5 subset).
    ///
    /// Note: `serde_json5` only supports deserialization, so serialization delegates
    /// to `serde_json`, producing standard JSON which is a valid strict subset of JSON5.
    pub fn to_json5(&self) -> Result<String, anyhow::Error> {
        self.to_json()
    }

    /// Serialize this `ParsedBoardDump` into formatted standard JSON string.
    pub fn to_json(&self) -> Result<String, anyhow::Error> {
        serde_json::to_string_pretty(self)
            .with_context(|| "Failed to serialize ParsedBoardDump to JSON")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_json5_round_trip() {
        let json5_data = r#"{
  // Kitchen-sink hardware platform node
  platform_bus_nodes: [
    {
      name: "kitchen-sink-device",
      vid: 0,
      pid: null,
      did: 50,
      instance_id: 2,
      driver_host: null,
      mmios: [
        {
          base: 0xff3f0000,
          length: 0x10000,
          name: "mmio_a"
        }
      ],
      irqs: [
        {
          number: 42,
          mode: "EDGE_HIGH",
          wake_vector: true,
          name: "irq_0",
          properties: []
        }
      ],
      btis: [
        {
          iommu_id: 93,
          bti_id: 1,
          name: "bti_x"
        }
      ],
      smcs: [
        {
          base: 0x1111,
          count: 2,
          exclusive: false,
          name: "smc_call"
        }
      ],
      metadata: [
        {
          id: "fuchsia.hardware.ethernet.Metadata",
          data_size: 64,
          decoded_dictionary: null
        }
      ],
      boot_metadata: [
        {
          zbi_type: 1128353133,
          zbi_extra: 0
        }
      ],
      properties: [
        {
          key: "fuchsia.devicetree.FIRST_COMPATIBLE",
          value: "amlogic,meson-g12a-dwmac"
        }
      ]
    }
  ],
  board_child_nodes: [
    {
      name: "sys",
      driver_host: null,
      bus_info: null,
      properties: []
    }
  ],
  composite_node_specs: [
    {
      name: "kitchen-sink-device",
      driver_host: null,
      parents: [
        {
          bind_rules: [
            {
              key: "fuchsia.BIND_PROTOCOL",
              condition: "Accept",
              values: [85]
            }
          ],
          properties: [
            {
              key: "fuchsia.BIND_PROTOCOL",
              value: 85
            }
          ]
        }
      ]
    }
  ],
  iommus: [
    {
      id: 93,
      description: "dummy-iommu"
    }
  ]
}"#;

        let parsed = ParsedBoardDump::from_json5(json5_data).expect("Parse JSON5");
        assert_eq!(parsed.platform_bus_nodes.len(), 1);
        let node = &parsed.platform_bus_nodes[0];
        assert_eq!(node.name, "kitchen-sink-device");
        assert_eq!(node.vid, Some(0));
        assert_eq!(node.did, Some(50));
        assert_eq!(node.mmios[0].base, Some(0xff3f0000));
        assert_eq!(node.irqs[0].number, Some(42));
        assert_eq!(node.btis[0].iommu_id, Some(93));
        assert_eq!(node.smcs[0].base, Some(0x1111));
        assert_eq!(node.metadata[0].id, "fuchsia.hardware.ethernet.Metadata");
        assert_eq!(node.boot_metadata[0].zbi_type, Some(1128353133));

        assert_eq!(parsed.board_child_nodes.len(), 1);
        assert_eq!(parsed.board_child_nodes[0].name, "sys");

        assert_eq!(parsed.composite_node_specs.len(), 1);
        assert_eq!(parsed.composite_node_specs[0].parents.len(), 1);

        assert_eq!(parsed.iommus.len(), 1);
        assert_eq!(parsed.iommus[0].id, 93);

        // Test serialization roundtrip
        let serialized = parsed.to_json5().expect("Serialize");
        let reparsed = parse_board_dump_json5(&serialized).expect("Reparse");
        assert_eq!(parsed, reparsed);
    }

    #[test]
    fn test_normalization_sorting() {
        let mut dump = ParsedBoardDump {
            platform_bus_nodes: vec![
                PlatformBusNode {
                    name: "b_node".to_string(),
                    mmios: vec![
                        MmioResource { base: Some(0x2000), length: Some(0x100), name: None },
                        MmioResource { base: Some(0x1000), length: Some(0x100), name: None },
                    ],
                    ..Default::default()
                },
                PlatformBusNode { name: "a_node".to_string(), ..Default::default() },
            ],
            board_child_nodes: vec![
                BoardChildNode { name: "child_z".to_string(), ..Default::default() },
                BoardChildNode { name: "child_a".to_string(), ..Default::default() },
            ],
            composite_node_specs: vec![
                CompositeNodeSpec { name: "spec_2".to_string(), ..Default::default() },
                CompositeNodeSpec { name: "spec_1".to_string(), ..Default::default() },
            ],
            iommus: vec![
                IommuEntry { id: 99, description: "b".to_string() },
                IommuEntry { id: 1, description: "a".to_string() },
            ],
        };

        dump.normalize();

        assert_eq!(dump.platform_bus_nodes[0].name, "a_node");
        assert_eq!(dump.platform_bus_nodes[1].name, "b_node");
        // Positional resource indices are preserved
        assert_eq!(dump.platform_bus_nodes[1].mmios[0].base, Some(0x2000));
        assert_eq!(dump.platform_bus_nodes[1].mmios[1].base, Some(0x1000));

        assert_eq!(dump.board_child_nodes[0].name, "child_a");
        assert_eq!(dump.board_child_nodes[1].name, "child_z");

        assert_eq!(dump.composite_node_specs[0].name, "spec_1");
        assert_eq!(dump.composite_node_specs[1].name, "spec_2");

        assert_eq!(dump.iommus[0].id, 1);
        assert_eq!(dump.iommus[1].id, 99);
    }

    #[test]
    fn test_bind_rule_values_sorting() {
        let mut dump = ParsedBoardDump {
            composite_node_specs: vec![CompositeNodeSpec {
                name: "spec_1".to_string(),
                driver_host: None,
                parents: vec![ParentSpec {
                    bind_rules: vec![BindRule {
                        key: "fuchsia.BIND_PROTOCOL".to_string(),
                        condition: BindCondition::Accept,
                        values: vec![
                            PropertyValue::IntValue(100),
                            PropertyValue::IntValue(10),
                            PropertyValue::IntValue(50),
                        ],
                    }],
                    properties: vec![],
                }],
            }],
            ..Default::default()
        };

        dump.normalize();

        assert_eq!(
            dump.composite_node_specs[0].parents[0].bind_rules[0].values,
            vec![
                PropertyValue::IntValue(10),
                PropertyValue::IntValue(50),
                PropertyValue::IntValue(100)
            ]
        );

        let json_str = dump.to_json().expect("Serialize to JSON");
        let reparsed = parse_board_dump_json(&json_str).expect("Reparse JSON");
        assert_eq!(dump, reparsed);
    }
}
