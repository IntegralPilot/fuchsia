// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_trait::async_trait;
use ffx_target_log_message_args::LogMessageCommand;
use ffx_writer::SimpleWriter;
use fho::{FfxContext, FfxMain, FfxTool};
use target_holders::RemoteControlProxyHolder;

#[derive(FfxTool)]
pub struct LogMessageTool {
    #[command]
    cmd: LogMessageCommand,
    rcs_proxy: RemoteControlProxyHolder,
}

fho::embedded_plugin!(LogMessageTool);

#[async_trait(?Send)]
impl FfxMain for LogMessageTool {
    type Writer = SimpleWriter;

    type Error = ::fho::Error;

    async fn main(self, mut writer: Self::Writer) -> fho::Result<()> {
        log_message_impl(self.rcs_proxy, &mut writer, self.cmd).await
    }
}

async fn log_message_impl<W>(
    rcs_proxy: RemoteControlProxyHolder,
    mut writer: W,
    cmd: LogMessageCommand,
) -> fho::Result<()>
where
    W: std::io::Write,
{
    rcs_proxy
        .log_message(&cmd.tag, &cmd.message, cmd.severity.into())
        .await
        .user_message("Failed to log message")?;
    writer.write_all(format!("Logged message to device successfully.\n").as_bytes()).unwrap();
    Ok(())
}

#[cfg(test)]
mod test {
    use super::*;
    use diagnostics_log_types::Severity;
    use fdomain_fuchsia_developer_remotecontrol as rcs;
    use target_holders::fake_proxy;

    const EXPECTED_TAG: &str = "some tag";
    const EXPECTED_MESSAGE: &str = "some message";
    const EXPECTED_SEVERITY: Severity = Severity::Fatal;

    fn setup_fake_log_write_server_proxy() -> rcs::RemoteControlProxy {
        let client = fdomain_local::local_client_empty();
        fake_proxy(client, move |req| match req {
            rcs::RemoteControlRequest::LogMessage { tag, message, severity, responder } => {
                assert_eq!(tag, EXPECTED_TAG);
                assert_eq!(message, EXPECTED_MESSAGE);
                assert_eq!(severity, EXPECTED_SEVERITY.into());
                responder.send().unwrap();
            }
            _ => {}
        })
    }

    #[fuchsia::test]
    async fn test_write_message() {
        let mut writer = Vec::new();
        let cmd = LogMessageCommand {
            tag: EXPECTED_TAG.to_string(),
            message: EXPECTED_MESSAGE.to_string(),
            severity: EXPECTED_SEVERITY,
        };
        log_message_impl(setup_fake_log_write_server_proxy().into(), &mut writer, cmd)
            .await
            .unwrap();
    }
}
