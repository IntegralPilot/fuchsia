// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use async_trait::async_trait;
use ffx_target_get_time_args::GetTimeCommand;
use ffx_writer::{ToolIO, VerifiedMachineWriter};
use fho::{FfxContext, FfxMain, FfxTool};
use std::io::Write;

#[derive(serde::Serialize, schemars::JsonSchema)]
pub struct TimeInfo {
    pub nanoseconds: i64,
}
use target_holders::RemoteControlProxyHolder;

#[derive(FfxTool)]
#[target(direct)]
pub struct GetTimeTool {
    #[command]
    cmd: GetTimeCommand,
    rcs_proxy: RemoteControlProxyHolder,
}

fho::embedded_plugin!(GetTimeTool);

#[async_trait(?Send)]
impl FfxMain for GetTimeTool {
    type Writer = VerifiedMachineWriter<TimeInfo>;

    type Error = ::fho::Error;

    async fn main(self, mut writer: Self::Writer) -> fho::Result<()> {
        get_time_impl(self.rcs_proxy, &mut writer, self.cmd.boot).await?;
        Ok(())
    }
}

async fn get_time_impl(
    rcs_proxy: RemoteControlProxyHolder,
    writer: &mut VerifiedMachineWriter<TimeInfo>,
    boot_time: bool,
) -> Result<()> {
    let time = if boot_time {
        rcs_proxy.get_boot_time().await.user_message("Failed to get boot time")?.into_nanos()
    } else {
        rcs_proxy.get_time().await.user_message("Failed to get monotonic time")?.into_nanos()
    };
    let info = TimeInfo { nanoseconds: time };
    if writer.is_machine() {
        writer.machine(&info)?;
    } else {
        write!(writer, "{}", info.nanoseconds)?;
    }
    Ok(())
}

#[cfg(test)]
mod test {
    use super::*;
    use fdomain_fuchsia_developer_remotecontrol as rcs;
    use ffx_writer::{Format, TestBuffers};
    use target_holders::fake_proxy;

    fn setup_fake_time_server_proxy() -> rcs::RemoteControlProxy {
        let client = fdomain_local::local_client_empty();
        fake_proxy(client, move |req| match req {
            rcs::RemoteControlRequest::GetTime { responder } => {
                responder.send(fidl::MonotonicInstant::from_nanos(123456789)).unwrap();
            }
            rcs::RemoteControlRequest::GetBootTime { responder } => {
                responder.send(fidl::BootInstant::from_nanos(234567890)).unwrap();
            }
            _ => {}
        })
    }

    #[fuchsia::test]
    async fn test_get_monotonic() {
        let test_buffers = TestBuffers::default();
        let mut writer = VerifiedMachineWriter::<TimeInfo>::new_test(None, &test_buffers);
        get_time_impl(setup_fake_time_server_proxy().into(), &mut writer, false).await.unwrap();

        let stdout = test_buffers.into_stdout_str();
        assert_eq!(stdout, "123456789");
    }

    #[fuchsia::test]
    async fn test_get_boot() {
        let test_buffers = TestBuffers::default();
        let mut writer = VerifiedMachineWriter::<TimeInfo>::new_test(None, &test_buffers);
        get_time_impl(setup_fake_time_server_proxy().into(), &mut writer, true).await.unwrap();

        let stdout = test_buffers.into_stdout_str();
        assert_eq!(stdout, "234567890");
    }

    #[fuchsia::test]
    async fn test_get_machine() {
        let test_buffers = TestBuffers::default();
        let mut writer =
            VerifiedMachineWriter::<TimeInfo>::new_test(Some(Format::Json), &test_buffers);
        get_time_impl(setup_fake_time_server_proxy().into(), &mut writer, false).await.unwrap();

        let stdout = test_buffers.into_stdout_str();
        assert_eq!(stdout, "{\"nanoseconds\":123456789}\n");
    }
}
