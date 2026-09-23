// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub mod args;
mod collaborative_reboot;
pub mod connector;
mod debugcmd;
mod suspend;

use anyhow::{Context, Result};
use args::{PowerCommand, PowerSubCommand};
use connector::Connector;
use std::io::Write;

pub async fn power(
    cmd: PowerCommand,
    connector: impl Connector,
    writer: &mut dyn Write,
) -> Result<()> {
    match cmd.subcommand {
        PowerSubCommand::Suspend(subcmd) => {
            let system_activity_control = connector
                .get_system_activity_control()
                .await
                .context("Failed to get system_activity_control")?;
            suspend::suspend(subcmd, writer, system_activity_control)
                .await
                .context("suspend subcommand failed")?;
        }
        PowerSubCommand::Debugcmd(subcmd) => {
            let debug_proxy =
                connector.get_debug().await.context("Failed to get power manager debug proxy")?;
            debugcmd::debugcmd(subcmd, debug_proxy).await.context("debugcmd subcommand failed")?;
        }
        PowerSubCommand::CollaborativeReboot(subcmd) => {
            let reboot_initiator = connector
                .get_reboot_initiator()
                .await
                .context("Failed to get system_activity_control")?;
            collaborative_reboot::collaborative_reboot(writer, subcmd, reboot_initiator)
                .await
                .context("collaborative-reboot subcommand failed")?;
        }
    };
    Ok(())
}
