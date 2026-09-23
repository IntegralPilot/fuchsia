// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_hardware_usb_policy as fpolicy;
use fidl_fuchsia_usb_policy as usb_policy;
use std::fmt;

struct DisplayableDeviceState(fpolicy::DeviceState);

impl fmt::Display for DisplayableDeviceState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            fpolicy::DeviceState::NotAttached => write!(f, "Not Attached"),
            fpolicy::DeviceState::Attached => write!(f, "Attached"),
            fpolicy::DeviceState::Powered => write!(f, "Powered"),
            fpolicy::DeviceState::Default => write!(f, "Default"),
            fpolicy::DeviceState::Address => write!(f, "Address"),
            fpolicy::DeviceState::Configured => write!(f, "Configured"),
            fpolicy::DeviceState::Suspended => write!(f, "Suspended"),
            _ => write!(f, "Unknown"),
        }
    }
}

fn format_function_status(
    status: Option<usb_policy::FunctionStatus>,
) -> (&'static str, &'static str) {
    match status {
        Some(usb_policy::FunctionStatus::Connected) => ("[CONNECTED]  ", "Connected"),
        Some(usb_policy::FunctionStatus::Disconnected) => ("[DISCONN]    ", "Disconnected"),
        Some(usb_policy::FunctionStatus::Disabled) => ("[DISABLED]   ", "Disabled"),
        Some(usb_policy::FunctionStatus::Error) => ("[ERROR]       ", "Error"),
        Some(_) | None => ("[UNKNOWN]    ", "Unknown"),
    }
}

pub fn format_dashboard(report: &usb_policy::HealthReport) -> String {
    let mut out = String::new();
    out.push_str("=========================== USB HEALTH DASHBOARD ===========================\n");

    let cable_tag = match report.cable_status {
        Some(usb_policy::CableStatus::Connected) => "[CONNECTED]  ",
        Some(usb_policy::CableStatus::Disconnected) => "[DISCONN]    ",
        Some(_) | None => "[UNKNOWN]    ",
    };
    let cable_desc = match report.cable_status {
        Some(usb_policy::CableStatus::Connected) => "Connected (VBUS present)",
        Some(usb_policy::CableStatus::Disconnected) => "Disconnected",
        Some(_) | None => "Unknown",
    };
    out.push_str(&format!("  {}  Cable:          {}\n", cable_tag, cable_desc));

    let ctrl_tag = match report.state {
        Some(fpolicy::DeviceState::Configured) => "[CONNECTED]  ",
        Some(fpolicy::DeviceState::Address) => "[ADDRESS]    ",
        Some(
            fpolicy::DeviceState::Attached
            | fpolicy::DeviceState::Powered
            | fpolicy::DeviceState::Default,
        ) => "[ENUMERATING]",
        Some(fpolicy::DeviceState::Suspended) => "[SUSPENDED]  ",
        Some(fpolicy::DeviceState::NotAttached) => "[DISCONN]    ",
        _ => "[UNKNOWN]    ",
    };

    let ctrl_state_str = report
        .state
        .map(|s| DisplayableDeviceState(s).to_string())
        .unwrap_or_else(|| "Unknown".to_string());
    let addr_str = report.address.map(|a| a.to_string()).unwrap_or_else(|| "None".to_string());
    out.push_str(&format!(
        "  {}  USB Controller: State: {}, Address: {}\n",
        ctrl_tag, ctrl_state_str, addr_str
    ));

    let (adb_tag, _) = format_function_status(report.adb.as_ref().and_then(|a| a.status));
    let mut adb_desc =
        report.adb.as_ref().and_then(|a| a.details.as_deref()).unwrap_or("Unknown").to_string();
    if let Some(state) = report.adb.as_ref().and_then(|a| a.driver_state.as_deref()) {
        adb_desc = format!("{} (State: {})", adb_desc, state);
    }
    out.push_str(&format!("  {}  ADB:            {}\n", adb_tag, adb_desc));

    let (cdc_tag, _) = format_function_status(report.cdc_ethernet.as_ref().and_then(|c| c.status));
    let cdc_desc = report
        .cdc_ethernet
        .as_ref()
        .and_then(|c| c.details.as_deref())
        .unwrap_or("Unknown")
        .to_string();
    out.push_str(&format!("  {}  CDC Ethernet:   {}\n", cdc_tag, cdc_desc));

    let (vsock_tag, _) = format_function_status(report.vsock.as_ref().and_then(|v| v.status));
    let mut vsock_desc =
        report.vsock.as_ref().and_then(|v| v.details.as_deref()).unwrap_or("Unknown").to_string();
    if let Some(channels) = report.vsock.as_ref().and_then(|v| v.active_channels) {
        vsock_desc = format!("{} (Channels: {})", vsock_desc, channels);
    }
    out.push_str(&format!("  {}  VSOCK:          {}\n", vsock_tag, vsock_desc));

    out.push_str("============================================================================");
    out
}

