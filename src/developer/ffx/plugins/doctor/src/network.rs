// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::doctor_ledger::{LedgerMode, LedgerNodeGuard, LedgerOutcome};
use crate::gcheck;
use anyhow::Result;
use doctor_utils::{CheckResult, DoctorCheck};
use ffx_command::{ExternalSubToolSuite, FfxCommandLine, ToolSuite};
use ffx_config::EnvironmentContext;
use std::io::Write;

pub async fn run_google_network_checks<W: Write>(
    ledger: &mut LedgerNodeGuard<'_, W>,
    env_context: &EnvironmentContext,
    gchecker: &impl gcheck::GChecker,
) -> Result<()> {
    match ExternalSubToolSuite::from_env(&env_context) {
        Ok(sub_tool_suite) => {
            let command_line_args =
                vec!["ffx".to_string(), "gdoctor".to_string(), "all-safe".to_string()];
            let mut command = FfxCommandLine::new(None, &command_line_args).unwrap();
            let mut ffx_args = vec!["--machine".to_string(), "json".to_string()];
            command.ffx_args.append(&mut ffx_args);
            let workspace_command = sub_tool_suite.find_workspace_tool(&command);
            match workspace_command {
                // If the command exists in the workspace call it and show the results
                Some(wcmd) => {
                    let mut main_node =
                        ledger.add_node("Google Network Checks", LedgerMode::Automatic);
                    let run_res = wcmd.run_and_capture();
                    let (_exit_status, stdout, _stderr) = run_res?;
                    for line in stdout.trim().lines().filter(|l| !l.trim().is_empty()) {
                        match serde_json::from_str::<DoctorCheck>(&line) {
                            Ok(data) => {
                                let node = main_node.add_node(
                                    &format!("{}: {}", data.name, data.message),
                                    LedgerMode::Automatic,
                                );
                                node.set_outcome(match data.result {
                                    CheckResult::Passed => LedgerOutcome::Success,
                                    CheckResult::Failed => LedgerOutcome::Failure,
                                    CheckResult::Info => LedgerOutcome::Info,
                                });
                            }
                            Err(e) => {
                                eprintln!(
                                    "Warning: Failed to parse gdoctor output line as DoctorCheck: {}",
                                    e
                                );
                            }
                        }
                    }
                }
                None => {
                    if gchecker.is_gcorp_machine() {
                        let mut network_check_node =
                            ledger.add_node("Google Network Checks", LedgerMode::Automatic);
                        let node = network_check_node.add_node(
                                &format!(
                                    "Google-corp tool missing, please run `fx add-internal-tools` and `fx build --host //vendor/google/tools/gdoctor`"
                                ),
                                LedgerMode::Automatic,
                            );
                        node.set_outcome(LedgerOutcome::Failure);
                    }
                }
            }
        }
        Err(e) => {
            eprintln!("Warning: could not find subtool suite: {}", e);
        }
    }
    Ok(())
}
