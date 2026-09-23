// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use ::async_trait::async_trait;
use ::ffx_bluetooth_discovery_args::{DiscoveryCommand, DiscoverySubCommand};
use ::fho::{AvailabilityFlag, FfxMain, FfxTool, Result};
use fdomain_fuchsia_bluetooth_affordances::{
    PeerControllerProxy, PeerControllerSetDiscoveryRequest,
};
use ffx_writer::{SimpleWriter, ToolIO as _};
use target_holders::toolbox;

#[derive(FfxTool)]
#[check(AvailabilityFlag("bluetooth.enabled"))]
pub struct DiscoveryTool {
    #[command]
    cmd: DiscoveryCommand,
    #[with(toolbox())]
    peer_controller: PeerControllerProxy,
}

fho::embedded_plugin!(DiscoveryTool);
#[async_trait(?Send)]
impl FfxMain for DiscoveryTool {
    type Writer = SimpleWriter;

    type Error = ::fho::Error;

    async fn main(mut self, mut writer: Self::Writer) -> Result<()> {
        match self.cmd.subcommand.clone() {
            // ffx bluetooth discovery start
            DiscoverySubCommand::Start(ref _cmd) => {
                self.set_discovery(true).await?;
                writer.line("Starting discovery")?;
            }
            // ffx bluetooth discovery stop
            DiscoverySubCommand::Stop(ref _cmd) => {
                self.set_discovery(false).await?;
                writer.line("Stopping discovery")?;
            }
        }
        Ok(())
    }
}

impl DiscoveryTool {
    // Set discovery state.
    async fn set_discovery(&self, discovery: bool) -> Result<(), fho::Error> {
        let request =
            PeerControllerSetDiscoveryRequest { discovery: Some(discovery), ..Default::default() };
        let _ = self.peer_controller.set_discovery(&request).await.map_err(|e| {
            let err = anyhow::anyhow!("Set discovery error: {:?}", e);
            fho::Error::Unexpected(err)
        })?;
        Ok(())
    }
}
