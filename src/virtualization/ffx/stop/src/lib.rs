// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use ffx_guest_stop_args::StopArgs;
use fho::{FfxMain, FfxTool, MachineWriter, Result, ToolIO as _, bug};
use fidl_fuchsia_developer_remotecontrol::RemoteControlProxy;
use std::io::Write as _;

#[derive(FfxTool)]
pub struct GuestStopTool {
    #[command]
    pub cmd: StopArgs,
    remote_control: RemoteControlProxy,
}

fho::embedded_plugin!(GuestStopTool);

#[async_trait::async_trait(?Send)]
impl FfxMain for GuestStopTool {
    type Writer = MachineWriter<guest_cli::stop::StopResult>;

    type Error = ::fho::Error;
    async fn main(self, mut writer: Self::Writer) -> fho::Result<()> {
        let services = guest_cli::platform::HostPlatformServices::new(self.remote_control);
        let output = guest_cli::stop::handle_stop(&services, &self.cmd).await?;

        if writer.is_machine() {
            writer.machine(&output)?;
        } else {
            writeln!(writer, "{output}").map_err(|e| bug!(e))?;
        }

        Ok(())
    }
}
