// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::parser::DmlBind;
use anyhow::anyhow;
use serde_json::Value;

#[derive(Clone, Debug)]
pub struct AdditionalParentInfo {
    pub parent_name: String,
    pub service_name: Option<String>,
    pub banjo_name: Option<String>,
    pub transport: String,
    pub optional: bool,
    pub bind: Option<DmlBind>,
}

fn format_bind_val(val: &Value) -> Result<String, anyhow::Error> {
    match val {
        Value::Number(n) => Ok(n.to_string()),
        Value::String(s) => {
            if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
                Ok(s.clone())
            } else if s.contains('.')
                || s.starts_with("0x")
                || s.starts_with("0X")
                || (!s.is_empty() && s.chars().all(|c| c.is_ascii_digit()))
            {
                Ok(s.clone())
            } else {
                Ok(format!("\"{}\"", s))
            }
        }
        Value::Bool(b) => Ok(b.to_string()),
        _ => Err(anyhow!("Invalid bind value type: {} (expected string, number or bool)", val)),
    }
}

fn emit_property_rule(content: &mut String, key: &str, val: &Value) -> Result<(), anyhow::Error> {
    match val {
        Value::Array(arr) => {
            content.push_str(&format!("accept {} {{\n", key));
            for v in arr {
                content.push_str(&format!("  {},\n", format_bind_val(v)?));
            }
            content.push_str("}\n");
        }
        _ => {
            content.push_str(&format!("{} == {};\n", key, format_bind_val(val)?));
        }
    }
    Ok(())
}

fn get_effective_protocol<'a>(bind: &'a DmlBind) -> Option<&'a str> {
    bind.protocol.as_deref().or_else(|| bind.usb.as_ref().and_then(|u| u.bind_protocol.as_deref()))
}

