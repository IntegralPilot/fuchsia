// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Utilities for Product Bundle Metadata (PBM).

use anyhow::{Context, Result, bail};
use emulator_instance::{
    AccelerationMode, ConsoleType, EmulatorConfiguration, EmulatorInstances, GpuType, LogLevel,
    NetworkingMode, OperatingSystem, PortMapping,
};
use ffx_config::EnvironmentContext;
use ffx_emulator_common::config::{
    BOOTLOADER_SECURE_ARM64, EMU_UPSCRIPT_FILE, KVM_PATH, OVMF_CODE_ARM64, OVMF_CODE_X64,
};
use ffx_emulator_common::split_once;
use ffx_emulator_common::tuntap::tap_available;
use ffx_emulator_config::convert_bundle_to_configs;
use ffx_emulator_start_args::StartCommand;
use fho::{bug, user_error};
use pbms::ProductBundle;
use regex::Regex;
use sdk_metadata::{CpuArchitecture, VirtualDeviceManifest};
use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::env;
use std::fs::File;
use std::hash::Hasher;
use std::path::PathBuf;
use std::str::FromStr;
use std::time::Duration;

/// Create a RuntimeConfiguration based on the command line args.
pub(crate) async fn make_configs(
    ctx: &EnvironmentContext,
    cmd: &StartCommand,
    product_bundle: Option<ProductBundle>,
    emu_instances: &EmulatorInstances,
) -> Result<EmulatorConfiguration> {
    // Start with either the custom config template passed in on the command line,
    // or from the product bundle.
    let mut emu_config =
    // If the user specified a path to a flag config file on the command line, use that.
    // This bypasses the rest of the configuration phase, which means the EmulationConfiguration
    // contents don't actually represent the configuration being used to launch the emulator.
    if let Some(template_file) = &cmd.config {
        let mut emu_config = EmulatorConfiguration::default();
        emu_config.runtime.template = Some(PathBuf::from(env::current_dir()?).join(template_file));
        emu_config.runtime.config_override = true;
        emu_config
    } else {
        let pb = product_bundle.ok_or_else(|| user_error!("Product bundle required for configuring the emulator instance."))?;
        // Apply the values from the manifest to an emulation configuration.
        let mut emu_config = convert_bundle_to_configs(&pb, cmd.device(ctx)?, cmd.uefi)
            .await.context("problem with convert_bundle_to_configs")?;
        // Set OVMF references for non riscv guests (at this time we have no efi support for riscv).
        if emu_config.device.cpu.architecture != CpuArchitecture::Riscv64 {
            emu_config.guest.ovmf_code = ffx_config::get_host_tool(&ctx,
                match emu_config.device.cpu.architecture {
                    CpuArchitecture::X64 => OVMF_CODE_X64,
                    CpuArchitecture::Arm64 => OVMF_CODE_ARM64,
                    arch @ _ => bail!("CPU architecture {} is currently unsupported with (U)EFI", arch),
                }).map_err(|e| bug!("cannot locate ovmf code in SDK: {e}"))?;

            log::info!("Found ovmf code at {:?}", emu_config.guest.ovmf_code);

            // Non-fatal error since infra may not always supply the file if it is not needed for the
            // tests being run.
            if !emu_config.guest.ovmf_code.exists() {
                log::warn!("cannot find OVMF code at {:?}", emu_config.guest.ovmf_code);
            }

            // vars is in the same directory with the same basename prefix. ARM64 and x64 have different
            // filenames:
            // x64: OVMF_CODE.fd, OVMF_VARS.fd
            // arm64: QEMU_EFI.fd, QEMU_VARS.fd
            let vars_filename = if let Some(code_name) = emu_config.guest.ovmf_code.file_name() {
                match emu_config.device.cpu.architecture {
                    CpuArchitecture::X64 => code_name.to_string_lossy().replace("_CODE.fd", "_VARS.fd"),
                    CpuArchitecture::Arm64 => code_name.to_string_lossy().replace("_EFI.fd", "_VARS.fd"),
                    arch @ _ => bail!("CPU architecture {} is currently unsupported with (U)EFI", arch),
                }
            } else {
                log::warn!("unrecognized OVMF code file name {:?}", emu_config.guest.ovmf_code);
                "OVMF_VARS.fd".to_string()
            };
            let vars =
                emu_config.guest.ovmf_code.parent().expect("ovmf has parent dir").join(vars_filename);
            if !vars.exists() {
                log::warn!("cannot find OVMF vars at {vars:?}");
            }
            emu_config.guest.ovmf_vars = vars;

            // If provided, pass the vbmeta signing key and metadata to the emulator config
            let vbmeta_key_filename = cmd.vbmeta_key(ctx)?.unwrap_or_default();
            if !vbmeta_key_filename.is_empty() {
                let p = PathBuf::from(vbmeta_key_filename);
                if p.exists() {
                    emu_config.guest.vbmeta_key_file = Some(p);
                } else {
                    log::warn!("cannot find PEM file at {p:?}");
                }
            }
            let vbmeta_metadata_filename = cmd.vbmeta_key_metadata(ctx)?.unwrap_or_default();
            if !vbmeta_metadata_filename.is_empty() {
                let p = PathBuf::from(vbmeta_metadata_filename);
                if p.exists() {
                    emu_config.guest.vbmeta_key_metadata_file = Some(p);
                } else {
                    log::warn!("cannot find key metadata file at {p:?}");
                }
            }
        }

        if emu_config.device.cpu.architecture == CpuArchitecture::Arm64 {
            emu_config.guest.secure_bootloader_arm64 = ffx_config::get_host_tool(&ctx, BOOTLOADER_SECURE_ARM64).ok();
        }
        emu_config
    };

    // HostConfig values that come from the OS environment.
    emu_config.host.os = std::env::consts::OS.to_string().into();
    emu_config.host.architecture = std::env::consts::ARCH.to_string().into();

    // Integrate the values from command line flags into the emulation configuration, and
    // return the result to the caller.
    apply_command_line_options(emu_config, cmd, emu_instances, ctx)
        .await
        .context("problem with apply command lines")
}

