// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_trait::async_trait;
use ffx_repository_create_args::RepoCreateCommand;
use ffx_writer::VerifiedMachineWriter;
use fho::{Error, FfxMain, FfxTool, Result, bug};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::marker::PhantomData;

#[derive(Debug, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum CommandStatus {
    /// Successfully waited for the target (either to come up or shut down).
    Ok {},
    /// Unexpected error with string denoting error message.
    UnexpectedError { message: String },
    /// A known error that can be reported to the user.
    UserError { message: String },
}

///  trait is used to allow mocking of
/// the method.

#[async_trait(?Send)]
pub trait PackageTools {
    async fn cmd_repo_create(cmd: RepoCreateCommand) -> Result<()>;
}

pub struct SizedPackageTools {}

#[async_trait(?Send)]
impl PackageTools for SizedPackageTools {
    async fn cmd_repo_create(cmd: RepoCreateCommand) -> Result<()> {
        package_tool::cmd_repo_create(cmd).await.map_err(Into::into)
    }
}

#[derive(FfxTool)]
#[target(None)]
pub struct CreateTool<T: PackageTools> {
    #[command]
    cmd: RepoCreateCommand,
    _phantom: PhantomData<T>,
}

fho::embedded_plugin!(CreateTool<SizedPackageTools>);

#[async_trait(?Send)]
impl<T: PackageTools> FfxMain for CreateTool<T> {
    type Writer = VerifiedMachineWriter<CommandStatus>;

    type Error = ::fho::Error;

    async fn main(self, mut writer: Self::Writer) -> Result<()> {
        match T::cmd_repo_create(self.cmd).await {
            Ok(()) => {
                writer.machine(&CommandStatus::Ok {})?;
                Ok(())
            }
            Err(e @ Error::User(_)) => {
                writer.machine(&CommandStatus::UserError { message: e.to_string() })?;
                Err(e)
            }
            Err(e) => {
                writer.machine(&CommandStatus::UnexpectedError { message: e.to_string() })?;
                Err(e)
            }
        }
        .map_err(|err| bug!("Error: failed to create repository: {err:?}"))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ffx_writer::{Format, TestBuffers};

    struct OkFakeTools {}
    #[async_trait(?Send)]
    impl PackageTools for OkFakeTools {
        async fn cmd_repo_create(_cmd: RepoCreateCommand) -> Result<()> {
            Ok(())
        }
    }

    struct ErrFakeTools {}
    #[async_trait(?Send)]
    impl PackageTools for ErrFakeTools {
        async fn cmd_repo_create(_cmd: RepoCreateCommand) -> Result<()> {
            Err(bug!("general error"))
        }
    }
    #[fuchsia::test]
    async fn test_machine_ok() {
        let tool = CreateTool::<OkFakeTools> {
            cmd: RepoCreateCommand {
                time_versioning: false,
                keys: None,
                repo_path: "/somewhere".into(),
            },
            _phantom: PhantomData,
        };
        let buffers = TestBuffers::default();
        let writer =
            <CreateTool<OkFakeTools> as FfxMain>::Writer::new_test(Some(Format::Json), &buffers);

        let res = tool.main(writer).await;

        let (stdout, stderr) = buffers.into_strings();
        assert!(res.is_ok(), "expected ok: {stdout} {stderr}");
        let err = format!("schema not valid {stdout}");
        let json = serde_json::from_str(&stdout).expect(&err);
        let err = format!("json must adhere to schema: {json}");
        <CreateTool<OkFakeTools> as FfxMain>::Writer::verify_schema(&json).expect(&err);
        assert_eq!(json, serde_json::json!({"ok":{}}));
    }

    #[fuchsia::test]
    async fn test_machine_error() {
        let tool = CreateTool::<ErrFakeTools> {
            cmd: RepoCreateCommand {
                time_versioning: false,
                keys: None,
                repo_path: "/somewhere".into(),
            },
            _phantom: PhantomData,
        };
        let buffers = TestBuffers::default();
        let writer =
            <CreateTool<ErrFakeTools> as FfxMain>::Writer::new_test(Some(Format::Json), &buffers);

        let res = tool.main(writer).await;

        let (stdout, stderr) = buffers.into_strings();
        assert!(res.is_err(), "expected err: {stdout} {stderr}");
        let err = format!("schema not valid {stdout}");
        let json = serde_json::from_str(&stdout).expect(&err);
        let err = format!("json must adhere to schema: {json}");
        <CreateTool<ErrFakeTools> as FfxMain>::Writer::verify_schema(&json).expect(&err);
        assert_eq!(
            json,
            serde_json::json!({"unexpected_error" :{"message": "BUG: An internal command error occurred.\nError: general error"}})
        );
    }
}