fn generate_simple_bind_statements_excluding(
    bind: &DmlBind,
    trigger: Option<&TriggerKind>,
) -> Result<String, anyhow::Error> {
    let mut content = String::new();
    if !matches!(trigger, Some(TriggerKind::Protocol(_))) {
        if let Some(proto) = get_effective_protocol(bind) {
            content.push_str(&format!("fuchsia.BIND_PROTOCOL == {};\n", proto));
        } else if let Some(banjo) = &bind.banjo {
            content.push_str(&format!("fuchsia.BIND_PROTOCOL == {};\n", banjo));
        }
    }
    if !matches!(trigger, Some(TriggerKind::Service(_))) {
        if let Some(svc) = &bind.service {
            content.push_str(&format!("fuchsia.Service == \"{}\";\n", svc));
        }
    }
    if !matches!(trigger, Some(TriggerKind::Vid(_))) {
        if let Some(vid) = &bind.vid {
            emit_property_rule(&mut content, "fuchsia.BIND_PLATFORM_DEV_VID", vid)?;
        }
    }
    if !matches!(trigger, Some(TriggerKind::Pid(_))) {
        if let Some(pid) = &bind.pid {
            emit_property_rule(&mut content, "fuchsia.BIND_PLATFORM_DEV_PID", pid)?;
        }
    }
    if !matches!(trigger, Some(TriggerKind::Did(_))) {
        if let Some(did) = &bind.did {
            emit_property_rule(&mut content, "fuchsia.BIND_PLATFORM_DEV_DID", did)?;
        }
    }
    if !matches!(trigger, Some(TriggerKind::Compat(_))) {
        if let Some(compat) = &bind.compat {
            match compat {
                Value::Array(arr) => {
                    content.push_str("accept fuchsia.COMPATIBLE {\n");
                    for v in arr {
                        if let Some(s) = v.as_str() {
                            content.push_str(&format!("  \"{}\",\n", s));
                        }
                    }
                    content.push_str("}\n");
                }
                Value::String(s) => {
                    content.push_str(&format!("fuchsia.COMPATIBLE == \"{}\";\n", s));
                }
                _ => {}
            }
        }
    }
    if let Some(pci) = &bind.pci {
        if !matches!(trigger, Some(TriggerKind::PciVid(_))) {
            if let Some(vid) = &pci.vid {
                emit_property_rule(&mut content, "fuchsia.BIND_PCI_VID", vid)?;
            }
        }
        if !matches!(trigger, Some(TriggerKind::PciDid(_))) {
            if let Some(did) = &pci.did {
                emit_property_rule(&mut content, "fuchsia.BIND_PCI_DID", did)?;
            }
        }
        if !matches!(trigger, Some(TriggerKind::PciClass(_))) {
            if let Some(class) = &pci.class {
                emit_property_rule(&mut content, "fuchsia.BIND_PCI_CLASS", class)?;
            }
        }
        if let Some(subclass) = &pci.subclass {
            emit_property_rule(&mut content, "fuchsia.BIND_PCI_SUBCLASS", subclass)?;
        }
        if let Some(interface) = &pci.interface {
            emit_property_rule(&mut content, "fuchsia.BIND_PCI_INTERFACE", interface)?;
        }
        if let Some(revision) = &pci.revision {
            emit_property_rule(&mut content, "fuchsia.BIND_PCI_REVISION", revision)?;
        }
        if let Some(topo) = &pci.topo {
            emit_property_rule(&mut content, "fuchsia.BIND_PCI_TOPO", topo)?;
        }
    }
    if let Some(usb) = &bind.usb {
        if !matches!(trigger, Some(TriggerKind::UsbVid(_))) {
            if let Some(vid) = &usb.vid {
                emit_property_rule(&mut content, "fuchsia.BIND_USB_VID", vid)?;
            }
        }
        if !matches!(trigger, Some(TriggerKind::UsbPid(_))) {
            if let Some(pid) = &usb.pid {
                emit_property_rule(&mut content, "fuchsia.BIND_USB_PID", pid)?;
            }
        }
        if !matches!(trigger, Some(TriggerKind::UsbClass(_))) {
            if let Some(class) = &usb.class {
                emit_property_rule(&mut content, "fuchsia.BIND_USB_CLASS", class)?;
            }
        }
        if let Some(subclass) = &usb.subclass {
            emit_property_rule(&mut content, "fuchsia.BIND_USB_SUBCLASS", subclass)?;
        }
        if let Some(protocol) = &usb.protocol {
            emit_property_rule(&mut content, "fuchsia.BIND_USB_PROTOCOL", protocol)?;
        }
        if let Some(interface_number) = &usb.interface_number {
            emit_property_rule(
                &mut content,
                "fuchsia.BIND_USB_INTERFACE_NUMBER",
                interface_number,
            )?;
        }
    }
    if let Some(node_name) = &bind.node_name {
        if !matches!(trigger, Some(TriggerKind::NodeName(_))) {
            emit_property_rule(&mut content, "fuchsia.NAME", node_name)?;
        }
    }
    if let Some(acpi) = &bind.acpi {
        if let Some(hid) = &acpi.hid {
            if !matches!(trigger, Some(TriggerKind::AcpiHid(_))) {
                emit_property_rule(&mut content, "fuchsia.acpi.HID", hid)?;
            }
        }
        if let Some(first_cid) = &acpi.first_cid {
            emit_property_rule(&mut content, "fuchsia.acpi.FIRST_CID", first_cid)?;
        }
        if let Some(bus) = &acpi.bus_type {
            if !matches!(trigger, Some(TriggerKind::AcpiBusType(_))) {
                emit_property_rule(&mut content, "fuchsia.BIND_ACPI_BUS_TYPE", bus)?;
            }
        }
    }
    if let Some(rules) = &bind.rules {
        let mut sorted_rules: Vec<_> = rules.iter().collect();
        sorted_rules.sort_unstable_by_key(|&(key, _)| key);
        for (key, val) in sorted_rules {
            if let Some(TriggerKind::Rule(ex_key, ex_val, _)) = trigger {
                if key == ex_key && val == ex_val {
                    continue;
                }
            }
            match val {
                Value::Array(arr) => {
                    content.push_str(&format!("accept {} {{\n", key));
                    for v in arr {
                        content.push_str(&format!("  {},\n", format_bind_val(v)?));
                    }
                    content.push_str("}\n");
                }
                Value::Object(map) => {
                    if let Some(neq_val) = map.get("neq") {
                        content.push_str(&format!("{} != {};\n", key, format_bind_val(neq_val)?));
                    } else {
                        return Err(anyhow!(
                            "Unsupported rule object operator for '{}': {:?} (expected 'neq')",
                            key,
                            map
                        ));
                    }
                }
                _ => {
                    content.push_str(&format!("{} == {};\n", key, format_bind_val(val)?));
                }
            }
        }
    }
    Ok(content)
}

fn generate_simple_bind_statements(bind: &DmlBind) -> Result<String, anyhow::Error> {
    generate_simple_bind_statements_excluding(bind, None)
}

enum TriggerKind {
    Protocol(String),
    Service(String),
    Compat(String),
    Vid(String),
    Pid(String),
    Did(String),
    PciVid(String),
    PciDid(String),
    PciClass(String),
    UsbVid(String),
    UsbPid(String),
    UsbClass(String),
    NodeName(String),
    AcpiHid(String),
    AcpiBusType(String),
    Rule(String, Value, String),
}

impl TriggerKind {
    fn condition_str(&self) -> &str {
        match self {
            TriggerKind::Protocol(s)
            | TriggerKind::Service(s)
            | TriggerKind::Compat(s)
            | TriggerKind::Vid(s)
            | TriggerKind::Pid(s)
            | TriggerKind::Did(s)
            | TriggerKind::PciVid(s)
            | TriggerKind::PciDid(s)
            | TriggerKind::PciClass(s)
            | TriggerKind::UsbVid(s)
            | TriggerKind::UsbPid(s)
            | TriggerKind::UsbClass(s)
            | TriggerKind::NodeName(s)
            | TriggerKind::AcpiHid(s)
            | TriggerKind::AcpiBusType(s)
            | TriggerKind::Rule(_, _, s) => s,
        }
    }
}