/// Given an EmulatorConfiguration and a StartCommand, write the values from the
/// StartCommand onto the EmulatorConfiguration, overriding any previous values.
#[allow(clippy::unused_async)] // TODO(https://fxbug.dev/386387845)
async fn apply_command_line_options(
    mut emu_config: EmulatorConfiguration,
    cmd: &StartCommand,
    emu_instances: &EmulatorInstances,
    ctx: &EnvironmentContext,
) -> Result<EmulatorConfiguration> {
    // Clone any fields that can simply copy over.
    emu_config.host.acceleration = cmd.accel.clone();

    // Process any values that are Options, have Auto values, or need any transformation.
    emu_config.host.gpu = GpuType::from_str(&cmd.gpu(ctx)?)?;
    emu_config.host.networking = NetworkingMode::from_str(&cmd.net(ctx)?)?;

    if let Some(log) = &cmd.log {
        // It'd be nice to canonicalize this path, to clean up relative bits like "..", but the
        // canonicalize method also checks for existence and symlinks, and we don't generally
        // expect the log file to exist ahead of time.
        emu_config.host.log = PathBuf::from(env::current_dir()?).join(log);
    } else {
        // TODO(https://fxbug.dev/42067481): Move logs to ffx log dir so `ffx doctor` collects them.
        let instance = emu_instances.get_instance_dir(&cmd.name(ctx)?, false)?;
        emu_config.host.log = instance.join("emulator.log");
    }

    if emu_config.host.acceleration == AccelerationMode::Auto {
        let check_kvm = emu_config.device.cpu.architecture == emu_config.host.architecture;

        match emu_config.host.os {
            OperatingSystem::Linux => {
                emu_config.host.acceleration = AccelerationMode::None;
                if check_kvm {
                    let path: String =
                        ctx.get(KVM_PATH).context("getting KVM path from ffx config")?;
                    match std::fs::OpenOptions::new().write(true).open(&path) {
                        Err(e) => match e.kind() {
                            std::io::ErrorKind::PermissionDenied => {
                                log::warn!(
                                    "No write permission on {}. Running without acceleration.",
                                    path
                                );
                                eprintln!(
                                    "Running without acceleration; emulator will be extremely \
                                    slow and may not establish a connection with ffx in the \
                                    allotted time.\n\n\
                                    Caused by: No write permission on {}.\n\n\
                                    To adjust permissions and enable acceleration via KVM:\n\n    \
                                    sudo usermod -a -G kvm $USER\n\n\
                                    You may need to reboot your machine for the permission change \
                                    to take effect.\n",
                                    path
                                );
                            }
                            std::io::ErrorKind::NotFound => {
                                log::info!(
                                    "KVM path {} does not exist. Running without acceleration.",
                                    path
                                );
                                eprintln!(
                                    "KVM path {path} does not exist. \
                                    Running without acceleration; emulator will be extremely \
                                    slow and may not establish a connection with ffx in the \
                                    allotted time.\n"
                                );
                            }
                            _ => bail!("Unknown error setting up acceleration: {:?}", e),
                        },
                        Ok(_) => emu_config.host.acceleration = AccelerationMode::Hyper,
                    }
                }
            }
            OperatingSystem::MacOS => {
                // We assume Macs always have HVF installed.
                emu_config.host.acceleration =
                    if check_kvm { AccelerationMode::Hyper } else { AccelerationMode::None };
            }
            _ => {
                // For anything else, acceleration is unsupported.
                emu_config.host.acceleration = AccelerationMode::None;
            }
        }
    }

    if emu_config.host.networking == NetworkingMode::Auto {
        if !cmd.additional_port_forwards.is_empty() {
            eprintln!("Port forwarding requested; defaulting to user-mode networking.");
            emu_config.host.networking = NetworkingMode::User;
        } else {
            let available = tap_available();
            if available.is_ok() {
                emu_config.host.networking = NetworkingMode::Tap;
                eprintln!(
                    "Auto resolving networking to tap-mode. For more information see \
                    https://fuchsia.dev/fuchsia-src/development/build/emulator#networking"
                );
            } else {
                log::debug!(
                    "Falling back on user-mode networking: {}",
                    available.as_ref().unwrap_err()
                );
                eprintln!(
                    "Auto resolving networking to user-mode. For more information see \
                    https://fuchsia.dev/fuchsia-src/development/build/emulator#networking"
                );
                emu_config.host.networking = NetworkingMode::User;
            }
        }
    }

    if emu_config.host.networking != NetworkingMode::User
        && !cmd.additional_port_forwards.is_empty()
    {
        bail!("Port forwarding is only supported with USER networking. Use '--net user' instead.");
    }

    // RuntimeConfig options, starting with simple copies.
    emu_config.runtime.debugger = cmd.debugger;
    emu_config.runtime.headless = cmd.headless;
    emu_config.runtime.startup_timeout = Duration::from_secs(cmd.startup_timeout(ctx)?);
    emu_config.runtime.smp = cmd.smp.clone();
    emu_config.runtime.hidpi_scaling = cmd.hidpi_scaling;
    emu_config.runtime.addl_kernel_args = cmd.kernel_args.clone();
    emu_config.runtime.name = cmd.name(ctx)?;
    emu_config.runtime.instance_directory =
        emu_instances.get_instance_dir(&emu_config.runtime.name, true)?;
    emu_config.runtime.reuse = cmd.reuse;

    // Collapsing multiple binary options into related fields.
    if cmd.console {
        emu_config.runtime.console = ConsoleType::Console;
    } else {
        emu_config.runtime.console = ConsoleType::None;
    }
    emu_config.runtime.log_level = if cmd.verbose { LogLevel::Verbose } else { LogLevel::Info };

    if emu_config.host.networking == NetworkingMode::User {
        for additional in &cmd.additional_port_forwards {
            let (name, guest) = parse_additional_port_forwards(additional)?;
            if emu_config.host.port_map.contains_key(&name) {
                bail!(
                    "Port '{}' is already defined. Cannot add it as an additional port forward.",
                    name
                );
            }
            emu_config.host.port_map.insert(name, PortMapping { host: None, guest });
        }
        // Reconcile the guest ports from device_spec with the host ports from the command line.
        if let Err(e) = parse_host_port_maps(&cmd.port_map, &mut emu_config) {
            bail!(
                "Problem parsing the port-map values from the command line. \
                Please check your spelling and syntax. {:?}",
                e
            );
        }
    } else {
        // If we're not running in user mode, we don't need a port map, so clear it.
        emu_config.host.port_map.clear();
    }

    // Any generated values or values from ffx_config.
    emu_config.runtime.mac_address = generate_mac_address(&cmd.name(ctx)?);
    let upscript: String =
        ctx.get(EMU_UPSCRIPT_FILE).context("Getting upscript path from ffx config")?;
    if !upscript.is_empty() {
        emu_config.runtime.upscript = Some(PathBuf::from(upscript));
    }

    if let Some(dev_config) = &cmd.dev_config {
        apply_dev_config(dev_config, &mut emu_config)?
    }

    Ok(emu_config)
}

