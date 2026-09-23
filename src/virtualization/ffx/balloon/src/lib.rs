// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use ffx_guest_balloon_args::BalloonArgs;
use fho::{FfxMain, FfxTool, MachineWriter, Result, ToolIO as _, bug};
use fidl_fuchsia_developer_remotecontrol::RemoteControlProxy;
use std::io::Write as _;

#[derive(FfxTool)]
pub struct GuestBalloonTool {
    #[command]
    pub cmd: BalloonArgs,
    remote_control: RemoteControlProxy,
}

fho::embedded_plugin!(GuestBalloonTool);

#[async_trait::async_trait(?Send)]
impl FfxMain for GuestBalloonTool {
    type Writer = MachineWriter<guest_cli::balloon::BalloonResult>;

    type Error = ::fho::Error;
    async fn main(self, mut writer: Self::Writer) -> fho::Result<()> {
        let services = guest_cli::platform::HostPlatformServices::new(self.remote_control);
        let output = guest_cli::balloon::handle_balloon(&services, &self.cmd).await;
        if writer.is_machine() {
            writer.machine(&output)?;
        } else {
            writeln!(writer, "{output}").map_err(|e| bug!(e))?;
        }
        Ok(())
    }
}
