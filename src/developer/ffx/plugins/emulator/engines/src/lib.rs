// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The ffx_emulator_engines crate contains the implementation
//! of each emulator "engine" such as aemu and qemu.

mod arg_templates;
mod qemu_based;
pub mod serialization;
mod show_output;

pub use arg_templates::process_flag_template;
use emulator_instance::{
    DeviceConfig, EmulatorConfiguration, EmulatorInstanceData, EmulatorInstanceInfo,
    EmulatorInstances, EngineOption, EngineState, EngineType, FlagData, GuestConfig, HostConfig,
    RuntimeConfig, SerialMode, read_from_disk,
};
use errors::ffx_bail;
use ffx_config::EnvironmentContext;
use ffx_emulator_common::config::EMU_SERIAL_ENABLED;
use ffx_emulator_config::EmulatorEngine;
use fho::{Result, bug, return_bug, return_user_error};
use port_picker::{is_free_tcp_port, pick_unused_port};
use qemu_based::crosvm::CrosvmEngine;
use qemu_based::femu::FemuEngine;
use qemu_based::qemu::QemuEngine;

/// The EngineBuilder is used to create and configure an EmulatorEngine, while ensuring the
/// configuration will result in a valid emulation instance.
///
/// Create an EngineBuilder using EngineBuilder::new(). This will populate the builder with the
/// defaults for all configuration options. Then use the setter methods to update configuration
/// options, and call "build()" when configuration is complete.
///
/// Setters are independent, optional, and idempotent; i.e. callers may call as many or as few of
/// the setters as needed, and repeat calls if necessary. However, setters consume the data that
/// are passed in, so the caller must set up a new structure for each call.
///
/// Once "build" is called, an engine will be instantiated of the indicated type, the configuration
/// will be loaded into that engine, and the engine's "configure" function will be invoked to
/// trigger validation and ensure the configuration is acceptable. If validation fails, the engine
/// will be destroyed. The EngineBuilder instance is consumed when invoking "build" regardless of
/// the outcome.
///
/// Example:
///
///    let builder = EngineBuilder::new()
///         .engine_type(EngineType::Femu)
///         .device(my_device_config)
///         .guest(my_guest_config)
///         .host(my_host_config)
///         .runtime(my_runtime_config);
///
///     let mut engine: Box<dyn EmulatorEngine> = builder.build()?;
///     (*engine).start().await
///
pub struct EngineBuilder {
    context: EnvironmentContext,
    emulator_configuration: EmulatorConfiguration,
    engine_type: EngineType,
    emu_instances: EmulatorInstances,
    serial_enabled: Option<bool>,
}

impl EngineBuilder {
    /// Create a new EngineBuilder, populated with default values for all configuration.
    pub fn new(context: &EnvironmentContext, emu_instances: EmulatorInstances) -> Self {
        Self {
            context: context.clone(),
            emulator_configuration: EmulatorConfiguration::default(),
            engine_type: EngineType::default(),
            emu_instances,
            serial_enabled: None,
        }
    }

    /// Set the configuration to use when building a new engine.
    pub fn config(mut self, config: EmulatorConfiguration) -> EngineBuilder {
        self.emulator_configuration = config;
        self
    }

    /// Override the "emu.serial_number.enabled" config setting.
    pub fn serial_enabled(mut self, enabled: bool) -> EngineBuilder {
        self.serial_enabled = Some(enabled);
        self
    }

    /// Set the engine's virtual device configuration.
    pub fn device(mut self, device_config: DeviceConfig) -> EngineBuilder {
        self.emulator_configuration.device = device_config;
        self
    }

    /// Set the type of the engine to be built.
    pub fn engine_type(mut self, engine_type: EngineType) -> EngineBuilder {
        self.engine_type = engine_type;
        self
    }

    /// Set the engine's guest configuration.
    pub fn guest(mut self, guest_config: GuestConfig) -> EngineBuilder {
        self.emulator_configuration.guest = guest_config;
        self
    }

