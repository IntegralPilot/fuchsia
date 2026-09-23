// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use diagnostics_reader::{ArchiveReader, DiagnosticsHierarchy, Property};

fn parse_numeric_suffix(name: &str) -> Option<u32> {
    if let Some(hex_part) = name.strip_prefix("endpoint-0x") {
        u32::from_str_radix(hex_part, 16).ok()
    } else if let Some(dec_part) = name.strip_prefix("interface-") {
        dec_part.parse::<u32>().ok()
    } else if let Some(dec_part) = name.strip_prefix("function-") {
        dec_part.parse::<u32>().ok()
    } else {
        None
    }
}

fn compare_hierarchy_names(a: &str, b: &str) -> std::cmp::Ordering {
    if let (Ok(num_a), Ok(num_b)) = (a.parse::<u64>(), b.parse::<u64>()) {
        return num_a.cmp(&num_b);
    }

    let prefix_a = if a.starts_with("endpoint-") {
        "endpoint"
    } else if a.starts_with("interface-") {
        "interface"
    } else if a.starts_with("function-") {
        "function"
    } else {
        ""
    };

    let prefix_b = if b.starts_with("endpoint-") {
        "endpoint"
    } else if b.starts_with("interface-") {
        "interface"
    } else if b.starts_with("function-") {
        "function"
    } else {
        ""
    };

    if prefix_a == prefix_b && !prefix_a.is_empty() {
        let num_a = parse_numeric_suffix(a);
        let num_b = parse_numeric_suffix(b);
        if let (Some(na), Some(nb)) = (num_a, num_b) {
            return na.cmp(&nb);
        }
    }

    a.cmp(b)
}

fn format_endpoint_address(addr: u64) -> String {
    let is_in = (addr & 0x80) != 0;
    let ep_num = addr & 0x0F;
    let dir_str = if is_in { "IN" } else { "OUT" };
    format!("0x{:02x} (EP {} {})", addr, ep_num, dir_str)
}

fn format_interface_class(class: u64) -> String {
    let name = match class {
        1 => "Audio",
        2 => "CDC-Control",
        3 => "HID",
        7 => "Printer",
        8 => "Mass Storage",
        9 => "Hub",
        10 => "CDC-Data",
        224 => "Wireless",
        239 => "Miscellaneous",
        255 => "Vendor Specific",
        _ => "unknown",
    };
    format!("{} ({})", class, name)
}

fn format_interface_subclass(class: u64, subclass: u64) -> String {
    if class == 255 {
        let name = match subclass {
            66 => "ADB/Fastboot",
            67 => "Overnet",
            _ => "unknown",
        };
        format!("{} ({})", subclass, name)
    } else {
        format!("{}", subclass)
    }
}

fn format_interface_protocol(class: u64, subclass: u64, protocol: u64) -> String {
    if class == 255 && subclass == 66 {
        let name = match protocol {
            1 => "ADB",
            3 => "Fastboot",
            _ => "unknown",
        };
        format!("{} ({})", protocol, name)
    } else {
        format!("{}", protocol)
    }
}

fn format_endpoint_attributes(attrs: u64) -> String {
    let transfer_type = attrs & 0x03;
    let type_str = match transfer_type {
        0 => "Control",
        1 => "Isochronous",
        2 => "Bulk",
        3 => "Interrupt",
        _ => "Unknown",
    };
    format!("{} ({})", attrs, type_str)
}

fn decode_bm_request_type(req_type: u64) -> String {
    let dir = if (req_type & 0x80) != 0 { "IN" } else { "OUT" };
    let type_str = match (req_type & 0x60) >> 5 {
        0 => "Standard",
        1 => "Class",
        2 => "Vendor",
        _ => "Reserved",
    };
    let recip = match req_type & 0x1F {
        0 => "Device",
        1 => "Interface",
        2 => "Endpoint",
        3 => "Other",
        _ => "Reserved",
    };
    format!("dir: {}, type: {}, recip: {}", dir, type_str, recip)
}

fn decode_b_request(req_type: u64, req: u64) -> String {
    let is_standard = (req_type & 0x60) == 0;
    if is_standard {
        let name = match req {
            0 => "GET_STATUS",
            1 => "CLEAR_FEATURE",
            3 => "SET_FEATURE",
            5 => "SET_ADDRESS",
            6 => "GET_DESCRIPTOR",
            7 => "SET_DESCRIPTOR",
            8 => "GET_CONFIGURATION",
            9 => "SET_CONFIGURATION",
            10 => "GET_INTERFACE",
            11 => "SET_INTERFACE",
            12 => "SYNCH_FRAME",
            _ => "unknown",
        };
        format!("{} ({})", name, req)
    } else {
        format!("{}", req)
    }
}

