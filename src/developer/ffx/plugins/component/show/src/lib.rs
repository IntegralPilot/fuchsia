// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_trait::async_trait;
use component_debug_fdomain::cli::show::ShowCmdInstance;
use component_debug_fdomain::cli::{show_cmd_print, show_cmd_serialized};
use errors::ffx_error;
use ffx_component::rcs::connect_to_realm_query;
use ffx_component_show_args::ComponentShowCommand;
use ffx_writer::{ToolIO as _, VerifiedMachineWriter};
use fho::{FfxMain, FfxTool};
use target_holders::RemoteControlProxyHolder;
#[derive(FfxTool)]
pub struct ShowTool {
    #[command]
    cmd: ComponentShowCommand,
    rcs: RemoteControlProxyHolder,
}

fho::embedded_plugin!(ShowTool);

#[async_trait(?Send)]
impl FfxMain for ShowTool {
    type Writer = VerifiedMachineWriter<ShowCmdInstance>;

    type Error = ::fho::Error;

    async fn main(self, mut writer: Self::Writer) -> fho::Result<()> {
        let realm_query = connect_to_realm_query(&self.rcs).await?;

        // All errors from component_debug library are user-visible.
        if writer.is_machine() {
            let output = show_cmd_serialized(self.cmd.query, realm_query)
                .await
                .map_err(|e| ffx_error!(e))?;
            writer.machine(&output)?;
        } else {
            let with_style = termion::is_tty(&std::io::stdout());
            show_cmd_print(self.cmd.query, realm_query, writer, with_style)
                .await
                .map_err(|e| ffx_error!(e))?;
        }
        Ok(())
    }
}
