// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_trait::async_trait;
use component_debug_fdomain::cli::{config_list_cmd, config_set_cmd, config_unset_cmd};
use errors::FfxError;
use ffx_component::rcs::{
    connect_to_config_override, connect_to_lifecycle_controller, connect_to_realm_query,
};
use ffx_component_config_args::{
    ConfigComponentCommand, ListArgs, SetArgs, SubCommandEnum, UnsetArgs,
};
use ffx_writer::SimpleWriter;
use fho::{FfxMain, FfxTool};
use target_holders::RemoteControlProxyHolder;

#[derive(FfxTool)]
pub struct ConfigTool {
    #[command]
    cmd: ConfigComponentCommand,
    rcs: RemoteControlProxyHolder,
}

fho::embedded_plugin!(ConfigTool);

#[async_trait(?Send)]
impl FfxMain for ConfigTool {
    type Writer = SimpleWriter;

    type Error = ::fho::Error;

    async fn main(self, writer: Self::Writer) -> fho::Result<()> {
        let lifecycle_controller = connect_to_lifecycle_controller(&self.rcs).await?;
        let realm_query = connect_to_realm_query(&self.rcs).await?;
        let config_override = connect_to_config_override(&self.rcs).await?;
        let ConfigComponentCommand { subcommand } = self.cmd;
        match subcommand {
            SubCommandEnum::Set(SetArgs { query, key_values, reload }) => config_set_cmd(
                query,
                key_values,
                reload,
                lifecycle_controller,
                realm_query,
                config_override,
                writer,
            )
            .await
            .map_err(|e| FfxError::Error(e, 1))?,
            SubCommandEnum::Unset(UnsetArgs { query, reload }) => config_unset_cmd(
                query,
                reload,
                lifecycle_controller,
                realm_query,
                config_override,
                writer,
            )
            .await
            .map_err(|e| FfxError::Error(e, 1))?,
            SubCommandEnum::List(ListArgs { query }) => config_list_cmd(query, realm_query, writer)
                .await
                .map_err(|e| FfxError::Error(e, 1))?,
        };
        Ok(())
    }
}