fn decode_zx_status(status: i64) -> String {
    format!("{:?}", zx::Status::ok(status as i32))
}

fn get_utc_string(m_event_ns: i64) -> String {
    let m_now = zx::MonotonicInstant::get().into_nanos();
    let u_now = chrono::Utc::now();
    let delta_ns = m_now.saturating_sub(m_event_ns);
    let u_event = u_now - chrono::Duration::nanoseconds(delta_ns);
    u_event.format("%Y-%m-%d %H:%M:%S%.3f").to_string()
}

fn print_control_history(hierarchy: &DiagnosticsHierarchy, indent: usize) {
    let spaces = "  ".repeat(indent);
    let prop_spaces = "  ".repeat(indent + 1);
    println!("{}{}:", spaces, hierarchy.name);

    let mut sorted_children = hierarchy.children.clone();
    sorted_children.sort_by(|a, b| {
        let num_a = a.name.parse::<u32>().unwrap_or(0);
        let num_b = b.name.parse::<u32>().unwrap_or(0);
        num_a.cmp(&num_b)
    });

    struct ControlEntry<'a> {
        timestamp: i64,
        node: &'a DiagnosticsHierarchy,
    }
    let mut entries = Vec::new();
    for child in &sorted_children {
        let mut ts = 0i64;
        if let Some(Property::Uint(_, val)) = child.properties.iter().find(|p| p.name() == "@time")
        {
            ts = *val as i64;
        } else if let Some(Property::Int(_, val)) =
            child.properties.iter().find(|p| p.name() == "@time")
        {
            ts = *val as i64;
        }
        entries.push(ControlEntry { timestamp: ts, node: child });
    }
    entries.sort_by_key(|e| e.timestamp);

    for entry in entries {
        let child = entry.node;
        let mut req_type = 0u64;
        let mut request = 0u64;
        let mut value = 0u64;
        let mut index = 0u64;
        let mut length = 0u64;
        let mut status = 0i64;
        let mut resp_len = 0u64;

        for prop in &child.properties {
            match prop {
                Property::Uint(n, v) => match n.as_str() {
                    "bm_request_type" => req_type = *v,
                    "b_request" => request = *v,
                    "w_value" => value = *v,
                    "w_index" => index = *v,
                    "w_length" => length = *v,
                    "response_length" => resp_len = *v,
                    _ => {}
                },
                Property::Int(n, v) => match n.as_str() {
                    "status" => status = *v,
                    _ => {}
                },
                _ => {}
            }
        }

        let time_utc_str = get_utc_string(entry.timestamp);
        let decoded_type = decode_bm_request_type(req_type);
        let decoded_req = decode_b_request(req_type, request);
        let status_str = decode_zx_status(status);

        println!(
            "{}[{}] Control: [{}] Req: {}, Val: 0x{:04x}, Idx: 0x{:04x}, Len: {} -> {}, Transferred: {} bytes",
            prop_spaces,
            time_utc_str,
            decoded_type,
            decoded_req,
            value,
            index,
            length,
            status_str,
            resp_len
        );
    }
}

fn print_connection_history(hierarchy: &DiagnosticsHierarchy, indent: usize) {
    let spaces = "  ".repeat(indent);
    let prop_spaces = "  ".repeat(indent + 1);
    println!("{}{}:", spaces, hierarchy.name);

    let mut sorted_children = hierarchy.children.clone();
    sorted_children.sort_by(|a, b| {
        let num_a = a.name.parse::<u32>().unwrap_or(0);
        let num_b = b.name.parse::<u32>().unwrap_or(0);
        num_a.cmp(&num_b)
    });

    struct ConnectionEntry<'a> {
        timestamp: i64,
        node: &'a DiagnosticsHierarchy,
    }
    let mut entries = Vec::new();
    for child in &sorted_children {
        let mut ts = 0i64;
        if let Some(Property::Uint(_, val)) = child.properties.iter().find(|p| p.name() == "@time")
        {
            ts = *val as i64;
        } else if let Some(Property::Int(_, val)) =
            child.properties.iter().find(|p| p.name() == "@time")
        {
            ts = *val as i64;
        }
        entries.push(ConnectionEntry { timestamp: ts, node: child });
    }
    entries.sort_by_key(|e| e.timestamp);

    for entry in entries {
        let child = entry.node;
        let mut event_type = "";
        let mut val_str = "";

        for prop in &child.properties {
            if let Property::String(n, v) = prop {
                match n.as_str() {
                    "event_type" => event_type = v.as_str(),
                    "value" => val_str = v.as_str(),
                    _ => {}
                }
            }
        }

        let time_utc_str = get_utc_string(entry.timestamp);
        println!(
            "{}[{}] Connection: {} changed to {}",
            prop_spaces, time_utc_str, event_type, val_str
        );
    }
}

