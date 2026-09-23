// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_trait::async_trait;
use component_debug_fdomain::cli::resolve_cmd;
use errors::ffx_error;
use ffx_component::rcs::{connect_to_lifecycle_controller, connect_to_realm_query};
use ffx_component_resolve_args::ComponentResolveCommand;
use ffx_writer::VerifiedMachineWriter;
use fho::{FfxMain, FfxTool};
use schemars::JsonSchema;
use serde::Serialize;
use target_holders::RemoteControlProxyHolder;

#[derive(Serialize, JsonSchema)]
pub struct ResolveResult {
    moniker: String,
}

#[derive(FfxTool)]
pub struct ResolveTool {
    #[command]
    cmd: ComponentResolveCommand,
    rcs: RemoteControlProxyHolder,
}

fho::embedded_plugin!(ResolveTool);

#[async_trait(?Send)]
impl FfxMain for ResolveTool {
    type Writer = VerifiedMachineWriter<ResolveResult>;

    type Error = ::fho::Error;

    async fn main(self, mut writer: Self::Writer) -> fho::Result<()> {
        let lifecycle_controller = connect_to_lifecycle_controller(&self.rcs).await?;
        let realm_query = connect_to_realm_query(&self.rcs).await?;

        // All errors from component_debug library are user-visible.
        let moniker = resolve_cmd(self.cmd.query, lifecycle_controller, realm_query, &mut writer)
            .await
            .map_err(|e| ffx_error!(e))?;

        writer.machine(&ResolveResult { moniker: moniker.to_string() })?;
        Ok(())
    }
}
