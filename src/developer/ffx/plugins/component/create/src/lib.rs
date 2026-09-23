// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_trait::async_trait;
use component_debug_fdomain::cli::create_cmd;
use component_debug_fdomain::config::resolve_raw_config_overrides;
use errors::ffx_error;
use ffx_component::rcs::{connect_to_lifecycle_controller, connect_to_realm_query};
use ffx_component_create_args::CreateComponentCommand;
use ffx_writer::SimpleWriter;
use fho::{FfxMain, FfxTool};
use target_holders::RemoteControlProxyHolder;

#[derive(FfxTool)]
pub struct CreateTool {
    #[command]
    cmd: CreateComponentCommand,
    rcs: RemoteControlProxyHolder,
}

fho::embedded_plugin!(CreateTool);

#[async_trait(?Send)]
impl FfxMain for CreateTool {
    type Writer = SimpleWriter;

    type Error = ::fho::Error;

    async fn main(self, writer: Self::Writer) -> fho::Result<()> {
        let lifecycle_controller = connect_to_lifecycle_controller(&self.rcs).await?;
        let realm_query = connect_to_realm_query(&self.rcs).await?;

        let config_overrides = resolve_raw_config_overrides(
            &realm_query,
            &self.cmd.moniker,
            &self.cmd.url.to_string(),
            &self.cmd.config,
        )
        .await
        .map_err(|e| ffx_error!("{e}"))?;

        // All errors from component_debug library are user-visible.
        create_cmd(self.cmd.url, self.cmd.moniker, config_overrides, lifecycle_controller, writer)
            .await
            .map_err(|e| ffx_error!(e))?;
        Ok(())
    }
}
