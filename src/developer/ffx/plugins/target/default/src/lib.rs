// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use async_trait::async_trait;
use ffx_config::keys::TARGET_DEFAULT_KEY;
use ffx_config::{ConfigLevel, EnvironmentContext};
use ffx_target_default_args::{SubCommand, TargetDefaultCommand};
use ffx_writer::{ToolIO, VerifiedMachineWriter};
use fho::{FfxContext, FfxMain, FfxTool};
use std::io::Write;

#[derive(serde::Serialize, schemars::JsonSchema)]
pub struct TargetDefaultInfo {
    pub target: Option<String>,
}

#[derive(FfxTool)]
pub struct TargetDefaultTool {
    #[command]
    cmd: TargetDefaultCommand,
    context: EnvironmentContext,
}

fho::embedded_plugin!(TargetDefaultTool);

#[async_trait(?Send)]
impl FfxMain for TargetDefaultTool {
    type Writer = VerifiedMachineWriter<TargetDefaultInfo>;

    type Error = ::fho::Error;

    async fn main(self, mut writer: Self::Writer) -> fho::Result<()> {
        exec_target_default_impl(&self.context, self.cmd, &mut writer).await?;
        Ok(())
    }
}

const TARGET_GET_NO_TARGET_MSG: &str = "\
No default target.\n\
If exactly one target is connected, ffx will use that.\n";

pub async fn exec_target_default_impl(
    context: &EnvironmentContext,
    cmd: TargetDefaultCommand,
    writer: &mut VerifiedMachineWriter<TargetDefaultInfo>,
) -> Result<()> {
    match &cmd.subcommand {
        SubCommand::Get(_) => {
            // get_target_specifier can be overridden by `-t|--target` and it
            // seems more reasonable for `ffx target default get` to just ignore
            // that flag.
            let target = context
                .query(TARGET_DEFAULT_KEY)
                .level(Some(ConfigLevel::Default))
                .build()
                .get_optional::<Option<String>>(context)
                .bug_context("Failed to get default target from config")?;
            let info = TargetDefaultInfo { target: target.filter(|t| !t.is_empty()) };
            if writer.is_machine() {
                writer.machine(&info)?;
            } else {
                match &info.target {
                    Some(target) => {
                        writeln!(writer, "{}", safe_string::TermSafe::from_str_escaped(target))?
                    }
                    _ => write!(writer.stderr(), "{}", TARGET_GET_NO_TARGET_MSG)?,
                }
            }
        }
    };
    Ok(())
}

///////////////////////////////////////////////////////////////////////////////
// tests

#[cfg(test)]
mod test {
    use super::*;
    use ffx_config::test_env;
    use ffx_target_default_args::*;
    use ffx_writer::{Format, TestBuffers};
    use fho::{FfxCommandLine, FhoEnvironment, TryFromEnv};
    use tempfile::tempdir;

    #[fuchsia::test]
    async fn test_get_env_unset() -> Result<()> {
        let env = test_env().build().unwrap();
        let test_buffers = TestBuffers::default();
        let mut writer = VerifiedMachineWriter::<TargetDefaultInfo>::new_test(None, &test_buffers);

        exec_target_default_impl(
            &env.context,
            TargetDefaultCommand { subcommand: SubCommand::Get(TargetDefaultGetCommand {}) },
            &mut writer,
        )
        .await
        .unwrap();

        let (stdout, stderr) = test_buffers.into_strings();
        assert_eq!(stdout, "");
        assert_eq!(stderr, TARGET_GET_NO_TARGET_MSG);
        Ok(())
    }

    #[fuchsia::test]
    async fn test_get_env_empty() -> Result<()> {
        let env = test_env()
            .env_var("FUCHSIA_NODENAME", "")
            .env_var("FUCHSIA_DEVICE_ADDR", "")
            .build()
            .unwrap();
        let test_buffers = TestBuffers::default();
        let mut writer = VerifiedMachineWriter::<TargetDefaultInfo>::new_test(None, &test_buffers);

        exec_target_default_impl(
            &env.context,
            TargetDefaultCommand { subcommand: SubCommand::Get(TargetDefaultGetCommand {}) },
            &mut writer,
        )
        .await
        .unwrap();

        let (stdout, stderr) = test_buffers.into_strings();
        assert_eq!(stdout, "");
        assert_eq!(stderr, TARGET_GET_NO_TARGET_MSG);
        Ok(())
    }