    /// Set the engine's host configuration.
    pub fn host(mut self, host_config: HostConfig) -> EngineBuilder {
        self.emulator_configuration.host = host_config;
        self
    }

    /// Set the engine's runtime configuration.
    pub fn runtime(mut self, runtime_config: RuntimeConfig) -> EngineBuilder {
        self.emulator_configuration.runtime = runtime_config;
        self
    }

    /// Create from an existing EmulatorInstanceData,
    /// Does not validate or perform any configuration steps. Call
    /// |build| for those steps to be performed.
    pub fn from_data(&self, data: EmulatorInstanceData) -> Box<dyn EmulatorEngine> {
        match data.get_engine_type() {
            EngineType::Femu => {
                Box::new(FemuEngine::new(&self.context, data, self.emu_instances.clone()))
            }
            EngineType::Qemu => {
                Box::new(QemuEngine::new(&self.context, data, self.emu_instances.clone()))
            }
            EngineType::Crosvm => {
                Box::new(CrosvmEngine::new(&self.context, data, self.emu_instances.clone()))
            }
        }
    }

    /// Finalize and validate the configuration, set up the engine's instance directory,
    /// and return the built engine.
    pub async fn build(mut self) -> Result<Box<dyn EmulatorEngine>> {
        // Set up the instance directory, now that we have enough information.
        let name = self.emulator_configuration.runtime.name.clone();
        self.emulator_configuration.runtime.engine_type = self.engine_type;
        self.emulator_configuration.runtime.instance_directory =
            self.emu_instances.get_instance_dir(&name, true).map_err(anyhow::Error::from)?;

        let serial_enabled = self
            .serial_enabled
            .unwrap_or_else(|| self.context.get(EMU_SERIAL_ENABLED).unwrap_or(true));
        update_serial_number(&mut self.emulator_configuration, serial_enabled);

        // Make sure we don't overwrite an existing instance.
        if let Ok(EngineOption::DoesExist(instance_data)) =
            read_from_disk(&self.emulator_configuration.runtime.instance_directory)
        {
            if instance_data.is_running() {
                return_user_error!(
                    "An emulator named {} is already running. \
                    Use a different name, or run `ffx emu stop {}` \
                    to stop the running emulator.",
                    name,
                    name
                );
            }
        }

        // Build and complete configuration on the engine, then pass it back to the caller.
        let instance_data = EmulatorInstanceData::new(
            self.emulator_configuration.clone(),
            self.engine_type,
            EngineState::Configured,
        );

        let mut engine: Box<dyn EmulatorEngine> = self.from_data(instance_data);
        engine.configure()?;

        engine.load_emulator_binary()?;

        engine.emu_config_mut().flags = process_flag_template(engine.emu_config())
            .map_err(|e| bug!("Engine builder failed to process the flags template file: {e}"))?;
        engine.save_to_disk().await?;

        Ok(engine)
    }

    /// Returns the EmulatorEngine instance based on the name.
    /// If `name` is None:
    ///    - If no emulator instances are found, returns Ok(None).
    ///    - If exactly 1 instance is found, returns Ok(Some(engine)) and updates the `name` parameter.
    ///    - If multiple instances are found, returns an error.
    /// If `name` is Some:
    ///    - Returns Ok(Some(engine)) if the instance exists.
    ///    - Returns Ok(None) if the instance does not exist.
    pub fn get_engine_by_name(
        &self,
        name: &mut Option<String>,
    ) -> Result<Option<Box<dyn EmulatorEngine>>> {
        if name.is_none() {
            let all_instances = match self.emu_instances.get_all_instances() {
                Ok(list) => list,
                Err(e) => {
                    ffx_bail!("Error encountered looking up emulator instances: {e:?}");
                }
            };

            let running_instances: Vec<_> = all_instances
                .iter()
                .filter(|i| i.get_engine_state() == EngineState::Running)
                .collect();

            if running_instances.len() == 1 {
                *name = Some(running_instances[0].get_name().to_string());
            } else if running_instances.len() > 1 {
                return_user_error!(
                    "Multiple running emulators found. Indicate which emulator to access\n\
                by specifying the emulator name with your command.\n\
                See all the emulators available using `ffx emu list`."
                );
            } else if all_instances.len() == 1 {
                *name = Some(all_instances[0].get_name().to_string());
            } else if all_instances.len() == 0 {
                log::debug!("No emulators found.");
                return Ok(None);
            } else {
                return_user_error!(
                    "Multiple emulators found but none are running. Indicate which emulator to access\n\
                by specifying the emulator name with your command.\n\
                See all the emulators available using `ffx emu list`."
                );
            }
        }

        // If we got this far, name is set to either what the user asked for, or the only one running.
        if let Some(local_name) = name {
            let instance_dir = self
                .emu_instances
                .get_instance_dir(local_name, false)
                .map_err(|e| anyhow::Error::from(e))?;
            match read_from_disk(&instance_dir).map_err(|e| anyhow::Error::from(e))? {
                EngineOption::DoesExist(data) => Ok(Some(self.from_data(*data))),
                EngineOption::DoesNotExist(_) => Ok(None),
            }
        } else {
            ffx_bail!("No emulator instances found")
        }
    }
}

