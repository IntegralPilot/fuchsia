// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use async_trait::async_trait;
use fdomain_fuchsia_session_power::HandoffProxy;
use ffx_session_common::CommandStatus;
use ffx_session_drop_power_lease_args::SessionDropPowerLeaseCommand;
use ffx_writer::{ToolIO, VerifiedMachineWriter};
use fho::{FfxMain, FfxTool, user_error};
use std::io::Write;
use target_holders::moniker;

#[derive(FfxTool)]
pub struct DropPowerLeaseTool {
    #[command]
    cmd: SessionDropPowerLeaseCommand,
    #[with(moniker("/core/session-manager"))]
    handoff_proxy: HandoffProxy,
}

fho::embedded_plugin!(DropPowerLeaseTool);

#[async_trait(?Send)]
impl FfxMain for DropPowerLeaseTool {
    type Writer = VerifiedMachineWriter<CommandStatus>;

    type Error = ::fho::Error;

    async fn main(self, mut writer: Self::Writer) -> fho::Result<()> {
        drop_power_lease_impl(self.handoff_proxy, self.cmd, &mut writer).await
    }
}

pub async fn drop_power_lease_impl(
    handoff_proxy: HandoffProxy,
    cmd: SessionDropPowerLeaseCommand,
    writer: &mut VerifiedMachineWriter<CommandStatus>,
) -> fho::Result<()> {
    if !writer.is_machine() {
        writeln!(writer, "Requesting to drop power lease on execution state")?;
    }
    match handoff_proxy.take().await {
        Ok(Ok(lease)) => {
            if !writer.is_machine() {
                writeln!(writer, "Success!")?;
            }
            writer.machine(&CommandStatus::Ok { message: None })?;
            drop(lease);
            Ok(())
        }
        Ok(Err(fdomain_fuchsia_session_power::HandoffError::AlreadyTaken)) if cmd.allow_missing => {
            if !writer.is_machine() {
                writeln!(writer, "Lease already dropped, ignoring error.")?;
                writeln!(writer, "Success!")?;
            }
            writer.machine(&CommandStatus::Ok {
                message: Some("Lease already dropped, ignoring error.".to_string()),
            })?;
            Ok(())
        }
        Ok(Err(err)) => {
            Err(user_error!("Failed to take power lease from session manager: {:?}", err))
        }
        Err(err) => Err(user_error!("Transport error taking power lease: {:?}", err)),
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use fdomain_fuchsia_session_power::HandoffRequest;
    use ffx_writer::{Format, TestBuffers};
    use target_holders::fake_proxy;

    #[fuchsia::test]
    async fn test_drop_power_lease() {
        let client = fdomain_local::local_client_empty();
        let client_clone = std::sync::Arc::clone(&client);

        let proxy = fake_proxy(client, move |req| match req {
            HandoffRequest::Take { responder } => {
                let _ = responder.send(Ok(client_clone.create_event().into()));
            }
            x => unimplemented!("{x:?}"),
        });

        let drop_power_lease_cmd = SessionDropPowerLeaseCommand { allow_missing: false };
        let test_buffers = TestBuffers::default();
        let mut writer = VerifiedMachineWriter::<CommandStatus>::new_test(None, &test_buffers);
        let result = drop_power_lease_impl(proxy, drop_power_lease_cmd, &mut writer).await;
        assert!(result.is_ok());
        let output = test_buffers.into_stdout_str();
        assert_eq!(output, "Requesting to drop power lease on execution state\nSuccess!\n");
    }

    #[fuchsia::test]
    async fn test_drop_power_lease_already_taken_error() {
        let client = fdomain_local::local_client_empty();
        let proxy = fake_proxy(std::sync::Arc::clone(&client), |req| match req {
            HandoffRequest::Take { responder } => {
                let _ =
                    responder.send(Err(fdomain_fuchsia_session_power::HandoffError::AlreadyTaken));
            }
            x => unimplemented!("{x:?}"),
        });

        let drop_power_lease_cmd = SessionDropPowerLeaseCommand { allow_missing: false };
        let test_buffers = TestBuffers::default();
        let mut writer = VerifiedMachineWriter::<CommandStatus>::new_test(None, &test_buffers);
        let result = drop_power_lease_impl(proxy, drop_power_lease_cmd, &mut writer).await;
        assert!(result.is_err());
    }

    #[fuchsia::test]
    async fn test_drop_power_lease_already_taken_allow_missing() {
        let client = fdomain_local::local_client_empty();
        let proxy = fake_proxy(std::sync::Arc::clone(&client), |req| match req {
            HandoffRequest::Take { responder } => {
                let _ =
                    responder.send(Err(fdomain_fuchsia_session_power::HandoffError::AlreadyTaken));
            }
            x => unimplemented!("{x:?}"),
        });

        let drop_power_lease_cmd = SessionDropPowerLeaseCommand { allow_missing: true };
        let test_buffers = TestBuffers::default();
        let mut writer = VerifiedMachineWriter::<CommandStatus>::new_test(None, &test_buffers);
        let result = drop_power_lease_impl(proxy, drop_power_lease_cmd, &mut writer).await;
        assert!(result.is_ok());
        let output = test_buffers.into_stdout_str();
        assert_eq!(
            output,
            "Requesting to drop power lease on execution state\nLease already dropped, ignoring error.\nSuccess!\n"
        );
    }

    #[fuchsia::test]
    async fn test_drop_power_lease_machine() -> Result<()> {
        let client = fdomain_local::local_client_empty();
        let client_clone = std::sync::Arc::clone(&client);

        let proxy = fake_proxy(client, move |req| match req {
            HandoffRequest::Take { responder } => {
                let _ = responder.send(Ok(client_clone.create_event().into()));
            }
            x => unimplemented!("{x:?}"),
        });

        let drop_power_lease_cmd = SessionDropPowerLeaseCommand { allow_missing: false };
        let test_buffers = TestBuffers::default();
        let mut writer =
            VerifiedMachineWriter::<CommandStatus>::new_test(Some(Format::Json), &test_buffers);
        let result = drop_power_lease_impl(proxy, drop_power_lease_cmd, &mut writer).await;
        assert!(result.is_ok());
        let output = test_buffers.into_stdout_str();
        let status: CommandStatus = serde_json::from_str(&output)?;
        assert_eq!(status, CommandStatus::Ok { message: None });
        Ok(())
    }

    #[fuchsia::test]
    async fn test_drop_power_lease_machine_already_taken_allow_missing() -> Result<()> {
        let client = fdomain_local::local_client_empty();
        let proxy = fake_proxy(std::sync::Arc::clone(&client), |req| match req {
            HandoffRequest::Take { responder } => {
                let _ =
                    responder.send(Err(fdomain_fuchsia_session_power::HandoffError::AlreadyTaken));
            }
            x => unimplemented!("{x:?}"),
        });

        let drop_power_lease_cmd = SessionDropPowerLeaseCommand { allow_missing: true };
        let test_buffers = TestBuffers::default();
        let mut writer =
            VerifiedMachineWriter::<CommandStatus>::new_test(Some(Format::Json), &test_buffers);
        let result = drop_power_lease_impl(proxy, drop_power_lease_cmd, &mut writer).await;
        assert!(result.is_ok());
        let output = test_buffers.into_stdout_str();
        let status: CommandStatus = serde_json::from_str(&output)?;
        assert_eq!(
            status,
            CommandStatus::Ok {
                message: Some("Lease already dropped, ignoring error.".to_string())
            }
        );
        Ok(())
    }
}