fn print_properties_with_decoders(hierarchy: &DiagnosticsHierarchy, child_indent: usize) {
    // Clone and sort properties alphabetically by key
    let mut sorted_props = hierarchy.properties.clone();
    sorted_props.sort_by(|a, b| a.name().cmp(b.name()));

    // Print properties
    for prop in &sorted_props {
        let name = prop.name();
        let prop_spaces = "  ".repeat(child_indent);

        // 1. Monotonic-to-UTC Historical Time Correlation for @time
        if name == "@time" {
            let m_event = match prop {
                Property::Int(_, val) => Some(*val as i64),
                Property::Uint(_, val) => Some(*val as i64),
                _ => None,
            };
            if let Some(me) = m_event {
                let time_utc_str = get_utc_string(me);
                println!("{}{}: {}", prop_spaces, name, me);
                println!("{}time_utc: \"{}\"", prop_spaces, time_utc_str);
                continue;
            }
        }

        // 2. Decoders
        if name == "usb_endpoint_address" || name == "endpoint_address" {
            let val_u64 = match prop {
                Property::Int(_, v) => Some(*v as u64),
                Property::Uint(_, v) => Some(*v),
                Property::String(_, s) => {
                    if s.starts_with("0x") {
                        u64::from_str_radix(s.strip_prefix("0x").unwrap(), 16).ok()
                    } else {
                        s.parse::<u64>().ok()
                    }
                }
                _ => None,
            };
            if let Some(addr) = val_u64 {
                let decoded = format_endpoint_address(addr);
                println!("{}{}: {}", prop_spaces, name, decoded);
                continue;
            }
        }

        if name == "interface_class" {
            let val_u64 = match prop {
                Property::Int(_, v) => Some(*v as u64),
                Property::Uint(_, v) => Some(*v),
                _ => None,
            };
            if let Some(class) = val_u64 {
                let decoded = format_interface_class(class);
                println!("{}{}: {}", prop_spaces, name, decoded);
                continue;
            }
        }

        if name == "interface_subclass" {
            let class_opt =
                hierarchy.properties.iter().find(|p| p.name() == "interface_class").and_then(|p| {
                    match p {
                        Property::Int(_, v) => Some(*v as u64),
                        Property::Uint(_, v) => Some(*v),
                        _ => None,
                    }
                });
            let val_u64 = match prop {
                Property::Int(_, v) => Some(*v as u64),
                Property::Uint(_, v) => Some(*v),
                _ => None,
            };
            if let (Some(class), Some(subclass)) = (class_opt, val_u64) {
                let decoded = format_interface_subclass(class, subclass);
                println!("{}{}: {}", prop_spaces, name, decoded);
                continue;
            }
        }

        if name == "interface_protocol" {
            let class_opt =
                hierarchy.properties.iter().find(|p| p.name() == "interface_class").and_then(|p| {
                    match p {
                        Property::Int(_, v) => Some(*v as u64),
                        Property::Uint(_, v) => Some(*v),
                        _ => None,
                    }
                });
            let subclass_opt =
                hierarchy.properties.iter().find(|p| p.name() == "interface_subclass").and_then(
                    |p| match p {
                        Property::Int(_, v) => Some(*v as u64),
                        Property::Uint(_, v) => Some(*v),
                        _ => None,
                    },
                );
            let val_u64 = match prop {
                Property::Int(_, v) => Some(*v as u64),
                Property::Uint(_, v) => Some(*v),
                _ => None,
            };
            if let (Some(class), Some(subclass), Some(protocol)) =
                (class_opt, subclass_opt, val_u64)
            {
                let decoded = format_interface_protocol(class, subclass, protocol);
                println!("{}{}: {}", prop_spaces, name, decoded);
                continue;
            }
        }

        if name == "attributes" || name == "bm_attributes" || name == "type" {
            let val_u64 = match prop {
                Property::Int(_, v) => Some(*v as u64),
                Property::Uint(_, v) => Some(*v),
                _ => None,
            };
            if let Some(attrs) = val_u64 {
                let decoded = format_endpoint_attributes(attrs);
                println!("{}{}: {}", prop_spaces, name, decoded);
                continue;
            }
        }

        // Fallback to standard printing
        match prop {
            Property::String(name, val) => {
                println!("{}{}: \"{}\"", prop_spaces, name, val);
            }
            Property::Int(name, val) => {
                println!("{}{}: {}", prop_spaces, name, val);
            }
            Property::Uint(name, val) => {
                println!("{}{}: {}", prop_spaces, name, val);
            }
            Property::Double(name, val) => {
                println!("{}{}: {}", prop_spaces, name, val);
            }
            Property::Bool(name, val) => {
                println!("{}{}: {}", prop_spaces, name, val);
            }
            _ => {}
        }
    }
}

