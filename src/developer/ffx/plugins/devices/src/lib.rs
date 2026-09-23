// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_trait::async_trait;
use ffx_config::EnvironmentContext;
use ffx_devices_args::DevicesCommand;
use ffx_writer::VerifiedMachineWriter;
use fho::{FfxMain, FfxTool};
use target_formatter::JsonTarget;

#[derive(FfxTool)]
#[target(None)]
#[main_error(ffx_list::ListError)]
pub struct DevicesTool {
    #[command]
    cmd: DevicesCommand,
    context: EnvironmentContext,
    fho_env: fho::FhoEnvironment,
}

fho::embedded_plugin!(DevicesTool, ffx_list::ListError);

#[async_trait(?Send)]
impl FfxMain for DevicesTool {
    type Error = ffx_list::ListError;
    type Writer = VerifiedMachineWriter<Vec<JsonTarget>>;

    async fn main(self, writer: Self::Writer) -> std::result::Result<(), Self::Error> {
        let list_tool = ffx_list::ListTool::new(self.cmd.into(), self.context, self.fho_env);
        list_tool.main(writer).await
    }
}
