// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The crosvm module encapsulates the interactions with the emulator instance
//! started via the crosvm emulator.

use super::get_host_tool;
use crate::qemu_based::QemuBasedEngine;
use async_trait::async_trait;
use emulator_instance::{
    AccelerationMode, ConsoleType, EmulatorConfiguration, EmulatorInstanceData,
    EmulatorInstanceInfo, EmulatorInstances, EngineState, EngineType, NetworkingMode,
    write_to_disk,
};
use ffx_config::EnvironmentContext;
use ffx_emulator_common::config::CROSVM_TOOL;
use ffx_emulator_common::{find_unused_vsock_cid, process};
use ffx_emulator_config::{EmulatorEngine, EngineConsoleType, ShowDetail};
use fho::{Result, bug, return_bug};
use std::collections::HashMap;
use std::path::Path;
use std::process::{Command, Output};

#[derive(Clone, Debug)]
pub struct CrosvmEngine {
    data: EmulatorInstanceData,
    emu_instances: EmulatorInstances,
    context: EnvironmentContext,
}

impl CrosvmEngine {
    pub(crate) fn new(
        context: &EnvironmentContext,
        data: EmulatorInstanceData,
        emu_instances: EmulatorInstances,
    ) -> Self {
        Self { context: context.clone(), data, emu_instances }
    }

    fn validate_configuration(&self) -> Result<()> {
        if !self.emu_config().runtime.headless {
            return_bug!("Launching crosvm is only supported with headless mode");
        }
        if self.emu_config().runtime.console != ConsoleType::Console {
            return_bug!("Launching crosvm is only supported in serial console mode");
        }
        if self.emu_config().host.networking == NetworkingMode::User {
            return_bug!("Launching crosvm is only supported with TAP network");
        }
        if self.emu_config().host.acceleration == AccelerationMode::None {
            return_bug!("Launching crosvm is only supported in with KVM acceleration");
        }
        if self.emu_config().runtime.debugger {
            return_bug!("Launching crosvm with debugger is not supported");
        }
        self.validate_network_flags(&self.emu_config())
            .and_then(|()| self.check_required_files(&self.emu_config().guest))
    }

    fn validate_staging(&self) -> Result<()> {
        self.check_required_files(&self.emu_config().guest)
    }
}

#[async_trait(?Send)]
impl EmulatorEngine for CrosvmEngine {
    fn get_instance_data(&self) -> &EmulatorInstanceData {
        &self.data
    }

    async fn stage(&mut self) -> Result<()> {
        let ctx_clone = self.context.clone();
        if let Err(e) = <Self as QemuBasedEngine>::stage(&mut self, &ctx_clone)
            .await
            .and_then(|()| self.validate_staging())
        {
            self.data.set_engine_state(EngineState::Error);
            return self.save_to_disk().await.and(Err(e));
        }

        if let Some(vsock) = &mut self.emu_config_mut().device.vsock {
            if vsock.enabled {
                vsock.cid = find_unused_vsock_cid()?;
            }
        }

        let emu_config = self.emu_config();
        // Fall back to 4 core is cpu_count isn't specified.
        let cpu_count = emu_config.device.cpu.count;
        let cpu_count = if cpu_count > 0 { cpu_count } else { 4 };
        // TODO(https://fxbug.dev/371614411): Move these into a template.
        let mut args = vec![
            "--extended-status".to_string(),
            "--log-level=debug".to_string(),
            "run".to_string(),
            "--disable-sandbox".to_string(),
            "-s".to_string(),
            format!("{}/control.sock", emu_config.runtime.instance_directory.to_str().unwrap()),
            "--mem".to_string(),
            emu_config.device.memory.quantity.to_string(),
            "--cpus".to_string(),
            cpu_count.to_string(),
            "--serial".to_string(),
            "type=stdout,stdin,hardware=serial,earlycon".to_string(),
            emu_config.guest.kernel_image.clone().unwrap().to_str().unwrap().into(),
        ];
        if let Some(ramdisk) = &emu_config.guest.ramdisk {
            args.extend(vec!["--initrd".to_string(), ramdisk.path.to_str().unwrap().into()]);
        }
        if let Some(vsock) = &emu_config.device.vsock {
            if vsock.enabled {
                args.extend(vec!["--vsock".to_string(), format!("cid={}", vsock.cid)]);
            }
        }
        if let Some(image) = &emu_config.guest.disk_image {
            args.extend(vec!["--block".to_string(), image.to_str().unwrap().into()]);
        }
        if emu_config.host.networking == NetworkingMode::Tap {
            args.extend(vec![
                "--net".to_string(),
                format!("tap-name=qemu,mac={}", emu_config.runtime.mac_address),
            ]);
        }
        self.emu_config_mut().flags.args = args;
        // We don't care about several of these fields filled out via the qemu specific
        // template.
        self.emu_config_mut().flags.envs = HashMap::new();
        self.emu_config_mut().flags.features = vec![];
        self.emu_config_mut().flags.options = vec![];
        self.data.set_engine_state(EngineState::Staged);
        self.save_to_disk().await
    }

