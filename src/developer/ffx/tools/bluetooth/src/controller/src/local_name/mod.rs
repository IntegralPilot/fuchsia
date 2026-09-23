// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::ControllerTool;
use ffx_bluetooth_controller_args::local_name::{LocalNameCommand, LocalNameSubCommand};
use ffx_writer::{SimpleWriter, ToolIO as _};
use fho::Result;
use fuchsia_bluetooth::types::HostInfo;

pub async fn handle_local_name(
    tool: &ControllerTool,
    cmd: &LocalNameCommand,
    writer: &mut SimpleWriter,
    hosts: &Vec<HostInfo>,
) -> Result<()> {
    match &cmd.subcommand {
        // ffx bluetooth controller local-name set
        LocalNameSubCommand::Set(cmd) => {
            tool.set_local_name(cmd.name.clone()).await?;
            writer.line(format!("Setting local name to: {}", cmd.name))?;
        }
        // ffx bluetooth controller local-name get
        LocalNameSubCommand::Get(_cmd) => {
            if let Some(host) = hosts.iter().find(|h| h.active) {
                if let Some(name) = host.local_name.as_ref() {
                    writer.line(format!("Local name: {}", name))?;
                } else {
                    writer.line("Controller has no local name.")?;
                }
            } else {
                writer.line("No controller found.")?;
            }
        }
    }
    Ok(())
}

impl ControllerTool {
    async fn set_local_name(&self, name: String) -> Result<()> {
        Ok(self
            .access_proxy
            .set_local_name(&name)
            .map_err(|err| fho::Error::Unexpected(anyhow::anyhow!("FIDL error: {err}")))?)
    }
}