// Given the string representation of a flag template, apply the provided configuration to resolve
// the template into a FlagData object.
pub fn process_flags_from_str(text: &str, emu_config: &EmulatorConfiguration) -> Result<FlagData> {
    arg_templates::process_flags_from_str(text, emu_config)
        .map_err(|e| bug!("Error processing flags: {e}"))
}

#[allow(dead_code)]
/// Ensures all ports are mapped with available port values, assigning free ports any that are
/// missing, and making sure there are no conflicts within the map.
pub(crate) fn finalize_port_mapping(emu_config: &mut EmulatorConfiguration) -> Result<()> {
    let port_map = &mut emu_config.host.port_map;
    let mut used_ports = Vec::new();
    for (name, port) in port_map {
        let mut need_allocation = true;
        if let Some(value) = port.host {
            if value != 0 {
                if used_ports.contains(&value) {
                    return_user_error!("Host port {} was mapped to multiple guest ports.", value);
                }
                if is_free_tcp_port(value).is_none() {
                    return_user_error!("Host port {} is already in use by another process.", value);
                }
                // This port is good, so we claim it to make sure there are no conflicts later.
                used_ports.push(value);
                need_allocation = false;
            }
        }
        if need_allocation {
            log::warn!(
                "No host-side port specified for '{:?}', a host port will be dynamically \
                assigned. Check `ffx emu show {}` to see which port is assigned.",
                name,
                emu_config.runtime.name
            );

            // There have been some incidents in automated tests of the same port
            // being returned multiple times.
            // So we'll try multiple times and avoid duplicates.
            let mut assigned = false;
            for _ in 0..10 {
                if let Some(value) = pick_unused_port() {
                    if !used_ports.contains(&value) {
                        port.host = Some(value);
                        used_ports.push(value);
                        assigned = true;
                        break;
                    } else {
                        log::warn!("pick unused port returned: {} multiple times\n", value);
                    }
                } else {
                    log::warn!("pick unused port returned: None\n");
                }
            }
            if !assigned {
                return_bug!("Unable to assign a host port for '{}'. Terminating emulation.", name);
            }
        }
    }
    log::debug!("Port map finalized: {:?}\n", emu_config.host.port_map);
    Ok(())
}

fn generate_serial() -> String {
    format!("EM-{:09X}", rand::random::<u64>() & 0xFFFFFFFFF)
}

