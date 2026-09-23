// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_trait::async_trait;
use fdomain_fuchsia_session::{LifecycleProxy, LifecycleStartRequest};
use ffx_session_common::CommandStatus;
use ffx_session_start_args::SessionStartCommand;
use ffx_writer::{ToolIO, VerifiedMachineWriter};
use fho::{FfxMain, FfxTool};
use std::io::Write;
use target_holders::moniker;

const STARTING_SESSION: &str = "Starting the default session\n";

#[derive(FfxTool)]
pub struct StartTool {
    #[command]
    cmd: SessionStartCommand,
    #[with(moniker("/core/session-manager"))]
    lifecycle_proxy: LifecycleProxy,
}

fho::embedded_plugin!(StartTool);

#[async_trait(?Send)]
impl FfxMain for StartTool {
    type Writer = VerifiedMachineWriter<CommandStatus>;
    type Error = ::fho::Error;

    async fn main(self, mut writer: Self::Writer) -> fho::Result<()> {
        start_impl(self.lifecycle_proxy, self.cmd, &mut writer).await
    }
}

pub async fn start_impl(
    lifecycle_proxy: LifecycleProxy,
    _cmd: SessionStartCommand,
    writer: &mut VerifiedMachineWriter<CommandStatus>,
) -> fho::Result<()> {
    if !writer.is_machine() {
        write!(writer, "{}", STARTING_SESSION)?;
    }
    match lifecycle_proxy.start(&LifecycleStartRequest { ..Default::default() }).await {
        Ok(Ok(())) => writer.machine(&CommandStatus::Ok { message: None }).map_err(Into::into),
        Ok(Err(err)) => Err(fho::Error::User(anyhow::anyhow!("{:?}", err))),
        Err(err) => {
            Err(fho::Error::User(anyhow::anyhow!("Transport error starting session: {:?}", err)))
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use fdomain_fuchsia_session::LifecycleRequest;
    use ffx_writer::{Format, TestBuffers};
    use target_holders::fake_proxy;

    #[fuchsia::test]
    async fn test_start_session() {
        let client = fdomain_local::local_client_empty();
        let proxy = fake_proxy(client, |req| match req {
            LifecycleRequest::Start { payload, responder, .. } => {
                assert_eq!(payload.session_url, None);
                let _ = responder.send(Ok(()));
            }
            _ => panic!("Unexpected Lifecycle request"),
        });

        let start_cmd = SessionStartCommand {};
        let test_buffers = TestBuffers::default();
        let mut writer = VerifiedMachineWriter::<CommandStatus>::new_test(None, &test_buffers);
        start_impl(proxy, start_cmd, &mut writer).await.unwrap();
        let output = test_buffers.into_stdout_str();
        assert_eq!(output, STARTING_SESSION);
    }

    #[fuchsia::test]
    async fn test_start_session_machine() {
        let client = fdomain_local::local_client_empty();
        let proxy = fake_proxy(client, |req| match req {
            LifecycleRequest::Start { payload, responder, .. } => {
                assert_eq!(payload.session_url, None);
                let _ = responder.send(Ok(()));
            }
            _ => panic!("Unexpected Lifecycle request"),
        });

        let start_cmd = SessionStartCommand {};
        let test_buffers = TestBuffers::default();
        let mut writer =
            VerifiedMachineWriter::<CommandStatus>::new_test(Some(Format::Json), &test_buffers);
        start_impl(proxy, start_cmd, &mut writer).await.unwrap();
        let output = test_buffers.into_stdout_str();
        let status: CommandStatus = serde_json::from_str(&output).unwrap();
        assert_eq!(status, CommandStatus::Ok { message: None });
    }

    #[fuchsia::test]
    async fn test_start_session_error() {
        let client = fdomain_local::local_client_empty();
        let proxy = fake_proxy(client, |req| match req {
            LifecycleRequest::Start { responder, .. } => {
                let _ = responder.send(Err(fdomain_fuchsia_session::LifecycleError::NotFound));
            }
            _ => panic!("Unexpected Lifecycle request"),
        });

        let start_cmd = SessionStartCommand {};
        let test_buffers = TestBuffers::default();
        let mut writer =
            VerifiedMachineWriter::<CommandStatus>::new_test(Some(Format::Json), &test_buffers);
        let response = start_impl(proxy, start_cmd, &mut writer).await;
        let err = response.unwrap_err();
        assert!(err.to_string().contains("NotFound"));
        let output = test_buffers.into_stdout_str();
        assert!(output.is_empty());
    }

    #[fuchsia::test]
    async fn test_start_session_transport_error() {
        let client = fdomain_local::local_client_empty();
        let (proxy, server) =
            client.create_proxy_and_stream::<fdomain_fuchsia_session::LifecycleMarker>();
        drop(server);

        let start_cmd = SessionStartCommand {};
        let test_buffers = TestBuffers::default();
        let mut writer =
            VerifiedMachineWriter::<CommandStatus>::new_test(Some(Format::Json), &test_buffers);
        let response = start_impl(proxy, start_cmd, &mut writer).await;
        assert!(response.is_err());
        assert!(response.unwrap_err().to_string().starts_with("Transport error starting session"));
        let output = test_buffers.into_stdout_str();
        assert!(output.is_empty());
    }
}
