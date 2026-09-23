// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Context as _;
use fdomain_client::fidl::{DiscoverableProtocolMarker, Proxy as _};
use fdomain_fuchsia_diagnostics::{
    ClientSelectorConfiguration, DataType, Format, StreamMode, StreamParameters,
};
use fdomain_fuchsia_diagnostics_host::ArchiveAccessorMarker;
use fdomain_fuchsia_usb_policy as usb_policy;
use ffx_writer::{MachineWriter, ToolIO as _};
use fho::Result;
use futures::AsyncReadExt as _;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use target_holders::RemoteControlProxyHolder;

use crate::{DiagnosticsCommand, device_state_to_str};

const TIMEOUT: Duration = Duration::from_secs(15);

#[derive(fho::FfxTool)]
pub struct DiagnosticsTool {
    #[command]
    pub cmd: DiagnosticsCommand,
    pub rcs_holder: fho::Result<RemoteControlProxyHolder>,
}

#[async_trait::async_trait(?Send)]
impl fho::FfxMain for DiagnosticsTool {
    type Writer = MachineWriter<DiagnosticsOutput>;
    type Error = fho::Error;

    async fn main(self, mut writer: Self::Writer) -> Result<()> {
        let out = self.run(&mut writer).await?;
        writer.machine(&out)?;
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SubsystemHealthReport {
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mac_address: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub online: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub driver_state: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_channels: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UsbHealthReport {
    pub controller_state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cable_status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub adb: Option<SubsystemHealthReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cdc_ethernet: Option<SubsystemHealthReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vsock: Option<SubsystemHealthReport>,
}

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct DiagnosticsOutput {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub health: Option<UsbHealthReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inspect: Option<Vec<serde_json::Value>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<String>,
}

fn print_dashboard_report(
    report: &UsbHealthReport,
    writer: &mut MachineWriter<DiagnosticsOutput>,
) -> Result<()> {
    writer
        .line("\n=========================== USB HEALTH DASHBOARD ===========================")?;
    let cable_tag = match report.cable_status.as_deref() {
        Some("Connected") => "[CONNECTED] ",
        Some("Disconnected") => "[DISCONN]   ",
        _ => "[UNKNOWN]   ",
    };
    let cable_desc = match report.cable_status.as_deref() {
        Some("Connected") => "Connected (VBUS present)",
        Some("Disconnected") => "Disconnected",
        _ => "Unknown",
    };
    writer.line(format!("  {}  Cable:          {}", cable_tag, cable_desc))?;

    let ctrl_tag = match report.controller_state.as_str() {
        "Configured" => "[CONNECTED] ",
        "Address" => "[ADDRESS]   ",
        "Attached" | "Powered" | "Default" => "[ENUMERATING]",
        "Suspended" => "[SUSPENDED] ",
        "Not Attached" => "[NOT ATTACH]",
        _ => "[UNKNOWN]   ",
    };
    let addr_str = report.address.map(|a| a.to_string()).unwrap_or_else(|| "None".to_string());
    writer.line(format!(
        "  {}  USB Controller: State: {}, Address: {}",
        ctrl_tag, report.controller_state, addr_str
    ))?;

    let format_subsystem_line = |name: &str, sub: Option<&SubsystemHealthReport>| -> String {
        let tag = match sub.map(|s| s.status.as_str()) {
            Some("Connected") => "[CONNECTED] ",
            Some("Disconnected") => "[DISCONN]   ",
            Some("Disabled") => "[DISABLED]  ",
            Some("Error") => "[ERROR]     ",
            _ => "[UNKNOWN]   ",
        };
        let mut desc = sub.and_then(|s| s.details.as_deref()).unwrap_or("Unknown").to_string();
        if let Some(sub) = sub {
            if let Some(mac) = &sub.mac_address {
                desc = format!("{} (MAC: {})", desc, mac);
            } else if let Some(channels) = sub.active_channels {
                desc = format!("{} (Channels: {})", desc, channels);
            } else if let Some(driver_state) = &sub.driver_state {
                desc = format!("{} (State: {})", desc, driver_state);
            }
        }

        format!("  {}  {:14}  {}", tag, name, desc)
    };

    writer.line(format_subsystem_line("ADB:", report.adb.as_ref()))?;
    writer.line(format_subsystem_line("CDC Ethernet:", report.cdc_ethernet.as_ref()))?;
    writer.line(format_subsystem_line("VSOCK:", report.vsock.as_ref()))?;
    writer.line("============================================================================")?;
    Ok(())
}

fn print_verbose_report(
    report: &UsbHealthReport,
    writer: &mut MachineWriter<DiagnosticsOutput>,
) -> Result<()> {
    writer.line("\n======================= USB HEALTH REPORT (VERBOSE) =======================")?;
    writer.line("► Cable & Physical Layer")?;
    let cable_str = match report.cable_status.as_deref() {
        Some("Connected") => "Connected (VBUS present)",
        Some("Disconnected") => "Disconnected",
        _ => "Unknown",
    };
    writer.line(format!("  • Connection:        {}", cable_str))?;

    writer.line("\n► USB Controller Subsystem")?;
    writer.line(format!("  • Device State:      {}", report.controller_state))?;
    let addr_str =
        report.address.map(|a| a.to_string()).unwrap_or_else(|| "None / Unassigned".to_string());
    writer.line(format!("  • Device Address:    {}", addr_str))?;

    writer.line("\n► ADB Subsystem")?;
    let adb_status = report.adb.as_ref().map(|a| a.status.as_str()).unwrap_or("Unknown");
    writer.line(format!("  • Status:            {}", adb_status))?;
    if let Some(state) = report.adb.as_ref().and_then(|a| a.driver_state.as_deref()) {
        writer.line(format!("  • Driver State:      {}", state))?;
    }
    if let Some(online) = report.adb.as_ref().and_then(|a| a.online) {
        writer.line(format!("  • Online:            {}", online))?;
    }
    if let Some(details) = report.adb.as_ref().and_then(|a| a.details.as_deref()) {
        writer.line(format!("  • Details:           {}", details))?;
    }

    writer.line("\n► CDC Ethernet Subsystem")?;
    let cdc_status = report.cdc_ethernet.as_ref().map(|c| c.status.as_str()).unwrap_or("Unknown");
    writer.line(format!("  • Status:            {}", cdc_status))?;
    if let Some(mac) = report.cdc_ethernet.as_ref().and_then(|c| c.mac_address.as_deref()) {
        writer.line(format!("  • MAC Address:       {}", mac))?;
    }
    if let Some(online) = report.cdc_ethernet.as_ref().and_then(|c| c.online) {
        writer.line(format!("  • Online:            {}", online))?;
    }
    if let Some(details) = report.cdc_ethernet.as_ref().and_then(|c| c.details.as_deref()) {
        writer.line(format!("  • Details:           {}", details))?;
    }

    writer.line("\n► VSOCK Subsystem")?;
    let vsock_status = report.vsock.as_ref().map(|v| v.status.as_str()).unwrap_or("Unknown");
    writer.line(format!("  • Status:            {}", vsock_status))?;
    if let Some(state) = report.vsock.as_ref().and_then(|v| v.driver_state.as_deref()) {
        writer.line(format!("  • Driver State:      {}", state))?;
    }
    if let Some(online) = report.vsock.as_ref().and_then(|v| v.online) {
        writer.line(format!("  • Online:            {}", online))?;
    }
    if let Some(channels) = report.vsock.as_ref().and_then(|v| v.active_channels) {
        writer.line(format!("  • Active Channels:   {}", channels))?;
    }
    if let Some(details) = report.vsock.as_ref().and_then(|v| v.details.as_deref()) {
        writer.line(format!("  • Details:           {}", details))?;
    }
    writer.line("")?;
    Ok(())
}

fn map_function_status(status: Option<usb_policy::FunctionStatus>) -> String {
    match status {
        Some(usb_policy::FunctionStatus::Connected) => "Connected".to_string(),
        Some(usb_policy::FunctionStatus::Disconnected) => "Disconnected".to_string(),
        Some(usb_policy::FunctionStatus::Disabled) => "Disabled".to_string(),
        Some(usb_policy::FunctionStatus::Error) => "Error".to_string(),
        _ => "Unknown".to_string(),
    }
}

fn find_inspect_property_recursive<T, F>(node: &serde_json::Value, predicate: &F) -> Option<T>
where
    F: Fn(&str, &serde_json::Value) -> Option<T>,
{
    match node {
        serde_json::Value::Object(map) => {
            for (k, v) in map {
                if let Some(res) = predicate(k, v) {
                    return Some(res);
                }
                if let Some(res) = find_inspect_property_recursive(v, predicate) {
                    return Some(res);
                }
            }
            None
        }
        serde_json::Value::Array(arr) => {
            for item in arr {
                if let Some(res) = find_inspect_property_recursive(item, predicate) {
                    return Some(res);
                }
            }
            None
        }
        _ => None,
    }
}

pub(crate) fn enrich_report_with_inspect(
    report: &mut UsbHealthReport,
    inspect_items: &[serde_json::Value],
) {
    for item in inspect_items {
        let moniker = item.pointer("/moniker").and_then(|m| m.as_str()).unwrap_or("");
        let payload = item.pointer("/payload").unwrap_or(item);

        // Extract CDC Ethernet telemetry
        if moniker.contains("cdc") || moniker.contains("ethernet") {
            if let Some(cdc) = report.cdc_ethernet.as_mut() {
                if cdc.mac_address.is_none() {
                    if let Some(mac) = find_inspect_property_recursive(payload, &|k, v| {
                        if k == "mac_address" { v.as_str().map(|s| s.to_string()) } else { None }
                    }) {
                        cdc.mac_address = Some(mac);
                    }
                }
                if cdc.online.is_none() {
                    if let Some(online) = find_inspect_property_recursive(payload, &|k, v| {
                        if k == "online" { v.as_bool() } else { None }
                    }) {
                        cdc.online = Some(online);
                    }
                }
            }
        }

        // Extract ADB telemetry
        if moniker.contains("adb") {
            if let Some(adb) = report.adb.as_mut() {
                if adb.online.is_none() {
                    if let Some(online) = find_inspect_property_recursive(payload, &|k, v| {
                        if k == "online" { v.as_bool() } else { None }
                    }) {
                        adb.online = Some(online);
                    }
                }
                if adb.driver_state.is_none() {
                    if let Some(state) = find_inspect_property_recursive(payload, &|k, v| {
                        if k == "state" || k == "driver_state" {
                            v.as_str().map(|s| s.to_string())
                        } else {
                            None
                        }
                    }) {
                        adb.driver_state = Some(state);
                    }
                }
            }
        }

        // Extract VSOCK telemetry
        if moniker.contains("vsock") {
            if let Some(vsock) = report.vsock.as_mut() {
                if vsock.online.is_none() {
                    if let Some(online) = find_inspect_property_recursive(payload, &|k, v| {
                        if k == "online" { v.as_bool() } else { None }
                    }) {
                        vsock.online = Some(online);
                    }
                }
                if vsock.driver_state.is_none() {
                    if let Some(state) = find_inspect_property_recursive(payload, &|k, v| {
                        if k == "state" || k == "driver_state" {
                            v.as_str().map(|s| s.to_string())
                        } else {
                            None
                        }
                    }) {
                        vsock.driver_state = Some(state);
                    }
                }
                if vsock.active_channels.is_none() {
                    if let Some(channels) = find_inspect_property_recursive(payload, &|k, v| {
                        if k == "active_channels" || k == "channels" {
                            v.as_u64().map(|n| n as u32)
                        } else {
                            None
                        }
                    }) {
                        vsock.active_channels = Some(channels);
                    }
                }
            }
        }
    }
}

impl DiagnosticsTool {
    pub async fn run(
        &self,
        writer: &mut MachineWriter<DiagnosticsOutput>,
    ) -> Result<DiagnosticsOutput> {
        let mut out = DiagnosticsOutput::default();

        let default_mode = !self.cmd.health && !self.cmd.inspect && !self.cmd.all;
        let run_health = self.cmd.health || self.cmd.all || default_mode;
        let run_inspect = self.cmd.inspect || self.cmd.all;

        let rcs = self.rcs_holder.as_ref().map_err(|e| {
            fho::Error::User(anyhow::anyhow!(
                "Could not connect to target for USB diagnostics: {e}\n\n\
                 Tip: If the target is unreachable over the network, you can run `usb-cli health` directly in a serial console."
            ))
        })?;

        let inspect_res =
            if run_inspect || run_health { query_usb_inspect(rcs).await.ok() } else { None };

        if run_health {
            match query_usb_health(rcs).await {
                Ok(mut report) => {
                    if let Some((inspect_items, _)) = inspect_res.as_ref() {
                        enrich_report_with_inspect(&mut report, inspect_items);
                    }
                    if !writer.is_machine() {
                        if self.cmd.verbose {
                            print_verbose_report(&report, writer)?;
                        } else {
                            print_dashboard_report(&report, writer)?;
                        }
                    }
                    out.health = Some(report);
                }
                Err(e) => {
                    let err_str = format!("Could not retrieve USB Health Report: {e}");
                    if !writer.is_machine() {
                        writer.line(&err_str)?;
                    }
                    out.errors.push(err_str);
                }
            }
        }

        if run_inspect {
            if !writer.is_machine() {
                writer.line("\n=== Device-Side USB Inspect Diagnostics ===")?;
            }
            let (inspect_items, parse_errors) = match inspect_res {
                Some(res) => res,
                None => match query_usb_inspect(rcs).await {
                    Ok(res) => res,
                    Err(e) => {
                        let err_str = format!("Could not retrieve USB Inspect Diagnostics: {e:#}");
                        if !writer.is_machine() {
                            writer.line(&err_str)?;
                        }
                        out.errors.push(err_str);
                        (Vec::new(), Vec::new())
                    }
                },
            };
            for err in parse_errors {
                out.errors.push(err);
            }
            if !writer.is_machine() {
                if inspect_items.is_empty() {
                    writer.line("  No USB-related Inspect monikers found.")?;
                } else {
                    for item in &inspect_items {
                        if let Some(moniker) = item.pointer("/moniker").and_then(|m| m.as_str()) {
                            writer.line(format!("\n--- Moniker: {} ---", moniker))?;
                            if let Some(payload) = item.get("payload") {
                                let pretty = serde_json::to_string_pretty(payload)
                                    .unwrap_or_else(|_| "[]".to_string());
                                for line in pretty.lines() {
                                    writer.line(format!("  {line}"))?;
                                }
                            }
                        }
                    }
                }
            }
            out.inspect = Some(inspect_items);
        }

        Ok(out)
    }
}

fn is_usb_moniker(moniker: &str) -> bool {
    let m = moniker.to_lowercase();
    m.contains("usb")
        || m.contains("dwc")
        || m.contains("xhci")
        || m.contains("ehci")
        || m.contains("ohci")
        || m.contains("cdc")
        || m.contains("rndis")
}

pub(crate) fn parse_json_diagnostics_stream(
    raw_bytes: &[u8],
    stream_label: &str,
) -> anyhow::Result<(Vec<serde_json::Value>, Vec<String>)> {
    if raw_bytes.is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }

    let mut items = Vec::new();
    let mut errors = Vec::new();

    let stream = serde_json::Deserializer::from_slice(raw_bytes).into_iter::<serde_json::Value>();
    for res in stream {
        match res {
            Ok(serde_json::Value::Array(arr)) => items.extend(arr),
            Ok(val) => items.push(val),
            Err(e) => errors.push(format!("JSON parsing error in {stream_label} stream: {e}")),
        }
    }

    if items.is_empty() && !errors.is_empty() {
        let preview_len = std::cmp::min(raw_bytes.len(), 500);
        let preview = String::from_utf8_lossy(&raw_bytes[..preview_len]);
        anyhow::bail!(
            "Parsing JSON {} snapshot failed ({} bytes received, {} items parsed, errors: {:?}). Payload preview: {:?}",
            stream_label,
            raw_bytes.len(),
            items.len(),
            errors,
            preview
        );
    }

    Ok((items, errors))
}

pub(crate) async fn connect_target_protocol<P: DiscoverableProtocolMarker>(
    rcs: &RemoteControlProxyHolder,
    primary_moniker: &str,
) -> anyhow::Result<P::Proxy> {
    match rcs::connect_to_protocol::<P>(TIMEOUT, primary_moniker, rcs).await {
        Ok(proxy) => Ok(proxy),
        Err(_) => rcs::toolbox::connect_with_timeout::<P>(rcs, TIMEOUT).await.with_context(|| {
            format!("Connecting to {} at {primary_moniker} or /toolbox", P::PROTOCOL_NAME)
        }),
    }
}

pub(crate) async fn query_usb_health(
    rcs: &RemoteControlProxyHolder,
) -> anyhow::Result<UsbHealthReport> {
    let health_proxy =
        connect_target_protocol::<usb_policy::HealthMarker>(rcs, "core/usb-policy").await?;

    let report_res = health_proxy
        .get_report()
        .await
        .context("Calling get_report on HealthProxy")?
        .map_err(|status| anyhow::anyhow!("get_report returned ZX_STATUS {status}"))?;

    let cable_status = match report_res.cable_status {
        Some(usb_policy::CableStatus::Connected) => Some("Connected".to_string()),
        Some(usb_policy::CableStatus::Disconnected) => Some("Disconnected".to_string()),
        Some(_) | None => None,
    };

    let adb = report_res.adb.map(|a| SubsystemHealthReport {
        status: map_function_status(a.status),
        details: a.details,
        mac_address: None,
        online: a.online,
        driver_state: a.driver_state,
        active_channels: None,
    });

    let cdc_ethernet = report_res.cdc_ethernet.map(|c| SubsystemHealthReport {
        status: map_function_status(c.status),
        details: c.details,
        mac_address: None,
        online: c.online,
        driver_state: None,
        active_channels: None,
    });

    let vsock = report_res.vsock.map(|v| SubsystemHealthReport {
        status: map_function_status(v.status),
        details: v.details,
        mac_address: None,
        online: v.online,
        driver_state: v.driver_state,
        active_channels: v.active_channels,
    });

    Ok(UsbHealthReport {
        controller_state: device_state_to_str(report_res.state).to_string(),
        address: report_res.address,
        cable_status,
        adb,
        cdc_ethernet,
        vsock,
    })
}

pub(crate) async fn connect_archive_accessor(
    rcs: &RemoteControlProxyHolder,
) -> anyhow::Result<fdomain_fuchsia_diagnostics_host::ArchiveAccessorProxy> {
    connect_target_protocol::<ArchiveAccessorMarker>(
        rcs,
        "core/diagnostics-accessors/archive-accessor",
    )
    .await
}

pub(crate) async fn query_usb_inspect(
    rcs: &RemoteControlProxyHolder,
) -> anyhow::Result<(Vec<serde_json::Value>, Vec<String>)> {
    let accessor = connect_archive_accessor(rcs).await?;

    let params = StreamParameters {
        stream_mode: Some(StreamMode::Snapshot),
        data_type: Some(DataType::Inspect),
        format: Some(Format::Json),
        client_selector_configuration: Some(ClientSelectorConfiguration::SelectAll(true)),
        ..Default::default()
    };

    let (mut client, server) = rcs.domain().create_stream_socket();
    accessor
        .stream_diagnostics(&params, server)
        .await
        .context("Invoking stream_diagnostics on ArchiveAccessor")?;

    let mut raw_bytes = Vec::new();
    client.read_to_end(&mut raw_bytes).await.context("Reading diagnostics stream from socket")?;

    let (items, errors) = parse_json_diagnostics_stream(&raw_bytes, "Inspect")?;

    let filtered: Vec<_> = items
        .into_iter()
        .filter(|item| {
            item.pointer("/moniker").and_then(|m| m.as_str()).is_some_and(is_usb_moniker)
        })
        .collect();

    Ok((filtered, errors))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ffx_writer::TestBuffers;

    #[test]
    fn test_print_dashboard_report() {
        let buffers = TestBuffers::default();
        let mut writer = MachineWriter::<DiagnosticsOutput>::new_test(None, &buffers);

        let report = UsbHealthReport {
            controller_state: "Configured".to_string(),
            address: Some(22),
            cable_status: Some("Connected".to_string()),
            adb: Some(SubsystemHealthReport {
                status: "Connected".to_string(),
                details: Some("Active and connected".to_string()),
                driver_state: Some("Online".to_string()),
                online: Some(true),
                mac_address: None,
                active_channels: None,
            }),
            cdc_ethernet: Some(SubsystemHealthReport {
                status: "Connected".to_string(),
                details: Some("Active and connected".to_string()),
                mac_address: Some("02:00:00:00:00:01".to_string()),
                online: Some(true),
                driver_state: None,
                active_channels: None,
            }),
            vsock: Some(SubsystemHealthReport {
                status: "Connected".to_string(),
                details: Some("Active and connected".to_string()),
                active_channels: Some(1),
                online: Some(true),
                driver_state: Some("Configured".to_string()),
                mac_address: None,
            }),
        };

        print_dashboard_report(&report, &mut writer).unwrap();
        let out = buffers.into_stdout_str();
        assert!(out.contains("[CONNECTED]   Cable:          Connected (VBUS present)"));
        assert!(out.contains("[CONNECTED]   USB Controller: State: Configured, Address: 22"));
        assert!(out.contains("[CONNECTED]   ADB:            Active and connected (State: Online)"));
        assert!(out.contains(
            "[CONNECTED]   CDC Ethernet:   Active and connected (MAC: 02:00:00:00:00:01)"
        ));
        assert!(out.contains("[CONNECTED]   VSOCK:          Active and connected (Channels: 1)"));
    }

    #[test]
    fn test_print_verbose_report() {
        let buffers = TestBuffers::default();
        let mut writer = MachineWriter::<DiagnosticsOutput>::new_test(None, &buffers);

        let report = UsbHealthReport {
            controller_state: "Configured".to_string(),
            address: Some(22),
            cable_status: Some("Connected".to_string()),
            adb: Some(SubsystemHealthReport {
                status: "Connected".to_string(),
                details: Some("Active and connected".to_string()),
                driver_state: Some("Online".to_string()),
                online: Some(true),
                mac_address: None,
                active_channels: None,
            }),
            cdc_ethernet: Some(SubsystemHealthReport {
                status: "Connected".to_string(),
                details: Some("Link Up".to_string()),
                mac_address: Some("02:1A:11:00:00:01".to_string()),
                online: Some(true),
                driver_state: None,
                active_channels: None,
            }),
            vsock: Some(SubsystemHealthReport {
                status: "Connected".to_string(),
                details: Some("Active and connected".to_string()),
                driver_state: Some("Configured".to_string()),
                online: Some(true),
                active_channels: Some(2),
                mac_address: None,
            }),
        };

        print_verbose_report(&report, &mut writer).unwrap();
        let out = buffers.into_stdout_str();
        assert!(out.contains("► Cable & Physical Layer"));
        assert!(out.contains("• Connection:        Connected (VBUS present)"));
        assert!(out.contains("► USB Controller Subsystem"));
        assert!(out.contains("• Device State:      Configured"));
        assert!(out.contains("• Device Address:    22"));
        assert!(out.contains("► ADB Subsystem"));
        assert!(out.contains("• Driver State:      Online"));
        assert!(out.contains("• Online:            true"));
        assert!(out.contains("► CDC Ethernet Subsystem"));
        assert!(out.contains("• MAC Address:       02:1A:11:00:00:01"));
        assert!(out.contains("• Online:            true"));
        assert!(out.contains("• Details:           Link Up"));
        assert!(out.contains("► VSOCK Subsystem"));
        assert!(out.contains("• Driver State:      Configured"));
        assert!(out.contains("• Online:            true"));
        assert!(out.contains("• Active Channels:   2"));
    }

    #[test]
    fn test_json_serialization() {
        let report = UsbHealthReport {
            controller_state: "Configured".to_string(),
            address: Some(22),
            cable_status: Some("Connected".to_string()),
            adb: Some(SubsystemHealthReport {
                status: "Connected".to_string(),
                details: Some("Active".to_string()),
                mac_address: None,
                online: None,
                driver_state: None,
                active_channels: None,
            }),
            cdc_ethernet: None,
            vsock: None,
        };

        let json = serde_json::to_string(&report).unwrap();
        assert!(json.contains(r#""controller_state":"Configured""#));
        assert!(json.contains(r#""address":22"#));
        assert!(json.contains(r#""cable_status":"Connected""#));
        assert!(!json.contains("cdc_ethernet"));
    }

    #[test]
    fn test_enrich_report_with_inspect() {
        let inspect_json = vec![
            serde_json::json!({
                "moniker": "bootstrap/driver_manager:usb-peripheral",
                "payload": {
                    "root": {
                        "dci": {
                            "speed": "high",
                            "state": "configured"
                        }
                    }
                }
            }),
            serde_json::json!({
                "moniker": "bootstrap/driver_manager:usb-cdc-function",
                "payload": {
                    "root": {
                        "mac_address": "02:00:00:00:00:01",
                        "online": true
                    }
                }
            }),
        ];

        let mut report = UsbHealthReport {
            controller_state: "Configured".to_string(),
            address: Some(22),
            cable_status: Some("Connected".to_string()),
            adb: None,
            cdc_ethernet: Some(SubsystemHealthReport {
                status: "Connected".to_string(),
                details: Some("Active".to_string()),
                mac_address: None,
                online: None,
                driver_state: None,
                active_channels: None,
            }),
            vsock: None,
        };

        enrich_report_with_inspect(&mut report, &inspect_json);
        assert_eq!(
            report.cdc_ethernet.as_ref().and_then(|c| c.mac_address.as_deref()),
            Some("02:00:00:00:00:01")
        );
        assert_eq!(report.cdc_ethernet.as_ref().and_then(|c| c.online), Some(true));
    }

    #[test]
    fn test_map_function_status() {
        assert_eq!(map_function_status(Some(usb_policy::FunctionStatus::Connected)), "Connected");
        assert_eq!(
            map_function_status(Some(usb_policy::FunctionStatus::Disconnected)),
            "Disconnected"
        );
        assert_eq!(map_function_status(Some(usb_policy::FunctionStatus::Disabled)), "Disabled");
        assert_eq!(map_function_status(Some(usb_policy::FunctionStatus::Error)), "Error");
        assert_eq!(map_function_status(None), "Unknown");
    }

    #[test]
    fn test_parse_json_diagnostics_stream_empty() {
        let (items, errors) = parse_json_diagnostics_stream(b"", "Inspect").unwrap();
        assert!(items.is_empty());
        assert!(errors.is_empty());
    }

    #[test]
    fn test_parse_json_diagnostics_stream_valid() {
        let json_data = br#"[{"moniker":"core/usb-policy","payload":{"root":{}}}]"#;
        let (items, errors) = parse_json_diagnostics_stream(json_data, "Inspect").unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["moniker"], "core/usb-policy");
        assert!(errors.is_empty());
    }

    #[test]
    fn test_parse_json_diagnostics_stream_partial_errors() {
        let json_data = br#"{"moniker":"core/usb-policy"}
{invalid_json}"#;
        let (items, errors) = parse_json_diagnostics_stream(json_data, "Inspect").unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["moniker"], "core/usb-policy");
        assert_eq!(errors.len(), 1);
    }

    #[test]
    fn test_parse_json_diagnostics_stream_all_invalid() {
        let json_data = b"{invalid_json}";
        assert!(parse_json_diagnostics_stream(json_data, "Inspect").is_err());
    }
}
