// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_trait::async_trait;
use ffx_config::EnvironmentContext;
use ffx_target::{TargetInfoQuery, knock_target};
use ffx_target_add_args::AddCommand;
use ffx_writer::VerifiedMachineWriter;
use fho::{FfxMain, FfxTool};
use manual_targets::{Config as ManualTargetsConfig, ManualTargets};
use schemars::JsonSchema;
use serde::Serialize;

#[derive(Debug, Serialize, JsonSchema)]
pub enum CommandStatus {
    /// Successful execution with an optional informational string.
    Ok { message: Option<String> },
    /// Unexpected error with string.
    UnexpectedError { message: String },
    /// A known kind of error that can be reported usefully to the user
    UserError { message: String },
}

#[derive(FfxTool)]
pub struct AddTool {
    #[command]
    cmd: AddCommand,
    context: EnvironmentContext,
}

fho::embedded_plugin!(AddTool);

#[async_trait(?Send)]
impl FfxMain for AddTool {
    type Writer = VerifiedMachineWriter<CommandStatus>;

    type Error = ::fho::Error;

    async fn main(self, mut writer: Self::Writer) -> fho::Result<()> {
        let addr = self.cmd.addr.clone();
        if !self.cmd.nowait {
            let query = match TargetInfoQuery::try_from(addr.clone()) {
                Ok(q) => q,
                Err(e) => {
                    let msg = format!("Could not parse '{addr}'. {e}");
                    let _ = writer.machine(&CommandStatus::UserError { message: msg.clone() });
                    return Err(fho::user_error!("{msg}"));
                }
            };
            if let Err(e) = knock_target(&query, &self.context, None).await {
                let msg = format!("Could not connect to target: {e}");
                let _ = writer.machine(&CommandStatus::UserError { message: msg.clone() });
                return Err(fho::user_error!("{msg}"));
            }
        }
        let mt = ManualTargetsConfig::new_from_context(&self.context);
        if let Err(e) = mt.add(addr.clone()).await {
            let msg = format!("Failed to add target to manual targets collection: {e}");
            let _ = writer.machine(&CommandStatus::UserError { message: msg.clone() });
            return Err(fho::user_error!("{msg}"));
        }
        writer.machine(&CommandStatus::Ok { message: None })?;
        Ok(())
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use ffx_writer::{Format, TestBuffers};

    #[fuchsia::test]
    async fn test_machine_output() {
        let env = ffx_config::test_init().expect("test_init");
        let tool = AddTool {
            cmd: AddCommand { addr: "[f000::1%1]:640".to_owned(), nowait: true },
            context: env.context.clone(),
        };

        let buffers = TestBuffers::default();
        let writer = VerifiedMachineWriter::new_test(Some(Format::Json), &buffers);
        tool.main(writer).await.expect("target add");

        let expected = String::from("{\"Ok\":{\"message\":null}}\n");
        let actual = buffers.into_stdout_str();
        assert_eq!(expected, actual)
    }

    #[fuchsia::test]
    async fn test_machine_output_err() {
        let env = ffx_config::test_init().expect("test_init");
        let tool = AddTool {
            cmd: AddCommand { addr: "invalid_address-100".into(), nowait: false },
            context: env.context.clone(),
        };

        let buffers = TestBuffers::default();
        let writer = VerifiedMachineWriter::new_test(Some(Format::Json), &buffers);
        let res = tool.main(writer).await;
        assert!(res.is_err());

        let expected = String::from(
            "{\"UserError\":{\"message\":\"Could not connect to target: non-critical error: Target not found: NodenameOrId(\\\"invalid_address-100\\\")\"}}\n",
        );
        let actual = buffers.into_stdout_str();
        assert_eq!(expected, actual)
    }

    #[fuchsia::test]
    async fn test_add_in_direct_mode_nowait() {
        let env = ffx_config::test_env()
            .runtime_config(ffx_config::keys::DIRECT_CONNECTIONS, true)
            .build()
            .expect("test_env build");
        let tool = AddTool {
            cmd: AddCommand { addr: "127.0.0.1:8022".into(), nowait: true },
            context: env.context.clone(),
        };
        let buffers = TestBuffers::default();
        let writer = VerifiedMachineWriter::new_test(Some(Format::Json), &buffers);
        tool.main(writer).await.expect("target add");

        let mt = ManualTargetsConfig::new_from_context(&env.context);
        let value = mt.get().await.unwrap();
        let targets = value.as_object().unwrap();
        assert!(targets.contains_key("127.0.0.1:8022"));
    }
}
