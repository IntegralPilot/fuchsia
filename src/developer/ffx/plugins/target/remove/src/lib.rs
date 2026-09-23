// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_trait::async_trait;
use ffx_config::{ConfigError, EnvironmentContext};
use ffx_target_remove_args::RemoveCommand;
use ffx_writer::{ToolIO as _, VerifiedMachineWriter};
use fho::{FfxMain, FfxTool, Result, bug, return_bug, return_user_error};
use fidl_fuchsia_developer_ffx as ffx;
use manual_targets::{Config, ManualTargets, ManualTargetsError};
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
pub struct RemoveTool {
    #[command]
    cmd: RemoveCommand,
    context: EnvironmentContext,
}

fho::embedded_plugin!(RemoveTool);

#[async_trait(?Send)]
impl FfxMain for RemoveTool {
    type Writer = VerifiedMachineWriter<CommandStatus>;

    type Error = ::fho::Error;

    async fn main(self, mut writer: Self::Writer) -> fho::Result<()> {
        let res = Self::remove_direct_impl(&self.context, self.cmd, &mut writer).await;
        match res {
            Ok(message) => {
                if writer.is_machine() {
                    writer.machine(&CommandStatus::Ok { message: Some(message) })?;
                } else if !message.is_empty() {
                    writeln!(writer.stderr(), "{message}")
                        .map_err(|e| bug!("writing to stderr: {e}"))?;
                }
                Ok(())
            }
            Err(fho::Error::User(e)) => {
                writer.machine(&CommandStatus::UserError { message: e.to_string() })?;
                Err(fho::Error::User(e))
            }
            Err(e) => {
                writer.machine(&CommandStatus::UnexpectedError { message: e.to_string() })?;
                Err(e)
            }
        }
    }
}

impl RemoveTool {
    async fn remove_direct_impl(
        context: &EnvironmentContext,
        cmd: RemoveCommand,
        writer: &mut <Self as FfxMain>::Writer,
    ) -> Result<String> {
        let cfg = Config::new_from_context(context);
        if cmd.all {
            let list = match cfg.storage_get().await {
                Ok(v) => v,
                Err(ManualTargetsError::Config(ConfigError::NoValueSet(_))) => {
                    return Ok("No manual targets found.".into());
                }
                Err(e) => return_bug!(e),
            };
            if let Some(arr) = list.as_object() {
                for (k, _) in arr {
                    writeln!(writer.stderr(), "Removed {k}").map_err(|e| bug!(e))?;
                }
            }
            cfg.storage_set(serde_json::Value::Object(serde_json::Map::new()))
                .await
                .map_err(|e| bug!(e))?;
            Ok("".to_string())
        } else if let Some(name_or_addr) = cmd.name_or_addr {
            let mut targets = cfg.get_or_default().await;
            if targets.remove(&name_or_addr).is_some() {
                cfg.storage_set(targets.into()).await.map_err(|e| bug!(e))?;
                Ok("Removed.".to_string())
            } else {
                Ok("No matching target found.".to_string())
            }
        } else {
            return_user_error!("need to specify a target name or address or use the --all option")
        }
    }
    #[allow(dead_code)]
    async fn remove_impl(
        context: &EnvironmentContext,
        target_collection: ffx::TargetCollectionProxy,
        cmd: RemoveCommand,
        writer: &mut <Self as FfxMain>::Writer,
    ) -> Result<String> {
        if cmd.all {
            let cfg = Config::new_from_context(context);
            Self::remove_all_targets(writer, &target_collection, &cfg).await
        } else if let Some(name_or_addr) = cmd.name_or_addr {
            if target_collection
                .remove_target(&name_or_addr)
                .await
                .map_err(|e| bug!("Cannot remove target: {e}"))?
            {
                Ok("Removed.".to_string())
            } else {
                Ok("No matching target found.".to_string())
            }
        } else {
            return_user_error!("need to specify a target name or address or use the --all option")
        }
    }

    #[allow(dead_code)]
    async fn remove_all_targets(
        writer: &mut <Self as FfxMain>::Writer,
        target_collection: &ffx::TargetCollectionProxy,
        cfg: &Config,
    ) -> Result<String> {
        let list = match cfg.storage_get().await {
            Ok(v) => v,
            Err(ManualTargetsError::Config(ConfigError::NoValueSet(_))) => {
                return Ok("No manual targets found.".into());
            }
            Err(e) => return_bug!(e),
        };

        if let Some(arr) = list.as_object() {
            for (k, _) in arr {
                if target_collection
                    .remove_target(&k)
                    .await
                    .map_err(|e| bug!("Cannot remove target: {e}"))?
                {
                    writeln!(writer.stderr(), "Removed {k}").map_err(|e| bug!(e))?;
                } else {
                    // This most likely happens when the daemon is restarted when running
                    // this command and the manual target collection has not been loaded yet.
                    // It will work the second time.
                    writeln!(writer.stderr(),"No matching target for {k} found. {}",
                     "This is most likely because the daemon just started. Please run this command again.").map_err(|e| bug!(e))?;
                }
            }
        }
        Ok(String::from(""))
    }
}

////////////////////////////////////////////////////////////////////////////////
// tests

#[cfg(test)]
mod test {
    use super::*;

    use ffx_writer::{Format, TestBuffers};
    use serde_json::json;

