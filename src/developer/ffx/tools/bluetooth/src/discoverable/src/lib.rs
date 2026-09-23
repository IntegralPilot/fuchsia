// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use ::async_trait::async_trait;
use ::ffx_bluetooth_discoverable_args::{DiscoverableCommand, DiscoverableSubCommand};
use ::fho::{AvailabilityFlag, FfxMain, FfxTool, Result};
use fdomain_fuchsia_bluetooth_affordances::{
    HostControllerProxy, HostControllerSetDiscoverabilityRequest,
};
use ffx_writer::{SimpleWriter, ToolIO as _};
use target_holders::toolbox;

#[derive(FfxTool)]
#[check(AvailabilityFlag("bluetooth.enabled"))]
pub struct DiscoverableTool {
    #[command]
    cmd: DiscoverableCommand,
    #[with(toolbox())]
    host_controller: HostControllerProxy,
}

fho::embedded_plugin!(DiscoverableTool);
#[async_trait(?Send)]
impl FfxMain for DiscoverableTool {
    type Writer = SimpleWriter;

    type Error = ::fho::Error;

    async fn main(mut self, mut writer: Self::Writer) -> Result<()> {
        match self.cmd.subcommand.clone() {
            // ffx bluetooth discoverable start
            DiscoverableSubCommand::Start(ref _cmd) => {
                self.set_discoverability(true).await?;
                writer.line("Becoming discoverable")?;
            }
            // ffx bluetooth discoverable stop
            DiscoverableSubCommand::Stop(ref _cmd) => {
                self.set_discoverability(false).await?;
                writer.line("Revoking discoverability")?;
            }
        }
        Ok(())
    }
}

impl DiscoverableTool {
    // Set discoverability state.
    async fn set_discoverability(&self, discoverable: bool) -> Result<(), fho::Error> {
        let request = HostControllerSetDiscoverabilityRequest {
            discoverable: Some(discoverable),
            ..Default::default()
        };
        Ok(self
            .host_controller
            .set_discoverability(&request)
            .await
            .map_err(|err| fho::Error::Unexpected(anyhow::anyhow!("FIDL error: {err}")))?
            .map_err(|err| {
                fho::Error::Unexpected(anyhow::anyhow!(
                    "fuchsia.bluetooth.affordances.HostController error: {err:?}"
                ))
            })?)
    }
}
