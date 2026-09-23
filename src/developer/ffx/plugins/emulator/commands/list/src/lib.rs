// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_trait::async_trait;
use emulator_instance::{
    EmulatorInstanceData, EmulatorInstanceInfo, EmulatorInstances, EngineState,
};
use ffx_emulator_list_args::ListCommand;
use ffx_emulator_list_command_output::EmuListItem;
use ffx_writer::{ToolIO as _, VerifiedMachineWriter};
use fho::{FfxContext, FfxMain, FfxTool, TryFromEnv, TryFromEnvWith, bug};
use std::marker::PhantomData;
use std::path::PathBuf;

// This message is used when the JSON instance file
// cannot be parsed.
const BROKEN_MESSAGE: &str = r#"
One or more emulators are in a 'Broken' state. This is an uncommon state.
Run `ffx emu stop --all` to stop all running emulators and clean up broken instances.
"#;

#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub trait Instances: TryFromEnv + 'static {
    async fn get_all_instances(&self) -> Result<Vec<EmulatorInstanceData>, fho::Error>;
}

pub struct InstanceData {
    emu_instances: EmulatorInstances,
}

#[derive(Debug, Clone, Default)]
pub struct WithInstances<P: Instances>(PhantomData<P>);

#[async_trait(?Send)]
impl TryFromEnv for InstanceData {
    type Error = fho::Error;
    async fn try_from_env(env: &fho::FhoEnvironment) -> std::result::Result<Self, Self::Error> {
        let instance_root: PathBuf = env
            .environment_context()
            .get(ffx_config::keys::EMU_INSTANCE_ROOT_DIR)
            .map_err(|e| bug!("{e}"))?;
        Ok(InstanceData { emu_instances: EmulatorInstances::new(instance_root) })
    }
}

#[async_trait]
impl Instances for InstanceData {
    async fn get_all_instances(&self) -> Result<Vec<EmulatorInstanceData>, fho::Error> {
        self.emu_instances.get_all_instances().map_err(|e| anyhow::Error::from(e).into())
    }
}

#[async_trait(?Send)]
impl<T: Instances> TryFromEnvWith for WithInstances<T> {
    type Output = T;
    type Error = fho::Error;
    async fn try_from_env_with(
        self,
        _env: &fho::FhoEnvironment,
    ) -> std::result::Result<T, Self::Error> {
        Ok(T::try_from_env(_env)
            .await
            .map_err(|e| fho::Error::Unexpected(anyhow::Error::new(e)))?)
    }
}

/// Sub-sub tool for `emu list`
#[derive(FfxTool)]
#[target(None)]
pub struct EmuListTool<T: Instances> {
    #[command]
    cmd: ListCommand,
    instances: T,
}

// Since this is a part of a legacy plugin, add
// the legacy entry points. If and when this
// is migrated to a subcommand, this macro can be
// removed.
fho::embedded_plugin!(EmuListTool<InstanceData>);

#[async_trait(?Send)]
impl<T: Instances> FfxMain for EmuListTool<T> {
    type Writer = VerifiedMachineWriter<Vec<EmuListItem>>;

    type Error = ::fho::Error;