fn get_trigger(alt: &DmlBind) -> Result<Option<TriggerKind>, anyhow::Error> {
    if let Some(node_name) = &alt.node_name {
        if !node_name.is_array() {
            return Ok(Some(TriggerKind::NodeName(format!(
                "fuchsia.NAME == {}",
                format_bind_val(node_name)?
            ))));
        }
    }
    if let Some(acpi) = &alt.acpi {
        if let Some(hid) = &acpi.hid {
            if !hid.is_array() {
                return Ok(Some(TriggerKind::AcpiHid(format!(
                    "fuchsia.acpi.HID == {}",
                    format_bind_val(hid)?
                ))));
            }
        }
        if let Some(bus) = &acpi.bus_type {
            if !bus.is_array() {
                return Ok(Some(TriggerKind::AcpiBusType(format!(
                    "fuchsia.BIND_ACPI_BUS_TYPE == {}",
                    format_bind_val(bus)?
                ))));
            }
        }
    }
    if let Some(rules) = &alt.rules {
        if let Some(autobind_val) = rules.get("fuchsia.BIND_AUTOBIND") {
            if !autobind_val.is_object() && !autobind_val.is_array() {
                return Ok(Some(TriggerKind::Rule(
                    "fuchsia.BIND_AUTOBIND".to_string(),
                    autobind_val.clone(),
                    format!("fuchsia.BIND_AUTOBIND == {}", format_bind_val(autobind_val)?),
                )));
            }
        }
        if let Some(acpi_bus) = rules.get("fuchsia.BIND_ACPI_BUS_TYPE") {
            if !acpi_bus.is_object() && !acpi_bus.is_array() {
                return Ok(Some(TriggerKind::Rule(
                    "fuchsia.BIND_ACPI_BUS_TYPE".to_string(),
                    acpi_bus.clone(),
                    format!("fuchsia.BIND_ACPI_BUS_TYPE == {}", format_bind_val(acpi_bus)?),
                )));
            }
        }
    }
    if let Some(compat) = &alt.compat
        && !compat.is_array()
    {
        match compat {
            Value::String(s) => {
                return Ok(Some(TriggerKind::Compat(format!("fuchsia.COMPATIBLE == \"{}\"", s))));
            }
            _ => {}
        }
    }
    if let Some(rules) = &alt.rules {
        if let Some(hid) = rules.get("fuchsia.acpi.HID") {
            if !hid.is_object() && !hid.is_array() {
                return Ok(Some(TriggerKind::Rule(
                    "fuchsia.acpi.HID".to_string(),
                    hid.clone(),
                    format!("fuchsia.acpi.HID == {}", format_bind_val(hid)?),
                )));
            }
        }
    }
    if let Some(proto) = get_effective_protocol(alt) {
        Ok(Some(TriggerKind::Protocol(format!("fuchsia.BIND_PROTOCOL == {}", proto))))
    } else if let Some(banjo) = &alt.banjo {
        Ok(Some(TriggerKind::Protocol(format!("fuchsia.BIND_PROTOCOL == {}", banjo))))
    } else if let Some(svc) = &alt.service {
        Ok(Some(TriggerKind::Service(format!("fuchsia.Service == \"{}\"", svc))))
    } else if let Some(vid) = &alt.vid
        && !vid.is_array()
    {
        Ok(Some(TriggerKind::Vid(format!(
            "fuchsia.BIND_PLATFORM_DEV_VID == {}",
            format_bind_val(vid)?
        ))))
    } else if let Some(pid) = &alt.pid
        && !pid.is_array()
    {
        Ok(Some(TriggerKind::Pid(format!(
            "fuchsia.BIND_PLATFORM_DEV_PID == {}",
            format_bind_val(pid)?
        ))))
    } else if let Some(did) = &alt.did
        && !did.is_array()
    {
        Ok(Some(TriggerKind::Did(format!(
            "fuchsia.BIND_PLATFORM_DEV_DID == {}",
            format_bind_val(did)?
        ))))
    } else if let Some(pci) = &alt.pci {
        if let Some(vid) = &pci.vid
            && !vid.is_array()
        {
            Ok(Some(TriggerKind::PciVid(format!(
                "fuchsia.BIND_PCI_VID == {}",
                format_bind_val(vid)?
            ))))
        } else if let Some(did) = &pci.did
            && !did.is_array()
        {
            Ok(Some(TriggerKind::PciDid(format!(
                "fuchsia.BIND_PCI_DID == {}",
                format_bind_val(did)?
            ))))
        } else if let Some(class) = &pci.class
            && !class.is_array()
        {
            Ok(Some(TriggerKind::PciClass(format!(
                "fuchsia.BIND_PCI_CLASS == {}",
                format_bind_val(class)?
            ))))
        } else {
            Ok(None)
        }
    } else if let Some(usb) = &alt.usb {
        if let Some(vid) = &usb.vid
            && !vid.is_array()
        {
            Ok(Some(TriggerKind::UsbVid(format!(
                "fuchsia.BIND_USB_VID == {}",
                format_bind_val(vid)?
            ))))
        } else if let Some(pid) = &usb.pid
            && !pid.is_array()
        {
            Ok(Some(TriggerKind::UsbPid(format!(
                "fuchsia.BIND_USB_PID == {}",
                format_bind_val(pid)?
            ))))
        } else if let Some(class) = &usb.class
            && !class.is_array()
        {
            Ok(Some(TriggerKind::UsbClass(format!(
                "fuchsia.BIND_USB_CLASS == {}",
                format_bind_val(class)?
            ))))
        } else {
            Ok(None)
        }
    } else if let Some(rules) = &alt.rules {
        if let Some(name_val) = rules.get("fuchsia.NAME") {
            if !name_val.is_object() && !name_val.is_array() {
                return Ok(Some(TriggerKind::Rule(
                    "fuchsia.NAME".to_string(),
                    name_val.clone(),
                    format!("fuchsia.NAME == {}", format_bind_val(name_val)?),
                )));
            }
        }
        Ok(None)
    } else {
        Ok(None)
    }
}

