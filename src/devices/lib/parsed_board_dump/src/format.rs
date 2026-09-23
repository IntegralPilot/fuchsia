// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Stringification and canonical diff-friendly text formatting for `ParsedBoardDump`.

use crate::ast::*;

pub fn stringify_protocol(val: u64) -> String {
    let name = match val {
        1 => "BLOCK",
        20 => "GPIO",
        24 => "I2C",
        31 => "PCI",
        33 => "USB",
        47 => "BT_HCI",
        54 => "SDHCI",
        55 => "SDMMC",
        72 => "POWER",
        85 => "PDEV",
        90 => "CLOCK",
        121 => "SPI",
        129 => "CPU_CTRL",
        147 => "ADC",
        152 => "REGISTERS",
        153 => "DAI",
        171 => "AUDIO_COMPOSITE",
        _ => return val.to_string(),
    };
    format!("{} ({})", name, val)
}

pub fn stringify_vid(val: u64) -> String {
    let name = match val {
        0 => "GENERIC",
        1 => "QEMU",
        3 => "GOOGLE",
        5 => "AMLOGIC",
        6 => "BROADCOM",
        8 => "NXP",
        9 => "QUALCOMM",
        _ => return val.to_string(),
    };
    format!("{} ({})", name, val)
}

pub fn stringify_pid(vid: u64, val: u64) -> String {
    let name = if vid == 3 {
        // PDEV_VID_GOOGLE
        match val {
            1 => "GAUSS",
            2 => "MACHINA",
            3 => "ASTRO",
            4 => "SHERLOCK",
            5 => "CLEO",
            6 => "EAGLE",
            7 => "VISALIA",
            8 => "C18",
            11 => "NELSON",
            12 => "VS680_EVK",
            13 => "LUIS",
            14 => "GOLDFISH",
            15 => "MOTMOT",
            16 => "AV400",
            17 => "PINECREST",
            18 => "CLOVER",
            19 => "VIOLET",
            20 => "KOLA",
            21 => "IRIS",
            _ => "",
        }
    } else {
        ""
    };
    if name.is_empty() { val.to_string() } else { format!("{} ({})", name, val) }
}

pub fn stringify_did(vid: u64, pid: u64, val: u64) -> String {
    let name = if vid == 0 && pid == 0 && val == 50 { "DEVICETREE" } else { "" };
    if name.is_empty() { val.to_string() } else { format!("{} ({})", name, val) }
}

pub fn stringify_property_value(val: &PropertyValue, key: &str, vid: u32, pid: u32) -> String {
    match val {
        PropertyValue::IntValue(v) => {
            if key == "fuchsia.BIND_PROTOCOL" {
                stringify_protocol(*v)
            } else if key == "fuchsia.BIND_PLATFORM_DEV_VID" {
                stringify_vid(*v)
            } else if key == "fuchsia.BIND_PLATFORM_DEV_PID" {
                stringify_pid(vid as u64, *v)
            } else if key == "fuchsia.BIND_PLATFORM_DEV_DID" {
                stringify_did(vid as u64, pid as u64, *v)
            } else {
                v.to_string()
            }
        }
        PropertyValue::StringValue(s) => format!("\"{}\"", s),
        PropertyValue::BoolValue(b) => b.to_string(),
    }
}

fn get_vid_from_props(props: &[NodeProperty]) -> u32 {
    for prop in props {
        if prop.key == "fuchsia.BIND_PLATFORM_DEV_VID" {
            if let PropertyValue::IntValue(v) = &prop.value {
                return *v as u32;
            }
        }
    }
    0
}

fn get_pid_from_props(props: &[NodeProperty]) -> u32 {
    for prop in props {
        if prop.key == "fuchsia.BIND_PLATFORM_DEV_PID" {
            if let PropertyValue::IntValue(v) = &prop.value {
                return *v as u32;
            }
        }
    }
    0
}

fn get_vid_from_rules(rules: &[BindRule]) -> u32 {
    for rule in rules {
        if rule.key == "fuchsia.BIND_PLATFORM_DEV_VID" {
            for val in &rule.values {
                if let PropertyValue::IntValue(v) = val {
                    return *v as u32;
                }
            }
        }
    }
    0
}

fn get_pid_from_rules(rules: &[BindRule]) -> u32 {
    for rule in rules {
        if rule.key == "fuchsia.BIND_PLATFORM_DEV_PID" {
            for val in &rule.values {
                if let PropertyValue::IntValue(v) = val {
                    return *v as u32;
                }
            }
        }
    }
    0
}