    #[fuchsia::test]
    async fn test_get_no_env() -> Result<()> {
        let test_build_dir = tempdir().expect("output directory");
        let env = test_env()
            .runtime_config(TARGET_DEFAULT_KEY, "distraction-target1")
            .in_tree(&test_build_dir.path())
            .user_config(TARGET_DEFAULT_KEY, "distraction-target2")
            .build_config(TARGET_DEFAULT_KEY, "distraction-target3")
            .global_config(TARGET_DEFAULT_KEY, "distraction-target4")
            .build()
            .unwrap();
        let test_buffers = TestBuffers::default();
        let mut writer = VerifiedMachineWriter::<TargetDefaultInfo>::new_test(None, &test_buffers);

        exec_target_default_impl(
            &env.context,
            TargetDefaultCommand { subcommand: SubCommand::Get(TargetDefaultGetCommand {}) },
            &mut writer,
        )
        .await
        .unwrap();

        let (stdout, stderr) = test_buffers.into_strings();
        assert_eq!(stdout, "");
        assert_eq!(stderr, TARGET_GET_NO_TARGET_MSG);
        Ok(())
    }

    #[fuchsia::test]
    async fn test_get_all() -> Result<()> {
        let test_build_dir = tempdir().expect("output directory");
        let env = test_env()
            .in_tree(&test_build_dir.path())
            .env_var("FUCHSIA_NODENAME", "stateless-nodename-target")
            .env_var("FUCHSIA_DEVICE_ADDR", "stateless-device-addr-target")
            .runtime_config(TARGET_DEFAULT_KEY, "distraction-target1")
            .user_config(TARGET_DEFAULT_KEY, "distraction-target2")
            .build_config(TARGET_DEFAULT_KEY, "distraction-target3")
            .global_config(TARGET_DEFAULT_KEY, "distraction-target4")
            .build()
            .unwrap();
        let test_buffers = TestBuffers::default();
        let mut writer = VerifiedMachineWriter::<TargetDefaultInfo>::new_test(None, &test_buffers);

        exec_target_default_impl(
            &env.context,
            TargetDefaultCommand { subcommand: SubCommand::Get(TargetDefaultGetCommand {}) },
            &mut writer,
        )
        .await
        .unwrap();

        let (stdout, stderr) = test_buffers.into_strings();
        assert_eq!(stdout, "stateless-device-addr-target\n");
        assert_eq!(stderr, "");
        Ok(())
    }

    #[fuchsia::test]
    async fn test_get_machine() -> Result<()> {
        let test_build_dir = tempdir().expect("output directory");
        let env = test_env()
            .in_tree(&test_build_dir.path())
            .env_var("FUCHSIA_NODENAME", "stateless-nodename-target")
            .env_var("FUCHSIA_DEVICE_ADDR", "stateless-device-addr-target")
            .build()
            .unwrap();
        let test_buffers = TestBuffers::default();
        let mut writer =
            VerifiedMachineWriter::<TargetDefaultInfo>::new_test(Some(Format::Json), &test_buffers);

        exec_target_default_impl(
            &env.context,
            TargetDefaultCommand { subcommand: SubCommand::Get(TargetDefaultGetCommand {}) },
            &mut writer,
        )
        .await
        .unwrap();

        let (stdout, stderr) = test_buffers.into_strings();
        assert_eq!(stdout, "{\"target\":\"stateless-device-addr-target\"}\n");
        assert_eq!(stderr, "");
        Ok(())
    }

    #[fuchsia::test]
    async fn test_target_default_with_machine_raw() {
        let config_env = ffx_config::test_env().build().unwrap();
        let ffx =
            FfxCommandLine::new(None, &["ffx", "--machine", "raw", "target", "default", "get"])
                .unwrap();
        let env = FhoEnvironment::new(&config_env.context, &ffx);

        let result = VerifiedMachineWriter::<TargetDefaultInfo>::try_from_env(&env).await;
        assert!(result.is_ok(), "VerifiedMachineWriter should support --machine raw");
    }

    #[fuchsia::test]
    async fn test_target_default_with_machine_json_succeeds() {
        let config_env = ffx_config::test_env().build().unwrap();
        let ffx =
            FfxCommandLine::new(None, &["ffx", "--machine", "json", "target", "default", "get"])
                .unwrap();
        let env = FhoEnvironment::new(&config_env.context, &ffx);

        let result = VerifiedMachineWriter::<TargetDefaultInfo>::try_from_env(&env).await;
        assert!(result.is_ok(), "VerifiedMachineWriter should support --machine json");
    }

    #[fuchsia::test]
    async fn test_get_escapes_ansi_sequences() -> Result<()> {
        let test_build_dir = tempdir().expect("output directory");
        let env = test_env()
            .in_tree(&test_build_dir.path())
            .env_var("FUCHSIA_NODENAME", "evil\x1b[31m_target\x1b[0m")
            .build()
            .unwrap();
        let test_buffers = TestBuffers::default();
        let mut writer = VerifiedMachineWriter::<TargetDefaultInfo>::new_test(None, &test_buffers);

        exec_target_default_impl(
            &env.context,
            TargetDefaultCommand { subcommand: SubCommand::Get(TargetDefaultGetCommand {}) },
            &mut writer,
        )
        .await
        .unwrap();

        let (stdout, stderr) = test_buffers.into_strings();
        assert_eq!(stdout, "evil\\u{1b}[31m_target\\u{1b}[0m\n");
        assert_eq!(stderr, "");
        Ok(())
    }
}