    async fn start(&mut self, context: &EnvironmentContext, emulator_cmd: Command) -> Result<i32> {
        self.run(context, emulator_cmd).await
    }

    fn show(&self, details: Vec<ShowDetail>) -> Vec<ShowDetail> {
        <Self as QemuBasedEngine>::show(self, details)
    }

    async fn stop(&mut self) -> Result<()> {
        if self.is_running().await {
            log::info!("Terminating running instance {:?}", self.get_pid());

            let result = Command::new(&self.data.get_emulator_binary())
                .arg("stop")
                .arg(format!(
                    "{}/control.sock",
                    self.emu_config().runtime.instance_directory.to_str().unwrap()
                ))
                .output()
                .map_err(|e| bug!("Failed to spawn crosvm stop command: {e}"))
                .and_then(|Output { status, stdout: _, stderr }| {
                    if status.success() {
                        Ok(())
                    } else {
                        Err(bug!("status: {status} stderr:\n{stderr:?}"))
                    }
                });
            if result.is_err() {
                log::warn!("crosvm stop command failed, falling back to killing PID: {result:?}");
                if let Some(terminate_error) = process::terminate(self.get_pid()).err() {
                    log::warn!("Error encountered terminating process: {:?}", terminate_error);
                }
            }
        }

        self.set_engine_state(EngineState::Staged);
        self.save_to_disk().await
    }

    fn configure(&mut self) -> Result<()> {
        let result = if self.emu_config().runtime.config_override {
            let message = "Custom configuration provided; bypassing validation.";
            eprintln!("{message}");
            log::info!("{message}");
            Ok(())
        } else {
            self.validate_configuration()
        };
        if result.is_ok() {
            self.data.set_engine_state(EngineState::Configured);
        } else {
            self.data.set_engine_state(EngineState::Error);
        }
        result
    }

    fn engine_state(&self) -> EngineState {
        self.get_engine_state()
    }

    fn engine_type(&self) -> EngineType {
        self.data.get_engine_type()
    }

    async fn is_running(&mut self) -> bool {
        let running = self.data.is_running();
        if self.engine_state() == EngineState::Running && running == false {
            self.set_engine_state(EngineState::Staged);
            if self.save_to_disk().await.is_err() {
                log::warn!("Problem saving serialized emulator to disk during state update.");
            }
        }
        running
    }

    fn attach(&self, _console: EngineConsoleType) -> Result<()> {
        // TODO(surajmalhotra): Support attaching to the console later.
        return_bug!("Attach not implemented for crosvm engine");
    }

    /// Build the Command to launch crosvm emulator running Fuchsia.
    fn build_emulator_cmd(&self) -> Command {
        let mut cmd = Command::new(&self.data.get_emulator_binary());
        let emu_config = self.emu_config();
        cmd.args(&emu_config.flags.args);

        // Can't have kernel args if there is no kernel, but if there is a custom configuration template,
        // add them anyway since the configuration and the custom template could be out of sync.
        if emu_config.guest.kernel_image.is_some() || emu_config.runtime.config_override {
            let extra_args = emu_config
                .flags
                .kernel_args
                .iter()
                .map(|x| x.to_string())
                .collect::<Vec<_>>()
                .join(" ");
            if extra_args.len() > 0 {
                cmd.args(["--params", &extra_args]);
            }
        }
        cmd
    }

    /// Loads the path to the crosvm binary to execute. This is based on the guest OS architecture.
    fn load_emulator_binary(&mut self) -> Result<()> {
        let emulator_binary = match get_host_tool(&self.context, CROSVM_TOOL) {
            Ok(crosvm_path) => crosvm_path.canonicalize().map_err(|e| {
                bug!("Failed to canonicalize the path to the emulator binary: {crosvm_path:?}: {e}")
            })?,
            Err(e) => return_bug!("Cannot find {CROSVM_TOOL} in the SDK: {e}"),
        };

        if !emulator_binary.exists() || !emulator_binary.is_file() {
            return_bug!("Giving up finding emulator binary. Tried {:?}", emulator_binary)
        }
        self.data.set_emulator_binary(emulator_binary);

        Ok(())
    }

    fn emu_config(&self) -> &EmulatorConfiguration {
        self.data.get_emulator_configuration()
    }

