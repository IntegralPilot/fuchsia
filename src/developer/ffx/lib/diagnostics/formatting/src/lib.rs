// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use discovery::query::TargetInfoQuery;
use discovery::{DiscoverySources, TargetState};
use fidl_fuchsia_developer_ffx as ffx;
use fidl_fuchsia_net as fnet;
use safe_string::TermSafe;

pub trait AsDiagnosticMessage {
    fn as_diagnostic_message(&self) -> String;
}

// WARN: This assumes the discovery sources enum has been condensed into a single value. Since it's
// a bitflags struct it could be more than one. This is intended to be used with an iterator rather
// than have the string handling happen in here.
impl AsDiagnosticMessage for u8 {
    fn as_diagnostic_message(&self) -> String {
        match *self {
            v if v == DiscoverySources::EMULATOR.bits() => "",
            v if v == DiscoverySources::MDNS.bits() => {
                "For mDNS debugging, see: https://fuchsia.dev/fuchsia-src/development/tools/ffx/workflows/network-connectivity/device-discovery#multicast-dns-resolution"
            }
            v if v == DiscoverySources::MANUAL.bits() => "",
            v if v == DiscoverySources::EMULATOR.bits() => "",
            v if v == DiscoverySources::FASTBOOT_FILE.bits() => "",
            v if v == DiscoverySources::USB_VSOCK.bits() => "",
            v if v == DiscoverySources::USB_FASTBOOT.bits() => "",
            v if v == DiscoverySources::GCE.bits() => "",
            b => panic!(
                "Un-handled bit type: {b}. This may be a failure from the discovery library of ffx. Please report this to {}",
                errors::BUG_REPORT_URL
            ),
        }
        .to_owned()
    }
}

/// A human-readable representation of a target query.
pub struct ReadableQuery {
    /// The kind of query in a readable form.
    pub kind: &'static str,
    /// The actual value behind the query.
    pub value: String,
}

fn format_ip_addr(ip: &fnet::IpAddress) -> String {
    match ip {
        fnet::IpAddress::Ipv4(ipv4) => std::net::Ipv4Addr::from(ipv4.addr).to_string(),
        fnet::IpAddress::Ipv6(ipv6) => std::net::Ipv6Addr::from(ipv6.addr).to_string(),
    }
}

fn format_target_ip(ip: &ffx::TargetIp) -> String {
    let ip_str = format_ip_addr(&ip.ip);
    if ip.scope_id > 0 { format!("{ip_str}%{}", ip.scope_id) } else { ip_str }
}

fn format_target_ip_port(ip_port: &ffx::TargetIpPort) -> String {
    let ip_str = format_ip_addr(&ip_port.ip);
    let s = if ip_port.scope_id > 0 { format!("{ip_str}%{}", ip_port.scope_id) } else { ip_str };

    if matches!(&ip_port.ip, fnet::IpAddress::Ipv6(_)) {
        format!("[{}]:{}", s, ip_port.port)
    } else {
        format!("{}:{}", s, ip_port.port)
    }
}

fn format_target_addr_info(addr: &ffx::TargetAddrInfo) -> String {
    match addr {
        ffx::TargetAddrInfo::Ip(ip) => format_target_ip(ip),
        ffx::TargetAddrInfo::IpPort(ip_port) => format_target_ip_port(ip_port),
        ffx::TargetAddrInfo::Vsock(vsock) => {
            format!("vsock(cid={}, namespace={:?})", vsock.cid, vsock.namespace)
        }
    }
}

fn format_target_ip_addr_info(addr: &ffx::TargetIpAddrInfo) -> String {
    match addr {
        ffx::TargetIpAddrInfo::Ip(ip) => format_target_ip(ip),
        ffx::TargetIpAddrInfo::IpPort(ip_port) => format_target_ip_port(ip_port),
    }
}

fn format_rcs_state(state: &ffx::RemoteControlState) -> &'static str {
    match state {
        ffx::RemoteControlState::Up => "Up",
        ffx::RemoteControlState::Down => "Down",
        ffx::RemoteControlState::Unknown => "Unknown",
    }
}