fn apply_dev_config(
    dev_config: &PathBuf,
    emu_config: &mut EmulatorConfiguration,
) -> fho::Result<()> {
    #[derive(serde::Deserialize, Debug)]
    struct DeveloperConfig {
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        kernel_args: Vec<String>,
        #[serde(default)]
        env: HashMap<String, String>,
    }

    let f = File::open(dev_config).map_err(|e| user_error!("Error opening {dev_config:?}: {e}"))?;
    let data: DeveloperConfig =
        serde_json::from_reader(f).map_err(|e| user_error!("Error parsing {dev_config:?}: {e}"))?;

    emu_config.runtime.addl_emu_args.extend_from_slice(&data.args);
    emu_config.runtime.addl_kernel_args.extend_from_slice(&data.kernel_args);
    for (k, v) in data.env {
        emu_config.runtime.addl_env.insert(k, v);
    }

    Ok(())
}

/// Reconciles the host ports specified on the command line with the guest ports defined in the
/// device specification, mapping them together. If a host port is ill-formed or specified for a
/// guest port that was not defined, this returns an error and stops processing, so the state of
/// the port_map is undefined at that time. Duplicate ports are allowed but not advised, and a
/// warning will be logged for each occurrence.
fn parse_host_port_maps(
    flag_contents: &Vec<String>,
    emu_config: &mut EmulatorConfiguration,
) -> Result<()> {
    // At call time, the device_spec should already be parsed into the map, so this function
    // should, for each value in the Vector, check the name exists in the map and populate the host
    // value for the corresponding structure.
    for port_text in flag_contents {
        if let Ok((name, port)) = split_once(port_text, ":") {
            let mapping = emu_config.host.port_map.get_mut(&name);
            if mapping.is_none() {
                bail!(
                    "Command attempts to set port '{}', which is not defined by the device \
                    specification or additional-port-forwards. Only ports with names defined \
                    by the device specification or additional-port-forwards can be set. \
                    Terminating emulation.",
                    name
                );
            }
            let mapping = mapping.unwrap();
            if mapping.host.is_some() {
                log::warn!(
                    "Command line attempts to set the '{}' port more than once. This may \
                    lead to unexpected behavior. The previous entry will be discarded.",
                    name
                );
            }
            let value = port.parse::<u16>()?;
            mapping.host = Some(value);
        } else {
            bail!(
                "Invalid syntax for flag --port-map: '{}'. \
                The expected syntax is <name>:<port>, e.g. '--port-map ssh:8022'.",
                port_text
            );
        }
    }

    log::debug!("Port map parsed: {:?}\n", emu_config.host.port_map);

    Ok(())
}

fn parse_additional_port_forwards(val: &str) -> Result<(String, u16)> {
    let parts: Vec<&str> = val.split(':').collect();
    if parts.len() != 2 {
        bail!(
            "Invalid syntax for flag --additional-port-forwards: '{}'. \
            The expected syntax is <portname>:<guest_port>, e.g. '--additional-port-forwards myport:80'.",
            val
        );
    }
    let name = parts[0].to_string();
    if name.is_empty() {
        bail!("Port name cannot be empty in additional-port-forwards");
    }
    let guest: u16 = parts[1].parse().map_err(|e| {
        anyhow::anyhow!("Invalid guest port '{}' in additional-port-forwards: {}", parts[1], e)
    })?;
    if guest == 0 {
        bail!(
            "Invalid guest port '{}' in additional-port-forwards. Guest port must be non-zero.",
            guest
        );
    }
    Ok((name, guest))
}

/// Check if name follows the scheme "fuchsia-5254-Y-Z" where the last three sections represent a
/// valid mac address. If so, return X and Y as u8s, else None.
fn check_for_emulator_mac(name: &str) -> Option<Vec<u8>> {
    let re =
        Regex::new(r"fuchsia-5254-([0-9a-f]{2})([0-9a-f]{2})-([0-9a-f]{2})([0-9a-f]{2})$").unwrap();
    let mac = re.replace_all(name, "$1:$2:$3:$4");
    if name == mac {
        // The name does not follow the expected scheme, cannot parse an acceptable mac.
        None
    } else {
        let v: Vec<_> =
            mac.split(":").into_iter().map(|x| u8::from_str_radix(x, 16).unwrap()).collect();
        Some(v)
    }
}