/// Updates the serial number configuration based on the `emu.serial_number.enabled` setting.
///
/// Handles generation of new serial numbers, preservation of existing stable serials,
/// and transition of the serial number state if the config changes.
pub fn update_serial_number(config: &mut EmulatorConfiguration, serial_enabled: bool) {
    let runtime = &mut config.runtime;
    match &runtime.serial_number {
        SerialMode::Uninitialized => {
            if serial_enabled {
                runtime.serial_number = SerialMode::Enabled(generate_serial());
            } else {
                runtime.serial_number = SerialMode::Disabled;
            }
        }
        SerialMode::Enabled(_) => {
            if !serial_enabled {
                runtime.serial_number = SerialMode::Disabled;
            }
        }
        SerialMode::Disabled => {
            if serial_enabled {
                runtime.serial_number = SerialMode::Enabled(generate_serial());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use emulator_instance::PortMapping;

    #[test]
    fn test_finalize_port_mapping_zero() {
        let mut emu_config = EmulatorConfiguration::default();
        emu_config
            .host
            .port_map
            .insert("test_port".to_string(), PortMapping { host: Some(0), guest: 2222 });

        let result = finalize_port_mapping(&mut emu_config);
        assert!(result.is_ok(), "Expected OK, got {:?}", result);

        let mapping = emu_config.host.port_map.get("test_port").unwrap();
        assert!(mapping.host.is_some());
        let host_port = mapping.host.unwrap();
        assert_ne!(host_port, 0, "Host port should not be 0 after finalization");
        assert_eq!(mapping.guest, 2222);
    }

    #[fuchsia::test]
    async fn test_serial_number_generation() {
        use tempfile::tempdir;

        let builder = ffx_config::test_env();
        let builder = crate::qemu_based::tests::make_fake_sdk(builder).await;
        let env = builder.build().expect("test env");
        let temp_dir = tempdir().expect("Couldn't get a temporary directory for testing.");
        let emu_instances = EmulatorInstances::new(temp_dir.path().to_path_buf());

        // 1. Default: Serial number generation is enabled.
        let mut cfg = EmulatorConfiguration::default();
        cfg.runtime.name = "test-emu-1".to_string();
        cfg.runtime.config_override = true;
        let builder = EngineBuilder::new(&env.context, emu_instances.clone())
            .config(cfg)
            .engine_type(EngineType::Qemu);

        let engine = builder.build().await.expect("engine built");
        let serial = match &engine.emu_config().runtime.serial_number {
            SerialMode::Enabled(s) => s,
            _ => panic!(
                "expected serial to be generated, got {:?}",
                engine.emu_config().runtime.serial_number
            ),
        };
        assert_eq!(serial.len(), 12);
        assert!(serial.starts_with("EM-"));
        assert!(
            serial[3..]
                .chars()
                .all(|c| c.is_ascii_hexdigit() && (c.is_numeric() || c.is_uppercase()))
        );

        // 2. Disabled: Config is false.
        let builder_disabled_env = ffx_config::test_env();
        let builder_disabled_env =
            crate::qemu_based::tests::make_fake_sdk(builder_disabled_env).await;
        let env_disabled = builder_disabled_env
            .user_config(EMU_SERIAL_ENABLED, "false")
            .build()
            .expect("test env disabled");
        let mut cfg_disabled = EmulatorConfiguration::default();
        cfg_disabled.runtime.name = "test-emu-2".to_string();
        cfg_disabled.runtime.config_override = true;
        let builder_disabled = EngineBuilder::new(&env_disabled.context, emu_instances.clone())
            .config(cfg_disabled)
            .engine_type(EngineType::Qemu);

        let engine_disabled = builder_disabled.build().await.expect("engine built");
        assert_eq!(engine_disabled.emu_config().runtime.serial_number, SerialMode::Disabled);

        // 3. Pre-existing: Some("custom-serial") is not overwritten.
        let mut cfg_custom = EmulatorConfiguration::default();
        cfg_custom.runtime.name = "test-emu-3".to_string();
        cfg_custom.runtime.serial_number = SerialMode::Enabled("custom-serial".to_string());
        cfg_custom.runtime.config_override = true;
        let builder_custom = EngineBuilder::new(&env.context, emu_instances.clone())
            .config(cfg_custom)
            .engine_type(EngineType::Qemu);

        let engine_custom = builder_custom.build().await.expect("engine built");
        assert_eq!(
            engine_custom.emu_config().runtime.serial_number,
            SerialMode::Enabled("custom-serial".to_string())
        );
    }
}