fn format_fidl_target_state(state: &ffx::TargetState) -> &'static str {
    match state {
        ffx::TargetState::Unknown => "Unknown",
        ffx::TargetState::Disconnected => "Disconnected",
        ffx::TargetState::Product => "Product",
        ffx::TargetState::Fastboot => "Fastboot",
        ffx::TargetState::Zedboot => "Zedboot",
    }
}

/// Formats a `TargetInfo` struct into a human-readable string.
pub fn format_target_info(info: &ffx::TargetInfo) -> String {
    let mut parts = Vec::new();
    if let Some(nodename) = &info.nodename {
        parts.push(format!("nodename: \"{}\"", TermSafe::from_str_escaped(nodename)));
    }
    if let Some(serial) = &info.serial_number {
        parts.push(format!("serial: \"{}\"", TermSafe::from_str_escaped(serial)));
    }
    if let Some(addresses) = &info.addresses {
        if !addresses.is_empty() {
            let addrs_str =
                addresses.iter().map(format_target_addr_info).collect::<Vec<_>>().join(", ");
            parts.push(format!("addresses: [{addrs_str}]"));
        }
    }
    if let Some(ssh_address) = &info.ssh_address {
        parts.push(format!("ssh_address: {}", format_target_ip_addr_info(ssh_address)));
    }
    if let Some(state) = &info.target_state {
        parts.push(format!("state: {}", format_fidl_target_state(state)));
    }
    if let Some(rcs_state) = &info.rcs_state {
        parts.push(format!("rcs: {}", format_rcs_state(rcs_state)));
    }
    if let Some(product) = &info.product_config {
        parts.push(format!("product: \"{}\"", TermSafe::from_str_escaped(product)));
    }
    if let Some(board) = &info.board_config {
        parts.push(format!("board: \"{}\"", TermSafe::from_str_escaped(board)));
    }
    if info.is_manual.unwrap_or(false) {
        parts.push("manual".to_string());
    }
    parts.join(", ")
}

/// Formats the target state into a human-readable string.
pub fn format_target_state(state: &TargetState) -> String {
    match state {
        TargetState::Product { addrs, serial } => {
            format!(
                "in product state (addrs: [{}]{})",
                addrs.iter().map(|a| a.optional_port_str()).collect::<Vec<_>>().join(", "),
                serial
                    .as_deref()
                    .map(|s| format!(", serial: \"{}\"", TermSafe::from_str_escaped(s)))
                    .unwrap_or_default()
            )
        }
        TargetState::Fastboot(state) => format!(
            "in fastboot ({}: {})",
            TermSafe::from_str_escaped(&state.serial_number),
            state.connection_state
        ),
        TargetState::Unknown => "in an unknown state".to_owned(),
        TargetState::Zedboot => "in zedboot".to_owned(),
    }
}

/// Formats the query into a human-readable struct.
pub fn format_query(query: &TargetInfoQuery) -> ReadableQuery {
    let (kind, value) = match query {
        TargetInfoQuery::NodenameOrId(v) => {
            ("nodename or id (serial number)", TermSafe::from_str_escaped(v).to_string())
        }
        TargetInfoQuery::First => {
            ("not set. We will search for any device on the network", "".to_string())
        }
        TargetInfoQuery::Addr(a) => ("address", a.to_string()),
        TargetInfoQuery::Id(s) => ("id (serial number)", TermSafe::from_str_escaped(s).to_string()),
        TargetInfoQuery::Usb(u) => ("usb", u.to_string()),
        TargetInfoQuery::VSock(v) => ("vsock", v.to_string()),
    };
    ReadableQuery { kind, value }
}

