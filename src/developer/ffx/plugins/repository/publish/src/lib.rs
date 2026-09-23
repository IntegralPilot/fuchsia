// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_trait::async_trait;
pub use ffx_repository_publish_args::RepoPublishCommand;
use ffx_writer::VerifiedMachineWriter;
use fho::{Error, FfxMain, FfxTool, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::marker::PhantomData;

#[derive(Debug, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum CommandStatus {
    /// Successfully executed the command.
    Ok {},
    /// Unexpected error with string denoting error message.
    UnexpectedError { message: String },
    /// A known error that can be reported to the user.
    UserError { message: String },
}

///  trait is used to allow mocking of the method.

#[async_trait(?Send)]
pub trait PackageTools {
    async fn cmd_repo_publish(mut cmd: RepoPublishCommand) -> Result<()>;
}

pub struct SizedPackageTools {}

#[async_trait(?Send)]
impl PackageTools for SizedPackageTools {
    async fn cmd_repo_publish(cmd: RepoPublishCommand) -> Result<()> {
        package_tool::cmd_repo_publish(cmd).await.map_err(Into::into)
    }
}

#[derive(FfxTool)]
#[target(None)]
pub struct PublishTool<T: PackageTools> {
    #[command]
    cmd: RepoPublishCommand,
    _phantom: PhantomData<T>,
}

fho::embedded_plugin!(PublishTool<SizedPackageTools>);

#[async_trait(?Send)]
impl<T: PackageTools> FfxMain for PublishTool<T> {
    type Writer = VerifiedMachineWriter<CommandStatus>;

    type Error = ::fho::Error;

    async fn main(self, mut writer: Self::Writer) -> Result<()> {
        match T::cmd_repo_publish(self.cmd).await {
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ffx_writer::{Format, TestBuffers};
    use fho::bug;

    struct OkFakeTools {}
    #[async_trait(?Send)]
    impl PackageTools for OkFakeTools {
        async fn cmd_repo_publish(_cmd: RepoPublishCommand) -> Result<()> {
            Ok(())
        }
    }

    struct ErrFakeTools {}
    #[async_trait(?Send)]
    impl PackageTools for ErrFakeTools {
        async fn cmd_repo_publish(_cmd: RepoPublishCommand) -> Result<()> {
            Err(bug!("general error"))
        }
    }
    #[fuchsia::test]
    async fn test_machine_ok() {
        let tool = PublishTool::<OkFakeTools> {
            cmd: RepoPublishCommand {
                signing_keys: None,
                trusted_keys: None,
                trusted_root: None,
                package_manifests: vec![],
                package_list_manifests: vec![],
                package_archives: vec![],
                product_bundle: vec![],
                time_versioning: false,
                metadata_current_time: chrono::Utc::now(),
                refresh_root: false,
                clean: false,
                depfile: None,
                copy_mode: fuchsia_repo::repository::CopyMode::Copy,
                delivery_blob_type: 1,
                watch: false,
                ignore_missing_packages: false,
                blob_manifest: None,
                blob_repo_dir: None,
                repo_path: "/somewhere".into(),
            },
            _phantom: PhantomData,
        };
        let buffers = TestBuffers::default();
        let writer =
            <PublishTool<OkFakeTools> as FfxMain>::Writer::new_test(Some(Format::Json), &buffers);

        let res = tool.main(writer).await;

        let (stdout, stderr) = buffers.into_strings();
        assert!(res.is_ok(), "expected ok: {stdout} {stderr}");
        let err = format!("schema not valid {stdout}");
        let json = serde_json::from_str(&stdout).expect(&err);
        let err = format!("json must adhere to schema: {json}");
        <PublishTool<OkFakeTools> as FfxMain>::Writer::verify_schema(&json).expect(&err);
        assert_eq!(json, serde_json::json!({"ok":{}}));
    }

    #[fuchsia::test]
    async fn test_machine_error() {
        let tool = PublishTool::<ErrFakeTools> {
            cmd: RepoPublishCommand {
                signing_keys: None,
                trusted_keys: None,
                trusted_root: None,
                package_manifests: vec![],
                package_list_manifests: vec![],
                package_archives: vec![],
                product_bundle: vec![],
                time_versioning: false,
                metadata_current_time: chrono::Utc::now(),
                refresh_root: false,
                clean: false,
                depfile: None,
                copy_mode: fuchsia_repo::repository::CopyMode::Copy,
                delivery_blob_type: 1,
                watch: false,
                ignore_missing_packages: false,
                blob_manifest: None,
                blob_repo_dir: None,
                repo_path: "/somewhere".into(),
            },
            _phantom: PhantomData,
        };
        let buffers = TestBuffers::default();
        let writer =
            <PublishTool<ErrFakeTools> as FfxMain>::Writer::new_test(Some(Format::Json), &buffers);

        let res = tool.main(writer).await;

        let (stdout, stderr) = buffers.into_strings();
        assert!(res.is_err(), "expected err: {stdout} {stderr}");
        let err = format!("schema not valid {stdout}");
        let json = serde_json::from_str(&stdout).expect(&err);
        let err = format!("json must adhere to schema: {json}");
        <PublishTool<ErrFakeTools> as FfxMain>::Writer::verify_schema(&json).expect(&err);
        assert_eq!(
            json,
            serde_json::json!({"unexpected_error" :{"message": "BUG: An internal command error occurred.\nError: general error"}})
        );
    }
}