/// Generate a unique MAC address based on the instance name. If using the default instance name
/// of fuchsia-emulator, this will be 52:54:47:5e:82:ef.
/// If the provided emulator name matches "fuchsia-5254-Y-Z" and the last three
/// sections can be converted into a valid MAC address, it will use this address directly.
///
/// Rationale for this behavior: We currently cannot persist the kernel parameter `zircon.nodename`
/// from a running Fuchsia system across reboots, but by passing in a kernel boot command line from
/// outside. In the case of performing an `fx ota` we typically do not have this kernel commandline
/// after the reboot. In the absence of a setting for `zircon.nodename`, the running Fuchsia will
/// call itself 'fuchsia-X-Y-Z' where X,Y,Z are derived from its mac address, and ffx will
/// recognise the target by this name. Thus, in order to ensure the target does not change its name
/// after `fx ota`, it needs to be named `fuchsia-X-Y-Z` upon startup. But we also must ensure that
/// the system's mac address is not created by hashing the string `fuchsia-X-Y-Z`, as this results
/// in a _different_ mac (as `X-Y-Z` does not hash to itself but to _different_ `P-Q-R`). This in
/// turn would cause the machine's name to change to `fuchsia-P-Q-R` where P,Q,R correspond to its
/// mac address and are different from X-Y-Z). Hence, if a machine name like "fuchsia-5254-Y-Z" and
/// Y,Z are hex numbers is provided, we use those as mac address directly, and skip hashing. This
/// results in the machine rebooting without change of mac address or nodename.
pub(crate) fn generate_mac_address(name: &str) -> String {
    let bytes = if let Some(v) = check_for_emulator_mac(name) {
        v
    } else {
        let mut hasher = DefaultHasher::new();
        hasher.write(name.as_bytes());
        let hashed = hasher.finish();
        hashed.to_be_bytes().to_vec()
    };
    format!("52:54:{:02x}:{:02x}:{:02x}:{:02x}", bytes[0], bytes[1], bytes[2], bytes[3])
}

