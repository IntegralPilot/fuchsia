// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use anyhow::Result;
use futures::future::LocalBoxFuture;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::process::{Command, ExitStatus};

pub mod strict;
pub mod test;

#[derive(Debug, Clone)]
pub struct CommandOutput {
    pub status: ExitStatus,
    pub stdout: String,
    pub stderr: String,
}

fn parse_json<'a, T>(s: &'a str) -> Result<T, ffx_writer::Error>
where
    T: Deserialize<'a>,
{
    serde_json::from_str(s)
        .map_err(|e| ffx_writer::Error::SchemaFailure(format!("err: {e:?} for {}", s)))
}

impl CommandOutput {
    pub fn machine_output<T>(&self) -> Result<T, ffx_writer::Error>
    where
        T: for<'a> Deserialize<'a> + Serialize + JsonSchema,
    {
        // This is parsed twice, as the verify_schema function requires an untyped `Value`, while
        // the return value from this function is the object we are parsing after schema
        // verification.
        let json = parse_json(&self.stdout)?;
        ffx_writer::VerifiedMachineWriter::<T>::verify_schema(&json)?;
        parse_json(&self.stdout)
    }
}

#[derive(thiserror::Error, Debug)]
pub enum CommandConstructionError {
    #[error("Must pass at least one argument")]
    EmptyArguments,

    #[error("Cannot pass the first argument as a flag")]
    FirstArgumentIsFlag,

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(thiserror::Error, Debug)]
pub enum ExecutionError {
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
}

fn run_ffx_command(mut cmd: Command) -> Result<CommandOutput, ExecutionError> {
    let out = cmd.output().map_err(ExecutionError::IoError)?;
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    Ok(CommandOutput { status: out.status, stdout, stderr })
}

/// Represents an object that is capable of creating an ffx command. Used primarily for integration
/// testing.
pub trait FfxExecutor {
    type Error: std::error::Error + Send + Sync + 'static;

    fn make_ffx_cmd(&self, args: &[&str]) -> std::result::Result<Command, Self::Error>;