    fn emu_config_mut(&mut self) -> &mut EmulatorConfiguration {
        self.data.get_emulator_configuration_mut()
    }

    async fn screenshot(&mut self, _path: &Path) -> Result<()> {
        Err(fho::user_error!("Screenshot not implemented for crosvm yet."))
    }

    async fn save_to_disk(&self) -> Result<()> {
        write_to_disk(
            &self.data,
            &self
                .emu_instances
                .get_instance_dir(self.data.get_name(), true)
                .unwrap_or_else(|_| panic!("instance directory for {}", self.data.get_name())),
        )
        .map_err(|e| bug!("Error saving instance to disk: {e}"))
    }
}

// This is a hack to get access to a lot of functionality which isn't really qemu specific.
impl QemuBasedEngine for CrosvmEngine {
    fn set_pid(&mut self, pid: u32) {
        self.data.set_pid(pid);
    }

    fn get_pid(&self) -> u32 {
        self.data.get_pid()
    }

    fn set_engine_state(&mut self, state: EngineState) {
        self.data.set_engine_state(state)
    }

    fn get_engine_state(&self) -> EngineState {
        self.data.get_engine_state()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EngineBuilder;
    use crate::qemu_based::tests::make_fake_sdk;
    use std::path::PathBuf;
    use std::{env, fs};
    use tempfile::tempdir;

    #[fuchsia::test]
    fn test_build_emulator_cmd() {
        let env = ffx_config::test_env().build().expect("test env");
        let program_name = "/test_crosvm_bin";
        let mut cfg = EmulatorConfiguration::default();
        cfg.host.networking = NetworkingMode::None;
        cfg.flags.envs.insert("FLAG_NAME_THAT_DOES_NOT_EXIST".into(), "1".into());

        let mut emu_data = EmulatorInstanceData::new(cfg, EngineType::Crosvm, EngineState::New);
        emu_data.set_emulator_binary(program_name.into());
        let test_engine =
            CrosvmEngine::new(&env.context, emu_data, EmulatorInstances::new(PathBuf::new()));
        let cmd = test_engine.build_emulator_cmd();
        assert_eq!(cmd.get_program(), program_name);
        assert_eq!(cmd.get_envs().collect::<Vec<_>>(), []);
    }

    #[fuchsia::test]
    fn test_build_emulator_cmd_existing_env() {
        unsafe { env::set_var("FLAG_NAME_THAT_DOES_EXIST", "preset_value") };
        let env = ffx_config::test_env().build().expect("test env");
        let program_name = "/test_crosvm_bin";
        let mut cfg = EmulatorConfiguration::default();
        cfg.host.networking = NetworkingMode::None;
        cfg.flags.envs.insert("FLAG_NAME_THAT_DOES_EXIST".into(), "1".into());

        let mut emu_data = EmulatorInstanceData::new(cfg, EngineType::Crosvm, EngineState::New);
        emu_data.set_emulator_binary(program_name.into());
        let test_engine: CrosvmEngine =
            CrosvmEngine::new(&env.context, emu_data, EmulatorInstances::new(PathBuf::new()));
        let cmd = test_engine.build_emulator_cmd();
        assert_eq!(cmd.get_program(), program_name);
        assert_eq!(cmd.get_envs().collect::<Vec<_>>(), []);
    }

    #[fuchsia::test]
    async fn test_build_cmd_with_custom_template() {
        let builder = ffx_config::test_env();
        let builder = make_fake_sdk(builder).await;
        let env = builder.build().unwrap();
        let temp = tempdir().expect("cannot get tempdir");
        let emu_instances = EmulatorInstances::new(temp.path().to_owned());

        let mut cfg = EmulatorConfiguration::default();
        let template_file = temp.path().join("custom-template.json");
        fs::write(
            &template_file,
            r#"
         {
         "args": [
             "-kernel",
             "boot-shim.bin",
             "-initrd",
             "test.zbi"
         ],
         "envs": {},
         "features": [],
         "kernel_args": ["zircon.nodename=some-emu","TERM=dumb"],
         "options": []
         }"#,
        )
        .expect("custom template contents");
        cfg.runtime.template = Some(template_file);
        cfg.runtime.config_override = true;
        let engine = EngineBuilder::new(&env.context, emu_instances.clone())
            .config(cfg.clone())
            .engine_type(EngineType::Crosvm)
            .build()
            .await
            .expect("engine built");

        let cmd = engine.build_emulator_cmd();
        let actual: Vec<_> = cmd.get_args().collect();

        let expected = vec![
            "-kernel",
            "boot-shim.bin",
            "-initrd",
            "test.zbi",
            "--params",
            "TERM=dumb zircon.nodename=some-emu",
        ];

        assert_eq!(actual, expected)
    }
}