pub fn format_properties(
    props: &[NodeProperty],
    indent: &str,
    override_vid: Option<u32>,
    override_pid: Option<u32>,
) -> String {
    let mut out = String::new();
    let vid = override_vid.unwrap_or_else(|| get_vid_from_props(props));
    let pid = override_pid.unwrap_or_else(|| get_pid_from_props(props));
    for prop in props {
        out.push_str(&format!(
            "{}property {{ {} = {} }}\n",
            indent,
            prop.key,
            stringify_property_value(&prop.value, &prop.key, vid, pid)
        ));
    }
    out
}

pub fn format_parent_spec(spec: &ParentSpec, indent: &str) -> String {
    let mut out = String::new();
    if spec.bind_rules.is_empty() && spec.properties.is_empty() {
        return out;
    }

    let mut vid = get_vid_from_rules(&spec.bind_rules);
    if vid == 0 {
        vid = get_vid_from_props(&spec.properties);
    }
    let mut pid = get_pid_from_rules(&spec.bind_rules);
    if pid == 0 {
        pid = get_pid_from_props(&spec.properties);
    }

    if !spec.bind_rules.is_empty() {
        out.push_str(&format!("{}bind_rules {{\n", indent));
        for rule in &spec.bind_rules {
            out.push_str(&format!("{}  {}({}, [", indent, rule.condition, rule.key));
            for (i, val) in rule.values.iter().enumerate() {
                out.push_str(&stringify_property_value(val, &rule.key, vid, pid));
                if i + 1 < rule.values.len() {
                    out.push_str(", ");
                }
            }
            out.push_str("])\n");
        }
        out.push_str(&format!("{}}}\n", indent));
    }

    if !spec.properties.is_empty() {
        out.push_str(&format!("{}properties {{\n", indent));
        out.push_str(&format_properties(
            &spec.properties,
            &format!("{}  ", indent),
            Some(vid),
            Some(pid),
        ));
        out.push_str(&format!("{}}}\n", indent));
    }

    out
}

pub fn format_platform_bus_node(node: &PlatformBusNode) -> String {
    let mut out = String::new();
    out.push_str(&format!("  \"{}\" {{\n", node.name));
    if let Some(vid) = node.vid {
        out.push_str(&format!("    vid = {}\n", stringify_vid(vid as u64)));
    }
    if let Some(pid) = node.pid {
        out.push_str(&format!(
            "    pid = {}\n",
            stringify_pid(node.vid.unwrap_or(0) as u64, pid as u64)
        ));
    }
    if let Some(did) = node.did {
        out.push_str(&format!(
            "    did = {}\n",
            stringify_did(node.vid.unwrap_or(0) as u64, node.pid.unwrap_or(0) as u64, did as u64)
        ));
    }
    if let Some(inst) = node.instance_id {
        out.push_str(&format!("    instance_id = {}\n", inst));
    }
    if let Some(ic) = node.interrupt_controller_id {
        out.push_str(&format!("    interrupt_controller_id = {}\n", ic));
    }

    for mmio in &node.mmios {
        out.push_str("    mmio {\n");
        if let Some(b) = mmio.base {
            out.push_str(&format!("      base = 0x{:x}\n", b));
        }
        if let Some(l) = mmio.length {
            out.push_str(&format!("      length = 0x{:x}\n", l));
        }
        if let Some(n) = &mmio.name {
            out.push_str(&format!("      name = \"{}\"\n", n));
        }
        out.push_str("    }\n");
    }

    for irq in &node.irqs {
        out.push_str("    irq {\n");
        if let Some(num) = irq.number {
            out.push_str(&format!("      number = {}\n", num));
        }
        if let Some(mode) = &irq.mode {
            out.push_str(&format!("      mode = {}\n", mode));
        }
        if let Some(wv) = irq.wake_vector {
            out.push_str(&format!("      wake_vector = {}\n", wv));
        }
        if let Some(n) = &irq.name {
            out.push_str(&format!("      name = \"{}\"\n", n));
        }
        if !irq.properties.is_empty() {
            out.push_str("      properties {\n");
            out.push_str(&format_properties(&irq.properties, "        ", None, None));
            out.push_str("      }\n");
        }
        out.push_str("    }\n");
    }

    for bti in &node.btis {
        out.push_str("    bti {\n");
        if let Some(iommu_id) = bti.iommu_id {
            out.push_str(&format!("      iommu_id = {}\n", iommu_id));
        }
        if let Some(bti_id) = bti.bti_id {
            out.push_str(&format!("      bti_id = {}\n", bti_id));
        }
        if let Some(n) = &bti.name {
            out.push_str(&format!("      name = \"{}\"\n", n));
        }
        out.push_str("    }\n");
    }

    for md in &node.metadata {
        out.push_str("    metadata {\n");
        out.push_str(&format!("      id = \"{}\"\n", md.id));
        if let Some(dict) = &md.decoded_dictionary {
            out.push_str("      data = { ");
            let entries: Vec<String> = dict.iter().map(|(k, v)| format!("{}: {}", k, v)).collect();
            out.push_str(&entries.join(", "));
            out.push_str(" }\n");
        } else if let Some(sz) = md.data_size {
            out.push_str(&format!("      data = [ <binary> size: {} bytes ]\n", sz));
        } else {
            out.push_str("      data = [ <binary> ]\n");
        }
        out.push_str("    }\n");
    }

    for smc in &node.smcs {
        out.push_str("    smc {\n");
        if let Some(b) = smc.base {
            out.push_str(&format!("      base = 0x{:x}\n", b));
        }
        if let Some(c) = smc.count {
            out.push_str(&format!("      count = {}\n", c));
        }
        if let Some(ex) = smc.exclusive {
            out.push_str(&format!("      exclusive = {}\n", ex));
        }
        if let Some(n) = &smc.name {
            out.push_str(&format!("      name = \"{}\"\n", n));
        }
        out.push_str("    }\n");
    }

    for bm in &node.boot_metadata {
        out.push_str("    boot_metadata {\n");
        if let Some(t) = bm.zbi_type {
            out.push_str(&format!("      zbi_type = {}\n", t));
        }
        if let Some(e) = bm.zbi_extra {
            out.push_str(&format!("      zbi_extra = {}\n", e));
        }
        out.push_str("    }\n");
    }

    if !node.properties.is_empty() {
        out.push_str(&format_properties(&node.properties, "    ", node.vid, node.pid));
    }

    if let Some(dh) = &node.driver_host {
        out.push_str(&format!("    driver_host = \"{}\"\n", dh));
    }

    out.push_str("  }\n");
    out
}