/// Formats an mDNS event into a human-readable string.
pub fn format_mdns_event(event: &ffx::MdnsEventType) -> String {
    let target_info_as_string = |t: &ffx::TargetInfo| -> String { format_target_info(t) };
    match event {
        ffx::MdnsEventType::TargetFound(info) => {
            format!("device found: {}", target_info_as_string(info))
        }
        ffx::MdnsEventType::TargetRediscovered(info) => {
            format!("device rediscovered: {}", target_info_as_string(info))
        }
        ffx::MdnsEventType::TargetExpired(info) => {
            format!("device expired: {}", target_info_as_string(info))
        }
        ffx::MdnsEventType::SocketBound(event) => {
            event.port.as_ref().map(|p| format!("binding on socket: {p}")).unwrap_or_else(|| {
                format!("mDNS bind event to unspecified socket (this is highly unexpected)")
            })
        }
    }
}

/// Extension trait for `TargetInfoQuery` to provide analytics tags.
/// This exists to avoid a circular dependency between the `discovery` and `ffx_diagnostics_formatting` crates.
pub trait TargetInfoQueryExt {
    fn to_analytics_tag(&self) -> String;
}

impl TargetInfoQueryExt for TargetInfoQuery {
    fn to_analytics_tag(&self) -> String {
        match self {
            TargetInfoQuery::First => "unspecified".to_owned(),
            _ => format_query(self).kind.to_owned(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use discovery::FastbootConnectionState;
    use std::net::SocketAddr;

    #[test]
    fn test_format_query() {
        let query = TargetInfoQuery::NodenameOrId("test".to_string());
        let f = format_query(&query);
        assert_eq!(f.kind, "nodename or id (serial number)");
        assert_eq!(f.value, "test");

        let query = TargetInfoQuery::First;
        let f = format_query(&query);
        assert_eq!(f.kind, "not set. We will search for any device on the network");
        assert_eq!(f.value, "");

        let addr = "192.168.1.1:8080".parse::<SocketAddr>().unwrap();
        let query = TargetInfoQuery::Addr(addr);
        let f = format_query(&query);
        assert_eq!(f.kind, "address");
        assert_eq!(f.value, "192.168.1.1:8080");

        let query = TargetInfoQuery::Id("1234".to_string());
        let f = format_query(&query);
        assert_eq!(f.kind, "id (serial number)");
        assert_eq!(f.value, "1234");

        let query = TargetInfoQuery::Usb(1);
        let f = format_query(&query);
        assert_eq!(f.kind, "usb");
        assert_eq!(f.value, "1");

        let query = TargetInfoQuery::VSock(2);
        let f = format_query(&query);
        assert_eq!(f.kind, "vsock");
        assert_eq!(f.value, "2");
    }

    #[test]
    fn test_format_target_state() {
        let state = TargetState::Unknown;
        assert_eq!(format_target_state(&state), "in an unknown state");

        let state = TargetState::Zedboot;
        assert_eq!(format_target_state(&state), "in zedboot");

        let state = TargetState::Fastboot(discovery::FastbootTargetState {
            serial_number: "1234".to_string(),
            connection_state: FastbootConnectionState::Usb,
        });
        assert_eq!(format_target_state(&state), "in fastboot (1234: Usb)");

        let addr = "192.168.1.1:8080".parse::<SocketAddr>().unwrap();
        let state =
            TargetState::Product { addrs: vec![addr.into()], serial: Some("1234".to_string()) };
        assert_eq!(
            format_target_state(&state),
            "in product state (addrs: [192.168.1.1:8080], serial: \"1234\")"
        );

        let state = TargetState::Product { addrs: vec![addr.into()], serial: None };
        assert_eq!(format_target_state(&state), "in product state (addrs: [192.168.1.1:8080])");

        let addr = "192.168.1.1:0".parse::<SocketAddr>().unwrap();
        let state =
            TargetState::Product { addrs: vec![addr.into()], serial: Some("1234".to_string()) };
        assert_eq!(
            format_target_state(&state),
            "in product state (addrs: [192.168.1.1], serial: \"1234\")"
        );
    }

    #[test]
    fn test_format_target_info() {
        let info = ffx::TargetInfo {
            addresses: Some(vec![ffx::TargetAddrInfo::IpPort(ffx::TargetIpPort {
                ip: fnet::IpAddress::Ipv6(fnet::Ipv6Address {
                    addr: [0xfe, 0x80, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1],
                }),
                scope_id: 2,
                port: 22,
            })]),
            ..Default::default()
        };
        assert_eq!(format_target_info(&info), "addresses: [[fe80::1%2]:22]");
    }

    #[test]
    fn test_format_mdns_event() {
        let info =
            ffx::TargetInfo { nodename: Some("test-nodename".to_string()), ..Default::default() };
        let info_str = format_target_info(&info);

        let event = ffx::MdnsEventType::TargetFound(info.clone());
        assert_eq!(format_mdns_event(&event), format!("device found: {info_str}"));

        let event = ffx::MdnsEventType::TargetRediscovered(info.clone());
        assert_eq!(format_mdns_event(&event), format!("device rediscovered: {info_str}"));

        let event = ffx::MdnsEventType::TargetExpired(info.clone());
        assert_eq!(format_mdns_event(&event), format!("device expired: {info_str}"));

        let event = ffx::MdnsEventType::SocketBound(ffx::MdnsBindEvent {
            port: Some(1234),
            ..Default::default()
        });
        assert_eq!(format_mdns_event(&event), "binding on socket: 1234");

        let event = ffx::MdnsEventType::SocketBound(ffx::MdnsBindEvent {
            port: None,
            ..Default::default()
        });
        assert_eq!(
            format_mdns_event(&event),
            "mDNS bind event to unspecified socket (this is highly unexpected)"
        );
    }
    #[test]
    fn test_formatting_escapes_control_characters() {
        let info = ffx::TargetInfo {
            nodename: Some("node\x1b[31m_evil\n".to_string()),
            serial_number: Some("serial\x1b]0;hack\x07".to_string()),
            product_config: Some("prod\x1b[2J_clear".to_string()),
            board_config: Some("board\r\0".to_string()),
            ..Default::default()
        };
        let formatted_info = format_target_info(&info);
        assert!(!formatted_info.contains('\x1b'));
        assert!(!formatted_info.contains('\n'));
        assert!(!formatted_info.contains('\x07'));
        assert!(!formatted_info.contains('\r'));
        assert!(!formatted_info.contains('\0'));
        assert_eq!(
            formatted_info,
            "nodename: \"node\\u{1b}[31m_evil\\n\", serial: \"serial\\u{1b}]0;hack\\u{7}\", product: \"prod\\u{1b}[2J_clear\", board: \"board\\r\\u{0}\""
        );

        let state_product =
            TargetState::Product { addrs: vec![], serial: Some("evil\x1b[32m_serial".to_string()) };
        let formatted_state = format_target_state(&state_product);
        assert!(!formatted_state.contains('\x1b'));
        assert_eq!(
            formatted_state,
            "in product state (addrs: [], serial: \"evil\\u{1b}[32m_serial\")"
        );

        let state_fastboot = TargetState::Fastboot(discovery::FastbootTargetState {
            serial_number: "fastboot\x1b[33m_serial".to_string(),
            connection_state: FastbootConnectionState::Usb,
        });
        let formatted_fastboot = format_target_state(&state_fastboot);
        assert!(!formatted_fastboot.contains('\x1b'));
        assert_eq!(formatted_fastboot, "in fastboot (fastboot\\u{1b}[33m_serial: Usb)");

        let query = TargetInfoQuery::NodenameOrId("query\x1b[34m_target".to_string());
        let formatted_query = format_query(&query);
        assert!(!formatted_query.value.contains('\x1b'));
        assert_eq!(formatted_query.value, "query\\u{1b}[34m_target");

        let query_id = TargetInfoQuery::Id("query\x1b[35m_id".to_string());
        let formatted_query_id = format_query(&query_id);
        assert!(!formatted_query_id.value.contains('\x1b'));
        assert_eq!(formatted_query_id.value, "query\\u{1b}[35m_id");
    }
}