pub fn format_verbose(report: &usb_policy::HealthReport) -> String {
    let mut out = String::new();
    out.push_str("======================= USB HEALTH REPORT (VERBOSE) ========================\n");

    out.push_str("► Cable & Physical Layer\n");
    let cable_str = match report.cable_status {
        Some(usb_policy::CableStatus::Connected) => "Connected (VBUS present)",
        Some(usb_policy::CableStatus::Disconnected) => "Disconnected",
        Some(_) | None => "Unknown",
    };

    out.push_str(&format!("  • Connection:        {}\n\n", cable_str));

    out.push_str("► USB Controller Subsystem\n");
    let state_str = report
        .state
        .map(|s| DisplayableDeviceState(s).to_string())
        .unwrap_or_else(|| "Unknown".to_string());
    out.push_str(&format!("  • Device State:      {}\n", state_str));
    let addr_str = report.address.map(|a| a.to_string()).unwrap_or_else(|| "None".to_string());
    out.push_str(&format!("  • Device Address:    {}\n\n", addr_str));

    out.push_str("► ADB Subsystem\n");
    let (_, adb_status_str) = format_function_status(report.adb.as_ref().and_then(|a| a.status));
    out.push_str(&format!("  • Status:            {}\n", adb_status_str));
    if let Some(state) = report.adb.as_ref().and_then(|a| a.driver_state.as_deref()) {
        out.push_str(&format!("  • Driver State:      {}\n", state));
    }
    if let Some(online) = report.adb.as_ref().and_then(|a| a.online) {
        out.push_str(&format!("  • Online:            {}\n", online));
    }
    if let Some(details) = report.adb.as_ref().and_then(|a| a.details.as_deref()) {
        out.push_str(&format!("  • Details:           {}\n", details));
    }
    out.push('\n');

    out.push_str("► CDC Ethernet Subsystem\n");
    let (_, cdc_status_str) =
        format_function_status(report.cdc_ethernet.as_ref().and_then(|c| c.status));
    out.push_str(&format!("  • Status:            {}\n", cdc_status_str));
    if let Some(online) = report.cdc_ethernet.as_ref().and_then(|c| c.online) {
        out.push_str(&format!("  • Online:            {}\n", online));
    }
    if let Some(details) = report.cdc_ethernet.as_ref().and_then(|c| c.details.as_deref()) {
        out.push_str(&format!("  • Details:           {}\n", details));
    }
    out.push('\n');

    out.push_str("► VSOCK Subsystem\n");
    let (_, vsock_status_str) =
        format_function_status(report.vsock.as_ref().and_then(|v| v.status));
    out.push_str(&format!("  • Status:            {}\n", vsock_status_str));
    if let Some(state) = report.vsock.as_ref().and_then(|v| v.driver_state.as_deref()) {
        out.push_str(&format!("  • Driver State:      {}\n", state));
    }
    if let Some(online) = report.vsock.as_ref().and_then(|v| v.online) {
        out.push_str(&format!("  • Online:            {}\n", online));
    }
    if let Some(channels) = report.vsock.as_ref().and_then(|v| v.active_channels) {
        out.push_str(&format!("  • Active Channels:   {}\n", channels));
    }
    if let Some(details) = report.vsock.as_ref().and_then(|v| v.details.as_deref()) {
        out.push_str(&format!("  • Details:           {}\n", details));
    }

    out.push_str("\n============================================================================");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_dashboard_configured() {
        let report = usb_policy::HealthReport {
            state: Some(fpolicy::DeviceState::Configured),
            address: Some(22),
            cable_status: Some(usb_policy::CableStatus::Connected),
            adb: Some(usb_policy::AdbHealth {
                status: Some(usb_policy::FunctionStatus::Connected),
                details: Some("Active and connected".to_string()),
                driver_state: Some("Online".to_string()),
                online: Some(true),
                ..Default::default()
            }),
            cdc_ethernet: Some(usb_policy::CdcEthernetHealth {
                status: Some(usb_policy::FunctionStatus::Connected),
                details: Some("Active and connected".to_string()),
                online: Some(true),
                ..Default::default()
            }),
            vsock: Some(usb_policy::VsockHealth {
                status: Some(usb_policy::FunctionStatus::Connected),
                details: Some("Active and connected".to_string()),
                active_channels: Some(1),
                online: Some(true),
                ..Default::default()
            }),
            ..Default::default()
        };

        let dashboard = format_dashboard(&report);
        assert!(dashboard.contains("[CONNECTED]    Cable:          Connected (VBUS present)"));
        assert!(
            dashboard.contains("[CONNECTED]    USB Controller: State: Configured, Address: 22\n")
        );
        assert!(
            dashboard
                .contains("[CONNECTED]    ADB:            Active and connected (State: Online)")
        );
        assert!(dashboard.contains("[CONNECTED]    CDC Ethernet:   Active and connected"));
        assert!(
            dashboard.contains("[CONNECTED]    VSOCK:          Active and connected (Channels: 1)")
        );
    }

    #[test]
    fn test_format_dashboard_disconnected() {
        let report = usb_policy::HealthReport {
            state: Some(fpolicy::DeviceState::NotAttached),
            address: Some(0),
            cable_status: Some(usb_policy::CableStatus::Disconnected),
            adb: Some(usb_policy::AdbHealth {
                status: Some(usb_policy::FunctionStatus::Disconnected),
                details: Some("Cable disconnected".to_string()),
                ..Default::default()
            }),
            cdc_ethernet: Some(usb_policy::CdcEthernetHealth {
                status: Some(usb_policy::FunctionStatus::Disconnected),
                details: Some("Cable disconnected".to_string()),
                ..Default::default()
            }),
            vsock: Some(usb_policy::VsockHealth {
                status: Some(usb_policy::FunctionStatus::Disconnected),
                details: Some("Cable disconnected".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        };

        let dashboard = format_dashboard(&report);
        assert!(dashboard.contains("[DISCONN]      Cable:          Disconnected"));
        assert!(
            dashboard.contains("[DISCONN]      USB Controller: State: Not Attached, Address: 0")
        );
        assert!(dashboard.contains("[DISCONN]      ADB:            Cable disconnected"));
    }

    #[test]
    fn test_format_dashboard_enumerating() {
        let report = usb_policy::HealthReport {
            state: Some(fpolicy::DeviceState::Default),
            address: Some(0),
            cable_status: Some(usb_policy::CableStatus::Connected),
            adb: Some(usb_policy::AdbHealth {
                status: Some(usb_policy::FunctionStatus::Disconnected),
                details: Some("Enumerating / Waiting for host configuration".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        };

        let dashboard = format_dashboard(&report);
        assert!(dashboard.contains("[CONNECTED]    Cable:          Connected (VBUS present)"));
        assert!(dashboard.contains("[ENUMERATING]  USB Controller: State: Default, Address: 0"));
        assert!(dashboard.contains(
            "[DISCONN]      ADB:            Enumerating / Waiting for host configuration"
        ));
    }

    #[test]
    fn test_format_dashboard_disabled_and_error() {
        let report = usb_policy::HealthReport {
            state: Some(fpolicy::DeviceState::Configured),
            address: Some(5),
            cable_status: Some(usb_policy::CableStatus::Connected),
            adb: Some(usb_policy::AdbHealth {
                status: Some(usb_policy::FunctionStatus::Disabled),
                details: Some("Not configured".to_string()),
                ..Default::default()
            }),
            vsock: Some(usb_policy::VsockHealth {
                status: Some(usb_policy::FunctionStatus::Error),
                details: Some("Endpoint error".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        };

        let dashboard = format_dashboard(&report);
        assert!(dashboard.contains("[DISABLED]     ADB:            Not configured"));
        assert!(dashboard.contains("[ERROR]         VSOCK:          Endpoint error"));
    }

    #[test]
    fn test_format_dashboard_empty_report() {
        let report = usb_policy::HealthReport::default();
        let dashboard = format_dashboard(&report);
        assert!(dashboard.contains("[UNKNOWN]      Cable:          Unknown"));
        assert!(dashboard.contains("[UNKNOWN]      USB Controller: State: Unknown, Address: None"));
    }

    #[test]
    fn test_format_verbose_complete() {
        let report = usb_policy::HealthReport {
            state: Some(fpolicy::DeviceState::Configured),
            address: Some(22),
            cable_status: Some(usb_policy::CableStatus::Connected),
            adb: Some(usb_policy::AdbHealth {
                status: Some(usb_policy::FunctionStatus::Connected),
                details: Some("Active and connected".to_string()),
                driver_state: Some("Online".to_string()),
                online: Some(true),
                ..Default::default()
            }),
            cdc_ethernet: Some(usb_policy::CdcEthernetHealth {
                status: Some(usb_policy::FunctionStatus::Connected),
                details: Some("Link Up".to_string()),
                online: Some(true),
                ..Default::default()
            }),
            vsock: Some(usb_policy::VsockHealth {
                status: Some(usb_policy::FunctionStatus::Connected),
                active_channels: Some(2),
                details: Some("Active and connected".to_string()),
                driver_state: Some("Configured".to_string()),
                online: Some(true),
                ..Default::default()
            }),
            ..Default::default()
        };

        let verbose = format_verbose(&report);
        assert!(verbose.contains("► Cable & Physical Layer"));
        assert!(verbose.contains("• Connection:        Connected (VBUS present)"));
        assert!(verbose.contains("► USB Controller Subsystem"));
        assert!(verbose.contains("• Device State:      Configured"));
        assert!(verbose.contains("• Device Address:    22"));
        assert!(verbose.contains("► ADB Subsystem"));
        assert!(verbose.contains("• Driver State:      Online"));
        assert!(verbose.contains("• Online:            true"));
        assert!(verbose.contains("► CDC Ethernet Subsystem"));
        assert!(verbose.contains("• Online:            true"));
        assert!(verbose.contains("• Details:           Link Up"));
        assert!(verbose.contains("► VSOCK Subsystem"));
        assert!(verbose.contains("• Driver State:      Configured"));
        assert!(verbose.contains("• Online:            true"));
        assert!(verbose.contains("• Active Channels:   2"));
    }

    #[test]
    fn test_format_verbose_empty_report() {
        let report = usb_policy::HealthReport::default();
        let verbose = format_verbose(&report);
        assert!(verbose.contains("• Connection:        Unknown"));
        assert!(verbose.contains("• Device State:      Unknown"));
        assert!(verbose.contains("• Device Address:    None"));
        assert!(verbose.contains("► ADB Subsystem\n  • Status:            Unknown"));
        assert!(verbose.contains("► CDC Ethernet Subsystem\n  • Status:            Unknown"));
        assert!(verbose.contains("► VSOCK Subsystem\n  • Status:            Unknown"));
    }
}