    fn exec_ffx<'a>(
        &'a self,
        cmd: Command,
    ) -> LocalBoxFuture<'a, Result<CommandOutput, ExecutionError>> {
        Box::pin(async move {
            log::info!("Executing ffx command: {cmd:?}");
            fuchsia_async::unblock(move || run_ffx_command(cmd)).await
        })
    }

    /// Like [`FfxExecutor::exec_ffx`], but runs synchronously.
    fn exec_ffx_sync(&self, cmd: Command) -> Result<CommandOutput, ExecutionError> {
        log::info!("Executing ffx command: {cmd:?}");
        run_ffx_command(cmd)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A nonsense executor that just echoes.
    struct Echoer;

    impl FfxExecutor for Echoer {
        type Error = CommandConstructionError;

        fn make_ffx_cmd(&self, args: &[&str]) -> std::result::Result<Command, Self::Error> {
            let mut cmd = Command::new("echo");
            cmd.args(args);
            Ok(cmd)
        }
    }

    #[fuchsia::test]
    async fn test_echoer() {
        let echoer = Echoer;
        let cmd = echoer.make_ffx_cmd(&["foo", "bar"]).unwrap();
        let output = echoer.exec_ffx(cmd).await.expect("echoing some nonsense");
        assert!(output.status.success(), "Got non-successful return value: {:?}", output.status);
        assert!(
            output.stdout.contains("foo") && output.stdout.contains("bar"),
            "stdout doesn't contain correct output: {:?}",
            output.stdout
        );
    }

    #[derive(thiserror::Error, Debug)]
    pub enum BadCommandConstructionError {
        #[error("General error: {0}")]
        General(String),
    }

    struct BadCommandBuilder;

    impl FfxExecutor for BadCommandBuilder {
        type Error = BadCommandConstructionError;

        fn make_ffx_cmd(&self, _args: &[&str]) -> std::result::Result<Command, Self::Error> {
            return Err(BadCommandConstructionError::General(
                "Oh no we can't build the command for some reason. I just work here...."
                    .to_string(),
            ));
        }
    }

    #[fuchsia::test]
    async fn test_bad_command_builder() {
        let bad_builder = BadCommandBuilder;
        let result = bad_builder.make_ffx_cmd(&["foo", "bar", "bazzlewazzle"]);
        assert!(result.is_err(), "expected error. Received: {:?}", result);
        assert!(
            matches!(result, Err(BadCommandConstructionError::General(_))),
            "received wrong error: {:?}",
            result
        );
    }

    struct NonExistentBinaryExecutor;

    impl FfxExecutor for NonExistentBinaryExecutor {
        type Error = CommandConstructionError;

        fn make_ffx_cmd(&self, args: &[&str]) -> std::result::Result<Command, Self::Error> {
            // Run a non-existent binary.
            let mut cmd = Command::new("fffffffffx");
            cmd.args(args);
            Ok(cmd)
        }
    }

    #[fuchsia::test]
    async fn test_nonexistent_binary() {
        let e = NonExistentBinaryExecutor;
        let cmd = e.make_ffx_cmd(&["foo", "bar"]).unwrap();
        let result = e.exec_ffx(cmd).await;
        assert!(result.is_err(), "expected error. Received: {:?}", result);
        assert!(
            matches!(result, Err(ExecutionError::IoError(_))),
            "received wrong error type: {:?}",
            result
        );
    }

    #[derive(Debug, Deserialize, Serialize, JsonSchema)]
    #[serde(rename_all = "snake_case")]
    enum JsonResultThing {
        Success { message: String },
    }

    struct JsonExecutor;

    // This just doesn't run a command, but creates some command output. Might have been simpler to
    // use mockall crate, but this seemed straightforward enough.
    impl FfxExecutor for JsonExecutor {
        type Error = CommandConstructionError;

        fn make_ffx_cmd(&self, _args: &[&str]) -> std::result::Result<Command, Self::Error> {
            Ok(Command::new("echo"))
        }

        fn exec_ffx<'a>(
            &'a self,
            _cmd: Command,
        ) -> LocalBoxFuture<'a, Result<CommandOutput, ExecutionError>> {
            Box::pin(std::future::ready(Ok(CommandOutput {
                stdout: r#"{"success": {"message": "foobar"}}"#.to_owned(),
                status: Default::default(),
                stderr: Default::default(),
            })))
        }
    }

    #[fuchsia::test]
    async fn test_json_result_success() {
        let e = JsonExecutor;
        let cmd = e.make_ffx_cmd(&["foo", "bar"]).unwrap();
        let output = e.exec_ffx(cmd).await.unwrap();
        let json = output.machine_output::<JsonResultThing>().unwrap();
        let JsonResultThing::Success { message } = json;
        assert_eq!(message, "foobar");
    }

    struct BadJsonExecutor;

    impl FfxExecutor for BadJsonExecutor {
        type Error = CommandConstructionError;

        fn make_ffx_cmd(&self, _args: &[&str]) -> std::result::Result<Command, Self::Error> {
            Ok(Command::new("echo"))
        }

        fn exec_ffx<'a>(
            &'a self,
            _cmd: Command,
        ) -> LocalBoxFuture<'a, Result<CommandOutput, ExecutionError>> {
            Box::pin(std::future::ready(Ok(CommandOutput {
                stdout: r#"{"blorp": {"message": "foobar"}}"#.to_owned(),
                status: Default::default(),
                stderr: Default::default(),
            })))
        }
    }

    #[fuchsia::test]
    async fn test_json_result_failure() {
        let e = BadJsonExecutor;
        let cmd = e.make_ffx_cmd(&["foo", "bar"]).unwrap();
        let output = e.exec_ffx(cmd).await.unwrap();
        let result = output.machine_output::<JsonResultThing>();
        assert!(result.is_err(), "expected error. Got: {result:?}");
    }
}
