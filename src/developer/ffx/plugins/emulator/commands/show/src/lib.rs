// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use async_trait::async_trait;
use emulator_instance::EmulatorInstances;
use errors::ffx_bail;
use ffx_config::EnvironmentContext;
use ffx_emulator_config::ShowDetail;
use ffx_emulator_engines::EngineBuilder;
use ffx_emulator_show_args::ShowCommand;
use ffx_writer::{ToolIO as _, VerifiedMachineWriter};
use fho::{FfxMain, FfxTool, bug};
use itertools::Itertools;
use std::path::PathBuf;

/// Sub-sub tool for `emu show`
#[derive(Clone, FfxTool)]
pub struct EmuShowTool {
    #[command]
    cmd: ShowCommand,
    context: EnvironmentContext,
}

fn which_details(cmd: &ShowCommand) -> Vec<ShowDetail> {
    let mut details = vec![];
    if cmd.cmd || cmd.all {
        details.push(ShowDetail::Cmd { program: None, args: None, env: None })
    }
    if cmd.config || cmd.all {
        details.push(ShowDetail::Config { flags: None })
    }
    if cmd.device || cmd.all {
        details.push(ShowDetail::Device { device: None })
    }
    if cmd.net || cmd.all {
        details.push(ShowDetail::Net { mode: None, mac_address: None, upscript: None, ports: None })
    }
    if cmd.instance || cmd.all {
        details.push(ShowDetail::Instance {
            name: None,
            engine_type: None,
            engine_state: None,
            pid: None,
            emulator_binary: None,
            instance_directory: None,
        })
    }

    if details.is_empty() {
        details = vec![
            ShowDetail::Cmd { program: None, args: None, env: None },
            ShowDetail::Config { flags: None },
            ShowDetail::Device { device: None },
            ShowDetail::Net { mode: None, mac_address: None, upscript: None, ports: None },
            ShowDetail::Instance {
                name: None,
                engine_type: None,
                engine_state: None,
                pid: None,
                emulator_binary: None,
                instance_directory: None,
            },
        ]
    }
    details
}

// Since this is a part of a legacy plugin, add
// the legacy entry points. If and when this
// is migrated to a subcommand, this macro can be
//removed.
fho::embedded_plugin!(EmuShowTool);

#[async_trait(?Send)]
impl FfxMain for EmuShowTool {
    type Writer = VerifiedMachineWriter<Vec<ShowDetail>>;

    type Error = ::fho::Error;

    async fn main(self, writer: Self::Writer) -> fho::Result<()> {
        let instance_dir: PathBuf =
            self.context.get(ffx_config::keys::EMU_INSTANCE_ROOT_DIR).map_err(|e| bug!("{e}"))?;
        let emu_instances = EmulatorInstances::new(instance_dir);
        self.show(&emu_instances, writer).await.map_err(|e| e.into())
    }
}

