// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use ffx_config::EnvironmentContext;
use ffx_repository_default_args::{RepositoryDefaultCommand, SubCommand};
use ffx_writer::{ToolIO as _, VerifiedMachineWriter};
use fho::{FfxMain, FfxTool, Result, bug};
use schemars::JsonSchema;
use serde::Serialize;
use std::io::Write as _;

pub(crate) const CONFIG_KEY_DEFAULT: &str = "repository.default";

#[derive(Serialize, JsonSchema)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum RepoDefaultOutput {
    Get(DefaultRepoInfo),
    Ok,
}

#[derive(Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DefaultRepoInfo {
    pub name: Option<String>,
}

#[derive(FfxTool)]
pub struct RepoDefaultTool {
    #[command]
    pub cmd: RepositoryDefaultCommand,
    context: EnvironmentContext,
}

fho::embedded_plugin!(RepoDefaultTool);

#[async_trait::async_trait(?Send)]
impl FfxMain for RepoDefaultTool {
    type Writer = VerifiedMachineWriter<RepoDefaultOutput>;

    type Error = ::fho::Error;

    async fn main(self, mut writer: Self::Writer) -> Result<()> {
        exec_repository_default_impl(&self.context, self.cmd, &mut writer).await
    }
}

pub async fn exec_repository_default_impl(
    context: &EnvironmentContext,
    cmd: RepositoryDefaultCommand,
    writer: &mut VerifiedMachineWriter<RepoDefaultOutput>,
) -> Result<()> {
    match &cmd.subcommand {
        SubCommand::Get(_) => {
            let default_repo: Option<String> =
                context.get::<String, _>(CONFIG_KEY_DEFAULT).ok().filter(|s| !s.is_empty());
            if writer.is_machine() {
                writer
                    .machine(&RepoDefaultOutput::Get(DefaultRepoInfo { name: default_repo }))
                    .map_err(|e| bug!(e))?;
            } else {
                writeln!(writer, "{}", default_repo.unwrap_or_default()).map_err(|e| bug!(e))?;
            }
        }
        SubCommand::Set(set) => {
            let env = context.load().map_err(|e| bug!(e))?;
            let mut config = ffx_config::Config::from_env(&env).map_err(|e| bug!(e))?;
            config
                .set(CONFIG_KEY_DEFAULT, set.level, serde_json::Value::String(set.name.clone()))
                .map_err(|e| bug!(e))?;
            config.save().map_err(|e| bug!(e))?;
            if writer.is_machine() {
                writer.machine(&RepoDefaultOutput::Ok).map_err(|e| bug!(e))?;
            }
        }
        SubCommand::Unset(unset) => {
            let env = context.load().map_err(|e| bug!(e))?;
            let mut config = ffx_config::Config::from_env(&env).map_err(|e| bug!(e))?;
            let res = (|| {
                config.remove(CONFIG_KEY_DEFAULT, unset.level)?;
                config.save()
            })();
            if let Err(e) = res {
                let _ = writeln!(writer.stderr(), "warning: {}", e);
            }
            if writer.is_machine() {
                writer.machine(&RepoDefaultOutput::Ok).map_err(|e| bug!(e))?;
            }
        }
    };
    Ok(())
}

#[cfg(test)]
mod test {
    use super::*;
    use ffx_config::ConfigLevel;
    use ffx_repository_default_args::{
        RepositoryDefaultGetCommand, RepositoryDefaultSetCommand, RepositoryDefaultUnsetCommand,
    };
    use ffx_writer::TestBuffers;

    #[fuchsia::test]
    async fn test_get_empty_text() {
        let env = ffx_config::test_env().build().unwrap();
        let cmd = RepositoryDefaultCommand {
            subcommand: SubCommand::Get(RepositoryDefaultGetCommand {}),
        };
        let buffers = TestBuffers::default();
        let mut writer = VerifiedMachineWriter::new_test(None, &buffers);
        let res = exec_repository_default_impl(&env.context, cmd, &mut writer).await;
        assert!(res.is_ok());
        assert_eq!(buffers.into_stdout_str(), "\n");
    }

    #[fuchsia::test]
    async fn test_get_empty_machine() {
        let env = ffx_config::test_env().build().unwrap();
        let cmd = RepositoryDefaultCommand {
            subcommand: SubCommand::Get(RepositoryDefaultGetCommand {}),
        };
        let buffers = TestBuffers::default();
        let mut writer = VerifiedMachineWriter::new_test(Some(ffx_writer::Format::Json), &buffers);
        let res = exec_repository_default_impl(&env.context, cmd, &mut writer).await;
        assert!(res.is_ok());
        assert_eq!(buffers.into_stdout_str(), "{\"type\":\"get\",\"data\":{\"name\":null}}\n");
    }

    #[fuchsia::test]
    async fn test_get_set_value_text() {
        let env =
            ffx_config::test_env().user_config(CONFIG_KEY_DEFAULT, "my-repo").build().unwrap();
        let cmd = RepositoryDefaultCommand {
            subcommand: SubCommand::Get(RepositoryDefaultGetCommand {}),
        };
        let buffers = TestBuffers::default();
        let mut writer = VerifiedMachineWriter::new_test(None, &buffers);
        let res = exec_repository_default_impl(&env.context, cmd, &mut writer).await;
        assert!(res.is_ok());
        assert_eq!(buffers.into_stdout_str(), "my-repo\n");
    }

