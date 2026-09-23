// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result};
use camino::Utf8Path;
use ffx_config::{Config, ConfigLevel, EnvironmentContext};
use ffx_sdk_args::{RunCommand, SdkCommand, SetCommand, SetRootCommand, SetSubCommand, SubCommand};
use ffx_writer::{ToolIO as _, VerifiedMachineWriter};
use fho::{FfxContext, FfxMain, FfxTool, bug, exit_with_code, return_user_error, user_error};
use schemars::JsonSchema;
use sdk::metadata::ElementType;
use sdk::{Sdk, SdkRoot, SdkVersion, in_tree_sdk_version};
use serde::Serialize;
use std::io::{ErrorKind, Write};

#[derive(Debug, Serialize, JsonSchema)]
pub struct SdkInfo {
    /// The version of SDK based on the
    /// local environment.
    pub version: String,
}

#[derive(FfxTool)]
pub struct SdkTool {
    context: EnvironmentContext,
    #[command]
    cmd: SdkCommand,
}

fho::embedded_plugin!(SdkTool);

#[async_trait::async_trait(?Send)]
impl FfxMain for SdkTool {
    type Writer = VerifiedMachineWriter<SdkInfo>;

    type Error = ::fho::Error;

    async fn main(self, writer: Self::Writer) -> fho::Result<()> {
        let sdk: Sdk =
            self.context.get_sdk().user_message("Could not load currently active SDK")?;
        match &self.cmd.sub {
            SubCommand::Version(_) => exec_version(sdk, writer).await.map_err(Into::into),
            SubCommand::Set(cmd) => {
                if writer.is_machine() {
                    return_user_error!("This command does not support machine output");
                }
                exec_set(self.context, cmd).await.map_err(Into::into)
            }
            SubCommand::Run(cmd) => {
                if writer.is_machine() {
                    return_user_error!("This command does not support machine output");
                }
                exec_run(sdk, cmd).await
            }
            SubCommand::PopulatePath(cmd) => {
                if writer.is_machine() {
                    return_user_error!("This command does not support machine output");
                }
                exec_populate_path(
                    writer,
                    self.context
                        .get_sdk_root()
                        .map_err(ffx_config::macro_deps::anyhow::Error::from)?,
                    &cmd.path,
                )
            }
        }
    }
}

async fn exec_run(sdk: Sdk, cmd: &RunCommand) -> fho::Result<()> {
    let status = sdk
        .get_host_tool_command(&cmd.name)
        .with_user_message(|| {
            format!("Could not find the host tool `{}` in the currently active sdk", cmd.name)
        })?
        .args(&cmd.args)
        .status()
        .map_err(|e| {
            user_error!(
                "Failed to spawn host tool `{}` from the sdk with system error: {e}",
                cmd.name
            )
        })?;

    if status.success() { Ok(()) } else { exit_with_code!(status.code().unwrap_or(1)) }
}

async fn exec_version(sdk: Sdk, mut writer: VerifiedMachineWriter<SdkInfo>) -> Result<()> {
    let info = SdkInfo {
        version: match sdk.get_version() {
            SdkVersion::Version(v) => v.clone(),
            SdkVersion::InTree => in_tree_sdk_version(),
            SdkVersion::Unknown => "unknown".into(),
        },
    };

    writer.machine_or_else(&info, || format!("{}", info.version)).map_err(|e| bug!("{e}"))?;

    Ok(())
}

async fn exec_set(context: EnvironmentContext, cmd: &SetCommand) -> Result<()> {
    match &cmd.sub {
        SetSubCommand::Root(SetRootCommand { path }) => {
            let abs_path =
                path.canonicalize().with_context(|| format!("making path absolute: {:?}", path))?;
            let env = context.load()?;
            let mut config = Config::from_env(&env)?;
            config.set("sdk.root", ConfigLevel::User, abs_path.to_string_lossy().into())?;
            config.save()?;
            Ok(())
        }
    }
}

