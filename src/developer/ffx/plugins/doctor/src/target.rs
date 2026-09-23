// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::ShowToolWrapper;
use crate::doctor_ledger::{LedgerMode, LedgerNode, LedgerNodeGuard, LedgerOutcome};
use crate::single_target_diagnostics::run_single_target_diagnostics;
use anyhow::Result;
use ffx_config::EnvironmentContext;
use ffx_target::TargetInfoQuery;
use fidl_fuchsia_developer_ffx::{TargetInfo, TargetState};
use std::io::Write;
use std::time::Duration;
use timeout::timeout;

pub fn target_name(target: &TargetInfo) -> String {
    target.nodename.clone().unwrap_or_else(|| ffx_target::UNKNOWN_TARGET_NAME.to_string())
}

pub async fn check_single_target_locally<W: Write>(
    ledger: &mut LedgerNodeGuard<'_, W>,
    target: &TargetInfo,
    env_context: &EnvironmentContext,
    show_tool: Option<&mut ShowToolWrapper>,
    retry_delay: Duration,
) -> Result<()> {
    let done = check_product_state(ledger, target);
    if done {
        return Ok(());
    }

    let done = check_identify_host(ledger, target, env_context, retry_delay).await;
    if done {
        return Ok(());
    }

    {
        let mut node = ledger.add_node(
            &format!("Running diagnostics against {}", target_name(target)),
            LedgerMode::Verbose,
        );
        run_target_diagnostics(&mut node, target, env_context, retry_delay).await;
    }

    show_target(ledger, target, show_tool).await;
    Ok(())
}

pub async fn check_identify_host<W: Write>(
    ledger: &mut LedgerNodeGuard<'_, W>,
    target: &TargetInfo,
    env_context: &EnvironmentContext,
    retry_delay: Duration,
) -> bool {
    let handle = match discovery::TargetHandle::try_from(target.clone()) {
        Ok(h) => h,
        Err(e) => {
            ledger
                .add_node(&format!("Error while communicating with RCS: {e}"), LedgerMode::Verbose)
                .set_outcome(LedgerOutcome::Failure);
            return true;
        }
    };
    let resolution = match ffx_target::Resolution::from_target_handle(handle) {
        Ok(r) => r,
        Err(e) => {
            ledger
                .add_node(&format!("Error while communicating with RCS: {e}"), LedgerMode::Verbose)
                .set_outcome(LedgerOutcome::Failure);
            return true;
        }
    };

    match timeout(retry_delay, resolution.identify(env_context)).await {
        Ok(Ok(_)) => {
            ledger
                .add(LedgerNode::new("Communicating with RCS".to_string(), LedgerMode::Verbose))
                .set_outcome(LedgerOutcome::Success);
            false
        }
        Ok(Err(e)) => {
            ledger
                .add_node(&format!("Error while communicating with RCS: {e}"), LedgerMode::Verbose)
                .set_outcome(LedgerOutcome::Failure);
            true
        }
        Err(_) => {
            ledger
                .add_node("Timeout while communicating with RCS", LedgerMode::Verbose)
                .set_outcome(LedgerOutcome::Failure);
            true
        }
    }
}

pub fn make_ssh_fix_suggestion(ssh_log: &str) -> Option<&'static str> {
    let lower = ssh_log.to_ascii_lowercase();
    if lower.contains("connection refused") {
        Some("SSH connection was refused. You may need to (re-)establish a tunnel connection.")
    } else if lower.contains("permission denied") {
        Some(
            "SSH connection could not authenticate. You may need to re-provision (pave or flash) your target to ensure SSH keys are appropriately setup.",
        )
    } else {
        None
    }
}

pub async fn run_target_diagnostics<W: Write>(
    ledger: &mut LedgerNodeGuard<'_, W>,
    target: &TargetInfo,
    env_context: &EnvironmentContext,
    retry_delay: Duration,
) {
    match run_single_target_diagnostics(env_context, target.clone(), ledger, retry_delay).await {
        Ok(()) => {}
        Err(e) => {
            let error_msg = format!("{e:#}");
            ledger
                .add_node(
                    &format!("Error encountered in diagnostics: {error_msg}"),
                    LedgerMode::Automatic,
                )
                .set_outcome(LedgerOutcome::Failure);
            if let Some(suggestion) = make_ssh_fix_suggestion(&error_msg) {
                ledger.add_node(suggestion, LedgerMode::Automatic).set_outcome(LedgerOutcome::Info);
            }
        }
    }
}