    async fn main(self, mut writer: VerifiedMachineWriter<Vec<EmuListItem>>) -> fho::Result<()> {
        let instance_list: Vec<EmulatorInstanceData> = self
            .instances
            .get_all_instances()
            .await
            .user_message("Error encountered looking up emulator instances")?;
        let items = instance_list
            .into_iter()
            .filter(|m| !(self.cmd.only_running && !m.is_running()))
            .map(|m| EmuListItem { name: m.get_name().into(), state: m.get_engine_state() })
            .collect::<Vec<_>>();
        if writer.is_machine() {
            writer.machine(&items)?;
        } else if !items.is_empty() {
            // If we use `writer.line()` on an empty list we get an unwanted '\n'. Hence the above
            // check. The same issue is why we can't use the `machine_or*` functions, as they
            // always print even if there's an empty string.
            let mut broken = false;
            let output = items
                .iter()
                .map(|instance| {
                    broken = broken || instance.state == EngineState::Error;
                    let engine_state_display_formatted = format!("[{}]", instance.state);
                    format!("{:16}{}", engine_state_display_formatted, instance.name)
                })
                .collect::<Vec<_>>()
                .join("\n");
            writer.line(output)?;
            if broken {
                writeln!(writer.stderr(), "{}", BROKEN_MESSAGE).bug()?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ffx_writer::{Format, TestBuffers};

    #[async_trait(?Send)]
    impl TryFromEnv for MockInstances {
        type Error = fho::Error;
        async fn try_from_env(
            _env: &fho::FhoEnvironment,
        ) -> std::result::Result<Self, Self::Error> {
            Ok(MockInstances::new())
        }
    }

    #[fuchsia::test]
    async fn test_empty_list() -> Result<(), fho::Error> {
        let _test_env = ffx_config::test_init().unwrap();
        let cmd = ListCommand { only_running: false };

        let mut tool = EmuListTool { cmd, instances: MockInstances::new() };
        // Text based writer
        let test_buffers = TestBuffers::default();
        let writer = VerifiedMachineWriter::<Vec<EmuListItem>>::new_test(None, &test_buffers);

        // Mock the return data
        let mock_return = || Ok(vec![]);

        let ctx = tool.instances.expect_get_all_instances();
        ctx.returning(mock_return);

        // Run both tools.
        tool.main(writer).await?;

        // Validate output
        let (stdout, stderr) = test_buffers.into_strings();
        assert_eq!(stdout, "");
        assert_eq!(stderr, "");

        Ok(())
    }

    #[fuchsia::test]
    async fn test_empty_list_machine() -> Result<(), fho::Error> {
        let _test_env = ffx_config::test_init().unwrap();
        let cmd = ListCommand { only_running: false };
        let mut tool = EmuListTool { cmd, instances: MockInstances::new() };

        // JSON based writer
        let machine_buffers = TestBuffers::default();
        let machine_writer = VerifiedMachineWriter::<Vec<EmuListItem>>::new_test(
            Some(Format::Json),
            &machine_buffers,
        );

        // Mock the return data
        let mock_return = || Ok(vec![]);

        let ctx_machine = tool.instances.expect_get_all_instances();
        ctx_machine.returning(mock_return);

        tool.main(machine_writer).await?;

        let (stdout, stderr) = machine_buffers.into_strings();
        assert_eq!(stdout, "[]\n");
        assert_eq!(stderr, "");
        let data = serde_json::from_str(&stdout).bug()?;
        assert!(
            matches!(data, serde_json::Value::Array(_)),
            "unexpected data. Expected array: {data:?}"
        );
        match VerifiedMachineWriter::<Vec<EmuListItem>>::verify_schema(&data) {
            Ok(_) => Ok(()),
            Err(e) => fho::return_bug!("Error verifying schema of {data:?}: {e}"),
        }
    }

    #[fuchsia::test]
    async fn test_new_list() -> Result<(), fho::Error> {
        let _test_env = ffx_config::test_init().unwrap();
        let cmd = ListCommand { only_running: false };
        let mut tool = EmuListTool { cmd, instances: MockInstances::new() };

        let test_buffers = TestBuffers::default();
        let writer = VerifiedMachineWriter::<Vec<EmuListItem>>::new_test(None, &test_buffers);

        let mock_return =
            || Ok(vec![EmulatorInstanceData::new_with_state("notrunning_emu", EngineState::New)]);

        let ctx = tool.instances.expect_get_all_instances();
        ctx.returning(mock_return);

        tool.main(writer).await?;

        let (stdout, stderr) = test_buffers.into_strings();
        let stdout_expected = "[new]           notrunning_emu\n";
        assert_eq!(stdout, stdout_expected);
        assert!(stderr.is_empty(), "stderr should be empty. Got: {:?}", stderr);

        Ok(())
    }

    #[fuchsia::test]
    async fn test_new_list_machine() -> Result<(), fho::Error> {
        let _test_env = ffx_config::test_init().unwrap();
        let cmd = ListCommand { only_running: false };
        let mut tool = EmuListTool { cmd, instances: MockInstances::new() };

        let machine_buffers = TestBuffers::default();
        let machine_writer = VerifiedMachineWriter::<Vec<EmuListItem>>::new_test(
            Some(Format::Json),
            &machine_buffers,
        );

        let mock_return =
            || Ok(vec![EmulatorInstanceData::new_with_state("notrunning_emu", EngineState::New)]);

        let ctx_machine = tool.instances.expect_get_all_instances();
        ctx_machine.returning(mock_return);

        tool.main(machine_writer).await?;

        let (stdout, stderr) = machine_buffers.into_strings();
        let stdout_expected = format!("[{}]\n", r#"{"name":"notrunning_emu","state":"new"}"#);
        assert_eq!(stdout, stdout_expected);
        assert!(stderr.is_empty(), "stderr should be empty. Got: {:?}", stderr);
        let data = serde_json::from_str(&stdout).bug()?;
        assert!(
            matches!(data, serde_json::Value::Array(_)),
            "unexpected data. Expected array: {data:?}"
        );
        match VerifiedMachineWriter::<Vec<EmuListItem>>::verify_schema(&data) {
            Ok(_) => Ok(()),
            Err(e) => fho::return_bug!("Error verifying schema of {data:?}: {e}"),
        }
    }

    #[fuchsia::test]
    async fn test_configured_list() -> Result<(), fho::Error> {
        let _test_env = ffx_config::test_init().unwrap();
        let cmd = ListCommand { only_running: false };
        let mut tool = EmuListTool { cmd, instances: MockInstances::new() };

        let test_buffers = TestBuffers::default();
        let writer = VerifiedMachineWriter::<Vec<EmuListItem>>::new_test(None, &test_buffers);

        let mock_return = || {
            Ok(vec![EmulatorInstanceData::new_with_state(
                "notrunning_config_emu",
                EngineState::Configured,
            )])
        };

        let ctx = tool.instances.expect_get_all_instances();
        ctx.returning(mock_return);

        tool.main(writer).await?;

        let (stdout, stderr) = test_buffers.into_strings();
        let stdout_expected = "[configured]    notrunning_config_emu\n";
        assert_eq!(stdout, stdout_expected);
        assert!(stderr.is_empty(), "stderr should be empty. Got: {:?}", stderr);

        Ok(())
    }

    #[fuchsia::test]
    async fn test_configured_list_machine() -> Result<(), fho::Error> {
        let _test_env = ffx_config::test_init().unwrap();
        let cmd = ListCommand { only_running: false };
        let mut tool = EmuListTool { cmd, instances: MockInstances::new() };

        let machine_buffers = TestBuffers::default();
        let machine_writer = VerifiedMachineWriter::<Vec<EmuListItem>>::new_test(
            Some(Format::Json),
            &machine_buffers,
        );

        let mock_return = || {
            Ok(vec![EmulatorInstanceData::new_with_state(
                "notrunning_config_emu",
                EngineState::Configured,
            )])
        };

        let machine_ctx = tool.instances.expect_get_all_instances();
        machine_ctx.returning(mock_return);

        tool.main(machine_writer).await?;

        let (stdout, stderr) = machine_buffers.into_strings();
        let stdout_expected =
            format!("[{}]\n", r#"{"name":"notrunning_config_emu","state":"configured"}"#);
        assert_eq!(stdout, stdout_expected);
        assert!(stderr.is_empty(), "stderr should be empty. Got: {:?}", stderr);
        let data = serde_json::from_str(&stdout).bug()?;
        assert!(
            matches!(data, serde_json::Value::Array(_)),
            "unexpected data. Expected array: {data:?}"
        );
        match VerifiedMachineWriter::<Vec<EmuListItem>>::verify_schema(&data) {
            Ok(_) => Ok(()),
            Err(e) => fho::return_bug!("Error verifying schema of {data:?}: {e}"),
        }
    }

    #[fuchsia::test]
    async fn test_staged_list() -> Result<(), fho::Error> {
        let _test_env = ffx_config::test_init().unwrap();
        let cmd = ListCommand { only_running: false };
        let mut tool = EmuListTool { cmd, instances: MockInstances::new() };

        let test_buffers = TestBuffers::default();
        let writer = VerifiedMachineWriter::<Vec<EmuListItem>>::new_test(None, &test_buffers);

        let mock_return = || {
            Ok(vec![EmulatorInstanceData::new_with_state("notrunning_emu", EngineState::Staged)])
        };

        let ctx = tool.instances.expect_get_all_instances();
        ctx.returning(mock_return);

        tool.main(writer).await?;

        let (stdout, stderr) = test_buffers.into_strings();
        let stdout_expected = "[staged]        notrunning_emu\n";
        assert_eq!(stdout, stdout_expected);
        assert!(stderr.is_empty(), "stderr should be empty. Got: {:?}", stderr);

        Ok(())
    }

    #[fuchsia::test]
    async fn test_staged_list_machine() -> Result<(), fho::Error> {
        let _test_env = ffx_config::test_init().unwrap();
        let cmd = ListCommand { only_running: false };
        let mut tool = EmuListTool { cmd, instances: MockInstances::new() };

        let machine_buffers = TestBuffers::default();
        let machine_writer = VerifiedMachineWriter::<Vec<EmuListItem>>::new_test(
            Some(Format::Json),
            &machine_buffers,
        );

        let mock_return = || {
            Ok(vec![EmulatorInstanceData::new_with_state("notrunning_emu", EngineState::Staged)])
        };

        let ctx_machine = tool.instances.expect_get_all_instances();
        ctx_machine.returning(mock_return);

        tool.main(machine_writer).await?;

        let (stdout, stderr) = machine_buffers.into_strings();
        let stdout_expected = format!("[{}]\n", r#"{"name":"notrunning_emu","state":"staged"}"#);
        assert_eq!(stdout, stdout_expected);
        assert!(stderr.is_empty(), "stderr should be empty. Got: {:?}", stderr);
        let data = serde_json::from_str(&stdout).bug()?;
        assert!(
            matches!(data, serde_json::Value::Array(_)),
            "unexpected data. Expected array: {data:?}"
        );
        match VerifiedMachineWriter::<Vec<EmuListItem>>::verify_schema(&data) {
            Ok(_) => Ok(()),
            Err(e) => fho::return_bug!("Error verifying schema of {data:?}: {e}"),
        }
    }

    #[fuchsia::test]
    async fn test_running_list() -> Result<(), fho::Error> {
        let _test_env = ffx_config::test_init().unwrap();
        let cmd = ListCommand { only_running: false };
        let mut tool = EmuListTool { cmd, instances: MockInstances::new() };

        let test_buffers = TestBuffers::default();
        let writer = VerifiedMachineWriter::<Vec<EmuListItem>>::new_test(None, &test_buffers);

        let mock_return = || {
            let mut running =
                EmulatorInstanceData::new_with_state("running_emu", EngineState::Running);
            running.set_pid(std::process::id());
            Ok(vec![running])
        };

        let ctx = tool.instances.expect_get_all_instances();
        ctx.returning(mock_return);

        tool.main(writer).await?;

        let (stdout, stderr) = test_buffers.into_strings();
        let stdout_expected = "[running]       running_emu\n";
        assert_eq!(stdout, stdout_expected);
        assert!(stderr.is_empty(), "stderr should be empty. Got: {:?}", stderr);

        Ok(())
    }

    #[fuchsia::test]
    async fn test_running_list_machine() -> Result<(), fho::Error> {
        let _test_env = ffx_config::test_init().unwrap();
        let cmd = ListCommand { only_running: false };
        let mut tool = EmuListTool { cmd, instances: MockInstances::new() };

        let machine_buffers = TestBuffers::default();
        let machine_writer = VerifiedMachineWriter::<Vec<EmuListItem>>::new_test(
            Some(Format::Json),
            &machine_buffers,
        );

        let mock_return = || {
            let mut running =
                EmulatorInstanceData::new_with_state("running_emu", EngineState::Running);
            running.set_pid(std::process::id());
            Ok(vec![running])
        };

        let ctx_machine = tool.instances.expect_get_all_instances();
        ctx_machine.returning(mock_return);

        tool.main(machine_writer).await?;

        let (stdout, stderr) = machine_buffers.into_strings();
        let stdout_expected = format!("[{}]\n", r#"{"name":"running_emu","state":"running"}"#);
        assert_eq!(stdout, stdout_expected);
        assert!(stderr.is_empty(), "stderr should be empty. Got: {:?}", stderr);
        let data = serde_json::from_str(&stdout).bug()?;
        assert!(
            matches!(data, serde_json::Value::Array(_)),
            "unexpected data. Expected array: {data:?}"
        );
        match VerifiedMachineWriter::<Vec<EmuListItem>>::verify_schema(&data) {
            Ok(_) => Ok(()),
            Err(e) => fho::return_bug!("Error verifying schema of {data:?}: {e}"),
        }
    }

    #[fuchsia::test]
    async fn test_running_flag() -> Result<(), fho::Error> {
        let _test_env = ffx_config::test_init().unwrap();
        let cmd = ListCommand { only_running: true };
        let mut tool = EmuListTool { cmd, instances: MockInstances::new() };

        let test_buffers = TestBuffers::default();
        let writer = VerifiedMachineWriter::<Vec<EmuListItem>>::new_test(None, &test_buffers);

        let mock_return = || {
            let mut running =
                EmulatorInstanceData::new_with_state("running_emu", EngineState::Running);
            running.set_pid(std::process::id());
            Ok(vec![
                EmulatorInstanceData::new_with_state("new_emu", EngineState::New),
                EmulatorInstanceData::new_with_state("config_emu", EngineState::Configured),
                EmulatorInstanceData::new_with_state("staged_emu", EngineState::Staged),
                running,
                EmulatorInstanceData::new_with_state("error_emu", EngineState::Error),
            ])
        };

        let ctx = tool.instances.expect_get_all_instances();
        ctx.returning(mock_return);

        tool.main(writer).await?;

        let (stdout, stderr) = test_buffers.into_strings();
        let stdout_expected = "[running]       running_emu\n";
        assert_eq!(stdout, stdout_expected);
        assert!(stderr.is_empty(), "stderr should be empty. Got: {:?}", stderr);
        Ok(())
    }

    #[fuchsia::test]
    async fn test_running_flag_machine() -> Result<(), fho::Error> {
        let _test_env = ffx_config::test_init().unwrap();
        let cmd = ListCommand { only_running: true };
        let mut tool = EmuListTool { cmd, instances: MockInstances::new() };

        let machine_buffers = TestBuffers::default();
        let machine_writer = VerifiedMachineWriter::<Vec<EmuListItem>>::new_test(
            Some(Format::Json),
            &machine_buffers,
        );

        let mock_return = || {
            let mut running =
                EmulatorInstanceData::new_with_state("running_emu", EngineState::Running);
            running.set_pid(std::process::id());
            Ok(vec![
                EmulatorInstanceData::new_with_state("new_emu", EngineState::New),
                EmulatorInstanceData::new_with_state("config_emu", EngineState::Configured),
                EmulatorInstanceData::new_with_state("staged_emu", EngineState::Staged),
                running,
                EmulatorInstanceData::new_with_state("error_emu", EngineState::Error),
            ])
        };

        let ctx_machine = tool.instances.expect_get_all_instances();
        ctx_machine.returning(mock_return);

        tool.main(machine_writer).await?;

        let (stdout, stderr) = machine_buffers.into_strings();
        let stdout_expected = format!("[{}]\n", r#"{"name":"running_emu","state":"running"}"#);
        assert_eq!(stdout, stdout_expected);
        assert!(stderr.is_empty(), "stderr should be empty. Got: {:?}", stderr);
        let data = serde_json::from_str(&stdout).bug()?;
        assert!(
            matches!(data, serde_json::Value::Array(_)),
            "unexpected data. Expected array: {data:?}"
        );
        match VerifiedMachineWriter::<Vec<EmuListItem>>::verify_schema(&data) {
            Ok(_) => Ok(()),
            Err(e) => fho::return_bug!("Error verifying schema of {data:?}: {e}"),
        }
    }

    #[fuchsia::test]
    async fn test_all_instances() -> Result<(), fho::Error> {
        let _test_env = ffx_config::test_init().unwrap();
        let cmd = ListCommand { only_running: false };
        let mut tool = EmuListTool { cmd, instances: MockInstances::new() };

        let test_buffers = TestBuffers::default();
        let writer = VerifiedMachineWriter::<Vec<EmuListItem>>::new_test(None, &test_buffers);

        let mock_return = || {
            let mut running =
                EmulatorInstanceData::new_with_state("running_emu", EngineState::Running);
            running.set_pid(std::process::id());
            Ok(vec![
                EmulatorInstanceData::new_with_state("new_emu", EngineState::New),
                EmulatorInstanceData::new_with_state("config_emu", EngineState::Configured),
                EmulatorInstanceData::new_with_state("staged_emu", EngineState::Staged),
                running,
                EmulatorInstanceData::new_with_state("error_emu", EngineState::Error),
            ])
        };

        let ctx = tool.instances.expect_get_all_instances();
        ctx.returning(mock_return);

        tool.main(writer).await?;

        let (stdout, stderr) = test_buffers.into_strings();
        let expected = "[new]           new_emu\n\
        [configured]    config_emu\n\
        [staged]        staged_emu\n\
        [running]       running_emu\n\
        [error]         error_emu\n";
        assert_eq!(stdout, expected);
        assert_eq!(
            stderr,
            format!("{BROKEN_MESSAGE}\n"),
            "Expected `BROKEN_MESSAGE` in stderr, got {stderr:?}"
        );
        Ok(())
    }

    #[fuchsia::test]
    async fn test_all_instances_machine() -> Result<(), fho::Error> {
        let _test_env = ffx_config::test_init().unwrap();
        let cmd = ListCommand { only_running: false };
        let mut tool = EmuListTool { cmd, instances: MockInstances::new() };

        let machine_buffers = TestBuffers::default();
        let machine_writer = VerifiedMachineWriter::<Vec<EmuListItem>>::new_test(
            Some(Format::Json),
            &machine_buffers,
        );

        let mock_return = || {
            let mut running =
                EmulatorInstanceData::new_with_state("running_emu", EngineState::Running);
            running.set_pid(std::process::id());
            Ok(vec![
                EmulatorInstanceData::new_with_state("new_emu", EngineState::New),
                EmulatorInstanceData::new_with_state("config_emu", EngineState::Configured),
                EmulatorInstanceData::new_with_state("staged_emu", EngineState::Staged),
                running,
                EmulatorInstanceData::new_with_state("error_emu", EngineState::Error),
            ])
        };

        let ctx_machine = tool.instances.expect_get_all_instances();
        ctx_machine.returning(mock_return);

        tool.main(machine_writer).await?;

        let expected_json = vec![
            r#"{"name":"new_emu","state":"new"}"#,
            r#"{"name":"config_emu","state":"configured"}"#,
            r#"{"name":"staged_emu","state":"staged"}"#,
            r#"{"name":"running_emu","state":"running"}"#,
            r#"{"name":"error_emu","state":"error"}"#,
        ];

        let (stdout, _stderr) = machine_buffers.into_strings();
        let stdout_expected = format!("[{}]\n", expected_json.join(","));
        assert_eq!(stdout, stdout_expected);
        let data = serde_json::from_str(&stdout).bug()?;
        assert!(
            matches!(data, serde_json::Value::Array(_)),
            "unexpected data. Expected array: {data:?}"
        );
        match VerifiedMachineWriter::<Vec<EmuListItem>>::verify_schema(&data) {
            Ok(_) => Ok(()),
            Err(e) => fho::return_bug!("Error verifying schema of {data:?}: {e}"),
        }
    }

    #[fuchsia::test]
    async fn test_error_list() -> Result<(), fho::Error> {
        let _test_env = ffx_config::test_init().unwrap();
        let cmd = ListCommand { only_running: false };
        let mut tool = EmuListTool { cmd, instances: MockInstances::new() };

        let test_buffers = TestBuffers::default();
        let writer = VerifiedMachineWriter::<Vec<EmuListItem>>::new_test(None, &test_buffers);

        let mock_return = || {
            Ok(vec![EmulatorInstanceData::new_with_state(
                "error_emu_error_list",
                EngineState::Error,
            )])
        };

        let ctx = tool.instances.expect_get_all_instances();
        ctx.returning(mock_return);

        tool.main(writer).await?;

        let (stdout, stderr) = test_buffers.into_strings();
        let stdout_expected = "[error]         error_emu_error_list\n";
        assert_eq!(stdout, stdout_expected);
        assert_eq!(stderr, format!("{}\n", BROKEN_MESSAGE));
        Ok(())
    }

    #[fuchsia::test]
    async fn test_error_list_machine() -> Result<(), fho::Error> {
        let _test_env = ffx_config::test_init().unwrap();
        let cmd = ListCommand { only_running: false };
        let mut tool = EmuListTool { cmd, instances: MockInstances::new() };

        let machine_buffers = TestBuffers::default();
        let machine_writer = VerifiedMachineWriter::<Vec<EmuListItem>>::new_test(
            Some(Format::Json),
            &machine_buffers,
        );

        let mock_return = || {
            Ok(vec![EmulatorInstanceData::new_with_state(
                "error_emu_error_list",
                EngineState::Error,
            )])
        };

        let ctx_machine = tool.instances.expect_get_all_instances();
        ctx_machine.returning(mock_return);

        tool.main(machine_writer).await?;

        let (stdout, _stderr) = machine_buffers.into_strings();
        let stdout_expected =
            format!("[{}]\n", r#"{"name":"error_emu_error_list","state":"error"}"#);
        assert_eq!(stdout, stdout_expected);
        let data = serde_json::from_str(&stdout).bug()?;
        assert!(
            matches!(data, serde_json::Value::Array(_)),
            "unexpected data. Expected array: {data:?}"
        );
        match VerifiedMachineWriter::<Vec<EmuListItem>>::verify_schema(&data) {
            Ok(_) => Ok(()),
            Err(e) => fho::return_bug!("Error verifying schema of {data:?}: {e}"),
        }
    }
}