pub fn format_composite_node_spec(spec: &CompositeNodeSpec) -> String {
    let mut out = String::new();
    out.push_str(&format!("  \"{}\" {{\n", spec.name));
    for parent in &spec.parents {
        out.push_str("    parent {\n");
        out.push_str(&format_parent_spec(parent, "      "));
        out.push_str("    }\n");
    }
    if let Some(dh) = &spec.driver_host {
        out.push_str(&format!("    driver_host = \"{}\"\n", dh));
    }
    out.push_str("  }\n");
    out
}

pub fn format_board_dump(dump: &ParsedBoardDump) -> String {
    let mut normalized = dump.clone();
    normalized.normalize();
    let mut out = String::new();

    // 1. Platform Bus Nodes (canonically sorted)
    out.push_str("platform_bus_nodes {\n");
    for node in &normalized.platform_bus_nodes {
        out.push_str(&format_platform_bus_node(node));
        out.push('\n');
    }
    out.push_str("}\n\n");

    // 2. Board Child Nodes (canonically sorted)
    out.push_str("board_child_nodes {\n");
    for node in &normalized.board_child_nodes {
        out.push_str(&format!("  \"{}\" {{\n", node.name));
        if let Some(dh) = &node.driver_host {
            out.push_str(&format!("    driver_host = \"{}\"\n", dh));
        }
        if let Some(bi) = &node.bus_info {
            out.push_str(&format!("    bus_info = \"{}\"\n", bi));
        }
        if !node.properties.is_empty() {
            out.push_str(&format_properties(&node.properties, "    ", None, None));
        }
        out.push_str("  }\n\n");
    }
    out.push_str("}\n\n");

    // 3. Composite Node Specs (canonically sorted)
    out.push_str("composite_node_specs {\n");
    for spec in &normalized.composite_node_specs {
        out.push_str(&format_composite_node_spec(spec));
        out.push('\n');
    }
    out.push_str("}\n\n");

    // 4. IOMMUs (canonically sorted)
    out.push_str("iommus {\n");
    for iommu in &normalized.iommus {
        out.push_str("  iommu {\n");
        out.push_str(&format!("    id = {}\n", iommu.id));
        out.push_str(&format!("    description = \"{}\"\n", iommu.description));
        out.push_str("  }\n");
    }
    out.push_str("}\n");

    out
}

impl ParsedBoardDump {
    /// Format this `ParsedBoardDump` as canonical sorted diff-friendly text matching legacy dump output.
    pub fn to_canonical_text(&self) -> String {
        format_board_dump(self)
    }
}