pub(crate) async fn get_virtual_devices(product_bundle: &ProductBundle) -> Result<Vec<String>> {
    match &product_bundle {
        ProductBundle::V2(pb) => {
            // Determine the correct device name from the user, or default to the "recommended"
            // device, if one is provided in the product bundle.
            let path = pb.get_virtual_devices_path();
            let manifest = VirtualDeviceManifest::from_path(&path).context("manifest from_path")?;
            Ok(manifest.device_names())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use emulator_instance::{CpuArchitecture, PortMapping};

    use ffx_emulator_common::config::{
        EMU_DEFAULT_DEVICE, EMU_DEFAULT_ENGINE, EMU_DEFAULT_GPU, EMU_START_TIMEOUT,
    };
    use regex::Regex;

    use std::fs::create_dir_all;
    use std::io::Write;
    use tempfile::tempdir;

    #[fuchsia::test]
    async fn test_apply_command_line_options() -> Result<()> {
        let env = ffx_config::test_env().build().unwrap();
        let emulator_instances = EmulatorInstances::new(PathBuf::new());

        // Set up some test data to be applied.
        let mut cmd = StartCommand {
            accel: AccelerationMode::Hyper,
            config: Some(PathBuf::from("/path/to/template")),
            console: true,
            debugger: true,
            gpu: Some(String::from("host")),
            headless: true,
            hidpi_scaling: true,
            log: Some(PathBuf::from("/path/to/log")),
            name: Some("SomeName".to_string()),
            net: Some("tap".to_string()),
            verbose: true,
            ..Default::default()
        };

        // Get a default configuration, and verify we know what those values are.
        let emu_config = EmulatorConfiguration::default();
        assert_eq!(emu_config.host.acceleration, AccelerationMode::None);
        assert_eq!(emu_config.host.gpu, GpuType::SwiftshaderIndirect);
        assert_eq!(emu_config.host.log, PathBuf::from(""));
        assert_eq!(emu_config.host.networking, NetworkingMode::Auto);
        assert_eq!(emu_config.runtime.console, ConsoleType::None);
        assert_eq!(emu_config.runtime.debugger, false);
        assert_eq!(emu_config.runtime.headless, false);
        assert_eq!(emu_config.runtime.hidpi_scaling, false);
        assert_eq!(emu_config.runtime.log_level, LogLevel::Info);
        assert_eq!(emu_config.runtime.name, "");
        assert_eq!(emu_config.runtime.upscript, None);

        // Apply the test data, which should change everything in the config.
        let opts =
            apply_command_line_options(emu_config.clone(), &cmd, &emulator_instances, &env.context)
                .await?;
        assert_eq!(opts.host.acceleration, AccelerationMode::Hyper);
        assert_eq!(opts.host.gpu, GpuType::HostExperimental);
        assert_eq!(opts.host.log, PathBuf::from("/path/to/log"));
        assert_eq!(opts.host.networking, NetworkingMode::Tap);
        assert_eq!(opts.runtime.console, ConsoleType::Console);
        assert_eq!(opts.runtime.debugger, true);
        assert_eq!(opts.runtime.headless, true);
        assert_eq!(opts.runtime.hidpi_scaling, true);
        assert_eq!(opts.runtime.log_level, LogLevel::Verbose);
        assert_eq!(opts.runtime.name, "SomeName");
        assert_eq!(opts.runtime.upscript, None);

        let env2 = ffx_config::test_env()
            .user_config(EMU_UPSCRIPT_FILE, "/path/to/upscript")
            .build()
            .unwrap();

        let opts =
            apply_command_line_options(opts, &cmd, &emulator_instances, &env2.context).await?;
        assert_eq!(opts.runtime.upscript, Some(PathBuf::from("/path/to/upscript")));

        // Test relative file paths
        let temp_path = PathBuf::from(tempdir().unwrap().path());
        let long_path = temp_path.join("longer/path/to/files");
        create_dir_all(&long_path)?;
        // Set the CWD to the temp directory
        let cwd = env::current_dir().context("Error getting cwd in test")?;
        env::set_current_dir(&temp_path).context("Error setting cwd in test")?;

        cmd.log = Some(PathBuf::from("tmp.log"));
        let result =
            apply_command_line_options(emu_config.clone(), &cmd, &emulator_instances, &env.context)
                .await;
        assert!(result.is_ok(), "{:?}", result.err());
        let opts = result.unwrap();
        assert_eq!(opts.host.log, temp_path.join("tmp.log"));

        cmd.log = Some(PathBuf::from("relative/path/to/emulator.file"));
        let result =
            apply_command_line_options(emu_config.clone(), &cmd, &emulator_instances, &env.context)
                .await;
        assert!(result.is_ok(), "{:?}", result.err());
        let opts = result.unwrap();
        assert_eq!(opts.host.log, temp_path.join("relative/path/to/emulator.file"));

        // Set the CWD to the longer directory, so we can test ".."
        env::set_current_dir(&long_path).context("Error setting cwd in test")?;
        cmd.log = Some(PathBuf::from("../other/file.log"));
        let result =
            apply_command_line_options(emu_config.clone(), &cmd, &emulator_instances, &env.context)
                .await;
        assert!(result.is_ok(), "{:?}", result.err());
        let opts = result.unwrap();
        // As mentioned in the code, it'd be nice to canonicalize this, but since the file doesn't
        // already exist that would lead to failures.
        assert_eq!(opts.host.log, temp_path.join("longer/path/to/files/../other/file.log"));

        // Test absolute path
        cmd.log = Some(long_path.join("absolute.file"));
        let result =
            apply_command_line_options(emu_config.clone(), &cmd, &emulator_instances, &env.context)
                .await;
        assert!(result.is_ok(), "{:?}", result.err());
        let opts = result.unwrap();
        assert_eq!(opts.host.log, long_path.join("absolute.file"));

        env::set_current_dir(cwd).context("Revert to previous CWD")?;

        // Test additional_port_forwards
        let mut cmd_with_ports = StartCommand {
            net: Some("user".to_string()),
            additional_port_forwards: vec!["port1:80".to_string(), "port2:90".to_string()],
            ..Default::default()
        };
        let opts = apply_command_line_options(
            emu_config.clone(),
            &cmd_with_ports,
            &emulator_instances,
            &env.context,
        )
        .await?;
        assert_eq!(opts.host.networking, NetworkingMode::User);
        assert_eq!(
            opts.host.port_map.get("port1").unwrap(),
            &PortMapping { host: None, guest: 80 }
        );
        assert_eq!(
            opts.host.port_map.get("port2").unwrap(),
            &PortMapping { host: None, guest: 90 }
        );

        // Test additional_port_forwards with multiple flags and port_map override
        let cmd_multiple_flags = StartCommand {
            net: Some("user".to_string()),
            additional_port_forwards: vec!["port1:80".to_string(), "port2:90".to_string()],
            port_map: vec!["port1:8080".to_string()],
            ..Default::default()
        };
        let opts = apply_command_line_options(
            emu_config.clone(),
            &cmd_multiple_flags,
            &emulator_instances,
            &env.context,
        )
        .await?;
        assert_eq!(opts.host.networking, NetworkingMode::User);
        assert_eq!(
            opts.host.port_map.get("port1").unwrap(),
            &PortMapping { host: Some(8080), guest: 80 }
        );
        assert_eq!(
            opts.host.port_map.get("port2").unwrap(),
            &PortMapping { host: None, guest: 90 }
        );

        // Test additional_port_forwards with invalid format (comma-separated is no longer supported)
        cmd_with_ports.additional_port_forwards = vec!["port1:80,port2:90".to_string()];
        let result = apply_command_line_options(
            emu_config.clone(),
            &cmd_with_ports,
            &emulator_instances,
            &env.context,
        )
        .await;
        assert!(result.is_err());

        // Test additional_port_forwards with zero guest port
        cmd_with_ports.additional_port_forwards = vec!["port1:0".to_string()];
        let result = apply_command_line_options(
            emu_config.clone(),
            &cmd_with_ports,
            &emulator_instances,
            &env.context,
        )
        .await;
        assert!(result.is_err());

        // Test additional_port_forwards with auto networking (defaults to User)
        let cmd_auto_net = StartCommand {
            net: None,
            additional_port_forwards: vec!["port1:80".to_string()],
            ..Default::default()
        };
        let opts = apply_command_line_options(
            emu_config.clone(),
            &cmd_auto_net,
            &emulator_instances,
            &env.context,
        )
        .await?;
        assert_eq!(opts.host.networking, NetworkingMode::User);
        assert!(opts.host.port_map.get("port1").is_some());

        // Test additional_port_forwards with tap networking (should fail)
        let cmd_tap_net = StartCommand {
            net: Some("tap".to_string()),
            additional_port_forwards: vec!["port1:80".to_string()],
            ..Default::default()
        };
        let result = apply_command_line_options(
            emu_config.clone(),
            &cmd_tap_net,
            &emulator_instances,
            &env.context,
        )
        .await;
        assert!(result.is_err());

        Ok(())
    }

    #[fuchsia::test]
    async fn test_config_backed_values() -> Result<()> {
        let env = ffx_config::test_env().build().unwrap();
        let mut cmd = StartCommand::default();
        let emu_config = EmulatorConfiguration::default();

        let emu_instances = EmulatorInstances::new(PathBuf::new());

        assert_eq!(cmd.device(&env.context).unwrap(), Some(String::from("")));
        assert_eq!(cmd.engine(&env.context).unwrap(), "femu");
        assert_eq!(cmd.gpu(&env.context).unwrap(), "swiftshader_indirect");
        assert_eq!(cmd.startup_timeout(&env.context).unwrap(), 60);

        let result =
            apply_command_line_options(emu_config.clone(), &cmd, &emu_instances, &env.context)
                .await;
        assert!(result.is_ok(), "{:?}", result.err());
        let opts = result.unwrap();
        assert_eq!(opts.host.gpu, GpuType::SwiftshaderIndirect);

        let env2 = ffx_config::test_env()
            .user_config(EMU_DEFAULT_DEVICE, "my_device")
            .user_config(EMU_DEFAULT_ENGINE, "qemu")
            .user_config(EMU_DEFAULT_GPU, "host")
            .user_config(EMU_START_TIMEOUT, 120)
            .build()
            .unwrap();

        assert_eq!(cmd.device(&env2.context).unwrap(), Some(String::from("my_device")));
        assert_eq!(cmd.engine(&env2.context).unwrap(), "qemu");
        assert_eq!(cmd.gpu(&env2.context).unwrap(), "host");
        assert_eq!(cmd.startup_timeout(&env2.context).unwrap(), 120);

        let result =
            apply_command_line_options(emu_config.clone(), &cmd, &emu_instances, &env2.context)
                .await;
        assert!(result.is_ok(), "{:?}", result.err());
        let opts = result.unwrap();
        assert_eq!(opts.host.gpu, GpuType::HostExperimental);

        cmd.gpu = Some(String::from("swiftshader_indirect"));

        assert_eq!(cmd.gpu(&env2.context).unwrap(), "swiftshader_indirect");
        let result =
            apply_command_line_options(emu_config.clone(), &cmd, &emu_instances, &env2.context)
                .await;
        assert!(result.is_ok(), "{:?}", result.err());
        let opts = result.unwrap();
        assert_eq!(opts.host.gpu, GpuType::SwiftshaderIndirect);

        Ok(())
    }

    #[fuchsia::test]
    async fn test_accel_auto() -> Result<()> {
        let temp_path = PathBuf::from(tempdir().unwrap().path());
        let file_path = temp_path.join("kvm");
        create_dir_all(&temp_path).expect("Create all temp directory");
        // The file at KVM_PATH is only tested for writability. It need not have contents.
        let file = File::create(&file_path).expect("Create temp file");
        let mut perms = file.metadata().expect("Get file metadata").permissions();

        let env = ffx_config::test_env()
            .user_config(KVM_PATH, file_path.to_str().unwrap())
            .build()
            .unwrap();
        let emu_instances = EmulatorInstances::new(temp_path.clone());

        // Set up some test data to be applied.
        let cmd = StartCommand { accel: AccelerationMode::Auto, ..Default::default() };
        let mut emu_config = EmulatorConfiguration::default();
        emu_config.host.os = OperatingSystem::Linux;

        perms.set_readonly(false);
        assert!(file.set_permissions(perms.clone()).is_ok());

        emu_config.device.cpu.architecture = CpuArchitecture::X64;
        emu_config.host.architecture = CpuArchitecture::X64;
        let opts =
            apply_command_line_options(emu_config.clone(), &cmd, &emu_instances, &env.context)
                .await?;
        assert_eq!(opts.host.acceleration, AccelerationMode::Hyper);

        emu_config.device.cpu.architecture = CpuArchitecture::X64;
        emu_config.host.architecture = CpuArchitecture::Arm64;
        let opts =
            apply_command_line_options(emu_config.clone(), &cmd, &emu_instances, &env.context)
                .await?;
        assert_eq!(opts.host.acceleration, AccelerationMode::None);

        emu_config.device.cpu.architecture = CpuArchitecture::Arm64;
        emu_config.host.architecture = CpuArchitecture::Arm64;
        let opts =
            apply_command_line_options(emu_config.clone(), &cmd, &emu_instances, &env.context)
                .await?;
        assert_eq!(opts.host.acceleration, AccelerationMode::Hyper);

        emu_config.device.cpu.architecture = CpuArchitecture::Arm64;
        emu_config.host.architecture = CpuArchitecture::X64;
        let opts =
            apply_command_line_options(emu_config.clone(), &cmd, &emu_instances, &env.context)
                .await?;
        assert_eq!(opts.host.acceleration, AccelerationMode::None);

        perms.set_readonly(true);
        assert!(file.set_permissions(perms.clone()).is_ok());

        emu_config.device.cpu.architecture = CpuArchitecture::X64;
        emu_config.host.architecture = CpuArchitecture::X64;
        let opts =
            apply_command_line_options(emu_config.clone(), &cmd, &emu_instances, &env.context)
                .await?;
        assert_eq!(opts.host.acceleration, AccelerationMode::None);

        emu_config.device.cpu.architecture = CpuArchitecture::X64;
        emu_config.host.architecture = CpuArchitecture::Arm64;
        let opts =
            apply_command_line_options(emu_config.clone(), &cmd, &emu_instances, &env.context)
                .await?;
        assert_eq!(opts.host.acceleration, AccelerationMode::None);

        emu_config.device.cpu.architecture = CpuArchitecture::Arm64;
        emu_config.host.architecture = CpuArchitecture::Arm64;
        let opts =
            apply_command_line_options(emu_config.clone(), &cmd, &emu_instances, &env.context)
                .await?;
        assert_eq!(opts.host.acceleration, AccelerationMode::None);

        emu_config.device.cpu.architecture = CpuArchitecture::Arm64;
        emu_config.host.architecture = CpuArchitecture::X64;
        let opts =
            apply_command_line_options(emu_config.clone(), &cmd, &emu_instances, &env.context)
                .await?;
        assert_eq!(opts.host.acceleration, AccelerationMode::None);

        emu_config.host.os = OperatingSystem::MacOS;

        emu_config.device.cpu.architecture = CpuArchitecture::X64;
        emu_config.host.architecture = CpuArchitecture::X64;
        let opts =
            apply_command_line_options(emu_config.clone(), &cmd, &emu_instances, &env.context)
                .await?;
        assert_eq!(opts.host.acceleration, AccelerationMode::Hyper);

        emu_config.device.cpu.architecture = CpuArchitecture::X64;
        emu_config.host.architecture = CpuArchitecture::Arm64;
        let opts =
            apply_command_line_options(emu_config.clone(), &cmd, &emu_instances, &env.context)
                .await?;
        assert_eq!(opts.host.acceleration, AccelerationMode::None);

        emu_config.device.cpu.architecture = CpuArchitecture::Arm64;
        emu_config.host.architecture = CpuArchitecture::Arm64;
        let opts =
            apply_command_line_options(emu_config.clone(), &cmd, &emu_instances, &env.context)
                .await?;
        assert_eq!(opts.host.acceleration, AccelerationMode::Hyper);

        emu_config.device.cpu.architecture = CpuArchitecture::Arm64;
        emu_config.host.architecture = CpuArchitecture::X64;
        let opts =
            apply_command_line_options(emu_config.clone(), &cmd, &emu_instances, &env.context)
                .await?;
        assert_eq!(opts.host.acceleration, AccelerationMode::None);

        Ok(())
    }

    #[test]
    fn test_generate_mac_address() -> Result<()> {
        let regex = Regex::new(
            r"^[[:xdigit:]]{2}:[[:xdigit:]]{2}:[[:xdigit:]]{2}:[[:xdigit:]]{2}:[[:xdigit:]]{2}:[[:xdigit:]]{2}$",
        )?;
        // Make sure we can generate the documented mac for the default string.
        // Generally we don't want to lock in implementation details but this value is included in
        // the comments, so expect to change this test case and those comments if you change the
        // generation routine.
        assert_eq!(generate_mac_address("fuchsia-emulator"), "52:54:47:5e:82:ef".to_string());

        // Make sure two reasonable names return valid MAC addresses and don't conflict.
        let first = generate_mac_address("emulator1");
        let second = generate_mac_address("emulator2");
        assert!(regex.is_match(&first), "{:?} isn't a valid MAC address", first);
        assert!(regex.is_match(&second), "{:?} isn't a valid MAC address", second);
        assert_ne!(first, second);

        // Make sure the same name generates the same MAC address when called multiple times (idempotency).
        let first = generate_mac_address("emulator");
        let second = generate_mac_address("emulator");
        assert!(regex.is_match(&first), "{:?} isn't a valid MAC address", first);
        assert_eq!(first, second);

        // We shouldn't run with an empty name, but we don't want the function to fail even if the
        // name is empty.
        let first = generate_mac_address("");
        assert!(regex.is_match(&first), "{:?} isn't a valid MAC address", first);

        // Make sure a name following the scheme "fuchsia-5254-Y-Z" returns the embedded mac address
        let first = generate_mac_address("fuchsia-5254-1234-abcd");
        assert_eq!(first, "52:54:12:34:ab:cd");

        Ok(())
    }

    #[test]
    fn test_parse_host_port_maps() {
        let mut emu_config = EmulatorConfiguration::default();
        let mut flag_contents;

        // No guest ports or hosts, expect success and empty map.
        emu_config.host.port_map = HashMap::new();
        flag_contents = vec![];
        let result = parse_host_port_maps(&flag_contents, &mut emu_config);
        assert!(result.is_ok(), "{:?}", result);
        assert_eq!(emu_config.host.port_map.len(), 0);

        // No guest ports, empty string for hosts, expect failure because it can't split an empty
        // host string.
        emu_config.host.port_map = HashMap::new();
        flag_contents = vec!["".to_string()];
        assert!(parse_host_port_maps(&flag_contents, &mut emu_config).is_err());

        // No guest ports, one host port, expect failure.
        flag_contents = vec!["ssh:1234".to_string()];
        assert!(parse_host_port_maps(&flag_contents, &mut emu_config).is_err());

        // One guest port, no host port, expect success.
        flag_contents = vec![];
        emu_config.host.port_map.clear();
        emu_config.host.port_map.insert("ssh".to_string(), PortMapping { host: None, guest: 22 });
        let result = parse_host_port_maps(&flag_contents, &mut emu_config);
        assert!(result.is_ok(), "{:?}", result);
        assert_eq!(emu_config.host.port_map.len(), 1);
        assert_eq!(
            emu_config.host.port_map.remove("ssh").unwrap(),
            PortMapping { host: None, guest: 22 }
        );

        // Single guest port, single host port, same name.
        flag_contents = vec!["ssh:1234".to_string()];
        emu_config.host.port_map.clear();
        emu_config.host.port_map.insert("ssh".to_string(), PortMapping { host: None, guest: 22 });
        let result = parse_host_port_maps(&flag_contents, &mut emu_config);
        assert!(result.is_ok(), "{:?}", result);
        assert_eq!(emu_config.host.port_map.len(), 1);
        assert_eq!(
            emu_config.host.port_map.remove("ssh").unwrap(),
            PortMapping { host: Some(1234), guest: 22 }
        );

        // Multiple guest ports, single host port, same name.
        flag_contents = vec!["ssh:1234".to_string()];
        emu_config.host.port_map.clear();
        emu_config.host.port_map.insert("ssh".to_string(), PortMapping { host: None, guest: 22 });
        emu_config
            .host
            .port_map
            .insert("debug".to_string(), PortMapping { host: None, guest: 2345 });
        emu_config
            .host
            .port_map
            .insert("mdns".to_string(), PortMapping { host: None, guest: 5353 });
        let result = parse_host_port_maps(&flag_contents, &mut emu_config);
        assert!(result.is_ok(), "{:?}", result);
        assert_eq!(emu_config.host.port_map.len(), 3);
        assert_eq!(
            emu_config.host.port_map.remove("ssh").unwrap(),
            PortMapping { host: Some(1234), guest: 22 }
        );
        assert_eq!(
            emu_config.host.port_map.remove("debug").unwrap(),
            PortMapping { host: None, guest: 2345 }
        );
        assert_eq!(
            emu_config.host.port_map.remove("mdns").unwrap(),
            PortMapping { host: None, guest: 5353 }
        );

        // Multiple guest port, multiple but not all host ports.
        flag_contents = vec!["ssh:1234".to_string(), "mdns:1236".to_string()];
        emu_config.host.port_map.clear();
        emu_config.host.port_map.insert("ssh".to_string(), PortMapping { host: None, guest: 22 });
        emu_config
            .host
            .port_map
            .insert("debug".to_string(), PortMapping { host: None, guest: 2345 });
        emu_config
            .host
            .port_map
            .insert("mdns".to_string(), PortMapping { host: None, guest: 5353 });
        let result = parse_host_port_maps(&flag_contents, &mut emu_config);
        assert!(result.is_ok(), "{:?}", result);
        assert_eq!(emu_config.host.port_map.len(), 3);
        assert_eq!(
            emu_config.host.port_map.remove("ssh").unwrap(),
            PortMapping { host: Some(1234), guest: 22 }
        );
        assert_eq!(
            emu_config.host.port_map.remove("debug").unwrap(),
            PortMapping { host: None, guest: 2345 }
        );
        assert_eq!(
            emu_config.host.port_map.remove("mdns").unwrap(),
            PortMapping { host: Some(1236), guest: 5353 }
        );

        // Multiple guest port, all matching host ports.
        flag_contents =
            vec!["ssh:1234".to_string(), "debug:1235".to_string(), "mdns:1236".to_string()];
        emu_config.host.port_map.clear();
        emu_config.host.port_map.insert("ssh".to_string(), PortMapping { host: None, guest: 22 });
        emu_config
            .host
            .port_map
            .insert("debug".to_string(), PortMapping { host: None, guest: 2345 });
        emu_config
            .host
            .port_map
            .insert("mdns".to_string(), PortMapping { host: None, guest: 5353 });
        let result = parse_host_port_maps(&flag_contents, &mut emu_config);
        assert!(result.is_ok(), "{:?}", result);
        assert_eq!(emu_config.host.port_map.len(), 3);
        assert_eq!(
            emu_config.host.port_map.remove("ssh").unwrap(),
            PortMapping { host: Some(1234), guest: 22 }
        );
        assert_eq!(
            emu_config.host.port_map.remove("debug").unwrap(),
            PortMapping { host: Some(1235), guest: 2345 }
        );
        assert_eq!(
            emu_config.host.port_map.remove("mdns").unwrap(),
            PortMapping { host: Some(1236), guest: 5353 }
        );

        // Multiple guest ports, extra host port, expect failure.
        flag_contents = vec![
            "ssh:1234".to_string(),
            "debug:1235".to_string(),
            "mdns:1236".to_string(),
            "undefined:1237".to_string(),
        ];
        emu_config.host.port_map.clear();
        emu_config.host.port_map.insert("ssh".to_string(), PortMapping { host: None, guest: 22 });
        emu_config
            .host
            .port_map
            .insert("debug".to_string(), PortMapping { host: None, guest: 2345 });
        emu_config
            .host
            .port_map
            .insert("mdns".to_string(), PortMapping { host: None, guest: 5353 });
        assert!(parse_host_port_maps(&flag_contents, &mut emu_config).is_err());

        // Duplicated host port specifications, expect success with earlier values discarded.
        flag_contents =
            vec!["ssh:9021".to_string(), "ssh:1984".to_string(), "ssh:8022".to_string()];
        emu_config.host.port_map.clear();
        emu_config.host.port_map.insert("ssh".to_string(), PortMapping { host: None, guest: 22 });
        let result = parse_host_port_maps(&flag_contents, &mut emu_config);
        assert!(result.is_ok(), "{:?}", result);
        assert_eq!(emu_config.host.port_map.len(), 1);
        assert_eq!(
            emu_config.host.port_map.remove("ssh").unwrap(),
            PortMapping { host: Some(8022), guest: 22 }
        );

        // Ill-formed flag contents, expect failure.
        emu_config.host.port_map.clear();
        emu_config.host.port_map.insert("ssh".to_string(), PortMapping { host: None, guest: 22 });
        flag_contents = vec!["ssh=9021".to_string()];
        assert!(parse_host_port_maps(&flag_contents, &mut emu_config).is_err());
        flag_contents = vec!["ssh:port1".to_string()];
        assert!(parse_host_port_maps(&flag_contents, &mut emu_config).is_err());
        flag_contents = vec!["ssh 1234".to_string()];
        assert!(parse_host_port_maps(&flag_contents, &mut emu_config).is_err());
        flag_contents = vec!["1234:ssh".to_string()];
        assert!(parse_host_port_maps(&flag_contents, &mut emu_config).is_err());
        flag_contents = vec!["ssh".to_string()];
        assert!(parse_host_port_maps(&flag_contents, &mut emu_config).is_err());
        flag_contents = vec!["1234".to_string()];
        assert!(parse_host_port_maps(&flag_contents, &mut emu_config).is_err());
    }

    #[test]
    fn test_apply_dev_config_empty() {
        let mut emu_config = EmulatorConfiguration::default();
        let mut dev_config = tempfile::NamedTempFile::new().expect("temp file");
        writeln!(dev_config, "{{}}").expect("empty dev-config");
        apply_dev_config(&dev_config.path().to_path_buf(), &mut emu_config)
            .expect("apply_dev_config ok");
        assert!(emu_config.runtime.addl_emu_args.is_empty());
        assert!(emu_config.runtime.addl_kernel_args.is_empty());
        assert!(emu_config.runtime.addl_env.is_empty());
    }

    #[test]
    fn test_apply_dev_config_something() {
        let mut emu_config = EmulatorConfiguration::default();
        let mut dev_config = tempfile::NamedTempFile::new().expect("temp file");
        write!(
            dev_config,
            r#"{{
                "args": [ "-some-arg"],
                "kernel_args": ["-karg"],
                "env" : {{
                    "a-key": "a-value"
                }}
            }}"#
        )
        .expect("dev-config contents");
        apply_dev_config(&dev_config.path().to_path_buf(), &mut emu_config)
            .expect("apply_dev_config ok");
        assert_eq!(emu_config.runtime.addl_emu_args, vec!["-some-arg"]);
        assert_eq!(emu_config.runtime.addl_kernel_args, vec!["-karg"]);
        assert_eq!(emu_config.runtime.addl_env.get("a-key").unwrap(), "a-value");
    }

    #[test]
    fn test_check_for_emulator_mac_conforming_to_scheme() {
        let name = "fuchsia-5254-0123-cdef";
        let v = vec![0x01, 0x23, 0xcd, 0xef];
        assert_eq!(check_for_emulator_mac(name), Some(v));
    }

    #[test]
    fn test_check_for_emulator_mac_almost_conforming_to_scheme() {
        let name = "fuchsia-5452-0123-cdef";
        assert!(check_for_emulator_mac(name).is_none());
    }

    #[test]
    fn test_check_for_emulator_mac_arbitrary_name() {
        let name = "New_York_Rio_Tokyo";
        assert!(check_for_emulator_mac(name).is_none());
    }

    #[test]
    fn test_parse_additional_port_forwards() {
        assert_eq!(parse_additional_port_forwards("port1:80").unwrap(), ("port1".to_string(), 80));
        assert!(parse_additional_port_forwards("port1").is_err());
        assert!(parse_additional_port_forwards("port1:").is_err());
        assert!(parse_additional_port_forwards(":80").is_err());
        assert!(parse_additional_port_forwards("port1:0").is_err());
        assert!(parse_additional_port_forwards("port1:port").is_err());
        assert!(parse_additional_port_forwards("port1:80:extra").is_err());
    }
}
