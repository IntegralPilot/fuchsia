// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Result, format_err};
use async_trait::async_trait;
use fdomain_fuchsia_session::RestarterProxy;
use ffx_session_restart_args::SessionRestartCommand;
use ffx_writer::{MachineWriter, ToolIO};
use fho::{FfxMain, FfxTool};
use std::io::Write;
use target_holders::moniker;
#[derive(FfxTool)]
pub struct RestartTool {
    #[command]
    cmd: SessionRestartCommand,
    #[with(moniker("/core/session-manager"))]
    restarter_proxy: RestarterProxy,
}

fho::embedded_plugin!(RestartTool);

#[async_trait(?Send)]
impl FfxMain for RestartTool {
    type Writer = MachineWriter<()>;

    type Error = ::fho::Error;

    async fn main(self, mut writer: Self::Writer) -> fho::Result<()> {
        restart_impl(self.restarter_proxy, self.cmd, &mut writer).await?;
        if writer.is_machine() {
            writer.machine(&())?;
        }
        Ok(())
    }
}

pub async fn restart_impl(
    restarter_proxy: RestarterProxy,
    _cmd: SessionRestartCommand,
    writer: &mut MachineWriter<()>,
) -> Result<()> {
    if !writer.is_machine() {
        writeln!(writer, "Restarting the current session")?;
    }
    restarter_proxy.restart().await?.map_err(|err| format_err!("{:?}", err))
}

#[cfg(test)]
mod test {
    use super::*;
    use fdomain_fuchsia_session::RestarterRequest;
    use target_holders::fake_proxy;

    #[fuchsia::test]
    async fn test_restart_session() {
        let client = fdomain_local::local_client_empty();
        let proxy = fake_proxy(client, |req| match req {
            RestarterRequest::Restart { responder } => {
                let _ = responder.send(Ok(()));
            }
        });

        let restart_cmd = SessionRestartCommand {};
        let test_buffers = ffx_writer::TestBuffers::default();
        let mut writer = MachineWriter::new_test(None, &test_buffers);
        let result = restart_impl(proxy, restart_cmd, &mut writer).await;
        assert!(result.is_ok());
        let output = test_buffers.into_stdout_str();
        assert_eq!(output, "Restarting the current session\n");
    }

    #[fuchsia::test]
    async fn test_machine_output_is_valid_json() {
        let client = fdomain_local::local_client_empty();
        let proxy = fake_proxy(client.clone(), |req| match req {
            RestarterRequest::Restart { responder } => {
                let _ = responder.send(Ok(()));
            }
        });

        let restart_cmd = SessionRestartCommand {};
        let test_buffers = ffx_writer::TestBuffers::default();
        let writer = MachineWriter::new_test(Some(ffx_writer::Format::Json), &test_buffers);

        let tool = RestartTool { cmd: restart_cmd, restarter_proxy: proxy };

        let result = tool.main(writer).await;
        assert!(result.is_ok());

        let output = test_buffers.into_stdout_str();
        assert_eq!(output, "null\n");
    }
}
