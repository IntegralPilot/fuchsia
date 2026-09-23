// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::args::StopCommand;
use async_trait::async_trait;
use discovery::gce_watcher;
use ffx_config::EnvironmentContext;
use ffx_gce::GceContext;
use ffx_gce::models::StopResult;
use ffx_writer::{MachineWriter, ToolIO as _};
use fho::{FfxMain, FfxTool, Result, return_user_error, user_error};
use std::io::Write;

#[derive(FfxTool)]
pub struct StopTool {
    #[command]
    cmd: StopCommand,
    context: EnvironmentContext,
}

#[async_trait(?Send)]
impl FfxMain for StopTool {
    type Writer = MachineWriter<StopResult>;
    type Error = fho::Error;

    async fn main(self, mut writer: Self::Writer) -> Result<()> {
        gce_watcher::Instance::validate_name(&self.cmd.name).map_err(|e| user_error!("{e}"))?;

        let gce = match GceContext::new(self.context, self.cmd.project, self.cmd.zone).await {
            Ok(c) => c,
            Err(e) => return_user_error!("{e}"),
        };

        let instance = gce_watcher::Instance::new(&gce.project, &gce.zone, &self.cmd.name)
            .map_err(|e| user_error!("{e}"))?;

        if let Err(e) = gce.stop_tunnel(&instance.name) {
            log::warn!("Failed to stop local SSH tunnel for '{}': {e}", instance.name);
        }

        let action = if self.cmd.keep {
            if !writer.is_machine() {
                writeln!(
                    writer,
                    "Stopping GCE instance '{}' in {}...",
                    instance.name, instance.zone
                )?;
            }
            let op = gce
                .client
                .stop_instance(&instance.project, &instance.zone, &instance.name)
                .await
                .map_err(|e| user_error!("{e}"))?;

            if let Some(op_name) = op.name {
                gce.client
                    .wait_for_zone_operation(&instance.project, &instance.zone, &op_name)
                    .await
                    .map_err(|e| user_error!("{e}"))?;
            }
            if !writer.is_machine() {
                writeln!(writer, "Instance '{}' stopped.", instance.name)?;
            }
            "stopped"
        } else {
            if !writer.is_machine() {
                writeln!(
                    writer,
                    "Deleting GCE instance '{}' in {}...",
                    instance.name, instance.zone
                )?;
            }
            let op = gce
                .client
                .delete_instance(&instance.project, &instance.zone, &instance.name)
                .await
                .map_err(|e| user_error!("{e}"))?;

            if let Some(op_name) = op.name {
                gce.client
                    .wait_for_zone_operation(&instance.project, &instance.zone, &op_name)
                    .await
                    .map_err(|e| user_error!("{e}"))?;
            }
            if !writer.is_machine() {
                writeln!(writer, "Instance '{}' deleted.", instance.name)?;
            }
            "deleted"
        };

        let result = StopResult {
            name: instance.name,
            project: instance.project,
            zone: instance.zone,
            action: action.to_string(),
        };

        output_stop_result(&result, &mut writer)
    }
}

fn output_stop_result(result: &StopResult, writer: &mut MachineWriter<StopResult>) -> Result<()> {
    if writer.is_machine() {
        writer.machine(result)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ffx_writer::{Format, TestBuffers};

    #[test]
    fn test_output_stop_result_machine_json() {
        let test_buffers = TestBuffers::default();
        let mut writer = MachineWriter::new_test(Some(Format::Json), &test_buffers);
        let result = StopResult {
            name: "test-vm".to_string(),
            project: "test-proj".to_string(),
            zone: "us-central1-a".to_string(),
            action: "stopped".to_string(),
        };

        output_stop_result(&result, &mut writer).unwrap();

        let (stdout, stderr) = test_buffers.into_strings();
        assert!(stderr.is_empty());
        let parsed: StopResult = serde_json::from_str(&stdout).expect("valid json output");
        assert_eq!(parsed, result);
    }

    #[test]
    fn test_output_stop_result_text() {
        let test_buffers = TestBuffers::default();
        let mut writer = MachineWriter::new_test(None, &test_buffers);
        let result = StopResult {
            name: "test-vm".to_string(),
            project: "test-proj".to_string(),
            zone: "us-central1-a".to_string(),
            action: "deleted".to_string(),
        };

        output_stop_result(&result, &mut writer).unwrap();

        let (stdout, stderr) = test_buffers.into_strings();
        assert!(stderr.is_empty());
        assert!(stdout.is_empty());
    }
}
