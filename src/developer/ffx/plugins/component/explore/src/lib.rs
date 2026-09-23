// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_trait::async_trait;
use component_debug_fdomain::cli::explore_cmd;
use errors::ffx_error;
use fdomain_fuchsia_dash::LauncherProxy;
use ffx_component::rcs::connect_to_realm_query;
use ffx_component_explore_args::ExploreComponentCommand;
use ffx_writer::MachineWriter;
use fho::{FfxMain, FfxTool};
use socket_to_stdio::Stdout;
use target_holders::{RemoteControlProxyHolder, moniker};

#[derive(FfxTool)]
pub struct ExploreTool {
    #[command]
    cmd: ExploreComponentCommand,
    rcs: RemoteControlProxyHolder,
    #[with(moniker("/core/debug-dash-launcher"))]
    dash_launcher: LauncherProxy,
}

fho::embedded_plugin!(ExploreTool);

// TODO(https://fxbug.dev/42053815): This plugin needs E2E tests.
#[async_trait(?Send)]
impl FfxMain for ExploreTool {
    // We use `MachineWriter<()>` to allow the `--machine` flag, which enables
    // JSON-formatted error output from the FFX framework. We do not produce
    // a success payload because this command is either interactive or streams
    // its output directly to stdout.
    type Writer = MachineWriter<()>;

    type Error = ::fho::Error;

    async fn main(self, _writer: Self::Writer) -> fho::Result<()> {
        let realm_query = connect_to_realm_query(&self.rcs).await?;
        let stdout = if self.cmd.command.is_some() { Stdout::buffered() } else { Stdout::raw()? };

        // All errors from component_debug library are user-visible.
        explore_cmd(
            self.cmd.query,
            self.cmd.ns_layout,
            self.cmd.command,
            self.cmd.tools,
            self.dash_launcher,
            realm_query,
            stdout,
        )
        .await
        .map_err(|e| ffx_error!(e))?;
        Ok(())
    }
}
