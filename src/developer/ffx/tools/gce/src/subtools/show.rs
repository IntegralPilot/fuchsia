// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::args::ShowCommand;
use async_trait::async_trait;
use discovery::gce_watcher;
use ffx_config::EnvironmentContext;
use ffx_gce::GceContext;
use ffx_gce::models::Instance;
use ffx_writer::{MachineWriter, ToolIO as _};
use fho::{FfxMain, FfxTool, Result, return_user_error, user_error};
use prettytable::format::FormatBuilder;
use prettytable::{Table, row};
use serde::{Deserialize, Serialize};
use std::io::Write;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct InstanceDetails {
    pub name: Option<String>,
    pub status: Option<String>,
    pub machine_type: Option<String>,
    pub zone: String,
    pub project: String,
    pub internal_ip: Option<String>,
    pub external_ip: Option<String>,
    pub serial_endpoint: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub creation_timestamp: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub self_link: Option<String>,
}

impl InstanceDetails {
    pub fn from_instance(
        inst: Instance,
        project: String,
        zone: String,
        serial_endpoint: String,
    ) -> Self {
        let internal_ip = inst.internal_ip().map(ToString::to_string);
        let external_ip = inst.external_ip().map(ToString::to_string);
        Self {
            name: inst.name,
            status: inst.status,
            machine_type: inst.machine_type,
            zone,
            project,
            internal_ip,
            external_ip,
            serial_endpoint,
            creation_timestamp: inst.creation_timestamp,
            self_link: inst.self_link,
        }
    }
}

#[derive(FfxTool)]
pub struct ShowTool {
    #[command]
    cmd: ShowCommand,
    context: EnvironmentContext,
}

#[async_trait(?Send)]
impl FfxMain for ShowTool {
    type Writer = MachineWriter<InstanceDetails>;
    type Error = fho::Error;

    async fn main(self, mut writer: Self::Writer) -> Result<()> {
        gce_watcher::Instance::validate_name(&self.cmd.name).map_err(|e| user_error!("{e}"))?;

        let gce = match GceContext::new(self.context, self.cmd.project, self.cmd.zone).await {
            Ok(c) => c,
            Err(e) => return_user_error!("{e}"),
        };
        let instance = gce_watcher::Instance::new(&gce.project, &gce.zone, &self.cmd.name)
            .map_err(|e| user_error!("{e}"))?;

        let inst = gce
            .client
            .get_instance(&instance.project, &instance.zone, &instance.name)
            .await
            .map_err(|e| user_error!("{e}"))?;

        let serial_endpoint = gce.serial_endpoint();
        let details =
            InstanceDetails::from_instance(inst, instance.project, instance.zone, serial_endpoint);

        output_instance(&details, &mut writer)
    }
}

fn output_instance(
    details: &InstanceDetails,
    writer: &mut MachineWriter<InstanceDetails>,
) -> Result<()> {
    if writer.is_machine() {
        writer.machine(details)?;
    } else {
        writeln!(writer, "Instance Details:")?;
        let table_format = FormatBuilder::new().indent(2).padding(0, 2).build();
        let mut table = Table::new();
        table.set_format(table_format);
        table.add_row(row!["Name:", details.name.as_deref().unwrap_or("-")]);
        table.add_row(row!["Status:", details.status.as_deref().unwrap_or("-")]);
        table.add_row(row!["Machine Type:", details.machine_type.as_deref().unwrap_or("-")]);
        table.add_row(row!["Zone:", &details.zone]);
        table.add_row(row!["Project:", &details.project]);
        table.add_row(row!["Internal IP:", details.internal_ip.as_deref().unwrap_or("-")]);
        table.add_row(row!["External IP:", details.external_ip.as_deref().unwrap_or("-")]);
        table.add_row(row!["Serial Gateway:", &details.serial_endpoint]);
        if let Some(ref created) = details.creation_timestamp {
            table.add_row(row!["Created At:", created]);
        }
        if let Some(ref link) = details.self_link {
            table.add_row(row!["Self Link:", link]);
        }
        table.print(writer).map_err(|e| user_error!("{e}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ffx_gce::models::{AccessConfig, NetworkInterface};
    use ffx_writer::{Format, TestBuffers};

    fn sample_instance() -> Instance {
        Instance {
            name: Some("test-vm".to_string()),
            machine_type: Some("zones/us-central1-a/machineTypes/n2-standard-4".to_string()),
            status: Some("RUNNING".to_string()),
            zone: Some("us-central1-a".to_string()),
            creation_timestamp: Some("2026-09-11T12:00:00.000-07:00".to_string()),
            self_link: Some("https://compute.googleapis.com/...".to_string()),
            network_interfaces: vec![NetworkInterface {
                network_ip: Some("10.128.0.2".to_string()),
                access_configs: vec![AccessConfig {
                    nat_ip: Some("34.120.10.20".to_string()),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    fn sample_details() -> InstanceDetails {
        InstanceDetails::from_instance(
            sample_instance(),
            "test-proj".to_string(),
            "us-central1-a".to_string(),
            "us-central1-ssh-serialport.googleapis.com:9600".to_string(),
        )
    }

    #[test]
    fn test_output_instance_machine_json() {
        let test_buffers = TestBuffers::default();
        let mut writer = MachineWriter::new_test(Some(Format::Json), &test_buffers);
        let details = sample_details();

        output_instance(&details, &mut writer).unwrap();

        let (stdout, stderr) = test_buffers.into_strings();
        assert!(stderr.is_empty());
        let parsed: InstanceDetails = serde_json::from_str(&stdout).expect("valid json output");
        assert_eq!(parsed.name.as_deref(), Some("test-vm"));
        assert_eq!(parsed.status.as_deref(), Some("RUNNING"));
        assert_eq!(parsed.project, "test-proj");
        assert_eq!(parsed.zone, "us-central1-a");
        assert_eq!(parsed.internal_ip.as_deref(), Some("10.128.0.2"));
        assert_eq!(parsed.external_ip.as_deref(), Some("34.120.10.20"));
        assert_eq!(parsed.serial_endpoint, "us-central1-ssh-serialport.googleapis.com:9600");
        assert_eq!(parsed.creation_timestamp.as_deref(), Some("2026-09-11T12:00:00.000-07:00"));
        assert_eq!(parsed.self_link.as_deref(), Some("https://compute.googleapis.com/..."));
    }

    #[test]
    fn test_output_instance_text() {
        let test_buffers = TestBuffers::default();
        let mut writer = MachineWriter::new_test(None, &test_buffers);
        let details = sample_details();

        output_instance(&details, &mut writer).unwrap();

        let (stdout, stderr) = test_buffers.into_strings();
        assert!(stderr.is_empty());
        assert!(stdout.contains("Instance Details:"));
        assert!(stdout.contains("Name:            test-vm"));
        assert!(stdout.contains("Status:          RUNNING"));
        assert!(stdout.contains("Machine Type:    zones/us-central1-a/machineTypes/n2-standard-4"));
        assert!(stdout.contains("Zone:            us-central1-a"));
        assert!(stdout.contains("Project:         test-proj"));
        assert!(stdout.contains("Internal IP:     10.128.0.2"));
        assert!(stdout.contains("External IP:     34.120.10.20"));
        assert!(stdout.contains("Serial Gateway:  us-central1-ssh-serialport.googleapis.com:9600"));
        assert!(stdout.contains("Created At:      2026-09-11T12:00:00.000-07:00"));
        assert!(stdout.contains("Self Link:       https://compute.googleapis.com/..."));
    }
}
