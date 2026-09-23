// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_trait::async_trait;
use fdomain_fuchsia_element::{ManagerError, ManagerProxy};
use ffx_session_common::CommandStatus;
use ffx_session_remove_args::SessionRemoveCommand;
use ffx_writer::{ToolIO, VerifiedMachineWriter};
use fho::{FfxMain, FfxTool};
use std::io::Write;
use target_holders::moniker;

#[derive(FfxTool)]
pub struct RemoveTool {
    #[command]
    cmd: SessionRemoveCommand,
    #[with(moniker("/core/session-manager"))]
    manager_proxy: ManagerProxy,
}

fho::embedded_plugin!(RemoveTool);

#[async_trait(?Send)]
impl FfxMain for RemoveTool {
    type Writer = VerifiedMachineWriter<CommandStatus>;
    type Error = ::fho::Error;

    async fn main(self, mut writer: Self::Writer) -> fho::Result<()> {
        remove_impl(self.manager_proxy, self.cmd, &mut writer).await?;
        Ok(())
    }
}

pub async fn remove_impl(
    manager_proxy: ManagerProxy,
    cmd: SessionRemoveCommand,
    writer: &mut VerifiedMachineWriter<CommandStatus>,
) -> fho::Result<()> {
    match manager_proxy.remove_element(&cmd.name).await {
        Ok(Ok(())) => {
            if !writer.is_machine() {
                writeln!(writer, "Removed {} from the current session", cmd.name)?;
            }
            writer.machine(&CommandStatus::Ok { message: None })?;
            Ok(())
        }
        Ok(Err(err)) => match err {
            ManagerError::NotFound => Err(fho::user_error!("Element not found")),
            ManagerError::InvalidArgs => Err(fho::user_error!("Invalid arguments")),
            ManagerError::UnableToPersist => Err(fho::user_error!("Unable to persist element")),
        },
        Err(err) => Err(fho::user_error!("Transport error removing element: {err:?}")),
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use fdomain_fuchsia_element::ManagerRequest;
    use ffx_writer::{Format, TestBuffers};
    use target_holders::fake_proxy;

    #[fuchsia::test]
    async fn test_remove_element() -> fho::Result<()> {
        let client = fdomain_local::local_client_empty();
        let proxy = fake_proxy(client, |req| match req {
            ManagerRequest::ProposeElement { .. } => unreachable!(),
            ManagerRequest::RemoveElement { name, responder } => {
                assert_eq!(name, "foo");
                let _ = responder.send(Ok(()));
            }
        });

        let remove_cmd = SessionRemoveCommand { name: "foo".to_string() };
        let test_buffers = TestBuffers::default();
        let mut writer = VerifiedMachineWriter::<CommandStatus>::new_test(None, &test_buffers);
        remove_impl(proxy, remove_cmd, &mut writer).await?;
        let output = test_buffers.into_stdout_str();
        assert_eq!(output, "Removed foo from the current session\n");
        Ok(())
    }

    #[fuchsia::test]
    async fn test_remove_element_machine() -> fho::Result<()> {
        let client = fdomain_local::local_client_empty();
        let proxy = fake_proxy(client, |req| match req {
            ManagerRequest::ProposeElement { .. } => unreachable!(),
            ManagerRequest::RemoveElement { name, responder } => {
                assert_eq!(name, "foo");
                let _ = responder.send(Ok(()));
            }
        });

        let remove_cmd = SessionRemoveCommand { name: "foo".to_string() };
        let test_buffers = TestBuffers::default();
        let mut writer =
            VerifiedMachineWriter::<CommandStatus>::new_test(Some(Format::Json), &test_buffers);
        remove_impl(proxy, remove_cmd, &mut writer).await?;
        let output = test_buffers.into_stdout_str();
        let status: CommandStatus = serde_json::from_str(&output).unwrap();
        assert_eq!(status, CommandStatus::Ok { message: None });
        Ok(())
    }

    #[fuchsia::test]
    async fn test_remove_element_error() -> fho::Result<()> {
        let client = fdomain_local::local_client_empty();
        let proxy = fake_proxy(client, |req| match req {
            ManagerRequest::ProposeElement { .. } => unreachable!(),
            ManagerRequest::RemoveElement { responder, .. } => {
                let _ = responder.send(Err(ManagerError::NotFound));
            }
        });

        let remove_cmd = SessionRemoveCommand { name: "foo".to_string() };
        let test_buffers = TestBuffers::default();
        let mut writer =
            VerifiedMachineWriter::<CommandStatus>::new_test(Some(Format::Json), &test_buffers);
        let response = remove_impl(proxy, remove_cmd, &mut writer).await;
        assert!(response.is_err());
        assert_eq!(response.unwrap_err().to_string(), "Element not found");
        let output = test_buffers.into_stdout_str();
        assert!(output.is_empty());
        Ok(())
    }

    #[fuchsia::test]
    async fn test_remove_element_transport_error() -> fho::Result<()> {
        let client = fdomain_local::local_client_empty();
        let (proxy, server) =
            client.create_proxy_and_stream::<fdomain_fuchsia_element::ManagerMarker>();
        drop(server);

        let remove_cmd = SessionRemoveCommand { name: "foo".to_string() };
        let test_buffers = TestBuffers::default();
        let mut writer =
            VerifiedMachineWriter::<CommandStatus>::new_test(Some(Format::Json), &test_buffers);
        let response = remove_impl(proxy, remove_cmd, &mut writer).await;
        assert!(response.is_err());
        assert!(response.unwrap_err().to_string().starts_with("Transport error removing element"));
        let output = test_buffers.into_stdout_str();
        assert!(output.is_empty());
        Ok(())
    }
}
