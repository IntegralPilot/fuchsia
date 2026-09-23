// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use emulator_instance::{EmulatorInstanceInfo, EmulatorInstances};
use ffx_config::EnvironmentContext;
use ffx_config::keys::EMU_INSTANCE_ROOT_DIR;
use ffx_emulator_engines::EngineBuilder;
use ffx_emulator_stop_args::StopCommand;
use ffx_emulator_stop_command_output::CommandStatus;
use ffx_writer::{ToolIO as _, VerifiedMachineWriter};
use fho::{Error, FfxMain, FfxTool, bug, return_user_error, user_error};
use std::io::Write;
use std::path::PathBuf;

/// Sub-sub tool for `emu stop`
#[derive(FfxTool)]
#[target(None)]
pub struct EmuStopTool {
    #[command]
    cmd: StopCommand,
    context: EnvironmentContext,
}

// Since this is a part of a legacy plugin, add
// the legacy entry points. If and when this
// is migrated to a subcommand, this macro can be
// removed.
fho::embedded_plugin!(EmuStopTool);

#[async_trait::async_trait(?Send)]
impl FfxMain for EmuStopTool {
    type Writer = VerifiedMachineWriter<CommandStatus>;

    type Error = ::fho::Error;

    async fn main(self, mut writer: <Self as fho::FfxMain>::Writer) -> fho::Result<()> {
        match self.stop_impl(&mut writer).await {
            Ok(errors) => writer
                .machine(&CommandStatus::Ok {
                    messages: errors.into_iter().map(|e| e.to_string()).collect(),
                })
                .map_err(|e| bug!(e)),
            Err(Error::User(e)) => {
                writer.machine(&CommandStatus::UserError { message: e.to_string() })?;
                Err(Error::User(e))
            }
            Err(e) => {
                writer.machine(&CommandStatus::UnexpectedError { message: e.to_string() })?;
                Err(e)
            }
        }
    }
}

