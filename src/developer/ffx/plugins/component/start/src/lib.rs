// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_trait::async_trait;
use component_debug::cli::format::format_start_error;
use component_debug::lifecycle::start_instance;
use component_debug::query::get_cml_moniker_from_query;
use component_debug_fdomain as component_debug;
use errors::ffx_error;
use ffx_component::rcs::{connect_to_lifecycle_controller, connect_to_realm_query};
use ffx_component_start_args::ComponentStartCommand;
use ffx_config::EnvironmentContext;
use ffx_writer::VerifiedMachineWriter;
use ffx_zxdb::Debugger;
use fho::{FfxMain, FfxTool, deferred};
use moniker::Moniker;
use schemars::JsonSchema;
use serde::Serialize;
use target_holders::{RemoteControlProxyHolder, moniker as moniker_helper};

#[derive(Serialize, JsonSchema)]
pub struct StartResult {
    moniker: String,
}

#[derive(FfxTool)]
pub struct StartTool {
    #[command]
    cmd: ComponentStartCommand,

    #[with(deferred(moniker_helper("/core/debugger")))]
    debugger_proxy: fho::Deferred<fdomain_fuchsia_debugger::LauncherProxy>,

    rcs: RemoteControlProxyHolder,

    context: EnvironmentContext,
}

fho::embedded_plugin!(StartTool);

#[async_trait(?Send)]
impl FfxMain for StartTool {
    type Writer = VerifiedMachineWriter<StartResult>;

    type Error = ::fho::Error;

    async fn main(self, mut writer: Self::Writer) -> fho::Result<()> {
        let moniker = start_tool_impl(self).await?;
        writer.machine(&StartResult { moniker: moniker.to_string() })?;
        Ok(())
    }
}

async fn start_tool_impl(tool: StartTool) -> fho::Result<Moniker> {
    let lifecycle_controller = connect_to_lifecycle_controller(&tool.rcs).await?;
    let realm_query = connect_to_realm_query(&tool.rcs).await?;
    let moniker = get_cml_moniker_from_query(&tool.cmd.query, &realm_query)
        .await
        .map_err(|e| ffx_error!(e))?;

    // If the user wants to debug the component, we need to start the debugger with a breakpoint
    // on `_start`, which will give the user a chance to set any further breakpoints they want.
    let maybe_session = if tool.cmd.debug {
        let mut debugger = Debugger::launch(&tool.context, tool.debugger_proxy.await?)
            .await
            .map_err(|e| ffx_error!(e))?;
        debugger.command.attach(&format!("{}", moniker));
        debugger.command.break_at("_start");
        let session = debugger.start().await.map_err(|e| ffx_error!(e))?;
        Some(session)
    } else {
        None
    };

    let _ = start_instance(&lifecycle_controller, &moniker)
        .await
        .map_err(|e| ffx_error!(format_start_error(&moniker, e)))?;

    // Wait for the user to interactively debug the component.
    if let Some(session) = maybe_session {
        session.wait().await.map_err(|e| ffx_error!(e))?;
    }
    Ok(moniker)
}
