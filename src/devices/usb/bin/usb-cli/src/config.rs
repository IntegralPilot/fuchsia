// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Error, anyhow, bail};
use fidl_fuchsia_hardware_usb_peripheral as peripheral;
use serde::{Deserialize, Serialize};
use std::fs;

pub const MAX_CONFIGURATIONS: usize = peripheral::MAX_CONFIG_DESCRIPTORS as usize;
pub const MAX_FUNCTIONS: usize = 8;
pub const MAX_FUNCTION_NAME_LEN: usize = 32;

// Communication Class (CDC / RNDIS)
pub const USB_CLASS_COMM: u8 = 0x02;
pub const USB_CDC_SUBCLASS_ACM: u8 = 0x02;
pub const USB_CDC_SUBCLASS_ETHERNET: u8 = 0x06;
pub const USB_CDC_PROTOCOL_NONE: u8 = 0x00;
pub const USB_PROTOCOL_RNDIS: u8 = 0xff;

// Mass Storage Class (UMS)
pub const USB_CLASS_MSC: u8 = 0x08;
pub const USB_SUBCLASS_MSC_SCSI: u8 = 0x06;
pub const USB_PROTOCOL_MSC_BULK_ONLY: u8 = 0x50;

// Vendor Specific Class (Test, ADB, VSOCK)
pub const USB_CLASS_VENDOR: u8 = 0xff;
pub const USB_SUBCLASS_TEST: u8 = 0x00;
pub const USB_SUBCLASS_ADB: u8 = 0x42;
pub const USB_PROTOCOL_ADB: u8 = 0x01;
pub const USB_SUBCLASS_VSOCK: u8 = 0x43;
pub const USB_PROTOCOL_VSOCK: u8 = 0x00;

// LINT.IfChange
// Standard Google USB Vendor and Product IDs (matching peripheral.h)
pub const GOOGLE_USB_VID: u16 = 0x18d1;
pub const GOOGLE_USB_CDC_PID: u16 = 0xa020;
pub const GOOGLE_USB_UMS_PID: u16 = 0xa021;
pub const GOOGLE_USB_FUNCTION_TEST_PID: u16 = 0xa022;
pub const GOOGLE_USB_CDC_AND_FUNCTION_TEST_PID: u16 = 0xa023;
pub const GOOGLE_USB_RNDIS_PID: u16 = 0xa024;
pub const GOOGLE_USB_ADB_PID: u16 = 0xa025;
pub const GOOGLE_USB_CDC_AND_ADB_PID: u16 = 0xa026;
pub const GOOGLE_USB_CDC_AND_FASTBOOT_PID: u16 = 0xa027;
pub const GOOGLE_USB_VSOCK_BRIDGE_PID: u16 = 0xa028;
pub const GOOGLE_USB_CDC_AND_VSOCK_BRIDGE_PID: u16 = 0xa029;
pub const GOOGLE_USB_ADB_AND_VSOCK_BRIDGE_PID: u16 = 0xa02a;
pub const GOOGLE_USB_CDC_AND_ADB_AND_VSOCK_BRIDGE_PID: u16 = 0xa02b;
pub const GOOGLE_USB_CDC_AND_ADB_AND_FASTBOOT_PID: u16 = 0xa02c;
pub const GOOGLE_USB_FASTBOOT_PID: u16 = 0x4ee0;
// LINT.ThenChange(//src/devices/usb/lib/usb/include/usb/peripheral.h)