    #[fuchsia::test]
    async fn test_remove_existing_target() {
        let env = ffx_config::test_init().expect("test_init");
        let mt = Config::new_from_context(&env.context);
        mt.storage_set(json!({"correct-horse-battery-staple": 0})).await.unwrap();
        let tool = RemoveTool {
            cmd: RemoveCommand {
                all: false,
                name_or_addr: Some("correct-horse-battery-staple".to_owned()),
            },
            context: env.context.clone(),
        };
        let test_buffers = TestBuffers::default();
        let writer = VerifiedMachineWriter::<CommandStatus>::new_test(None, &test_buffers);
        tool.main(writer).await.expect("run main");
        assert_eq!(test_buffers.into_stderr_str(), "Removed.\n");
    }

    #[fuchsia::test]
    async fn test_remove_nonexisting_target() {
        let env = ffx_config::test_init().expect("test_init");
        let tool = RemoveTool {
            cmd: RemoveCommand {
                all: false,
                name_or_addr: Some("incorrect-donkey-battery-jazz".to_owned()),
            },
            context: env.context.clone(),
        };
        let test_buffers = TestBuffers::default();
        let writer = VerifiedMachineWriter::<CommandStatus>::new_test(None, &test_buffers);

        tool.main(writer).await.expect("run main");
        assert_eq!(test_buffers.into_stderr_str(), "No matching target found.\n");
    }

    #[fuchsia::test]
    async fn test_remove_machine_nonexisting_target() {
        let env = ffx_config::test_init().expect("test_init");
        let tool = RemoveTool {
            cmd: RemoveCommand {
                all: false,
                name_or_addr: Some("incorrect-donkey-battery-jazz".to_owned()),
            },
            context: env.context.clone(),
        };
        let test_buffers = TestBuffers::default();
        let writer =
            VerifiedMachineWriter::<CommandStatus>::new_test(Some(Format::Json), &test_buffers);

        tool.main(writer).await.expect("run main");
        let (actual_stdout, actual_stderr) = test_buffers.into_strings();
        assert_eq!(actual_stderr, "");
        assert_eq!(actual_stdout, "{\"Ok\":{\"message\":\"No matching target found.\"}}\n");
    }

    #[fuchsia::test]
    async fn test_remove_all_targets_some() {
        let env = ffx_config::test_init().expect("test_init");
        let mt = Config::new_from_context(&env.context);
        mt.storage_set(json!({"127.0.0.1:8022": 0, "127.0.0.1:8023": 12345})).await.unwrap();
        let tool = RemoveTool {
            cmd: RemoveCommand { all: true, name_or_addr: None },
            context: env.context.clone(),
        };
        let test_buffers = TestBuffers::default();
        let writer =
            VerifiedMachineWriter::<CommandStatus>::new_test(Some(Format::Json), &test_buffers);

        tool.main(writer).await.expect("run main");
        let (actual_stdout, actual_stderr) = test_buffers.into_strings();
        assert_eq!(actual_stderr, "Removed 127.0.0.1:8022\nRemoved 127.0.0.1:8023\n");
        assert_eq!(actual_stdout, "{\"Ok\":{\"message\":\"\"}}\n");
    }

    #[fuchsia::test]
    async fn test_remove_all_targets_none() {
        let env = ffx_config::test_init().expect("test_init");

        let tool = RemoveTool {
            cmd: RemoveCommand { all: true, name_or_addr: None },
            context: env.context.clone(),
        };
        let test_buffers = TestBuffers::default();
        let writer =
            VerifiedMachineWriter::<CommandStatus>::new_test(Some(Format::Json), &test_buffers);

        tool.main(writer).await.expect("run main");
        let (actual_stdout, actual_stderr) = test_buffers.into_strings();
        assert_eq!(actual_stderr, "");
        assert_eq!(actual_stdout, "{\"Ok\":{\"message\":\"No manual targets found.\"}}\n");
    }

    #[fuchsia::test]
    async fn test_remove_in_direct_mode() {
        let env = ffx_config::test_env()
            .runtime_config(ffx_config::keys::DIRECT_CONNECTIONS, true)
            .build()
            .expect("test_env build");
        let mt = Config::new_from_context(&env.context);
        mt.storage_set(json!({"127.0.0.1:8022": 0, "127.0.0.1:8023": 12345})).await.unwrap();

        let tool = RemoveTool {
            cmd: RemoveCommand { all: false, name_or_addr: Some("127.0.0.1:8022".to_owned()) },
            context: env.context.clone(),
        };
        let test_buffers = TestBuffers::default();
        let writer = VerifiedMachineWriter::<CommandStatus>::new_test(None, &test_buffers);
        tool.main(writer).await.expect("run main");
        assert_eq!(test_buffers.into_stderr_str(), "Removed.\n");

        let value = mt.get().await.unwrap();
        let targets = value.as_object().unwrap();
        assert!(!targets.contains_key("127.0.0.1:8022"));
        assert!(targets.contains_key("127.0.0.1:8023"));
    }

    #[fuchsia::test]
    async fn test_remove_all_in_direct_mode() {
        let env = ffx_config::test_env()
            .runtime_config(ffx_config::keys::DIRECT_CONNECTIONS, true)
            .build()
            .expect("test_env build");
        let mt = Config::new_from_context(&env.context);
        mt.storage_set(json!({"127.0.0.1:8022": 0, "127.0.0.1:8023": 12345})).await.unwrap();

        let tool = RemoveTool {
            cmd: RemoveCommand { all: true, name_or_addr: None },
            context: env.context.clone(),
        };
        let test_buffers = TestBuffers::default();
        let writer = VerifiedMachineWriter::<CommandStatus>::new_test(None, &test_buffers);
        tool.main(writer).await.expect("run main");
        assert_eq!(
            test_buffers.into_stderr_str(),
            "Removed 127.0.0.1:8022\nRemoved 127.0.0.1:8023\n"
        );

        let targets = mt.get_or_default().await;
        assert!(targets.is_empty());
    }
}