impl EmuStopTool {
    async fn stop_impl(
        self,
        writer: &mut <EmuStopTool as fho::FfxMain>::Writer,
    ) -> fho::Result<Vec<Error>> {
        let mut names = vec![self.cmd.name];
        let instance_dir: PathBuf =
            self.context.get(EMU_INSTANCE_ROOT_DIR).map_err(|e| bug!("{e}"))?;
        let emu_instances = EmulatorInstances::new(instance_dir);

        if self.cmd.all {
            names = match emu_instances.get_all_instances() {
                Ok(list) => list.into_iter().map(|v| Some(v.get_name().to_string())).collect(),
                Err(e) => {
                    return_user_error!("Error encountered loading emulator instances: {e}");
                }
            };
        }
        let mut errors: Vec<Error> = vec![];
        for mut some_name in names {
            let builder = EngineBuilder::new(&self.context, emu_instances.clone());
            let engine = builder.get_engine_by_name(&mut some_name);
            if engine.is_err() && some_name.is_none() {
                // This happens when the program doesn't know which instance to use. The
                // get_engine_by_name returns a good error message, and the loop should terminate
                // early.
                return_user_error!(engine.err().unwrap());
            }
            let name = some_name.unwrap_or_else(|| "<unspecified>".to_string());
            match engine {
                Err(e) => {
                    let message = format!(
                        "Couldn't deserialize engine '{name}' from disk. Continuing stop, \
                    but you may need to terminate the emulator process manually: {e:?}"
                    );
                    write!(writer.stderr(), "{message}").map_err(|e| bug!(e))?;
                    if writer.is_machine() {
                        errors.push(user_error!(message))
                    }
                }
                Ok(None) => {
                    return_user_error!("{name} does not exist.");
                }
                Ok(Some(mut engine)) => {
                    if !writer.is_machine() {
                        writeln!(writer, "Stopping emulator '{name}'...").map_err(|e| bug!(e))?;
                    }
                    if let Err(e) = engine.stop().await {
                        let err = bug!("Failed with the following error: {e}");
                        writeln!(writer.stderr(), "{err}").map_err(|e| bug!(e))?;
                        if writer.is_machine() {
                            errors.push(err);
                        }
                    }
                }
            }
            if !self.cmd.persist {
                let cleanup = emu_instances.clean_up_instance_dir(&name);
                if cleanup.is_err() {
                    let message = format!(
                        "Cleanup of '{}' failed with the following error: {:?}",
                        name,
                        cleanup.unwrap_err()
                    );

                    writeln!(writer.stderr(), "{message}").map_err(|e| bug!(e))?;
                    if writer.is_machine() {
                        errors.push(bug!(message));
                    }
                }
            }
        }
        Ok(errors)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use emulator_instance::{EmulatorInstanceData, EngineState, write_to_disk};

    use tempfile::tempdir;

    #[fuchsia::test]
    async fn test_stop_existing() {
        let temp_path = PathBuf::from(tempdir().unwrap().path());
        let env = ffx_config::test_env()
            .user_config(EMU_INSTANCE_ROOT_DIR, temp_path.to_str().unwrap())
            .build()
            .unwrap();

        let emu_instances = EmulatorInstances::new(temp_path.clone());
        let the_name = "one_instance".to_string();
        let cmd = StopCommand { name: Some(the_name.clone()), ..Default::default() };
        let data = EmulatorInstanceData::new_with_state(&the_name, EngineState::Running);
        let instance_dir = emu_instances.get_instance_dir(&the_name, true).unwrap();
        write_to_disk(&data, &instance_dir).unwrap();

        let tool = EmuStopTool { cmd, context: env.context.clone() };
        tool.main(<EmuStopTool as fho::FfxMain>::Writer::new(None))
            .await
            .expect("unexpected error");
    }

    #[fuchsia::test]
    async fn test_stop_unknown() {
        let env = ffx_config::test_init().unwrap();
        let cmd = StopCommand { name: Some("unknown_instance".to_string()), ..Default::default() };
        let tool = EmuStopTool { cmd, context: env.context.clone() };
        let expected_phrase = "unknown_instance does not exist";
        let err = tool
            .main(<EmuStopTool as fho::FfxMain>::Writer::new(None))
            .await
            .expect_err("expected error");
        assert!(err.to_string().contains(expected_phrase), "expected '{expected_phrase}' in {err}");
    }

    #[fuchsia::test]
    async fn test_stop_multiple_running_error() {
        let temp_path = PathBuf::from(tempdir().unwrap().path());
        let env = ffx_config::test_env()
            .user_config(EMU_INSTANCE_ROOT_DIR, temp_path.to_str().unwrap())
            .build()
            .unwrap();
        let emu_instances = EmulatorInstances::new(temp_path.clone());
        let cmd = StopCommand::default();
        let data = EmulatorInstanceData::new_with_state("one_instance", EngineState::Staged);
        let instance_dir = emu_instances.get_instance_dir("one_instance", true).unwrap();
        write_to_disk(&data, &instance_dir).unwrap();
        let data2 = EmulatorInstanceData::new_with_state("two_instance", EngineState::Staged);
        let instance_dir2 = emu_instances.get_instance_dir("two_instance", true).unwrap();
        write_to_disk(&data2, &instance_dir2).unwrap();

        let tool = EmuStopTool { cmd, context: env.context.clone() };
        let expected_phrase = "Multiple emulators found but none are running";
        let err = tool
            .main(<EmuStopTool as fho::FfxMain>::Writer::new(None))
            .await
            .expect_err("expected error");
        assert!(err.to_string().contains(expected_phrase), "expected '{expected_phrase}' in {err}");
    }

    #[fuchsia::test]
    async fn test_stop_prefer_running() {
        let temp_path = PathBuf::from(tempdir().unwrap().path());
        let env = ffx_config::test_env()
            .user_config(EMU_INSTANCE_ROOT_DIR, temp_path.to_str().unwrap())
            .build()
            .unwrap();
        let emu_instances = EmulatorInstances::new(temp_path.clone());
        let cmd = StopCommand::default();

        // Spawn a test process to get a valid running PID
        let mut test_child = std::process::Command::new("sleep")
            .arg("60")
            .spawn()
            .expect("failed to spawn test process");
        let test_pid = test_child.id();

        // Instance 1 is running
        let mut data = EmulatorInstanceData::new_with_state("one_instance", EngineState::Running);
        data.set_pid(test_pid);
        let instance_dir = emu_instances.get_instance_dir("one_instance", true).unwrap();
        write_to_disk(&data, &instance_dir).unwrap();

        // Instance 2 is stopped (Staged)
        let data2 = EmulatorInstanceData::new_with_state("two_instance", EngineState::Staged);
        let instance_dir2 = emu_instances.get_instance_dir("two_instance", true).unwrap();
        write_to_disk(&data2, &instance_dir2).unwrap();

        let tool = EmuStopTool { cmd, context: env.context.clone() };

        // Should succeed because it selects the running one.
        tool.main(<EmuStopTool as fho::FfxMain>::Writer::new(None))
            .await
            .expect("unexpected error");

        // Clean up the test process if it's still alive
        let _ = test_child.kill();
    }

    #[fuchsia::test]
    async fn test_stop_multiple_running_auto_fail() {
        let temp_path = PathBuf::from(tempdir().unwrap().path());
        let env = ffx_config::test_env()
            .user_config(EMU_INSTANCE_ROOT_DIR, temp_path.to_str().unwrap())
            .build()
            .unwrap();
        let emu_instances = EmulatorInstances::new(temp_path.clone());
        let cmd = StopCommand::default();

        // Spawn a test process to get a valid running PID
        let mut test_child = std::process::Command::new("sleep")
            .arg("60")
            .spawn()
            .expect("failed to spawn test process");
        let test_pid = test_child.id();

        // Instance 1 is running
        let mut data = EmulatorInstanceData::new_with_state("one_instance", EngineState::Running);
        data.set_pid(test_pid);
        let instance_dir = emu_instances.get_instance_dir("one_instance", true).unwrap();
        write_to_disk(&data, &instance_dir).unwrap();

        // Instance 2 is also running (sharing same PID for test simplicity)
        let mut data2 = EmulatorInstanceData::new_with_state("two_instance", EngineState::Running);
        data2.set_pid(test_pid);
        let instance_dir2 = emu_instances.get_instance_dir("two_instance", true).unwrap();
        write_to_disk(&data2, &instance_dir2).unwrap();

        let tool = EmuStopTool { cmd, context: env.context.clone() };

        // Should fail because multiple are running.
        let expected_phrase = "Multiple running emulators found";
        let err = tool
            .main(<EmuStopTool as fho::FfxMain>::Writer::new(None))
            .await
            .expect_err("expected error");
        assert!(err.to_string().contains(expected_phrase), "expected '{expected_phrase}' in {err}");

        let _ = test_child.kill();
    }

    #[fuchsia::test]
    async fn test_stop_only_stopped_auto_succeed() {
        let temp_path = PathBuf::from(tempdir().unwrap().path());
        let env = ffx_config::test_env()
            .user_config(EMU_INSTANCE_ROOT_DIR, temp_path.to_str().unwrap())
            .build()
            .unwrap();
        let emu_instances = EmulatorInstances::new(temp_path.clone());
        let cmd = StopCommand::default();

        // Only one instance, and it is stopped (Staged)
        let data = EmulatorInstanceData::new_with_state("one_instance", EngineState::Staged);
        let instance_dir = emu_instances.get_instance_dir("one_instance", true).unwrap();
        write_to_disk(&data, &instance_dir).unwrap();

        let tool = EmuStopTool { cmd, context: env.context.clone() };

        // Should succeed because it selects the only instance even though it's stopped.
        tool.main(<EmuStopTool as fho::FfxMain>::Writer::new(None))
            .await
            .expect("unexpected error");
    }

    #[fuchsia::test]
    async fn test_stop_multiple_running() {
        let temp_path = PathBuf::from(tempdir().unwrap().path());
        let env = ffx_config::test_env()
            .user_config(EMU_INSTANCE_ROOT_DIR, temp_path.to_str().unwrap())
            .build()
            .unwrap();
        let emu_instances = EmulatorInstances::new(temp_path.clone());
        let cmd = StopCommand { all: true, ..Default::default() };
        let data = EmulatorInstanceData::new_with_state("one_instance", EngineState::Staged);
        let instance_dir = emu_instances.get_instance_dir("one_instance", true).unwrap();
        write_to_disk(&data, &instance_dir).unwrap();
        let data2 = EmulatorInstanceData::new_with_state("two_instance", EngineState::Staged);
        let instance_dir2 = emu_instances.get_instance_dir("two_instance", true).unwrap();
        write_to_disk(&data2, &instance_dir2).unwrap();

        let tool = EmuStopTool { cmd, context: env.context.clone() };
        tool.main(<EmuStopTool as fho::FfxMain>::Writer::new(None))
            .await
            .expect("unexpected error");
    }

    #[fuchsia::test]
    async fn test_stop_not_running() {
        let temp_dir = tempfile::tempdir().unwrap();
        let env = ffx_config::test_env()
            .user_config(EMU_INSTANCE_ROOT_DIR, temp_dir.path().to_string_lossy())
            .build()
            .unwrap();
        let emu_instances = EmulatorInstances::new(temp_dir.path().to_path_buf());
        let mut cmd = StopCommand::default();
        let data = EmulatorInstanceData::new_with_state("one_instance", EngineState::Staged);
        let instance_dir = emu_instances.get_instance_dir("one_instance", true).unwrap();
        write_to_disk(&data, &instance_dir).unwrap();
        cmd.name = Some("one_instance".to_string());

        let tool = EmuStopTool { cmd, context: env.context.clone() };
        tool.main(<EmuStopTool as fho::FfxMain>::Writer::new(None))
            .await
            .expect("unexpected error");
    }
}