bitflags::bitflags! {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub struct FunctionTag: u8 {
        const CDC = 1 << 0;
        const UMS = 1 << 1;
        const RNDIS = 1 << 2;
        const ADB = 1 << 3;
        const FASTBOOT = 1 << 4;
        const TEST = 1 << 5;
        const VSOCK = 1 << 6;
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UsbConfigJson {
    pub configurations: Vec<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id_vendor: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id_product: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub product: Option<String>,
}

impl UsbConfigJson {
    pub fn from_configurations(configurations: Vec<Vec<String>>) -> Self {
        Self { configurations, id_vendor: None, id_product: None, product: None }
    }
}

pub const USB_PROTOCOL_SOURCESINK: u8 = 0x01;
pub const USB_PROTOCOL_LOOPBACK: u8 = 0x02;

pub fn function_name_to_descriptor(name: &str) -> Result<peripheral::FunctionDescriptor, Error> {
    let lower = name.to_ascii_lowercase();
    match lower.as_str() {
        "cdc" | "cdc_eth" | "cdc-eth" => Ok(peripheral::FunctionDescriptor {
            interface_class: USB_CLASS_COMM,
            interface_subclass: USB_CDC_SUBCLASS_ETHERNET,
            interface_protocol: USB_CDC_PROTOCOL_NONE,
        }),
        "rndis" => Ok(peripheral::FunctionDescriptor {
            interface_class: USB_CLASS_COMM,
            interface_subclass: USB_CDC_SUBCLASS_ACM,
            interface_protocol: USB_PROTOCOL_RNDIS,
        }),
        "ums" | "mass_storage" => Ok(peripheral::FunctionDescriptor {
            interface_class: USB_CLASS_MSC,
            interface_subclass: USB_SUBCLASS_MSC_SCSI,
            interface_protocol: USB_PROTOCOL_MSC_BULK_ONLY,
        }),
        "sourcesink" | "source_sink" | "source-sink" => Ok(peripheral::FunctionDescriptor {
            interface_class: USB_CLASS_VENDOR,
            interface_subclass: USB_SUBCLASS_TEST,
            interface_protocol: USB_PROTOCOL_SOURCESINK,
        }),
        "loopback" => Ok(peripheral::FunctionDescriptor {
            interface_class: USB_CLASS_VENDOR,
            interface_subclass: USB_SUBCLASS_TEST,
            interface_protocol: USB_PROTOCOL_LOOPBACK,
        }),
        "adb" => Ok(peripheral::FunctionDescriptor {
            interface_class: USB_CLASS_VENDOR,
            interface_subclass: USB_SUBCLASS_ADB,
            interface_protocol: USB_PROTOCOL_ADB,
        }),
        "vsock" | "ffx" => Ok(peripheral::FunctionDescriptor {
            interface_class: USB_CLASS_VENDOR,
            interface_subclass: USB_SUBCLASS_VSOCK,
            interface_protocol: USB_PROTOCOL_VSOCK,
        }),
        _ => {
            if let Some(spec) = lower.strip_prefix("custom_") {
                let mut parts = spec.split('_');
                if let (Some(cls_str), Some(sub_str), Some(proto_str), None) =
                    (parts.next(), parts.next(), parts.next(), parts.next())
                {
                    fn parse_hex(s: &str) -> Result<u8, Error> {
                        let hex_str = s.strip_prefix("0x").unwrap_or(s);
                        u8::from_str_radix(hex_str, 16)
                            .with_context(|| format!("Invalid hex byte '{}'", s))
                    }
                    let cls = parse_hex(cls_str)?;
                    let sub = parse_hex(sub_str)?;
                    let proto = parse_hex(proto_str)?;
                    return Ok(peripheral::FunctionDescriptor {
                        interface_class: cls,
                        interface_subclass: sub,
                        interface_protocol: proto,
                    });
                }
                bail!("Invalid custom function descriptor format: {}", name);
            }
            Err(anyhow!("Unknown USB function: {}", name))
        }
    }
}

pub fn descriptor_to_function_name(desc: &peripheral::FunctionDescriptor) -> String {
    match (desc.interface_class, desc.interface_subclass, desc.interface_protocol) {
        (USB_CLASS_COMM, USB_CDC_SUBCLASS_ETHERNET, USB_CDC_PROTOCOL_NONE) => "cdc".to_string(),
        (USB_CLASS_COMM, USB_CDC_SUBCLASS_ACM, USB_PROTOCOL_RNDIS) => "rndis".to_string(),
        (USB_CLASS_MSC, USB_SUBCLASS_MSC_SCSI, USB_PROTOCOL_MSC_BULK_ONLY) => "ums".to_string(),
        (USB_CLASS_VENDOR, USB_SUBCLASS_TEST, USB_PROTOCOL_SOURCESINK) => "sourcesink".to_string(),
        (USB_CLASS_VENDOR, USB_SUBCLASS_TEST, USB_PROTOCOL_LOOPBACK) => "loopback".to_string(),
        (USB_CLASS_VENDOR, USB_SUBCLASS_TEST, _) => "sourcesink".to_string(),
        (USB_CLASS_VENDOR, USB_SUBCLASS_ADB, USB_PROTOCOL_ADB) => "adb".to_string(),
        (USB_CLASS_VENDOR, USB_SUBCLASS_VSOCK, USB_PROTOCOL_VSOCK) => "vsock".to_string(),
        (cls, sub, proto) => format!("custom_0x{:02x}_0x{:02x}_0x{:02x}", cls, sub, proto),
    }
}

pub fn config_descriptors_to_names(
    descriptors: &[Vec<peripheral::FunctionDescriptor>],
) -> Vec<Vec<String>> {
    descriptors.iter().map(|cfg| cfg.iter().map(descriptor_to_function_name).collect()).collect()
}

fn validate_functions(funcs: &[String], config_idx: Option<usize>) -> Result<(), Error> {
    let config_str = config_idx.map_or(String::new(), |i| format!("{} ", i));

    if funcs.is_empty() {
        bail!("Configuration {}must specify at least one USB function", config_str);
    }
    if funcs.len() > MAX_FUNCTIONS {
        bail!(
            "Configuration {}exceeds maximum of {} USB functions (got {})",
            config_str,
            MAX_FUNCTIONS,
            funcs.len()
        );
    }
    if let Some(func) = funcs.iter().find(|f| f.len() > MAX_FUNCTION_NAME_LEN) {
        let config_in_str = config_idx.map_or(String::new(), |i| format!(" in config {}", i));
        bail!(
            "Function name '{}'{} exceeds maximum allowed length of {} bytes",
            func,
            config_in_str,
            MAX_FUNCTION_NAME_LEN
        );
    }
    Ok(())
}

fn validate_config(config: UsbConfigJson) -> Result<UsbConfigJson, Error> {
    if config.configurations.is_empty() {
        bail!("Configuration must specify at least one configuration");
    }
    if config.configurations.len() > MAX_CONFIGURATIONS {
        bail!(
            "Configuration exceeds maximum of {} configurations (got {})",
            MAX_CONFIGURATIONS,
            config.configurations.len()
        );
    }
    for (i, cfg) in config.configurations.iter().enumerate() {
        validate_functions(cfg, Some(i))?;
    }
    if config.id_vendor == Some(0) {
        bail!("USB Vendor ID (id_vendor) cannot be 0");
    }
    Ok(config)
}

pub fn load_config_input(input: &str) -> Result<UsbConfigJson, Error> {
    let input = input.trim();
    if input.is_empty() {
        bail!("Configuration input cannot be empty");
    }

    let config = if input.starts_with('{') {
        serde_json::from_str::<UsbConfigJson>(input).context("Failed to parse JSON string")?
    } else if input.ends_with(".json")
        || input.contains('/')
        || std::path::Path::new(input).is_file()
    {
        let content = fs::read_to_string(input)
            .with_context(|| format!("Failed to read JSON config file '{}'", input))?;
        serde_json::from_str::<UsbConfigJson>(&content)
            .with_context(|| format!("Failed to parse JSON file '{}'", input))?
    } else if input.contains(';') {
        let configurations: Vec<Vec<String>> = input
            .split(';')
            .map(|cfg| {
                cfg.split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .collect();
        UsbConfigJson::from_configurations(configurations)
    } else {
        // Treat as comma-separated function list for a single configuration
        let functions: Vec<String> =
            input.split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect();
        if functions.is_empty() {
            bail!("Configuration must specify at least one USB function");
        }
        UsbConfigJson::from_configurations(vec![functions])
    };

    validate_config(config)
}

fn is_test_function(f: &str) -> bool {
    f.eq_ignore_ascii_case("sourcesink")
        || f.eq_ignore_ascii_case("source_sink")
        || f.eq_ignore_ascii_case("source-sink")
        || f.eq_ignore_ascii_case("loopback")
}

fn function_tag(func: &str) -> Option<FunctionTag> {
    let lower = func.to_ascii_lowercase();
    match lower.as_str() {
        "cdc" | "cdc_eth" | "cdc-eth" => Some(FunctionTag::CDC),
        "ums" | "mass_storage" => Some(FunctionTag::UMS),
        "rndis" => Some(FunctionTag::RNDIS),
        "adb" => Some(FunctionTag::ADB),
        "fastboot" => Some(FunctionTag::FASTBOOT),
        "vsock" | "ffx" => Some(FunctionTag::VSOCK),
        _ if is_test_function(func) => Some(FunctionTag::TEST),
        _ => None,
    }
}

pub fn derive_pid_and_product(config: &UsbConfigJson) -> Option<(u16, String)> {
    // Only Gadget Zero allows multi-configuration; all other composite PIDs require a single configuration.
    let is_gadget_zero = match config.configurations.as_slice() {
        [c1] => matches!(c1.as_slice(), [f] if is_test_function(f)),
        [c1, c2] => {
            matches!(c1.as_slice(), [f] if is_test_function(f))
                && matches!(c2.as_slice(), [f] if is_test_function(f))
        }
        _ => false,
    };

    if config.configurations.len() > 1 && !is_gadget_zero {
        return None;
    }

    let mut tag = FunctionTag::empty();
    for func in config.configurations.iter().flatten() {
        if let Some(t) = function_tag(func) {
            tag |= t;
        } else {
            return None; // Unknown or custom function: do not assign standard Google PID
        }
    }

    let total_functions: usize = config.configurations.iter().map(Vec::len).sum();
    // Prevent duplicate functions (e.g. cdc,cdc or cdc,cdc,adb) from matching standard 1:1 composite PIDs.
    if !is_gadget_zero && tag.bits().count_ones() as usize != total_functions {
        return None;
    }

    let pid = match tag {
        FunctionTag::CDC => GOOGLE_USB_CDC_PID,
        FunctionTag::UMS => GOOGLE_USB_UMS_PID,
        FunctionTag::RNDIS => GOOGLE_USB_RNDIS_PID,
        FunctionTag::ADB => GOOGLE_USB_ADB_PID,
        FunctionTag::FASTBOOT => GOOGLE_USB_FASTBOOT_PID,
        FunctionTag::TEST => GOOGLE_USB_FUNCTION_TEST_PID,
        FunctionTag::VSOCK => GOOGLE_USB_VSOCK_BRIDGE_PID,
        t if t == (FunctionTag::CDC | FunctionTag::TEST) => GOOGLE_USB_CDC_AND_FUNCTION_TEST_PID,
        t if t == (FunctionTag::CDC | FunctionTag::ADB) => GOOGLE_USB_CDC_AND_ADB_PID,
        t if t == (FunctionTag::ADB | FunctionTag::VSOCK) => GOOGLE_USB_ADB_AND_VSOCK_BRIDGE_PID,
        t if t == (FunctionTag::CDC | FunctionTag::VSOCK) => GOOGLE_USB_CDC_AND_VSOCK_BRIDGE_PID,
        t if t == (FunctionTag::CDC | FunctionTag::FASTBOOT) => GOOGLE_USB_CDC_AND_FASTBOOT_PID,
        t if t == (FunctionTag::CDC | FunctionTag::ADB | FunctionTag::VSOCK) => {
            GOOGLE_USB_CDC_AND_ADB_AND_VSOCK_BRIDGE_PID
        }
        t if t == (FunctionTag::CDC | FunctionTag::ADB | FunctionTag::FASTBOOT) => {
            GOOGLE_USB_CDC_AND_ADB_AND_FASTBOOT_PID
        }
        _ => return None,
    };

    // Construct canonical product description in PID order (deterministic regardless of input order)
    let mut parts = Vec::new();
    if tag.contains(FunctionTag::CDC) {
        parts.push("CDC Ethernet");
    }
    if tag.contains(FunctionTag::UMS) {
        parts.push("USB Mass Storage");
    }
    if tag.contains(FunctionTag::TEST) {
        parts.push("USB Function Test");
    }
    if tag.contains(FunctionTag::RNDIS) {
        parts.push("RNDIS Ethernet");
    }
    if tag.contains(FunctionTag::ADB) {
        parts.push("ADB");
    }
    if tag.contains(FunctionTag::VSOCK) {
        parts.push("VSOCK Bridge");
    }
    if tag.contains(FunctionTag::FASTBOOT) {
        parts.push("Fastboot");
    }

    Some((pid, parts.join(" & ")))
}

/// Updates `device_desc` with configuration settings and descriptor counts.
///
/// Returns the updated `DeviceDescriptor` and a boolean indicating whether standard
/// Google USB PID/product descriptors were derived to populate any unspecified fields.
pub fn update_device_descriptor(
    config: &UsbConfigJson,
    mut device_desc: peripheral::DeviceDescriptor,
    num_configurations: u8,
) -> (peripheral::DeviceDescriptor, bool) {
    device_desc.b_num_configurations = num_configurations;

    if let Some(vid) = config.id_vendor {
        device_desc.id_vendor = vid;
    } else if device_desc.id_vendor == 0 {
        device_desc.id_vendor = GOOGLE_USB_VID;
    }

    let derived = (config.id_product.is_none() || config.product.is_none())
        .then(|| derive_pid_and_product(config))
        .flatten();
    let standard_derived = derived.is_some();

    if let Some(pid) = config.id_product {
        device_desc.id_product = pid;
    } else if let Some((derived_pid, _)) = &derived {
        device_desc.id_product = *derived_pid;
    }

    if let Some(prod) = &config.product {
        device_desc.product = prod.clone();
    } else if let Some((_, derived_product)) = derived {
        device_desc.product = derived_product;
    }

    (device_desc, standard_derived)
}

pub fn resolve_config_descriptors(
    config: &UsbConfigJson,
) -> Result<Vec<Vec<peripheral::FunctionDescriptor>>, Error> {
    if config.configurations.len() == 1
        && config.configurations[0].len() == 1
        && is_test_function(&config.configurations[0][0])
    {
        // Gadget Zero dual-configuration specification (Config 1: Source/Sink, Config 2: Loopback)
        return Ok(vec![
            vec![function_name_to_descriptor("sourcesink")?],
            vec![function_name_to_descriptor("loopback")?],
        ]);
    }

    config
        .configurations
        .iter()
        .map(|cfg| {
            cfg.iter().map(|name| function_name_to_descriptor(name)).collect::<Result<Vec<_>, _>>()
        })
        .collect()
}

#[cfg(test)]
pub fn load_config_descriptors(
    input: &str,
) -> Result<Vec<Vec<peripheral::FunctionDescriptor>>, Error> {
    let config = load_config_input(input)?;
    resolve_config_descriptors(&config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_config_json_string() -> Result<(), Error> {
        let json_str = r#"{"configurations": [["cdc", "test"]]}"#;
        let config = load_config_input(json_str)?;
        assert_eq!(config.configurations, vec![vec!["cdc", "test"]]);
        Ok(())
    }

    #[test]
    fn test_load_config_comma_separated() -> Result<(), Error> {
        let input = " cdc, test, rndis ";
        let config = load_config_input(input)?;
        assert_eq!(config.configurations, vec![vec!["cdc", "test", "rndis"]]);
        Ok(())
    }

    #[test]
    fn test_load_config_comma_separated_with_empty_elements() -> Result<(), Error> {
        let input = "cdc,,test, ";
        let config = load_config_input(input)?;
        assert_eq!(config.configurations, vec![vec!["cdc", "test"]]);
        Ok(())
    }

    #[test]
    fn test_empty_config_rejected() {
        assert!(load_config_input("").is_err());
        assert!(load_config_input("   ").is_err());
        assert!(load_config_input(",,,").is_err());
    }

    #[test]
    fn test_bounds_validation_too_many_functions() {
        let functions = vec!["f1", "f2", "f3", "f4", "f5", "f6", "f7", "f8", "f9"].join(",");
        assert!(load_config_input(&functions).is_err());
    }

    #[test]
    fn test_bounds_validation_name_too_long() {
        let long_name = "a".repeat(33);
        assert!(load_config_input(&long_name).is_err());
    }

    #[test]
    fn test_standard_functions_mapping() -> Result<(), Error> {
        let test_cases = [
            ("cdc", USB_CLASS_COMM, USB_CDC_SUBCLASS_ETHERNET, USB_CDC_PROTOCOL_NONE),
            ("cdc_eth", USB_CLASS_COMM, USB_CDC_SUBCLASS_ETHERNET, USB_CDC_PROTOCOL_NONE),
            ("cdc-eth", USB_CLASS_COMM, USB_CDC_SUBCLASS_ETHERNET, USB_CDC_PROTOCOL_NONE),
            ("rndis", USB_CLASS_COMM, USB_CDC_SUBCLASS_ACM, USB_PROTOCOL_RNDIS),
            ("ums", USB_CLASS_MSC, USB_SUBCLASS_MSC_SCSI, USB_PROTOCOL_MSC_BULK_ONLY),
            ("mass_storage", USB_CLASS_MSC, USB_SUBCLASS_MSC_SCSI, USB_PROTOCOL_MSC_BULK_ONLY),
            ("sourcesink", USB_CLASS_VENDOR, USB_SUBCLASS_TEST, USB_PROTOCOL_SOURCESINK),
            ("source_sink", USB_CLASS_VENDOR, USB_SUBCLASS_TEST, USB_PROTOCOL_SOURCESINK),
            ("source-sink", USB_CLASS_VENDOR, USB_SUBCLASS_TEST, USB_PROTOCOL_SOURCESINK),
            ("loopback", USB_CLASS_VENDOR, USB_SUBCLASS_TEST, USB_PROTOCOL_LOOPBACK),
            ("adb", USB_CLASS_VENDOR, USB_SUBCLASS_ADB, USB_PROTOCOL_ADB),
            ("vsock", USB_CLASS_VENDOR, USB_SUBCLASS_VSOCK, USB_PROTOCOL_VSOCK),
            ("ffx", USB_CLASS_VENDOR, USB_SUBCLASS_VSOCK, USB_PROTOCOL_VSOCK),
        ];

        for (name, cls, sub, proto) in test_cases {
            let desc = function_name_to_descriptor(name)?;
            assert_eq!(desc.interface_class, cls);
            assert_eq!(desc.interface_subclass, sub);
            assert_eq!(desc.interface_protocol, proto);
        }
        Ok(())
    }

    #[test]
    fn test_case_insensitivity() -> Result<(), Error> {
        let desc = function_name_to_descriptor("CDC")?;
        assert_eq!(desc.interface_class, USB_CLASS_COMM);

        let desc = function_name_to_descriptor("Adb")?;
        assert_eq!(desc.interface_subclass, USB_SUBCLASS_ADB);

        let desc = function_name_to_descriptor("CUSTOM_0X01_0X02_0X03")?;
        assert_eq!(desc.interface_class, 0x01);
        assert_eq!(desc.interface_subclass, 0x02);
        assert_eq!(desc.interface_protocol, 0x03);
        Ok(())
    }

    #[test]
    fn test_custom_hex_parsing_and_roundtrip() -> Result<(), Error> {
        let name = "custom_0x12_0x34_0x56";
        let desc = function_name_to_descriptor(name)?;
        assert_eq!(desc.interface_class, 0x12);
        assert_eq!(desc.interface_subclass, 0x34);
        assert_eq!(desc.interface_protocol, 0x56);

        let roundtrip_name = descriptor_to_function_name(&desc);
        assert_eq!(roundtrip_name, name);
        Ok(())
    }

    #[test]
    fn test_invalid_names() {
        assert!(function_name_to_descriptor("foobar").is_err());
        assert!(function_name_to_descriptor("invalid_fn").is_err());
        assert!(function_name_to_descriptor("custom_0x12_0x34").is_err());
        assert!(function_name_to_descriptor("custom_0xzz_0x12_0x34").is_err());
        assert!(function_name_to_descriptor("custom_0x100_0x12_0x34").is_err());
    }

    #[test]
    fn test_resolve_config_descriptors_unknown_function_rejected() {
        let err = load_config_descriptors("foobar").unwrap_err().to_string();
        assert!(err.contains("Unknown USB function: foobar"), "got: {err}");

        let err_multi = load_config_descriptors("cdc;foobar").unwrap_err().to_string();
        assert!(err_multi.contains("Unknown USB function: foobar"), "got: {err_multi}");
    }

    #[test]
    fn test_config_descriptors_to_names() -> Result<(), Error> {
        let descriptors = load_config_descriptors("cdc,adb;sourcesink")?;
        let names = config_descriptors_to_names(&descriptors);
        assert_eq!(names, vec![vec!["cdc", "adb"], vec!["sourcesink"]]);
        Ok(())
    }

    #[test]
    fn test_load_config_semicolon_separated() -> Result<(), Error> {
        let input = "cdc ; sourcesink";
        let config = load_config_input(input)?;
        assert_eq!(config.configurations, vec![vec!["cdc"], vec!["sourcesink"]]);
        Ok(())
    }

    #[test]
    fn test_load_config_json_configurations() -> Result<(), Error> {
        let json_str = r#"{"configurations": [["cdc", "adb"], ["ums"]]}"#;
        let config = load_config_input(json_str)?;
        assert_eq!(config.configurations, vec![vec!["cdc", "adb"], vec!["ums"]]);
        Ok(())
    }

    #[test]
    fn test_resolve_config_descriptors_gadget_zero_dual_config() -> Result<(), Error> {
        let descriptors = load_config_descriptors("sourcesink;loopback")?;
        assert_eq!(descriptors.len(), 2);
        assert_eq!(descriptors[0].len(), 1);
        assert_eq!(descriptors[1].len(), 1);
        assert_eq!(descriptors[0][0].interface_class, USB_CLASS_VENDOR);
        assert_eq!(descriptors[0][0].interface_subclass, USB_SUBCLASS_TEST);
        assert_eq!(descriptors[0][0].interface_protocol, USB_PROTOCOL_SOURCESINK);
        assert_eq!(descriptors[1][0].interface_class, USB_CLASS_VENDOR);
        assert_eq!(descriptors[1][0].interface_subclass, USB_SUBCLASS_TEST);
        assert_eq!(descriptors[1][0].interface_protocol, USB_PROTOCOL_LOOPBACK);
        Ok(())
    }

    #[test]
    fn test_resolve_config_descriptors_single_config() -> Result<(), Error> {
        let descriptors = load_config_descriptors("cdc,adb")?;
        assert_eq!(descriptors.len(), 1);
        assert_eq!(descriptors[0].len(), 2);
        assert_eq!(descriptors[0][0].interface_class, USB_CLASS_COMM);
        assert_eq!(descriptors[0][1].interface_class, USB_CLASS_VENDOR);
        Ok(())
    }

    #[test]
    fn test_resolve_config_descriptors_explicit_multi_config() -> Result<(), Error> {
        let descriptors = load_config_descriptors("cdc;ums")?;
        assert_eq!(descriptors.len(), 2);
        assert_eq!(descriptors[0].len(), 1);
        assert_eq!(descriptors[1].len(), 1);
        assert_eq!(descriptors[0][0].interface_class, USB_CLASS_COMM);
        assert_eq!(descriptors[1][0].interface_class, USB_CLASS_MSC);
        Ok(())
    }

    #[test]
    fn test_load_config_json_single_config() -> Result<(), Error> {
        let json_str = r#"{"configurations": [["cdc", "adb", "ums"]]}"#;
        let config = load_config_input(json_str)?;
        assert_eq!(config.configurations, vec![vec!["cdc", "adb", "ums"]]);
        Ok(())
    }

    #[test]
    fn test_config_exceeds_max_configurations() {
        let input = "cdc;adb;ums;test;rndis;vsock"; // 6 configurations > MAX_CONFIGURATIONS (5)
        let result = load_config_input(input);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("exceeds maximum of 5 configurations"), "got: {}", err);
    }

    #[test]
    fn test_config_empty_configuration_in_multi_config() {
        let json_str = r#"{"configurations": [["cdc"], []]}"#;
        let result = load_config_input(json_str);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("must specify at least one USB function"), "got: {}", err);
    }

    #[test]
    fn test_config_semicolons_only_rejected() {
        assert!(load_config_input(";;;").is_err());
        assert!(load_config_input(" ; ; ").is_err());
    }

    #[test]
    fn test_config_empty_configuration_in_semicolon_syntax() {
        let result = load_config_input("cdc ; ; test");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("Configuration 1 must specify at least one USB function"),
            "got: {}",
            err
        );

        let result_trailing = load_config_input("cdc ; ");
        assert!(result_trailing.is_err());
        let err = result_trailing.unwrap_err().to_string();
        assert!(
            err.contains("Configuration 1 must specify at least one USB function"),
            "got: {}",
            err
        );

        let result_leading = load_config_input("; cdc");
        assert!(result_leading.is_err());
        let err = result_leading.unwrap_err().to_string();
        assert!(
            err.contains("Configuration 0 must specify at least one USB function"),
            "got: {}",
            err
        );
    }

    #[test]
    fn test_config_function_name_too_long_in_multi_config() {
        let long_name = "a".repeat(MAX_FUNCTION_NAME_LEN + 1);
        let input = format!("cdc;{}", long_name);
        let result = load_config_input(&input);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("exceeds maximum allowed length"), "got: {}", err);
    }

    #[test]
    fn test_legacy_functions_json_rejected() {
        let json_str = r#"{"functions": ["cdc", "adb"]}"#;
        let err =
            load_config_input(json_str).expect_err("legacy 'functions' JSON must be rejected");
        let err_msg = format!("{err:#}");
        assert!(err_msg.contains("unknown field `functions`"), "got: {err_msg}");

        // Missing required configurations field
        let json_empty = r#"{"product": "custom"}"#;
        let err =
            load_config_input(json_empty).expect_err("missing configurations must be rejected");
        let err_msg = format!("{err:#}");
        assert!(err_msg.contains("missing field `configurations`"), "got: {err_msg}");

        // Unknown fields rejected when configurations is present
        let json_unknown = r#"{"configurations": [["cdc"]], "functions": ["adb"]}"#;
        let err = load_config_input(json_unknown).expect_err("unknown fields must be rejected");
        let err_msg = format!("{err:#}");
        assert!(err_msg.contains("unknown field `functions`"), "got: {err_msg}");
    }

    #[test]
    fn test_config_json_roundtrip() -> Result<(), Error> {
        let original = UsbConfigJson {
            configurations: vec![vec!["cdc".to_string()], vec!["adb".to_string()]],
            id_vendor: None,
            id_product: None,
            product: None,
        };
        let serialized = serde_json::to_string(&original)?;
        assert!(!serialized.contains("id_vendor"));
        assert!(!serialized.contains("id_product"));
        assert!(!serialized.contains("product"));
        let deserialized: UsbConfigJson = serde_json::from_str(&serialized)?;
        assert_eq!(original, deserialized);
        Ok(())
    }

    #[test]
    fn test_config_json_with_metadata_roundtrip() -> Result<(), Error> {
        let original = UsbConfigJson {
            configurations: vec![vec!["cdc".to_string(), "vsock".to_string()]],
            id_vendor: Some(GOOGLE_USB_VID),
            id_product: Some(GOOGLE_USB_CDC_AND_VSOCK_BRIDGE_PID),
            product: Some("CDC Ethernet & VSOCK Bridge".to_string()),
        };
        let serialized = serde_json::to_string(&original)?;
        let deserialized: UsbConfigJson = serde_json::from_str(&serialized)?;
        assert_eq!(original, deserialized);

        let loaded = load_config_input(&serialized)?;
        assert_eq!(original, loaded);
        Ok(())
    }

    #[test]
    fn test_derive_pid_and_product() -> Result<(), Error> {
        let test_config = load_config_input("sourcesink")?;
        assert_eq!(
            derive_pid_and_product(&test_config),
            Some((GOOGLE_USB_FUNCTION_TEST_PID, "USB Function Test".to_string()))
        );

        let dual_test = load_config_input("sourcesink;loopback")?;
        assert_eq!(
            derive_pid_and_product(&dual_test),
            Some((GOOGLE_USB_FUNCTION_TEST_PID, "USB Function Test".to_string()))
        );

        let cdc_vsock = load_config_input("cdc,vsock")?;
        assert_eq!(
            derive_pid_and_product(&cdc_vsock),
            Some((GOOGLE_USB_CDC_AND_VSOCK_BRIDGE_PID, "CDC Ethernet & VSOCK Bridge".to_string()))
        );

        let cdc_test = load_config_input("cdc,sourcesink")?;
        assert_eq!(
            derive_pid_and_product(&cdc_test),
            Some((
                GOOGLE_USB_CDC_AND_FUNCTION_TEST_PID,
                "CDC Ethernet & USB Function Test".to_string()
            ))
        );

        let fastboot = load_config_input("fastboot")?;
        assert_eq!(
            derive_pid_and_product(&fastboot),
            Some((GOOGLE_USB_FASTBOOT_PID, "Fastboot".to_string()))
        );
        Ok(())
    }

    #[test]
    fn test_update_device_descriptor() -> Result<(), Error> {
        let config = load_config_input("sourcesink;loopback")?;
        let base_desc = peripheral::DeviceDescriptor {
            bcd_usb: 0x0200,
            b_device_class: 0,
            b_device_sub_class: 0,
            b_device_protocol: 0,
            b_max_packet_size0: 64,
            id_vendor: 0x18d1,
            id_product: 0xa029,
            bcd_device: 0x0100,
            manufacturer: "Zircon".to_string(),
            product: "CDC Ethernet & VSOCK Bridge".to_string(),
            serial: "1234".to_string(),
            b_num_configurations: 1,
        };

        let (updated, derived) = update_device_descriptor(&config, base_desc.clone(), 2);
        assert!(derived);
        assert_eq!(updated.b_num_configurations, 2);
        assert_eq!(updated.id_vendor, 0x18d1);
        assert_eq!(updated.id_product, 0xa022);
        assert_eq!(updated.product, "USB Function Test");

        // Explicit override in JSON takes precedence
        let json_override = r#"{"configurations": [["sourcesink"]], "id_product": 42000, "product": "Custom Test"}"#;
        let config_override = load_config_input(json_override)?;
        let (updated_override, derived_override) =
            update_device_descriptor(&config_override, base_desc.clone(), 2);
        assert!(!derived_override);
        assert_eq!(updated_override.id_product, 42000);
        assert_eq!(updated_override.product, "Custom Test");

        // Partial override: only id_product overridden, product derived
        let json_pid_only = r#"{"configurations": [["sourcesink"]], "id_product": 42001}"#;
        let config_pid_only = load_config_input(json_pid_only)?;
        let (updated_pid_only, derived_pid_only) =
            update_device_descriptor(&config_pid_only, base_desc.clone(), 2);
        assert!(derived_pid_only);
        assert_eq!(updated_pid_only.id_product, 42001);
        assert_eq!(updated_pid_only.product, "USB Function Test");

        // Partial override: only product overridden, id_product derived
        let json_prod_only =
            r#"{"configurations": [["sourcesink"]], "product": "Custom Test Name"}"#;
        let config_prod_only = load_config_input(json_prod_only)?;
        let (updated_prod_only, derived_prod_only) =
            update_device_descriptor(&config_prod_only, base_desc.clone(), 2);
        assert!(derived_prod_only);
        assert_eq!(updated_prod_only.id_product, 0xa022);
        assert_eq!(updated_prod_only.product, "Custom Test Name");

        // Custom configuration with no standard PID: retain base descriptor values
        let json_unsupported = r#"{"configurations": [["custom_unknown_function"]]}"#;
        let config_unsupported = load_config_input(json_unsupported)?;
        let (updated_unsupported, derived_unsupported) =
            update_device_descriptor(&config_unsupported, base_desc, 1);
        assert!(!derived_unsupported);
        assert_eq!(updated_unsupported.id_product, 0xa029);
        assert_eq!(updated_unsupported.product, "CDC Ethernet & VSOCK Bridge");
        Ok(())
    }

    #[test]
    fn test_derive_pid_and_product_order_independence() -> Result<(), Error> {
        let forward = load_config_input("cdc,vsock")?;
        let reverse = load_config_input("vsock,cdc")?;
        assert_eq!(derive_pid_and_product(&forward), derive_pid_and_product(&reverse));
        assert_eq!(
            derive_pid_and_product(&reverse),
            Some((GOOGLE_USB_CDC_AND_VSOCK_BRIDGE_PID, "CDC Ethernet & VSOCK Bridge".to_string()))
        );
        Ok(())
    }

    #[test]
    fn test_derive_pid_and_product_unsupported_combinations() -> Result<(), Error> {
        // Fastboot + VSOCK is not a standard composite device
        let invalid = load_config_input("fastboot,vsock")?;
        assert_eq!(derive_pid_and_product(&invalid), None);
        Ok(())
    }

    #[test]
    fn test_derive_pid_and_product_custom_descriptor_returns_none() -> Result<(), Error> {
        let custom = load_config_input("cdc,custom_0x12_0x34_0x56")?;
        assert_eq!(derive_pid_and_product(&custom), None);
        Ok(())
    }

    #[test]
    fn test_derive_pid_and_product_multi_config_returns_none() -> Result<(), Error> {
        let multi = load_config_input("cdc;adb")?;
        assert_eq!(derive_pid_and_product(&multi), None);
        Ok(())
    }

    #[test]
    fn test_derive_pid_and_product_rejects_duplicates() -> Result<(), Error> {
        let dup = load_config_input("cdc,cdc")?;
        assert_eq!(derive_pid_and_product(&dup), None);

        let dup2 = load_config_input("cdc,cdc,adb")?;
        assert_eq!(derive_pid_and_product(&dup2), None);

        let dup_test = load_config_input("sourcesink,sourcesink")?;
        assert_eq!(derive_pid_and_product(&dup_test), None);
        Ok(())
    }

    #[test]
    fn test_derive_pid_and_product_rejects_invalid_test_multi_config() -> Result<(), Error> {
        let triple = load_config_input("sourcesink;sourcesink;sourcesink")?;
        assert_eq!(derive_pid_and_product(&triple), None);

        let multi_func = load_config_input("sourcesink,cdc;loopback")?;
        assert_eq!(derive_pid_and_product(&multi_func), None);
        Ok(())
    }

    #[test]
    fn test_validate_config_rejects_vid_zero() {
        let json = r#"{"configurations": [["sourcesink"]], "id_vendor": 0}"#;
        let result = load_config_input(json);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("cannot be 0"));
    }
}
