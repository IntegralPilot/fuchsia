// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_trait::async_trait;
use component_debug::cli::capability::RouteSegment;
use component_debug::cli::{capability_cmd_print, capability_cmd_serialized};
use component_debug_fdomain as component_debug;
use errors::ffx_error;
use ffx_component::rcs::connect_to_realm_query;
use ffx_component_capability_args::ComponentCapabilityCommand;
use ffx_writer::{ToolIO as _, VerifiedMachineWriter};
use fho::{FfxMain, FfxTool};
use target_holders::RemoteControlProxyHolder;

#[derive(FfxTool)]
pub struct CapabilityTool {
    #[command]
    cmd: ComponentCapabilityCommand,
    rcs: RemoteControlProxyHolder,
}

fho::embedded_plugin!(CapabilityTool);

#[async_trait(?Send)]
impl FfxMain for CapabilityTool {
    type Writer = VerifiedMachineWriter<Vec<RouteSegment>>;
    type Error = ::fho::Error;

    async fn main(self, mut writer: Self::Writer) -> fho::Result<()> {
        let realm_query = connect_to_realm_query(&self.rcs).await?;
        if writer.is_machine() {
            let output = capability_cmd_serialized(self.cmd.capability, realm_query)
                .await
                .map_err(|e| ffx_error!(e))?;
            writer.machine(&output)?;
        } else {
            capability_cmd_print(self.cmd.capability, realm_query, writer)
                .await
                .map_err(|e| ffx_error!(e))?;
        }
        Ok(())
    }
}
