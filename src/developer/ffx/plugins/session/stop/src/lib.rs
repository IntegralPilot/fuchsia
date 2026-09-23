// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use async_trait::async_trait;
use fdomain_fuchsia_session::LifecycleProxy;
use ffx_session_common::CommandStatus;
use ffx_session_stop_args::SessionStopCommand;
use ffx_writer::{ToolIO, VerifiedMachineWriter};
use fho::{FfxMain, FfxTool};
use std::io::Write;
use target_holders::moniker;

const STOPPING_SESSION: &str = "Stopping the session\n";

#[derive(FfxTool)]
pub struct StopTool {
    #[command]
    cmd: SessionStopCommand,
    #[with(moniker("/core/session-manager"))]
    lifecycle_proxy: LifecycleProxy,
}

fho::embedded_plugin!(StopTool);

#[async_trait(?Send)]
impl FfxMain for StopTool {
    type Writer = VerifiedMachineWriter<CommandStatus>;
    type Error = ::fho::Error;

    async fn main(self, mut writer: Self::Writer) -> fho::Result<()> {
        stop_impl(self.lifecycle_proxy, self.cmd, &mut writer).await?;
        Ok(())
    }
}

pub async fn stop_impl(
    lifecycle_proxy: LifecycleProxy,
    _cmd: SessionStopCommand,
    writer: &mut VerifiedMachineWriter<CommandStatus>,
) -> fho::Result<()> {
    if !writer.is_machine() {
        write!(writer, "{}", STOPPING_SESSION)?;
    }
    match lifecycle_proxy.stop().await {
        Ok(Ok(())) => {
            writer.machine(&CommandStatus::Ok { message: None })?;
            Ok(())
        }
        Ok(Err(err)) => Err(fho::user_error!("Failed to stop session: {err:?}")),
        Err(err) => Err(fho::user_error!("Transport error stopping session: {err:?}")),
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use fdomain_fuchsia_session::LifecycleRequest;
    use ffx_writer::{Format, TestBuffers};
    use target_holders::fake_proxy;

    #[fuchsia::test]
    async fn test_stop_session() -> Result<()> {
        let client = fdomain_local::local_client_empty();
        let proxy = fake_proxy(client, |req| match req {
            LifecycleRequest::Stop { responder } => {
                let _ = responder.send(Ok(()));
            }
            _ => panic!("Unexpected Lifecycle request"),
        });

        let stop_cmd = SessionStopCommand {};
        let test_buffers = TestBuffers::default();
        let mut writer = VerifiedMachineWriter::<CommandStatus>::new_test(None, &test_buffers);
        stop_impl(proxy, stop_cmd, &mut writer).await?;
        let output = test_buffers.into_stdout_str();
        assert_eq!(output, STOPPING_SESSION);
        Ok(())
    }

    #[fuchsia::test]
    async fn test_stop_session_machine() -> Result<()> {
        let client = fdomain_local::local_client_empty();
        let proxy = fake_proxy(client, |req| match req {
            LifecycleRequest::Stop { responder } => {
                let _ = responder.send(Ok(()));
            }
            _ => panic!("Unexpected Lifecycle request"),
        });

        let stop_cmd = SessionStopCommand {};
        let test_buffers = TestBuffers::default();
        let mut writer =
            VerifiedMachineWriter::<CommandStatus>::new_test(Some(Format::Json), &test_buffers);
        stop_impl(proxy, stop_cmd, &mut writer).await?;
        let output = test_buffers.into_stdout_str();
        let status: CommandStatus = serde_json::from_str(&output)?;
        assert_eq!(status, CommandStatus::Ok { message: None });
        Ok(())
    }

    #[fuchsia::test]
    async fn test_stop_session_error() -> Result<()> {
        let client = fdomain_local::local_client_empty();
        let proxy = fake_proxy(client, |req| match req {
            LifecycleRequest::Stop { responder } => {
                let _ = responder.send(Err(fdomain_fuchsia_session::LifecycleError::NotFound));
            }
            _ => panic!("Unexpected Lifecycle request"),
        });

        let stop_cmd = SessionStopCommand {};
        let test_buffers = TestBuffers::default();
        let mut writer =
            VerifiedMachineWriter::<CommandStatus>::new_test(Some(Format::Json), &test_buffers);
        let response = stop_impl(proxy, stop_cmd, &mut writer).await;
        assert!(response.is_err());
        assert_eq!(response.unwrap_err().to_string(), "Failed to stop session: NotFound");
        let output = test_buffers.into_stdout_str();
        assert!(output.is_empty());
        Ok(())
    }

    #[fuchsia::test]
    async fn test_stop_session_transport_error() -> Result<()> {
        let client = fdomain_local::local_client_empty();
        let (proxy, server) =
            client.create_proxy_and_stream::<fdomain_fuchsia_session::LifecycleMarker>();
        drop(server);

        let stop_cmd = SessionStopCommand {};
        let test_buffers = TestBuffers::default();
        let mut writer =
            VerifiedMachineWriter::<CommandStatus>::new_test(Some(Format::Json), &test_buffers);
        let response = stop_impl(proxy, stop_cmd, &mut writer).await;
        assert!(response.is_err());
        assert!(response.unwrap_err().to_string().starts_with("Transport error stopping session"));
        let output = test_buffers.into_stdout_str();
        assert!(output.is_empty());
        Ok(())
    }
}