pub async fn show_target<W: Write>(
    ledger: &mut LedgerNodeGuard<'_, W>,
    target: &TargetInfo,
    show_tool: Option<&mut ShowToolWrapper>,
) {
    if let Some(show_tool) = show_tool {
        let mut node =
            ledger.add_node("Running `ffx target show` against device", LedgerMode::Automatic);
        match show_tool.allocate(target.nodename.clone()).await {
            Ok(_) => {
                node.add(LedgerNode::new(
                    "Allocating proxies for `target show`".to_string(),
                    LedgerMode::Verbose,
                ))
                .set_outcome(LedgerOutcome::Success);
                match show_tool.run().await {
                    Ok((stdout, stderr)) => {
                        node.add(LedgerNode::new(
                            "Executing `ffx target show`".to_string(),
                            LedgerMode::Verbose,
                        ))
                        .set_outcome(LedgerOutcome::Success);
                        node.add(LedgerNode::new(
                            format!("stdout:\n\t{}", stdout.replace("\n", "\n\t"),),
                            LedgerMode::Verbose,
                        ))
                        .set_outcome(LedgerOutcome::Info);
                        if !stderr.is_empty() {
                            node.add(LedgerNode::new(
                                format!("stderr:\n\t{}", stderr.replace("\n", "\n\t")),
                                LedgerMode::Verbose,
                            ))
                            .set_outcome(LedgerOutcome::Info);
                        }
                    }
                    Err(e) => {
                        node.add_node(
                            &format!("Error executing `target show`: {:?}", e),
                            LedgerMode::Verbose,
                        )
                        .set_outcome(LedgerOutcome::Failure);
                    }
                }
            }
            Err(e) => {
                node.add_node(
                    &format!("Error while setting up `target show`: {:?}", e),
                    LedgerMode::Normal,
                )
                .set_outcome(LedgerOutcome::Failure);
            }
        };
        node.set_outcome(LedgerOutcome::Info);
    }
}

pub fn check_product_state<W: Write>(
    ledger: &mut LedgerNodeGuard<'_, W>,
    target: &TargetInfo,
) -> bool {
    match target.target_state {
        None => false,
        Some(TargetState::Unknown | TargetState::Disconnected | TargetState::Product) => false,
        Some(TargetState::Fastboot) => {
            ledger
                .add_node(
                    &format!(
                        "Target found in fastboot mode: {}",
                        target.serial_number.as_deref().unwrap_or("UNKNOWN serial number")
                    ),
                    LedgerMode::Automatic,
                )
                .set_outcome(LedgerOutcome::Success);
            true
        }
        Some(TargetState::Zedboot) => {
            ledger
                .add_node(
                    &format!("Skipping target in zedboot: {}", target_name(target)),
                    LedgerMode::Automatic,
                )
                .set_outcome(LedgerOutcome::SoftWarning);
            true
        }
    }
}

pub async fn check_targets_locally<W: Write>(
    ledger: &mut LedgerNodeGuard<'_, W>,
    target_str: &str,
    env_context: &EnvironmentContext,
    mut show_tool: Option<ShowToolWrapper>,
    retry_delay: Duration,
) -> Result<()> {
    let query = TargetInfoQuery::try_from(target_str)?;
    let targets = {
        let mut discovery_node = ledger.add_node("Searching for targets", LedgerMode::Automatic);
        let find_res = find_targets_locally(env_context, query).await;
        check_target_discovery(&mut discovery_node, find_res)
    };
    if targets.is_empty() {
        return Ok(());
    }
    for target in targets.iter() {
        let mut target_node =
            ledger.add_node(&format!("Target: {}", target_name(target)), LedgerMode::Normal);
        check_single_target_locally(
            &mut target_node,
            target,
            env_context,
            show_tool.as_mut(),
            retry_delay,
        )
        .await?;
    }
    Ok(())
}

pub fn check_target_discovery<W: Write>(
    ledger: &mut LedgerNodeGuard<'_, W>,
    targets_result: Result<Vec<TargetInfo>>,
) -> Vec<TargetInfo> {
    match targets_result {
        Ok(targets) => {
            if !targets.is_empty() {
                ledger
                    .add_node(&format!("{} targets found", targets.len()), LedgerMode::Automatic)
                    .set_outcome(LedgerOutcome::Success);
                targets
            } else {
                ledger
                    .add_node("No targets found!", LedgerMode::Automatic)
                    .set_outcome(LedgerOutcome::Failure);
                vec![]
            }
        }
        Err(e) => {
            ledger
                .add_node(&format!("Error getting targets: {e}"), LedgerMode::Normal)
                .set_outcome(LedgerOutcome::Failure);
            vec![]
        }
    }
}

pub async fn find_targets_locally(
    env_context: &EnvironmentContext,
    query: TargetInfoQuery,
) -> Result<Vec<TargetInfo>> {
    let targets = ffx_target::get_discovered_targets(query, true, true, env_context).await?;
    Ok(targets.into_iter().map(|t| TargetInfo::from(t)).collect::<Vec<TargetInfo>>())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_make_ssh_fix_suggestion() {
        assert_eq!(
            make_ssh_fix_suggestion("ssh: Connection refused"),
            Some("SSH connection was refused. You may need to (re-)establish a tunnel connection.")
        );
        assert_eq!(
            make_ssh_fix_suggestion("connection refused"),
            Some("SSH connection was refused. You may need to (re-)establish a tunnel connection.")
        );
        assert_eq!(
            make_ssh_fix_suggestion("Permission denied (publickey)"),
            Some(
                "SSH connection could not authenticate. You may need to re-provision (pave or flash) your target to ensure SSH keys are appropriately setup."
            )
        );
        assert_eq!(
            make_ssh_fix_suggestion("permission denied"),
            Some(
                "SSH connection could not authenticate. You may need to re-provision (pave or flash) your target to ensure SSH keys are appropriately setup."
            )
        );
        assert_eq!(make_ssh_fix_suggestion("some other error"), None);
    }
}
