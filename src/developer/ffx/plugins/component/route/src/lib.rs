// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_trait::async_trait;
use component_debug::cli;
use component_debug::route::RouteReport;
use component_debug_fdomain as component_debug;
use errors::ffx_error;
use ffx_component::rcs;
use ffx_component_route_args::RouteCommand;
use ffx_writer::{MachineWriter, ToolIO as _};
use fho::{FfxMain, FfxTool};
use target_holders::RemoteControlProxyHolder;

#[derive(FfxTool)]
pub struct RouteTool {
    #[command]
    cmd: RouteCommand,
    rcs: RemoteControlProxyHolder,
}

fho::embedded_plugin!(RouteTool);

#[async_trait(?Send)]
impl FfxMain for RouteTool {
    type Writer = MachineWriter<Vec<RouteReport>>;

    type Error = ::fho::Error;

    async fn main(self, mut writer: Self::Writer) -> fho::Result<()> {
        let realm_query = rcs::connect_to_realm_query(&self.rcs).await?;
        let route_validator = rcs::connect_to_route_validator(&self.rcs).await?;

        // All errors from component_debug library are user-visible.
        if writer.is_machine() {
            let output = cli::route_cmd_serialized(
                self.cmd.target,
                self.cmd.filter,
                route_validator,
                realm_query,
            )
            .await
            .map_err(|e| ffx_error!(e))?;
            writer.machine(&output)?;
        } else {
            cli::route_cmd_print(
                self.cmd.target,
                self.cmd.filter,
                route_validator,
                realm_query,
                writer,
            )
            .await
            .map_err(|e| ffx_error!(e))?;
        }
        Ok(())
    }
}
