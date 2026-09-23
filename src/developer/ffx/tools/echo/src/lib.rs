// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use argh::{ArgsInfo, FromArgs};
use async_trait::async_trait;
use fdomain_fuchsia_developer_ffx as ffx;
use ffx_writer::{MachineWriter, ToolIO as _};
use fho::{FfxContext, FfxMain, FfxTool, Result};
use target_holders::moniker;

// [START command_struct]
#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "echo", description = "run echo test against the target")]
pub struct EchoCommand {
    #[argh(positional)]
    /// text string to echo back and forth
    pub text: Option<String>,
}
// [END command_struct]

// [START tool_struct]
#[derive(FfxTool)]
pub struct EchoTool {
    #[command]
    cmd: EchoCommand,
    #[with(moniker("core/echo"))]
    echo_proxy: ffx::EchoProxy,
}
// [END tool_struct]

// [START tool_impl]
#[async_trait(?Send)]
impl FfxMain for EchoTool {
    type Writer = MachineWriter<String>;

    type Error = ::fho::Error;

    async fn main(self, mut writer: Self::Writer) -> Result<()> {
        let text = self.cmd.text.as_deref().unwrap_or("FFX");
        let echo_out = self
            .echo_proxy
            .echo_string(text)
            .await
            .user_message("Error returned from echo service")?;
        writer.item(&echo_out)?;
        Ok(())
    }
}
// [END tool_impl]

#[cfg(test)]
mod tests {
    use super::*;
    use ffx_writer::TestBuffer;

    // [START fake_proxy]
    fn setup_fake_echo_proxy() -> ffx::EchoProxy {
        let client = fdomain_local::local_client_empty();
        target_holders::fake_proxy::<ffx::EchoProxy>(client, move |req| match req {
            ffx::EchoRequest::EchoString { value, responder } => {
                responder.send(value.as_ref()).unwrap();
            }
        })
    }
    // [END fake_proxy]

    // [START echo_test]
    #[fuchsia::test]
    async fn test_regular_run() {
        const ECHO: &'static str = "foo";
        let cmd = EchoCommand { text: Some(ECHO.to_owned()) };
        let echo_proxy = setup_fake_echo_proxy();
        let test_stdout = TestBuffer::default();
        let writer = MachineWriter::new_buffers(None, test_stdout.clone(), Vec::new());
        let tool = EchoTool { cmd, echo_proxy };
        tool.main(writer).await.unwrap();
        assert_eq!(format!("{ECHO}\n"), test_stdout.into_string());
    }
    // [END echo_test]
}
