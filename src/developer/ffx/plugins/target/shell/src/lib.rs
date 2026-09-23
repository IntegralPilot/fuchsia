// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_trait::async_trait;
use fdomain_client::fidl::Proxy as _;
use fdomain_fuchsia_developer_console as fconsole;
use ffx_target_shell_args::ShellCommand;
use ffx_writer::SimpleWriter;
use fho::{FfxMain, FfxTool};
use futures::{FutureExt as _, TryFutureExt as _};
use socket_to_stdio::Stdout;
use target_holders::toolbox;

#[derive(FfxTool)]
pub struct ShellTool {
    #[command]
    cmd: ShellCommand,
    #[with(toolbox())]
    launcher: fconsole::LauncherProxy,
}

fho::embedded_plugin!(ShellTool);

#[async_trait(?Send)]
impl FfxMain for ShellTool {
    type Writer = SimpleWriter;

    type Error = ::fho::Error;

    async fn main(self, _writer: Self::Writer) -> fho::Result<()> {
        let Self { cmd: ShellCommand { shell_command }, launcher } = self;

        let interactive = shell_command.is_empty();
        let (client, server) = launcher.domain().create_stream_socket();
        // Create an eventpair associated with this launch. If we or the
        // connection to the target gets killed we want the console to go down
        // as well.
        let (_my_stopper, stopper) = launcher.domain().create_event_pair();

        let options = fconsole::LaunchOptions {
            name: Some("ffx-target-shell".to_string()),
            args: (!interactive).then(|| vec![shell_command.join(" ")]),
            stopper: Some(stopper),
            io_handles: Some(fconsole::IoHandles::PtySocket(server)),
            ..Default::default()
        };

        let stdio = async move {
            let stdout = if interactive { Stdout::raw()? } else { Stdout::buffered() };
            socket_to_stdio::connect_fdomain_socket_to_stdio(client, stdout)
                .map_err(|e| fho::bug!(e))
                .await
        };
        let launch = launcher.launch(options).map(|r| match r {
            Ok(Ok(r)) => Ok(r),
            Ok(Err(e)) => Err(fho::bug!("failed to launch console: {e:?}")),
            Err(e) => Err(fho::bug!(e)),
        });

        let (exit_code, ()) = futures::future::try_join(launch, stdio).await?;
        if !interactive {
            fho::exit_with_code!(exit_code as i32);
        }
        Ok(())
    }
}
