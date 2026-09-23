// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::args::ListCommand;
use async_trait::async_trait;
use ffx_config::EnvironmentContext;
use ffx_gce::GceContext;
use ffx_gce::models::Instance;
use ffx_writer::{MachineWriter, ToolIO as _};
use fho::{FfxMain, FfxTool, Result, return_user_error, user_error};
use prettytable::format::FormatBuilder;
use prettytable::{Table, row};
use std::io::Write;

#[derive(FfxTool)]
pub struct ListTool {
    #[command]
    cmd: ListCommand,
    context: EnvironmentContext,
}

#[async_trait(?Send)]
impl FfxMain for ListTool {
    type Writer = MachineWriter<Vec<Instance>>;
    type Error = fho::Error;

    async fn main(self, mut writer: Self::Writer) -> Result<()> {
        let gce = match GceContext::new(self.context, self.cmd.project, self.cmd.zone).await {
            Ok(c) => c,
            Err(e) => return_user_error!("{e}"),
        };

        let instances = gce
            .client
            .list_instances(&gce.project, &gce.zone)
            .await
            .map_err(|e| user_error!("{e}"))?;

        output_instances(&instances, &gce.project, &gce.zone, &mut writer)
    }
}

fn output_instances(
    instances: &[Instance],
    project: &str,
    zone: &str,
    writer: &mut MachineWriter<Vec<Instance>>,
) -> Result<()> {
    if writer.is_machine() {
        writer.machine(&instances.to_vec())?;
    } else if instances.is_empty() {
        writeln!(writer, "No instances found in project '{project}' zone '{zone}'.")?;
    } else {
        let mut table = Table::new();
        let table_format = FormatBuilder::new().padding(0, 2).build();
        table.set_format(table_format);
        table.set_titles(row![
            "NAME",
            "ZONE",
            "MACHINE TYPE",
            "INTERNAL IP",
            "EXTERNAL IP",
            "STATUS"
        ]);
        for inst in instances {
            let name = inst.name.as_deref().unwrap_or("-");
            let machine_type =
                inst.machine_type.as_deref().and_then(|m| m.split('/').next_back()).unwrap_or("-");
            let internal_ip = inst.internal_ip().unwrap_or("-");
            let external_ip = inst.external_ip().unwrap_or("-");
            let status = inst.status.as_deref().unwrap_or("-");

            table.add_row(row!(name, zone, machine_type, internal_ip, external_ip, status));
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
            name: Some("test-inst".to_string()),
            machine_type: Some("zones/us-central1-a/machineTypes/n1-standard-1".to_string()),
            status: Some("RUNNING".to_string()),
            zone: Some("us-central1-a".to_string()),
            network_interfaces: vec![NetworkInterface {
                network_ip: Some("10.0.0.2".to_string()),
                access_configs: vec![AccessConfig {
                    nat_ip: Some("34.0.0.1".to_string()),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    #[test]
    fn test_output_instances_machine_json() {
        let test_buffers = TestBuffers::default();
        let mut writer = MachineWriter::new_test(Some(Format::Json), &test_buffers);
        let instances = vec![sample_instance()];

        output_instances(&instances, "test-proj", "us-central1-a", &mut writer).unwrap();

        let (stdout, stderr) = test_buffers.into_strings();
        assert!(stderr.is_empty());
        let parsed: Vec<Instance> = serde_json::from_str(&stdout).expect("valid json output");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].name.as_deref(), Some("test-inst"));
    }

    #[test]
    fn test_output_instances_text_table() {
        let test_buffers = TestBuffers::default();
        let mut writer = MachineWriter::new_test(None, &test_buffers);
        let instances = vec![sample_instance()];

        output_instances(&instances, "test-proj", "us-central1-a", &mut writer).unwrap();

        let (stdout, stderr) = test_buffers.into_strings();
        assert!(stderr.is_empty());
        assert!(stdout.contains("NAME"));
        assert!(stdout.contains("ZONE"));
        assert!(stdout.contains("test-inst"));
        assert!(stdout.contains("n1-standard-1"));
        assert!(stdout.contains("10.0.0.2"));
        assert!(stdout.contains("34.0.0.1"));
        assert!(stdout.contains("RUNNING"));
    }

    #[test]
    fn test_output_instances_empty() {
        let test_buffers = TestBuffers::default();
        let mut writer = MachineWriter::new_test(None, &test_buffers);

        output_instances(&[], "test-proj", "us-central1-a", &mut writer).unwrap();

        let (stdout, _) = test_buffers.into_strings();
        assert!(stdout.contains("No instances found in project 'test-proj' zone 'us-central1-a'."));
    }
}
