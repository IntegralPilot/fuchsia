// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use ffx_guest_launch_args::LaunchArgs;
use fho::{FfxMain, FfxTool, MachineWriter, Result, ToolIO as _, bug, return_user_error};
use fidl_fuchsia_developer_remotecontrol::RemoteControlProxy;
use std::io::Write as _;

#[derive(FfxTool)]
pub struct GuestLaunchTool {
    #[command]
    pub cmd: LaunchArgs,
    remote_control: RemoteControlProxy,
}

fho::embedded_plugin!(GuestLaunchTool);

#[async_trait::async_trait(?Send)]
impl FfxMain for GuestLaunchTool {
    type Writer = MachineWriter<guest_cli::launch::LaunchResult>;

    type Error = ::fho::Error;
    async fn main(self, mut writer: Self::Writer) -> fho::Result<()> {
        let services = guest_cli::platform::HostPlatformServices::new(self.remote_control);

        // TODO(https://fxbug.dev/42068091): Remove when overnet supports duplicated socket handles.
        if !self.cmd.detach {
            return_user_error!(
                "The ffx guest plugin doesn't support attaching to a running guest.\
        Re-run using the -d flag to detach, or use the guest tool via fx shell.\
        See https://fxbug.dev/42068091 for updates."
            );
        }

        let output = guest_cli::launch::handle_launch(&services, &self.cmd).await;
        if writer.is_machine() {
            writer.machine(&output)?;
        } else {
            writeln!(writer, "{output}").map_err(|e| bug!(e))?;
        }
        Ok(())
    }
}