impl EmuShowTool {
    pub async fn show(
        &self,
        emu_instances: &EmulatorInstances,
        mut writer: <Self as FfxMain>::Writer,
    ) -> Result<()> {
        let mut instance_name = self.cmd.name.clone();
        let builder = EngineBuilder::new(&self.context, emu_instances.clone());
        match builder.get_engine_by_name(&mut instance_name) {
            Ok(Some(engine)) => {
                let info = engine.show(which_details(&self.cmd));

                if engine.emu_config().runtime.config_override && self.cmd.net {
                    writeln!(
                        writer.stderr(),
                        "Configuration was provided manually to the start command using the --config flag.\n\
                Network details for this instance cannot be shown with this tool; try\n    \
                    `ffx emu show --config`\n\
                to review the emulator flags directly."
                    )?;
                }
                writer.machine_or_else(&info, || {
                    info.clone().into_iter().map(|i| i.to_string()).join("\n")
                })?
            }
            Ok(None) => {
                if let Some(name) = instance_name {
                    println!("Instance {name} not found.");
                } else {
                    println!("No instances found");
                }
            }
            Err(e) => ffx_bail!("{:?}", e),
        };
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use emulator_instance::{
        EmulatorInstanceData, EmulatorInstanceInfo, EngineState, EngineType, FlagData,
        NetworkingMode, write_to_disk,
    };
    use ffx_emulator_config::VirtualDeviceInfo;

    use ffx_writer::{Format, TestBuffers};
    use std::collections::HashMap;

    #[fuchsia::test]
    async fn test_show() -> Result<()> {
        let env = ffx_config::test_init().unwrap();
        let mut tool = EmuShowTool { cmd: ShowCommand::default(), context: env.context.clone() };
        let emu_instances = EmulatorInstances::new(PathBuf::from(env.isolate_root.path()));

        let test_buffers = TestBuffers::default();
        let writer = <EmuShowTool as FfxMain>::Writer::new_test(None, &test_buffers);

        let machine_buffers = TestBuffers::default();
        let machine_writer =
            <EmuShowTool as FfxMain>::Writer::new_test(Some(Format::JsonPretty), &machine_buffers);

        let mut data = EmulatorInstanceData::new_with_state("one_instance", EngineState::Running);
        let instance_dir = emu_instances.get_instance_dir("one_instance", true)?;
        data.get_emulator_configuration_mut().device.cpu.count = 2;
        write_to_disk(&data, &instance_dir)?;
        tool.cmd.name = Some("one_instance".to_string());

        tool.clone().show(&emu_instances, writer).await?;
        tool.show(&emu_instances, machine_writer).await?;

        let (stdout, stderr) = test_buffers.into_strings();
        let stdout_expected = include_str!("../test_data/test_show_expected.txt");
        assert_eq!(stdout, stdout_expected);
        assert!(stderr.is_empty());

        let (stdout, stderr) = machine_buffers.into_strings();

        let got_data: Vec<ShowDetail> = serde_json::from_str(&stdout).expect("json details");
        let want_data = vec![
            ShowDetail::Cmd {
                program: Some("".into()),
                args: Some(vec!["-fuchsia".to_string()]),
                env: Some(HashMap::<String, String>::new()),
            },
            ShowDetail::Config {
                flags: Some(FlagData {
                    args: vec![],
                    envs: HashMap::<String, String>::new(),
                    features: vec![],
                    kernel_args: vec![],
                    options: vec![],
                }),
            },
            ShowDetail::Device {
                device: Some(VirtualDeviceInfo {
                    name: "one_instance_device".into(),
                    description: Some(
                        "The virtual device used to launch the one_instance emulator.".into(),
                    ),
                    cpu: "x64".into(),
                    cpu_count: 2,
                    audio: "none".into(),
                    storage_bytes: 0,
                    pointing_device: "none".into(),
                    memory_bytes: 0,
                    ports: None,
                    window_height: 0,
                    window_width: 0,
                    cid: 0,
                }),
            },
            ShowDetail::Net {
                mode: Some(NetworkingMode::Auto),
                mac_address: None,
                upscript: None,
                ports: None,
            },
            ShowDetail::Instance {
                name: Some("one_instance".into()),
                engine_type: Some(EngineType::default()),
                engine_state: Some(EngineState::Staged),
                pid: Some(0),
                emulator_binary: Some(PathBuf::from("")),
                instance_directory: Some(PathBuf::from("")),
            },
        ];

        assert_eq!(got_data, want_data);
        assert!(stderr.is_empty());
        let value = serde_json::to_value(&got_data).expect("value from data");
        <EmuShowTool as FfxMain>::Writer::verify_schema(&value).expect("schema OK");
        Ok(())
    }
    #[fuchsia::test]
    async fn test_show_unknown() -> Result<()> {
        let env = ffx_config::test_init().unwrap();

        let mut tool = EmuShowTool { cmd: ShowCommand::default(), context: env.context.clone() };
        let emu_instances = EmulatorInstances::new(PathBuf::from(env.isolate_root.path()));
        let data = EmulatorInstanceData::new_with_state("one_instance", EngineState::Running);
        let instance_dir = emu_instances.get_instance_dir("one_instance", true)?;
        write_to_disk(&data, &instance_dir)?;
        tool.cmd.name = Some("one_instance".to_string());

        let test_buffers = TestBuffers::default();
        let writer = <EmuShowTool as FfxMain>::Writer::new_test(None, &test_buffers);

        let machine_buffers = TestBuffers::default();
        let machine_writer =
            <EmuShowTool as FfxMain>::Writer::new_test(Some(Format::Json), &machine_buffers);

        tool.cmd.name = Some("unknown_instance".to_string());

        tool.show(&emu_instances, writer).await?;
        tool.show(&emu_instances, machine_writer).await?;

        let (stdout, stderr) = test_buffers.into_strings();

        assert_eq!(stdout, "");
        assert!(stderr.is_empty());

        let (stdout, stderr) = machine_buffers.into_strings();
        let stdout_expected = "";

        assert_eq!(stdout, stdout_expected);
        assert!(stderr.is_empty());

        Ok(())
    }

    #[fuchsia::test]
    async fn test_show_part() {
        let mut builder = ffx_config::test_env();
        let temp_dir = builder.isolate_root().join("instances");
        std::fs::create_dir_all(&temp_dir).unwrap();
        let env = builder
            .user_config(ffx_config::keys::EMU_INSTANCE_ROOT_DIR, temp_dir.to_string_lossy())
            .build()
            .unwrap();
        let tool = EmuShowTool {
            cmd: ShowCommand { device: true, ..Default::default() },
            context: env.context.clone(),
        };

        let mut data = EmulatorInstanceData::new_with_state("one_instance", EngineState::Running);
        data.get_emulator_configuration_mut().device.cpu.count = 2;
        let emu_instances = EmulatorInstances::new(temp_dir);
        let instance_dir = emu_instances
            .get_instance_dir("one_instance", true)
            .expect("test instance dir created");
        write_to_disk(&data, &instance_dir).expect("test data written");

        let test_buffers = TestBuffers::default();
        let writer =
            <EmuShowTool as FfxMain>::Writer::new_test(Some(Format::JsonPretty), &test_buffers);

        let result = tool.main(writer).await;
        let (stdout, stderr) = test_buffers.into_strings();

        if !result.is_ok() {
            panic!("Expected OK result got {stdout} {stderr}")
        }

        let got_data: Vec<ShowDetail> = match serde_json::from_str(&stdout) {
            Ok(data) => data,
            Err(e) => panic!("Error {e} parsing json  from {stdout} {stderr}"),
        };

        let want_data: Vec<ShowDetail> = vec![ShowDetail::Device {
            device: Some(VirtualDeviceInfo {
                name: "one_instance_device".into(),
                description: Some(
                    "The virtual device used to launch the one_instance emulator.".into(),
                ),
                cpu: "x64".into(),
                cpu_count: 2,
                audio: "none".into(),
                storage_bytes: 0,
                pointing_device: "none".into(),
                memory_bytes: 0,
                ports: None,
                window_height: 0,
                window_width: 0,
                cid: 0,
            }),
        }];
        assert_eq!(got_data, want_data);
        let value = serde_json::to_value(&got_data).expect("value from data");
        <EmuShowTool as FfxMain>::Writer::verify_schema(&value).expect("schema OK");
    }
}