fn print_custom_hierarchy(hierarchy: &DiagnosticsHierarchy, indent: usize) {
    if hierarchy.name == "control_history" {
        print_control_history(hierarchy, indent);
        return;
    }

    if hierarchy.name == "connection_history" {
        print_connection_history(hierarchy, indent);
        return;
    }

    if hierarchy.name != "root" {
        let spaces = "  ".repeat(indent);
        println!("{}{}:", spaces, hierarchy.name);
    }

    let child_indent = if hierarchy.name == "root" { indent } else { indent + 1 };

    print_properties_with_decoders(hierarchy, child_indent);

    // Clone and sort children numerically/prefix
    let mut sorted_children = hierarchy.children.clone();
    sorted_children.sort_by(|a, b| compare_hierarchy_names(&a.name, &b.name));

    // Print children
    for child in &sorted_children {
        print_custom_hierarchy(child, child_indent);
    }
}

pub async fn print_usb_inspect_diagnostics() -> Result<(), Error> {
    println!("\n=== Device-Side USB Inspect Diagnostics ===");
    let reader = ArchiveReader::inspect();

    // Blank snapshot to fetch everything
    let results = match reader.snapshot().await {
        Ok(res) => res,
        Err(e) => {
            println!("  Failed to fetch Inspect snapshot: {:?}", e);
            return Ok(());
        }
    };

    if results.is_empty() {
        println!("  No Inspect data found.");
        return Ok(());
    }

    // Sort results alphabetically by moniker segment to ensure 100% deterministic print order!
    let mut sorted_results = results;
    sorted_results.sort_by_cached_key(|result| result.moniker.to_string());

    let mut printed_monikers = std::collections::HashSet::new();
    let mut found = false;
    for result in sorted_results {
        let moniker = result.moniker.to_string();
        // Deduplicate monikers
        if !printed_monikers.insert(moniker.clone()) {
            continue;
        }

        // Dynamic filter for USB / DWC / Policy / Peripheral
        if moniker.contains("usb")
            || moniker.contains("dwc")
            || moniker.contains("policy")
            || moniker.contains("peripheral")
        {
            if let Some(payload) = result.payload {
                let has_usb_peripheral =
                    payload.children.iter().any(|c| c.name == "usb-peripheral");
                let has_dwc3 = payload.children.iter().any(|c| c.name == "dwc3");

                if moniker.contains("usb-policy") {
                    println!("\n=== USB Policy State History ===");
                } else if has_usb_peripheral {
                    println!("\n=== USB-Peripheral Driver Diagnostics ===");
                } else if has_dwc3 {
                    println!("\n=== Synopsys DWC3 Controller Diagnostics ===");
                } else {
                    println!("\n=== Inspect: {} ===", moniker);
                }

                print_custom_hierarchy(&payload, 1);
                found = true;
            }
        }
    }

    if !found {
        println!("  No USB-related Inspect data discovered on this platform.");
    }
    println!("=== End of Device-Side USB Inspect Diagnostics ===");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_numeric_suffix() {
        assert_eq!(parse_numeric_suffix("endpoint-0x81"), Some(0x81));
        assert_eq!(parse_numeric_suffix("interface-2"), Some(2));
        assert_eq!(parse_numeric_suffix("function-0"), Some(0));
        assert_eq!(parse_numeric_suffix("random-name"), None);
    }

    #[test]
    fn test_compare_hierarchy_names() {
        assert_eq!(compare_hierarchy_names("10", "2"), std::cmp::Ordering::Greater);
        assert_eq!(
            compare_hierarchy_names("endpoint-0x01", "endpoint-0x81"),
            std::cmp::Ordering::Less
        );
        assert_eq!(
            compare_hierarchy_names("interface-1", "interface-10"),
            std::cmp::Ordering::Less
        );
        assert_eq!(compare_hierarchy_names("apple", "banana"), std::cmp::Ordering::Less);
    }

    #[test]
    fn test_format_endpoint_address() {
        assert_eq!(format_endpoint_address(0x81), "0x81 (EP 1 IN)");
        assert_eq!(format_endpoint_address(0x02), "0x02 (EP 2 OUT)");
    }

    #[test]
    fn test_format_interface_class() {
        assert_eq!(format_interface_class(1), "1 (Audio)");
        assert_eq!(format_interface_class(2), "2 (CDC-Control)");
        assert_eq!(format_interface_class(3), "3 (HID)");
        assert_eq!(format_interface_class(7), "7 (Printer)");
        assert_eq!(format_interface_class(8), "8 (Mass Storage)");
        assert_eq!(format_interface_class(9), "9 (Hub)");
        assert_eq!(format_interface_class(10), "10 (CDC-Data)");
        assert_eq!(format_interface_class(224), "224 (Wireless)");
        assert_eq!(format_interface_class(239), "239 (Miscellaneous)");
        assert_eq!(format_interface_class(255), "255 (Vendor Specific)");
        assert_eq!(format_interface_class(99), "99 (unknown)");
    }

    #[test]
    fn test_format_interface_subclass() {
        assert_eq!(format_interface_subclass(255, 66), "66 (ADB/Fastboot)");
        assert_eq!(format_interface_subclass(255, 67), "67 (Overnet)");
        assert_eq!(format_interface_subclass(255, 99), "99 (unknown)");
        assert_eq!(format_interface_subclass(3, 1), "1");
    }

    #[test]
    fn test_format_interface_protocol() {
        assert_eq!(format_interface_protocol(255, 66, 1), "1 (ADB)");
        assert_eq!(format_interface_protocol(255, 66, 3), "3 (Fastboot)");
        assert_eq!(format_interface_protocol(255, 66, 99), "99 (unknown)");
        assert_eq!(format_interface_protocol(3, 1, 2), "2");
    }

    #[test]
    fn test_format_endpoint_attributes() {
        assert_eq!(format_endpoint_attributes(0), "0 (Control)");
        assert_eq!(format_endpoint_attributes(1), "1 (Isochronous)");
        assert_eq!(format_endpoint_attributes(2), "2 (Bulk)");
        assert_eq!(format_endpoint_attributes(3), "3 (Interrupt)");
    }

    #[test]
    fn test_decode_bm_request_type() {
        assert_eq!(decode_bm_request_type(0x80), "dir: IN, type: Standard, recip: Device");
        assert_eq!(decode_bm_request_type(0x21), "dir: OUT, type: Class, recip: Interface");
        assert_eq!(decode_bm_request_type(0x42), "dir: OUT, type: Vendor, recip: Endpoint");
        assert_eq!(decode_bm_request_type(0xE3), "dir: IN, type: Reserved, recip: Other");
        assert_eq!(decode_bm_request_type(0x04), "dir: OUT, type: Standard, recip: Reserved");
    }

    #[test]
    fn test_decode_b_request() {
        assert_eq!(decode_b_request(0x80, 0), "GET_STATUS (0)");
        assert_eq!(decode_b_request(0x00, 1), "CLEAR_FEATURE (1)");
        assert_eq!(decode_b_request(0x00, 3), "SET_FEATURE (3)");
        assert_eq!(decode_b_request(0x00, 5), "SET_ADDRESS (5)");
        assert_eq!(decode_b_request(0x80, 6), "GET_DESCRIPTOR (6)");
        assert_eq!(decode_b_request(0x00, 7), "SET_DESCRIPTOR (7)");
        assert_eq!(decode_b_request(0x80, 8), "GET_CONFIGURATION (8)");
        assert_eq!(decode_b_request(0x00, 9), "SET_CONFIGURATION (9)");
        assert_eq!(decode_b_request(0x80, 10), "GET_INTERFACE (10)");
        assert_eq!(decode_b_request(0x00, 11), "SET_INTERFACE (11)");
        assert_eq!(decode_b_request(0x00, 12), "SYNCH_FRAME (12)");
        assert_eq!(decode_b_request(0x80, 99), "unknown (99)");
        assert_eq!(decode_b_request(0x21, 6), "6");
    }
}