    #[fuchsia::test]
    async fn test_get_set_value_machine() {
        let env =
            ffx_config::test_env().user_config(CONFIG_KEY_DEFAULT, "my-repo").build().unwrap();
        let cmd = RepositoryDefaultCommand {
            subcommand: SubCommand::Get(RepositoryDefaultGetCommand {}),
        };
        let buffers = TestBuffers::default();
        let mut writer = VerifiedMachineWriter::new_test(Some(ffx_writer::Format::Json), &buffers);
        let res = exec_repository_default_impl(&env.context, cmd, &mut writer).await;
        assert!(res.is_ok());
        assert_eq!(
            buffers.into_stdout_str(),
            "{\"type\":\"get\",\"data\":{\"name\":\"my-repo\"}}\n"
        );
    }

    #[fuchsia::test]
    async fn test_set_text() {
        let mut env = ffx_config::test_env().build().unwrap();
        let cmd = RepositoryDefaultCommand {
            subcommand: SubCommand::Set(RepositoryDefaultSetCommand {
                name: "my-repo".to_string(),
                level: ConfigLevel::User,
                build_dir: None,
            }),
        };
        let buffers = TestBuffers::default();
        let mut writer = VerifiedMachineWriter::new_test(None, &buffers);
        let res = exec_repository_default_impl(&env.context, cmd, &mut writer).await;
        assert!(res.is_ok());
        assert_eq!(buffers.into_stdout_str(), "");
        env.reload_context().unwrap();
        let val: String = env.context.get(CONFIG_KEY_DEFAULT).unwrap();
        assert_eq!(val, "my-repo");
    }

    #[fuchsia::test]
    async fn test_set_machine() {
        let mut env = ffx_config::test_env().build().unwrap();
        let cmd = RepositoryDefaultCommand {
            subcommand: SubCommand::Set(RepositoryDefaultSetCommand {
                name: "my-repo".to_string(),
                level: ConfigLevel::User,
                build_dir: None,
            }),
        };
        let buffers = TestBuffers::default();
        let mut writer = VerifiedMachineWriter::new_test(Some(ffx_writer::Format::Json), &buffers);
        let res = exec_repository_default_impl(&env.context, cmd, &mut writer).await;
        assert!(res.is_ok());
        assert_eq!(buffers.into_stdout_str(), "{\"type\":\"ok\"}\n");
        env.reload_context().unwrap();
        let val: String = env.context.get(CONFIG_KEY_DEFAULT).unwrap();
        assert_eq!(val, "my-repo");
    }

    #[fuchsia::test]
    async fn test_unset_text() {
        let mut env =
            ffx_config::test_env().user_config(CONFIG_KEY_DEFAULT, "my-repo").build().unwrap();
        let cmd = RepositoryDefaultCommand {
            subcommand: SubCommand::Unset(RepositoryDefaultUnsetCommand {
                level: ConfigLevel::User,
                build_dir: None,
            }),
        };
        let buffers = TestBuffers::default();
        let mut writer = VerifiedMachineWriter::new_test(None, &buffers);
        let res = exec_repository_default_impl(&env.context, cmd, &mut writer).await;
        assert!(res.is_ok());
        assert_eq!(buffers.into_stdout_str(), "");
        env.reload_context().unwrap();
        let val: Option<String> = env.context.get(CONFIG_KEY_DEFAULT).ok();
        assert!(val.is_none() || val.unwrap().is_empty());
    }

    #[fuchsia::test]
    async fn test_unset_machine() {
        let mut env =
            ffx_config::test_env().user_config(CONFIG_KEY_DEFAULT, "my-repo").build().unwrap();
        let cmd = RepositoryDefaultCommand {
            subcommand: SubCommand::Unset(RepositoryDefaultUnsetCommand {
                level: ConfigLevel::User,
                build_dir: None,
            }),
        };
        let buffers = TestBuffers::default();
        let mut writer = VerifiedMachineWriter::new_test(Some(ffx_writer::Format::Json), &buffers);
        let res = exec_repository_default_impl(&env.context, cmd, &mut writer).await;
        assert!(res.is_ok());
        assert_eq!(buffers.into_stdout_str(), "{\"type\":\"ok\"}\n");
        env.reload_context().unwrap();
        let val: Option<String> = env.context.get(CONFIG_KEY_DEFAULT).ok();
        assert!(val.is_none() || val.unwrap().is_empty());
    }

    #[fuchsia::test]
    async fn test_unset_error_text() {
        let env = ffx_config::test_env().build().unwrap();
        let cmd = RepositoryDefaultCommand {
            subcommand: SubCommand::Unset(RepositoryDefaultUnsetCommand {
                level: ConfigLevel::User,
                build_dir: None,
            }),
        };
        let buffers = TestBuffers::default();
        let mut writer = VerifiedMachineWriter::new_test(None, &buffers);
        let res = exec_repository_default_impl(&env.context, cmd, &mut writer).await;
        assert!(res.is_ok());
        assert!(!buffers.into_stderr_str().is_empty());
    }

    #[fuchsia::test]
    async fn test_unset_error_machine() {
        let env = ffx_config::test_env().build().unwrap();
        let cmd = RepositoryDefaultCommand {
            subcommand: SubCommand::Unset(RepositoryDefaultUnsetCommand {
                level: ConfigLevel::User,
                build_dir: None,
            }),
        };
        let buffers = TestBuffers::default();
        let mut writer = VerifiedMachineWriter::new_test(Some(ffx_writer::Format::Json), &buffers);
        let res = exec_repository_default_impl(&env.context, cmd, &mut writer).await;
        assert!(res.is_ok());
        let (stdout, stderr) = buffers.into_strings();
        assert_eq!(stdout, "{\"type\":\"ok\"}\n");
        assert!(!stderr.is_empty());
    }
}
