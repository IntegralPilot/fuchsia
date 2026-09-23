// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use ffx_guest_attach_args::AttachArgs;
use fho::{FfxMain, FfxTool, MachineWriter, Result, ToolIO as _, bug, return_user_error};
use fidl_fuchsia_developer_remotecontrol::RemoteControlProxy;
use std::io::Write as _;

#[derive(FfxTool)]
pub struct GuestAttachTool {
    #[command]
    pub cmd: AttachArgs,
    remote_control: RemoteControlProxy,
}

fho::embedded_plugin!(GuestAttachTool);

#[async_trait::async_trait(?Send)]
impl FfxMain for GuestAttachTool {
    type Writer = MachineWriter<guest_cli::attach::AttachResult>;

    type Error = ::fho::Error;
    async fn main(self, mut _writer: Self::Writer) -> fho::Result<()> {
        // TODO(https://fxbug.dev/42068091): Remove when overnet supports duplicated socket handles.
        return_user_error!(
            "The ffx guest plugin doesn't support attaching to a running guest. \
    Use the guest tool instead: `fx shell guest attach {}`. \
    See https://fxbug.dev/42068091 for updates.",
            self.cmd.guest_type
        );

        // TODO(https://fxbug.dev/42068091): Enable when overnet supports duplicated socket handles.
        #[allow(unreachable_code)]
        {
            let services = guest_cli::platform::HostPlatformServices::new(self.remote_control);

            let output = guest_cli::attach::handle_attach(&services, &self.cmd).await?;
            if _writer.is_machine() {
                _writer.machine(&output)?;
            } else {
                writeln!(_writer, "{output}").map_err(|e| bug!(e))?;
            }
            Ok(())
        }
    }
}