fn generate_simple_bind_rules(bind: &DmlBind) -> Result<String, anyhow::Error> {
    let mut content = String::new();
    if let Some(alternatives) = &bind.one_of {
        content.push_str(&generate_simple_bind_statements(bind)?);

        let mut last_had_trigger = false;
        let num_alts = alternatives.len();
        for (i, alt) in alternatives.iter().enumerate() {
            let is_last = i == num_alts - 1 && num_alts > 1;
            let trigger_opt = if is_last { None } else { get_trigger(alt)? };
            let has_trigger = trigger_opt.is_some();

            if i == 0 {
                if let Some(trigger) = &trigger_opt {
                    content.push_str(&format!("if {} {{\n", trigger.condition_str()));
                    last_had_trigger = true;
                } else {
                    return generate_simple_bind_statements(alt);
                }
            } else {
                if let Some(trigger) = &trigger_opt {
                    content.push_str(&format!("}} else if {} {{\n", trigger.condition_str()));
                    last_had_trigger = true;
                } else {
                    content.push_str("} else {\n");
                    last_had_trigger = false;
                }
            }

            let statements = generate_simple_bind_statements_excluding(alt, trigger_opt.as_ref())?;

            if statements.trim().is_empty() {
                content.push_str("    true;\n");
            } else {
                for line in statements.lines() {
                    if !line.trim().is_empty() {
                        content.push_str(&format!("    {}\n", line));
                    }
                }
            }

            if !has_trigger {
                break;
            }
        }
        if last_had_trigger {
            content.push_str("} else {\n    false;\n}\n");
        } else {
            content.push_str("}\n");
        }
    } else {
        content.push_str(&generate_simple_bind_statements(bind)?);
    }
    Ok(content)
}

