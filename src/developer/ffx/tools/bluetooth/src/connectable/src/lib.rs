// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use ::async_trait::async_trait;
use ::ffx_bluetooth_connectable_args::{ConnectableCommand, ConnectableSubCommand};
use ::fho::{AvailabilityFlag, FfxMain, FfxTool, Result};
use fdomain_fuchsia_bluetooth_affordances::{
    HostControllerProxy, HostControllerSetConnectabilityRequest,
};
use ffx_writer::{SimpleWriter, ToolIO as _};
use target_holders::toolbox;

#[derive(FfxTool)]
#[check(AvailabilityFlag("bluetooth.enabled"))]
pub struct ConnectableTool {
    #[command]
    cmd: ConnectableCommand,
    #[with(toolbox())]
    host_controller: HostControllerProxy,
}

fho::embedded_plugin!(ConnectableTool);
#[async_trait(?Send)]
impl FfxMain for ConnectableTool {
    type Writer = SimpleWriter;
    type Error = ::fho::Error;
    async fn main(mut self, mut writer: Self::Writer) -> Result<()> {
        match self.cmd.subcommand.clone() {
            // ffx bluetooth connectable start
            ConnectableSubCommand::Start(_) => {
                self.set_connectability(true).await?;
                writer.line("Becoming connectable")?;
            }
            // ffx bluetooth connectable stop
            ConnectableSubCommand::Stop(_) => {
                self.set_connectability(false).await?;
                writer.line("Revoking connectability")?;
            }
        }
        Ok(())
    }
}

impl ConnectableTool {
    // Set connection policy.
    async fn set_connectability(&self, connectable: bool) -> Result<(), fho::Error> {
        let request = HostControllerSetConnectabilityRequest {
            connectable: Some(connectable),
            ..Default::default()
        };
        Ok(self
            .host_controller
            .set_connectability(&request)
            .await
            .map_err(|err| fho::Error::Unexpected(anyhow::anyhow!("FIDL error: {err}")))?
            .map_err(|err| {
                fho::Error::Unexpected(anyhow::anyhow!(
                    "fuchsia.bluetooth.affordances.HostController error: {err:?}"
                ))
            })?)
    }
}