fn exec_populate_path(
    mut writer: impl Write,
    sdk_root: SdkRoot,
    bin_path: &Utf8Path,
) -> fho::Result<()> {
    let inner_bin_path = bin_path.join("fuchsia-sdk");
    let full_fuchsia_sdk_run_path = inner_bin_path.join("fuchsia-sdk-run");
    log::debug!(
        "Installing host tool stubs to {bin_path:?} (and `fuchsia-sdk-run` to {inner_bin_path:?}) from SDK {sdk_root:?}"
    );

    let sdk = sdk_root.get_sdk().map_err(anyhow::Error::from)?;
    let sdk_run_tool = sdk.get_host_tool("fuchsia-sdk-run").user_message("SDK does not contain `fuchsia-sdk-run` host tool. You may need to update your SDK to use this command.")?;
    log::debug!("Found `fuchsia-sdk-run` in the SDK at {sdk_run_tool:?}");

    std::fs::create_dir_all(&inner_bin_path).with_user_message(|| {
        format!("Could not create {inner_bin_path:?}. Do you have write access to {bin_path:?}?")
    })?;
    std::fs::copy(&sdk_run_tool, &full_fuchsia_sdk_run_path).with_user_message(|| format!("Could not install `fuchsia-sdk-run` tool to {inner_bin_path:?}. Do you have write access to {bin_path:?}"))?;

    writeln!(writer, "Installing host tool stubs to {bin_path:?}").bug()?;
    for tool in sdk.get_all_host_tools_metadata() {
        let tool_name = &tool.name;
        if tool.kind != ElementType::HostTool && tool_name != "fuchsia-sdk-run" {
            log::trace!("Skipping companion tool (or other kind of tool) {tool_name}");
        }

        writeln!(writer, "Installing {tool_name}").bug()?;
        match std::os::unix::fs::symlink("fuchsia-sdk/fuchsia-sdk-run", bin_path.join(tool_name)) {
            Ok(_) => {}
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
            other => other.with_user_message(|| {
                format!("Could not create symlink for `{tool_name}` in {bin_path:?}")
            })?,
        }
    }

    writeln!(writer, "\n\
        All tools installed in {bin_path:?}. Add the following to your environment to use project local configuration to run SDK host tools:\n\
        \n\
        PATH={bin_path}:$PATH").bug()?;

    Ok(())
}

#[cfg(test)]
mod test {
    use super::*;
    use ffx_writer::TestBuffers;

    #[fuchsia::test]
    async fn test_version_with_string() {
        let sdk = Sdk::get_empty_sdk_with_version(SdkVersion::Version("Test.0".to_owned()));
        let output = TestBuffers::default();
        let writer = VerifiedMachineWriter::new_test(None, &output);

        exec_version(sdk, writer).await.expect("exec_version");
        let actual = output.into_stdout_str();

        assert_eq!("Test.0\n", actual);
    }

    #[fuchsia::test]
    async fn test_version_in_tree() {
        let output = TestBuffers::default();
        let writer = VerifiedMachineWriter::new_test(None, &output);
        let sdk = Sdk::get_empty_sdk_with_version(SdkVersion::InTree);

        exec_version(sdk, writer).await.expect("exec_version");
        let actual = output.into_stdout_str();

        let re = regex::Regex::new(r"^\d+.99991231.0.1\n$").expect("creating regex");
        assert!(re.is_match(&actual));
    }

    #[fuchsia::test]
    async fn test_version_unknown() {
        let output = TestBuffers::default();
        let writer = VerifiedMachineWriter::new_test(None, &output);
        let sdk = Sdk::get_empty_sdk_with_version(SdkVersion::Unknown);

        exec_version(sdk, writer).await.expect("exec_version");
        let actual = output.into_stdout_str();

        assert_eq!("unknown\n", actual);
    }
}