pub fn generate_bind_file(
    driver_name: &str,
    bind: &DmlBind,
    additional_parents: &[AdditionalParentInfo],
    year: &str,
) -> Result<String, anyhow::Error> {
    let mut content = String::new();

    let is_composite = bind.primary.is_some() || !additional_parents.is_empty();

    if is_composite {
        let comp_name = bind.composite_name.as_deref().unwrap_or(driver_name).replace("-", "_");
        content.push_str(&format!("composite {};\n\n", comp_name));
        content.push_str("using fuchsia;\n\n");

        if let Some(primary) = &bind.primary {
            content.push_str(&format!("primary parent \"{}\" {{\n", primary.node));

            let dml_bind = DmlBind {
                protocol: primary.protocol.clone(),
                service: primary.service.clone(),
                banjo: primary.banjo.clone(),
                transport: primary.transport.clone(),
                vid: primary.vid.clone(),
                pid: primary.pid.clone(),
                did: primary.did.clone(),
                compat: primary.compat.clone(),
                pci: primary.pci.clone(),
                usb: primary.usb.clone(),
                acpi: primary.acpi.clone(),
                node_name: primary.node_name.clone(),
                match_name: primary.match_name,
                primary: None,
                one_of: primary.one_of.clone(),
                rules: primary.rules.clone(),
                composite_name: None,
            };
            let rules_str = generate_simple_bind_rules(&dml_bind)?;
            if rules_str.trim().is_empty() {
                content.push_str(
                    "  fuchsia.BIND_PROTOCOL == fuchsia.platform.BIND_PROTOCOL.DEVICE;\n",
                );
            } else {
                for line in rules_str.lines() {
                    if !line.trim().is_empty() {
                        content.push_str(&format!("  {}\n", line));
                    }
                }
            }
            content.push_str("}\n\n");
        }

        let primary_node_name = bind.primary.as_ref().map(|p| p.node.as_str());
        let mut grouped_parents = std::collections::BTreeMap::<
            String,
            (Vec<(Option<String>, Option<String>, String)>, bool, Option<DmlBind>),
        >::new();
        for parent in additional_parents {
            if Some(parent.parent_name.as_str()) == primary_node_name {
                continue;
            }
            let entry = grouped_parents
                .entry(parent.parent_name.clone())
                .or_insert_with(|| (Vec::new(), true, None));
            entry.0.push((
                parent.service_name.clone(),
                parent.banjo_name.clone(),
                parent.transport.clone(),
            ));
            entry.1 = entry.1 && parent.optional;
            if parent.bind.is_some() && entry.2.is_none() {
                entry.2 = parent.bind.clone();
            }
        }

        for (parent_name, (capabilities, optional, parent_bind)) in grouped_parents {
            let prefix = if optional { "optional " } else { "" };
            content.push_str(&format!("{}parent \"{}\" {{\n", prefix, parent_name));
            let mut sorted_capabilities: Vec<_> = capabilities.iter().collect();
            sorted_capabilities.sort_unstable();
            for (service_name, banjo_name, _transport) in sorted_capabilities {
                if let Some(banjo_name) = banjo_name {
                    content.push_str(&format!("  fuchsia.BIND_PROTOCOL == {};\n", banjo_name));
                }
                if let Some(service_name) = service_name {
                    if let Some(rule) =
                        crate::workarounds::try_generate_init_step_bind_rule(&service_name)
                    {
                        content.push_str(&rule);
                    } else {
                        content.push_str(&format!("  fuchsia.Service == \"{}\";\n", service_name));
                    }
                }
            }
            if let Some(bind_rules) = parent_bind {
                let rules_str = generate_simple_bind_rules(&bind_rules)?;
                for line in rules_str.lines() {
                    if !line.trim().is_empty() {
                        content.push_str(&format!("  {}\n", line));
                    }
                }
            }
            content.push_str("}\n\n");
        }
    } else {
        let rules_str = generate_simple_bind_rules(bind)?;
        if rules_str.trim().is_empty() {
            content.push_str("true;\n");
        } else {
            content.push_str(&rules_str);
        }
    }
    let mut header = format!(
        r#"// Copyright {} The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// WARNING: THIS FILE IS GENERATED BY dmlc. DO NOT EDIT.

"#,
        year
    );
    if !is_composite && content.contains("fuchsia.Service") {
        header.push_str("using fuchsia;\n\n");
    }
    content.insert_str(0, &header);
    Ok(content)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_bind_file_composite_grouping() {
        let bind = DmlBind {
            primary: Some(crate::parser::BindPrimary {
                node: "pdev".to_string(),
                compat: Some(Value::String("fuchsia,gpio-buttons".to_string())),
                ..Default::default()
            }),
            ..Default::default()
        };

        let additional_parents = vec![
            AdditionalParentInfo {
                parent_name: "gpio-init".to_string(),
                service_name: Some("fuchsia.gpio.Init".to_string()),
                banjo_name: None,
                transport: "Driver".to_string(),
                optional: false,
                bind: None,
            },
            AdditionalParentInfo {
                parent_name: "gpio-init".to_string(),
                service_name: Some("fuchsia.hardware.gpio.Service".to_string()),
                banjo_name: None,
                transport: "Driver".to_string(),
                optional: true,
                bind: None,
            },
            AdditionalParentInfo {
                parent_name: "pwm-init".to_string(),
                service_name: Some("fuchsia.pwm.Init".to_string()),
                banjo_name: None,
                transport: "Driver".to_string(),
                optional: true,
                bind: None,
            },
        ];

        let content = generate_bind_file("buttons", &bind, &additional_parents, "2026").unwrap();

        assert!(content.contains("using fuchsia;"));

        let expected_gpio_init = "parent \"gpio-init\" {\n  fuchsia.BIND_INIT_STEP == fuchsia.gpio.BIND_INIT_STEP.GPIO;\n  fuchsia.Service == \"fuchsia.hardware.gpio.Service\";\n}";
        let expected_pwm_init = "optional parent \"pwm-init\" {\n  fuchsia.BIND_INIT_STEP == fuchsia.pwm.BIND_INIT_STEP.PWM;\n}";

        assert!(
            content.contains(expected_gpio_init),
            "Expected:\n{}\n\nGot:\n{}",
            expected_gpio_init,
            content
        );
        assert!(
            content.contains(expected_pwm_init),
            "Expected:\n{}\n\nGot:\n{}",
            expected_pwm_init,
            content
        );
    }

    #[test]
    fn test_generate_bind_file_simple_one_of() {
        let bind = DmlBind {
            one_of: Some(vec![
                DmlBind { vid: Some(Value::Number(125.into())), ..Default::default() },
                DmlBind {
                    compat: Some(Value::String("fuchsia,my-compat".to_string())),
                    ..Default::default()
                },
            ]),
            ..Default::default()
        };

        let content = generate_bind_file("my_driver", &bind, &[], "2025").unwrap();

        assert!(content.contains("// Copyright 2025 The Fuchsia Authors. All rights reserved."));
        let expected = "if fuchsia.BIND_PLATFORM_DEV_VID == 125 {\n    true;\n} else {\n    fuchsia.COMPATIBLE == \"fuchsia,my-compat\";\n}";
        assert!(content.contains(expected), "Expected:\n{}\n\nGot:\n{}", expected, content);
    }

    #[test]
    fn test_generate_bind_file_simple_one_of_array_compat() {
        let bind = DmlBind {
            one_of: Some(vec![
                DmlBind { vid: Some(Value::Number(125.into())), ..Default::default() },
                DmlBind {
                    compat: Some(Value::Array(vec![Value::String(
                        "fuchsia,my-compat".to_string(),
                    )])),
                    ..Default::default()
                },
            ]),
            ..Default::default()
        };

        let content = generate_bind_file("my_driver", &bind, &[], "2026").unwrap();

        assert!(content.contains("// Copyright 2026 The Fuchsia Authors. All rights reserved."));
        let expected = "if fuchsia.BIND_PLATFORM_DEV_VID == 125 {\n    true;\n} else {\n    accept fuchsia.COMPATIBLE {\n      \"fuchsia,my-compat\",\n    }\n}";
        assert!(content.contains(expected), "Expected:\n{}\n\nGot:\n{}", expected, content);
    }

    #[test]
    fn test_generate_bind_file_name_branching_and_fallback_else() {
        let mut rules_adc0 = std::collections::HashMap::new();
        rules_adc0.insert("fuchsia.NAME".to_string(), Value::String("adc-0".to_string()));

        let mut rules_chan = std::collections::HashMap::new();
        rules_chan.insert("fuchsia.adc.CHANNEL".to_string(), Value::Number(0.into()));

        let bind = DmlBind {
            one_of: Some(vec![
                DmlBind { rules: Some(rules_adc0), ..Default::default() },
                DmlBind { rules: Some(rules_chan), ..Default::default() },
            ]),
            ..Default::default()
        };

        let content = generate_bind_file("aml_thermistor", &bind, &[], "2026").unwrap();
        assert!(content.contains(
            "if fuchsia.NAME == \"adc-0\" {\n    true;\n} else {\n    fuchsia.adc.CHANNEL == 0;\n}"
        ));
    }

    #[test]
    fn test_generate_bind_file_composite_name_override() {
        let bind = DmlBind {
            composite_name: Some("custom_composite_name".to_string()),
            primary: Some(crate::parser::BindPrimary {
                node: "pdev".to_string(),
                compat: Some(Value::String("fuchsia,custom".to_string())),
                ..Default::default()
            }),
            ..Default::default()
        };

        let content = generate_bind_file("original_driver_name", &bind, &[], "2026").unwrap();
        assert!(content.contains("composite custom_composite_name;\n"));
    }

    #[test]
    fn test_generate_bind_file_non_equality_rule() {
        let mut rules = std::collections::HashMap::new();
        let mut neq_map = serde_json::Map::new();
        neq_map.insert("neq".to_string(), Value::Number(1.into()));
        rules.insert("fuchsia.BIND_COMPOSITE".to_string(), Value::Object(neq_map));

        let bind = DmlBind { rules: Some(rules), ..Default::default() };

        let content = generate_bind_file("serial", &bind, &[], "2026").unwrap();
        assert!(content.contains("fuchsia.BIND_COMPOSITE != 1;\n"));
    }

    #[test]
    fn test_generate_bind_file_empty_bind() {
        let bind = DmlBind::default();
        let content = generate_bind_file("driver_serve_fidl", &bind, &[], "2026").unwrap();
        assert!(content.contains("// Copyright 2026 The Fuchsia Authors. All rights reserved."));
        assert!(content.contains("true;\n"), "Expected true; in content:\n{}", content);
    }

    #[test]
    fn test_generate_bind_file_banjo_capability() {
        let bind = DmlBind {
            primary: Some(crate::parser::BindPrimary {
                node: "pdev".to_string(),
                banjo: Some("fuchsia.platform.BIND_PROTOCOL.DEVICE".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let additional = vec![AdditionalParentInfo {
            parent_name: "gpio".to_string(),
            service_name: None,
            banjo_name: Some("fuchsia.gpio.BIND_PROTOCOL.DEVICE".to_string()),
            transport: "Banjo".to_string(),
            optional: false,
            bind: None,
        }];

        let content = generate_bind_file("my_driver", &bind, &additional, "2026").unwrap();
        assert!(content.contains("primary parent \"pdev\" {\n  fuchsia.BIND_PROTOCOL == fuchsia.platform.BIND_PROTOCOL.DEVICE;\n}"));
        assert!(content.contains(
            "parent \"gpio\" {\n  fuchsia.BIND_PROTOCOL == fuchsia.gpio.BIND_PROTOCOL.DEVICE;\n}"
        ));
    }

    #[test]
    fn test_generate_bind_file_pci_block() {
        let bind = DmlBind {
            pci: Some(crate::parser::PciBind {
                vid: Some(Value::String("fuchsia.pci.BIND_PCI_VID.INTEL".to_string())),
                did: Some(Value::String("0x1234".to_string())),
                class: Some(Value::String("0x01".to_string())),
                subclass: Some(Value::String("0x02".to_string())),
                interface: Some(Value::String("0x03".to_string())),
                revision: Some(Value::String("0x04".to_string())),
                topo: Some(Value::String("0x05".to_string())),
            }),
            ..Default::default()
        };
        let content = generate_bind_file("pci_driver", &bind, &[], "2026").unwrap();
        assert!(content.contains("fuchsia.BIND_PCI_VID == fuchsia.pci.BIND_PCI_VID.INTEL;\n"));
        assert!(content.contains("fuchsia.BIND_PCI_DID == 0x1234;\n"));
        assert!(content.contains("fuchsia.BIND_PCI_CLASS == 0x01;\n"));
        assert!(content.contains("fuchsia.BIND_PCI_SUBCLASS == 0x02;\n"));
        assert!(content.contains("fuchsia.BIND_PCI_INTERFACE == 0x03;\n"));
        assert!(content.contains("fuchsia.BIND_PCI_REVISION == 0x04;\n"));
        assert!(content.contains("fuchsia.BIND_PCI_TOPO == 0x05;\n"));
    }

    #[test]
    fn test_generate_bind_file_pci_accept_array() {
        let bind = DmlBind {
            pci: Some(crate::parser::PciBind {
                vid: Some(Value::String("fuchsia.pci.BIND_PCI_VID.INTEL".to_string())),
                did: Some(Value::Array(vec![
                    Value::String("0x1234".to_string()),
                    Value::String("0x5678".to_string()),
                ])),
                ..Default::default()
            }),
            ..Default::default()
        };
        let content = generate_bind_file("pci_driver", &bind, &[], "2026").unwrap();
        assert!(content.contains("fuchsia.BIND_PCI_VID == fuchsia.pci.BIND_PCI_VID.INTEL;\n"));
        assert!(content.contains("accept fuchsia.BIND_PCI_DID {\n  0x1234,\n  0x5678,\n}\n"));
    }

    #[test]
    fn test_generate_bind_file_usb_block() {
        let bind = DmlBind {
            usb: Some(crate::parser::UsbBind {
                vid: Some(Value::String("fuchsia.usb.BIND_USB_VID.GOOGLE".to_string())),
                pid: Some(Value::String("0x1234".to_string())),
                class: Some(Value::String("0x01".to_string())),
                subclass: Some(Value::String("0x02".to_string())),
                protocol: Some(Value::Number(0.into())),
                interface_number: Some(Value::Number(1.into())),
                bind_protocol: None,
            }),
            ..Default::default()
        };
        let content = generate_bind_file("usb_driver", &bind, &[], "2026").unwrap();
        assert!(content.contains("fuchsia.BIND_USB_VID == fuchsia.usb.BIND_USB_VID.GOOGLE;\n"));
        assert!(content.contains("fuchsia.BIND_USB_PID == 0x1234;\n"));
        assert!(content.contains("fuchsia.BIND_USB_CLASS == 0x01;\n"));
        assert!(content.contains("fuchsia.BIND_USB_SUBCLASS == 0x02;\n"));
        assert!(content.contains("fuchsia.BIND_USB_PROTOCOL == 0;\n"));
        assert!(content.contains("fuchsia.BIND_USB_INTERFACE_NUMBER == 1;\n"));
    }

    #[test]
    fn test_generate_bind_file_usb_accept_array() {
        let bind = DmlBind {
            usb: Some(crate::parser::UsbBind {
                vid: Some(Value::String("fuchsia.usb.BIND_USB_VID.GOOGLE".to_string())),
                pid: Some(Value::Array(vec![
                    Value::String("0x1234".to_string()),
                    Value::String("0x5678".to_string()),
                ])),
                ..Default::default()
            }),
            ..Default::default()
        };
        let content = generate_bind_file("usb_driver", &bind, &[], "2026").unwrap();
        assert!(content.contains("fuchsia.BIND_USB_VID == fuchsia.usb.BIND_USB_VID.GOOGLE;\n"));
        assert!(content.contains("accept fuchsia.BIND_USB_PID {\n  0x1234,\n  0x5678,\n}\n"));
    }

    #[test]
    fn test_generate_bind_file_one_of_pci_triggers() {
        let bind = DmlBind {
            one_of: Some(vec![
                DmlBind {
                    pci: Some(crate::parser::PciBind {
                        vid: Some(Value::String("fuchsia.pci.BIND_PCI_VID.INTEL".to_string())),
                        did: Some(Value::String("0x1234".to_string())),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                DmlBind {
                    pci: Some(crate::parser::PciBind {
                        class: Some(Value::String("0x02".to_string())),
                        subclass: Some(Value::String("0x00".to_string())),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            ]),
            ..Default::default()
        };
        let content = generate_bind_file("pci_driver", &bind, &[], "2026").unwrap();
        assert!(content.contains("if fuchsia.BIND_PCI_VID == fuchsia.pci.BIND_PCI_VID.INTEL {\n    fuchsia.BIND_PCI_DID == 0x1234;\n} else {\n    fuchsia.BIND_PCI_CLASS == 0x02;\n    fuchsia.BIND_PCI_SUBCLASS == 0x00;\n}"));
    }

    #[test]
    fn test_generate_bind_file_one_of_usb_triggers() {
        let bind = DmlBind {
            one_of: Some(vec![
                DmlBind {
                    usb: Some(crate::parser::UsbBind {
                        vid: Some(Value::String("fuchsia.usb.BIND_USB_VID.GOOGLE".to_string())),
                        pid: Some(Value::String("0x1234".to_string())),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                DmlBind {
                    usb: Some(crate::parser::UsbBind {
                        pid: Some(Value::String("0x5678".to_string())),
                        class: Some(Value::String("0x03".to_string())),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            ]),
            ..Default::default()
        };
        let content = generate_bind_file("usb_driver", &bind, &[], "2026").unwrap();
        assert!(content.contains("if fuchsia.BIND_USB_VID == fuchsia.usb.BIND_USB_VID.GOOGLE {\n    fuchsia.BIND_USB_PID == 0x1234;\n} else {\n    fuchsia.BIND_USB_PID == 0x5678;\n    fuchsia.BIND_USB_CLASS == 0x03;\n}"));
    }

    #[test]
    fn test_generate_bind_file_usb_bind_protocol() {
        let bind = DmlBind {
            usb: Some(crate::parser::UsbBind {
                bind_protocol: Some("fuchsia.usb.BIND_PROTOCOL.INTERFACE".to_string()),
                class: Some(Value::String("0x01".to_string())),
                ..Default::default()
            }),
            ..Default::default()
        };
        let content = generate_bind_file("usb_driver", &bind, &[], "2026").unwrap();
        assert!(
            content.contains("fuchsia.BIND_PROTOCOL == fuchsia.usb.BIND_PROTOCOL.INTERFACE;\n")
        );
        assert!(content.contains("fuchsia.BIND_USB_CLASS == 0x01;\n"));
    }

    #[test]
    fn test_generate_bind_file_node_name() {
        let bind = DmlBind {
            node_name: Some(Value::String("mic-mute".to_string())),
            ..Default::default()
        };
        let content = generate_bind_file("node_driver", &bind, &[], "2026").unwrap();
        assert!(content.contains("fuchsia.NAME == \"mic-mute\";\n"));
    }

    #[test]
    fn test_generate_bind_file_one_of_node_name() {
        let bind = DmlBind {
            one_of: Some(vec![
                DmlBind {
                    node_name: Some(Value::String("node-a".to_string())),
                    vid: Some(Value::Number(1.into())),
                    ..Default::default()
                },
                DmlBind {
                    node_name: Some(Value::String("node-b".to_string())),
                    vid: Some(Value::Number(2.into())),
                    ..Default::default()
                },
            ]),
            ..Default::default()
        };
        let content = generate_bind_file("node_driver", &bind, &[], "2026").unwrap();
        let expected = "if fuchsia.NAME == \"node-a\" {\n    fuchsia.BIND_PLATFORM_DEV_VID == 1;\n} else {\n    fuchsia.BIND_PLATFORM_DEV_VID == 2;\n    fuchsia.NAME == \"node-b\";\n}";
        assert!(content.contains(expected), "Expected:\n{}\n\nGot:\n{}", expected, content);
    }

    #[test]
    fn test_generate_bind_file_acpi_block() {
        let bind = DmlBind {
            acpi: Some(crate::parser::AcpiBind {
                hid: Some(Value::String("PNP0C0A".to_string())),
                first_cid: Some(Value::String("PNP0C0B".to_string())),
                bus_type: Some(Value::String("fuchsia.acpi.BIND_ACPI_BUS_TYPE.PCI".to_string())),
            }),
            ..Default::default()
        };
        let content = generate_bind_file("acpi_driver", &bind, &[], "2026").unwrap();
        assert!(content.contains("fuchsia.acpi.HID == \"PNP0C0A\";\n"));
        assert!(content.contains("fuchsia.acpi.FIRST_CID == \"PNP0C0B\";\n"));
        assert!(
            content
                .contains("fuchsia.BIND_ACPI_BUS_TYPE == fuchsia.acpi.BIND_ACPI_BUS_TYPE.PCI;\n")
        );
    }

    #[test]
    fn test_generate_bind_file_acpi_accept_array_hid() {
        let bind = DmlBind {
            acpi: Some(crate::parser::AcpiBind {
                hid: Some(Value::Array(vec![
                    Value::String("PNP0C0A".to_string()),
                    Value::String("PNP0C0B".to_string()),
                ])),
                ..Default::default()
            }),
            ..Default::default()
        };
        let content = generate_bind_file("acpi_driver", &bind, &[], "2026").unwrap();
        assert!(content.contains("accept fuchsia.acpi.HID {\n  \"PNP0C0A\",\n  \"PNP0C0B\",\n}\n"));
    }

    #[test]
    fn test_generate_bind_file_one_of_acpi_triggers() {
        let bind = DmlBind {
            one_of: Some(vec![
                DmlBind {
                    acpi: Some(crate::parser::AcpiBind {
                        hid: Some(Value::String("PNP0C0A".to_string())),
                        ..Default::default()
                    }),
                    vid: Some(Value::Number(10.into())),
                    ..Default::default()
                },
                DmlBind {
                    acpi: Some(crate::parser::AcpiBind {
                        bus_type: Some(Value::String(
                            "fuchsia.acpi.BIND_ACPI_BUS_TYPE.I2C".to_string(),
                        )),
                        ..Default::default()
                    }),
                    vid: Some(Value::Number(20.into())),
                    ..Default::default()
                },
            ]),
            ..Default::default()
        };
        let content = generate_bind_file("acpi_driver", &bind, &[], "2026").unwrap();
        let expected = "if fuchsia.acpi.HID == \"PNP0C0A\" {\n    fuchsia.BIND_PLATFORM_DEV_VID == 10;\n} else {\n    fuchsia.BIND_PLATFORM_DEV_VID == 20;\n    fuchsia.BIND_ACPI_BUS_TYPE == fuchsia.acpi.BIND_ACPI_BUS_TYPE.I2C;\n}";
        assert!(content.contains(expected), "Expected:\n{}\n\nGot:\n{}", expected, content);
    }
}
