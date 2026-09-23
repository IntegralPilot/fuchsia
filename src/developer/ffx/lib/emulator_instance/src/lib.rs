// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use schemars::JsonSchema;
pub use sdk_metadata::{AudioDevice, DataAmount, DataUnits, PointingDevice, Screen, VsockDevice};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::time::Duration;
use thiserror::Error;

mod enumerations;
pub mod fletcher64;
mod instances;
pub mod targets;

pub use enumerations::{
    AccelerationMode, ConsoleType, CpuArchitecture, EngineState, EngineType, GpuType, LogLevel,
    NetworkingMode, OperatingSystem, VirtualCpu,
};
#[derive(Error, Debug)]
pub enum EmulatorInstanceError {
    #[error("Could not calculate hash for {path:?}: {source}")]
    HashCalculation { path: PathBuf, source: std::io::Error },

    #[error("Kernel file {0:?} does not exist.")]
    MissingKernel(PathBuf),

    #[error("FAT file {0:?} does not exist.")]
    MissingFatImage(PathBuf),

    #[error("Full GPT disk file {0:?} does not exist.")]
    MissingGptImage(PathBuf),

    #[error("No kernel file or bootloader file configured.")]
    MissingConfiguration,

    #[error("Ramdisk {0:?} does not exist.")]
    MissingRamdisk(PathBuf),

    #[error("Disk image file {0:?} does not exist.")]
    MissingDiskImage(PathBuf),

    #[error("Failed to remove directory {path:?}: {source}")]
    RemoveDirectory { path: PathBuf, source: std::io::Error },

    #[error("Unable to open file {path:?}: {source}")]
    OpenFile { path: PathBuf, source: std::io::Error },

    #[error("Invalid JSON syntax in {path:?}: {source}")]
    ParseJson { path: PathBuf, source: serde_json::Error },

    #[error("Engine file doesn't exist at {0:?}")]
    MissingEngineFile(PathBuf),

    #[error("Unable to create file {path:?}: {source}")]
    CreateFile { path: PathBuf, source: std::io::Error },

    #[error("Failed to serialize JSON: {0}")]
    SerializeJson(#[from] serde_json::Error),

    #[error("Failed to create directory {path:?}: {source}")]
    CreateDirectory { path: PathBuf, source: std::io::Error },

    #[error("Failed to create watcher: {0}")]
    WatcherCreation(#[from] notify::Error),

    #[error("Failed to watch directory {path:?}: {source}")]
    WatchDirectory { path: PathBuf, source: notify::Error },

    #[error("Failed to send event: {0}")]
    SendError(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, EmulatorInstanceError>;

use fletcher64::get_file_hash;
pub use instances::{EmulatorInstances, read_from_disk, read_from_disk_untyped, write_to_disk};
pub use targets::{
    EmulatorTargetAction, EmulatorWatcher, get_all_targets, get_target, instance_name_from_path,
    start_emulator_watching,
};

/// Holds a single mapping from a host port to the guest.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize, JsonSchema)]
pub struct PortMapping {
    pub guest: u16,
    pub host: Option<u16>,
}

/// Used when reading the instance data as a return value.
#[derive(Debug)]
pub enum EngineOption {
    DoesExist(Box<EmulatorInstanceData>),
    DoesNotExist(String),
}

pub trait EmulatorInstanceInfo {
    fn get_name(&self) -> &str;
    fn is_running(&self) -> bool;
    fn get_engine_state(&self) -> EngineState;
    fn get_engine_type(&self) -> EngineType;
    fn get_pid(&self) -> u32;
    fn get_emulator_configuration(&self) -> &EmulatorConfiguration;
    fn get_emulator_configuration_mut(&mut self) -> &mut EmulatorConfiguration;
    fn get_emulator_binary(&self) -> &PathBuf;
    fn get_networking_mode(&self) -> &NetworkingMode;
    fn get_ssh_port(&self) -> Option<u16>;
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize, JsonSchema)]
pub struct FlagData {
    /// Arguments. The set of flags which follow the "-fuchsia" option. These are not processed by
    /// Femu, but are passed through to Qemu.
    pub args: Vec<String>,

    /// Environment Variables. These are not passed on the command line, but are set in the
    /// process's environment before execution.
    pub envs: HashMap<String, String>,

    /// Features. A Femu-only field. Features are the first set of command line flags passed to the
    /// Femu binary. These are single words, capitalized, comma-separated, and immediately follow
    /// the flag "-feature".
    pub features: Vec<String>,

    /// Kernel Arguments. The last part of the command line. A set of text values that are passed
    /// through the emulator executable directly to the guest system's kernel.
    pub kernel_args: Vec<String>,

    /// Options. A Femu-only field. Options come immediately after features. Options may be boolean
    /// flags (e.g. -no-hidpi-scaling) or have associated values (e.g. -window-size 1280x800).
    pub options: Vec<String>,
}

/// A pre-formatted disk image to be used by the emulator
///
/// The disk images may contain the base packages of the system, or multiple data partitions.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub enum DiskImage {
    Fat(PathBuf),
    Fvm(PathBuf),
    Fxfs(PathBuf),
    Gpt(PathBuf),
}

impl AsRef<Path> for DiskImage {
    fn as_ref(&self) -> &Path {
        self.as_path()
    }
}

impl Deref for DiskImage {
    type Target = PathBuf;

    fn deref(&self) -> &Self::Target {
        match self {
            DiskImage::Fat(path) => path,
            DiskImage::Fvm(path) => path,
            DiskImage::Fxfs(path) => path,
            DiskImage::Gpt(path) => path,
        }
    }
}

/// The ramdisk to provide to the emulator.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Ramdisk {
    pub path: PathBuf,
    pub kind: RamdiskKind,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RamdiskKind {
    /// Corresponds to an arbitrary ZBI to be booted in the normal workflow.
    #[default]
    Zbi,

    /// Indicates an opaque ramdisk intended for testing, not to be modified.
    Test,
}

/// Image files and other information specific to the guest OS.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct GuestConfig {
    /// The guest's virtual storage device.
    pub disk_image: Option<DiskImage>,

    /// The Fuchsia kernel, which loads alongside the ZBI and brings up the OS.
    /// This can also be a efi image, in which case there is no zbi needed.
    /// There can also be bootloader images (.fatfs) that boot directly and do not
    /// require a kernel.
    pub kernel_image: Option<PathBuf>,

    /// The ramdisk to be booted with the provided kernel.
    /// Note: This may be absent (e.g., when booting a boot shim test or a EFI
    /// disk image).
    pub ramdisk: Option<Ramdisk>,

    /// Hash of zbi or kernel if the kernel is efi. Used to detect changes when reusing
    /// an emulator instance.
    #[serde(default)]
    pub zbi_hash: String,

    /// Path to a PEM style key file. This is used when re-signing a vbmeta file for a modified ZBI.
    #[serde(default)]
    pub vbmeta_key_file: Option<PathBuf>,

    // Path to the key metadata. This is required when a PEM file is used to re-sign a vbmeta file.
    #[serde(default)]
    pub vbmeta_key_metadata_file: Option<PathBuf>,

    /// Hash of disk_image. Used to detect changes when reusing an emulator instance.
    #[serde(default)]
    pub disk_hash: String,

    /// Firmware emulation files, primarily used only if the guest is efi. The code is usually
    /// read-only.
    #[serde(default)]
    pub ovmf_code: PathBuf,

    /// Firmware emulation data file. This is read-write, so it should be unique per emulator instance.
    #[serde(default)]
    pub ovmf_vars: PathBuf,

    /// And arm64 bootloader that runs in EL3 and is capable of ultimately
    /// loading -kernel and -initrd QEMU specifications via Linux's boot
    /// protocol.
    #[serde(default)]
    pub secure_bootloader_arm64: Option<PathBuf>,

    /// Path to the product bundle from where the emulator is staged.
    /// TODO(https://fxbug.dev/381263769): Note that this is passed to make-fuchsia-vol for
    /// constructing GPT images until a better solution is in place. Please avoid using it, as it
    /// may go away without warning.
    #[serde(default)]
    pub product_bundle_path: Option<PathBuf>,

    /// Whether this guest is running from a GPT partitioned full disk image.
    /// Note that this is used to pass the info whether this is a GPT-image based instance to the
    /// arguments template parsing in arg_templates.rs as it cannot call `is_gpt()`.
    #[serde(default)]
    pub is_gpt: bool,
}

impl GuestConfig {
    pub fn is_efi(&self) -> bool {
        match &self.kernel_image {
            Some(file_path) => file_path.extension().unwrap_or_default() == "efi",
            None => {
                if let Some(DiskImage::Fat(_)) = self.disk_image {
                    true
                } else {
                    false
                }
            }
        }
    }

    pub fn get_image_hashes(&self) -> Result<(u64, u64)> {
        // If there is an efi kernel, and no zbi, use the kernel to calculate the hash.

        let zbi_hash = if let Some(Ramdisk { path, kind: RamdiskKind::Zbi }) = &self.ramdisk {
            get_file_hash(path).map_err(|e| EmulatorInstanceError::HashCalculation {
                path: path.to_path_buf(),
                source: e,
            })?
        } else {
            0
        };

        let disk_hash = if let Some(disk) = &self.disk_image {
            get_file_hash(disk.as_ref()).map_err(|e| EmulatorInstanceError::HashCalculation {
                path: disk.as_path().to_path_buf(),
                source: e,
            })?
        } else {
            0
        };

        Ok((zbi_hash, disk_hash))
    }

    pub fn save_disk_hashes(&mut self) -> Result<()> {
        let (new_zbi_hash, new_disk_hash) = self.get_image_hashes()?;
        self.zbi_hash = format!("{new_zbi_hash:x}");
        self.disk_hash = format!("{new_disk_hash:x}");
        Ok(())
    }

    pub fn check_required_files(&self) -> Result<()> {
        let kernel_path: &_ = &self.kernel_image;
        let ramdisk = &self.ramdisk;
        let disk_image_path = &self.disk_image;

        // If no kernel is provided, a FAT diskimage or a full GPT disk containing the bootloader
        // needs to be present.
        match kernel_path {
            Some(file_path) => {
                if !file_path.exists() {
                    return Err(EmulatorInstanceError::MissingKernel(file_path.to_path_buf()));
                }
            }
            None => match disk_image_path {
                Some(DiskImage::Fat(fat_path)) => {
                    if !fat_path.exists() {
                        return Err(EmulatorInstanceError::MissingFatImage(fat_path.to_path_buf()));
                    }
                }
                Some(DiskImage::Gpt(gpt_path)) => {
                    if !gpt_path.exists() {
                        return Err(EmulatorInstanceError::MissingGptImage(gpt_path.to_path_buf()));
                    }
                }
                _ => {
                    return Err(EmulatorInstanceError::MissingConfiguration);
                }
            },
        };

        if let Some(ramdisk) = ramdisk {
            if !ramdisk.path.exists() {
                return Err(EmulatorInstanceError::MissingRamdisk(ramdisk.path.to_path_buf()));
            }
        }

        if let Some(file_path) = disk_image_path.as_ref() {
            if !file_path.exists() {
                return Err(EmulatorInstanceError::MissingDiskImage(file_path.to_path_buf()));
            }
        }
        Ok(())
    }
}

/// Host-side configuration data, such as physical hardware and host OS details.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct HostConfig {
    /// Determines the type of hardware acceleration to use for emulation, such as KVM.
    pub acceleration: AccelerationMode,

    /// Indicates the CPU architecture of the host system.
    pub architecture: CpuArchitecture,

    /// Determines the type of graphics acceleration, to improve rendering in the guest OS.
    pub gpu: GpuType,

    /// Specifies the path to the emulator's log files.
    pub log: PathBuf,

    /// Determines the networking type for the emulator.
    pub networking: NetworkingMode,

    /// Indicates the operating system the host system is running.
    pub os: OperatingSystem,

    /// Holds a set of named ports, with the mapping from host to guest for each one.
    /// Generally only useful when networking is set to "user".
    pub port_map: HashMap<String, PortMapping>,
}

/// A collection of properties which control/influence the
/// execution of an emulator instance. These are different from the
/// DeviceConfig and GuestConfig which defines the hardware configuration
/// and behavior of Fuchsia running within the emulator instance.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct RuntimeConfig {
    /// Additional arguments to pass directly to the guest kernel.
    #[serde(default)]
    pub addl_kernel_args: Vec<String>,

    /// Additional arguments to pass directly to the emulator.
    #[serde(default)]
    pub addl_emu_args: Vec<String>,

    /// Additional environment variables to use when starting the emulator
    #[serde(default)]
    pub addl_env: HashMap<String, String>,

    /// A flag to indicate that the --config flag was used to override the standard configuration.
    /// This matters because the contents of the EmulatorConfiguration no longer represent a
    /// consistent description of the emulator instance.
    #[serde(default)]
    pub config_override: bool,

    /// The emulator's output, which might come from the serial console, the guest, or nothing.
    pub console: ConsoleType,

    /// Pause the emulator and wait for the user to attach a debugger to the process.
    pub debugger: bool,

    /// Engine type name. Added here to be accessible in the configuration template processing.
    #[serde(default)]
    pub engine_type: EngineType,

    /// Run the emulator without a GUI. Graphics drivers will still be loaded.
    pub headless: bool,

    /// On machines with high-density screens (such as MacBook Pro), window size may be
    /// scaled to match the host's resolution which results in a much smaller GUI.
    pub hidpi_scaling: bool,

    /// The staging and working directory for the emulator instance.
    pub instance_directory: PathBuf,

    /// The verbosity level of the logs for this instance.
    pub log_level: LogLevel,

    // A generated MAC address for the emulators virtual network.
    pub mac_address: String,

    /// The human-readable name for this instance. Must be unique from any other current
    /// instance on the host.
    pub name: String,

    /// Whether or not the emulator should reuse a previous instance's image files.
    #[serde(default)]
    pub reuse: bool,

    /// Maximum amount of time to wait on the emulator health check to succeed before returning
    /// control to the user.
    pub startup_timeout: Duration,

    /// Specify a custom smp (symmetric multiprocessing) configuration.
    #[serde(default)]
    pub smp: Option<String>,

    /// Path to an enumeration flags template file, which contains a Handlebars-renderable
    /// set of arguments to be passed to the Command which starts the emulator.
    /// If this is None, the internal emulator_flags.json.template is used.
    pub template: Option<PathBuf>,

    /// Optional path to a Tap upscript file, which is passed to the emulator when Tap networking
    /// is enabled.
    pub upscript: Option<PathBuf>,

    /// Serial number of the emulator.
    ///
    /// - `SerialMode::Uninitialized` represents a legacy or uninitialized instance.
    /// - `SerialMode::Disabled` represents that serial number generation is explicitly disabled.
    /// - `SerialMode::Enabled(serial)` contains the generated or custom stable serial number.
    #[serde(default)]
    pub serial_number: SerialMode,
}

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize)]
#[serde(tag = "status", content = "serial", rename_all = "snake_case")]
pub enum SerialMode {
    /// Legacy or uninitialized instance.
    #[default]
    Uninitialized,
    /// Serial number generation is explicitly disabled.
    Disabled,
    /// Enabled with a specific serial number (generated or custom).
    Enabled(String),
}

// Manual implementation of Deserialize is required to maintain backward compatibility.
// Stale emulator instances saved on disk prior to the introduction of SerialMode
// serializes the serial_number as a flat JSON String. The new schema expects
// an adjacently-tagged object structure. Hand-writing the deserializer allows us
// to fallback gracefully to SerialMode::Enabled(string) if we parse the legacy format.
impl<'de> serde::Deserialize<'de> for SerialMode {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(serde::Deserialize)]
        #[serde(untagged)]
        enum Helper {
            Legacy(String),
            New {
                status: String,
                #[serde(default)]
                serial: Option<String>,
            },
        }

        match <Option<Helper> as serde::Deserialize>::deserialize(deserializer)? {
            None => Ok(SerialMode::Uninitialized),
            Some(Helper::Legacy(s)) => Ok(SerialMode::Enabled(s)),
            Some(Helper::New { status, serial }) => match status.as_str() {
                "uninitialized" => Ok(SerialMode::Uninitialized),
                "disabled" => Ok(SerialMode::Disabled),
                "enabled" => {
                    if let Some(s) = serial {
                        Ok(SerialMode::Enabled(s))
                    } else {
                        Err(serde::de::Error::missing_field("serial"))
                    }
                }
                _ => Err(serde::de::Error::unknown_variant(
                    &status,
                    &["uninitialized", "disabled", "enabled"],
                )),
            },
        }
    }
}

fn default_avx2_enabled() -> bool {
    true
}

/// Specifications of the virtual device to be emulated.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct DeviceConfig {
    /// The model of audio device being emulated, if any.
    pub audio: AudioDevice,

    /// The architecture and number of CPUs to emulate on the guest system.
    pub cpu: VirtualCpu,

    /// The amount of virtual memory to emulate on the guest system.
    pub memory: DataAmount,

    /// Which input source to emulate for screen interactions on the guest, if any.
    pub pointing_device: PointingDevice,

    /// The dimensions of the virtual device's screen, if any.
    pub screen: Screen,

    /// The amount of virtual storage to allocate to the guest's storage device, which will be
    /// populated by the GuestConfig's fvm_image. Only one virtual storage device is supported
    /// at this time.
    pub storage: DataAmount,

    /// Whether vsock support is enabled for this device and what cid it has been allocated.
    pub vsock: Option<VsockDevice>,

    /// Whether AVX2 (and AVX512) is enabled.
    #[serde(default = "default_avx2_enabled")]
    pub avx2_enabled: bool,
}

impl Default for DeviceConfig {
    fn default() -> Self {
        Self {
            audio: Default::default(),
            cpu: Default::default(),
            memory: Default::default(),
            pointing_device: Default::default(),
            screen: Default::default(),
            storage: Default::default(),
            vsock: Default::default(),
            avx2_enabled: default_avx2_enabled(),
        }
    }
}

/// Collects the specific configurations into a single struct for ease of passing around.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct EmulatorConfiguration {
    pub device: DeviceConfig,
    pub flags: FlagData,
    pub guest: GuestConfig,
    pub host: HostConfig,
    pub runtime: RuntimeConfig,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct EmulatorInstanceData {
    #[serde(default)]
    pub(crate) emulator_binary: PathBuf,
    pub(crate) emulator_configuration: EmulatorConfiguration,
    pub(crate) pid: u32,
    pub(crate) engine_type: EngineType,
    #[serde(default)]
    pub(crate) engine_state: EngineState,
}

impl EmulatorInstanceData {
    pub fn new_with_state(name: &str, state: EngineState) -> Self {
        let mut ret = Self::default();
        ret.emulator_configuration.runtime.name = name.to_string();
        ret.engine_state = state;
        ret
    }
    pub fn new(
        emulator_configuration: EmulatorConfiguration,
        engine_type: EngineType,
        engine_state: EngineState,
    ) -> Self {
        EmulatorInstanceData {
            emulator_configuration,
            engine_state,
            engine_type,
            ..Default::default()
        }
    }

    pub fn set_engine_state(&mut self, state: EngineState) {
        self.engine_state = state
    }
    pub fn set_pid(&mut self, pid: u32) {
        self.pid = pid
    }

    pub fn set_emulator_binary(&mut self, binary: PathBuf) {
        self.emulator_binary = binary
    }

    pub fn set_engine_type(&mut self, engine_type: EngineType) {
        self.engine_type = engine_type
    }

    pub fn set_instance_directory(&mut self, instance_dir: &str) {
        self.emulator_configuration.runtime.instance_directory = instance_dir.into()
    }
}

impl EmulatorInstanceInfo for EmulatorInstanceData {
    fn get_name(&self) -> &str {
        &self.emulator_configuration.runtime.name
    }

    fn is_running(&self) -> bool {
        is_pid_running(self.pid)
    }
    fn get_engine_state(&self) -> EngineState {
        // If the static state is running, compare it with the process state
        // They can get out of sync, for example when rebooting the host.
        match self.engine_state {
            EngineState::Running if self.is_running() => EngineState::Running,
            EngineState::Running if !self.is_running() => EngineState::Staged,
            _ => self.engine_state,
        }
    }
    fn get_engine_type(&self) -> EngineType {
        self.engine_type
    }
    fn get_pid(&self) -> u32 {
        self.pid
    }
    fn get_emulator_configuration(&self) -> &EmulatorConfiguration {
        &self.emulator_configuration
    }
    fn get_emulator_configuration_mut(&mut self) -> &mut EmulatorConfiguration {
        &mut self.emulator_configuration
    }
    fn get_emulator_binary(&self) -> &PathBuf {
        &self.emulator_binary
    }
    fn get_networking_mode(&self) -> &NetworkingMode {
        &self.emulator_configuration.host.networking
    }
    fn get_ssh_port(&self) -> Option<u16> {
        if let Some(ssh) = self.emulator_configuration.host.port_map.get("ssh") {
            return ssh.host;
        }
        None
    }
}

/// Returns true if the process identified by the pid is running.
fn is_pid_running(pid: u32) -> bool {
    if pid != 0 {
        // First do a no-hang wait to collect the process if it's defunct.
        let _ = nix::sys::wait::waitpid(
            nix::unistd::Pid::from_raw(pid.try_into().unwrap()),
            Some(nix::sys::wait::WaitPidFlag::WNOHANG),
        );
        // Check to see if it is running by sending signal 0. If there is no error,
        // the process is running.
        return nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid.try_into().unwrap()), None)
            .is_ok();
    }
    return false;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_serial_mode_deserialize() {
        // Legacy format (just a string)
        let legacy_json = r#""EM-B89E30F7F""#;
        let legacy_mode: SerialMode = serde_json::from_str(legacy_json).unwrap();
        assert_eq!(legacy_mode, SerialMode::Enabled("EM-B89E30F7F".to_string()));

        // Legacy format (null)
        let legacy_null_json = r#"null"#;
        let legacy_null_mode: SerialMode = serde_json::from_str(legacy_null_json).unwrap();
        assert_eq!(legacy_null_mode, SerialMode::Uninitialized);

        // New format: uninitialized
        let new_uninit_json = r#"{"status": "uninitialized"}"#;
        let new_uninit_mode: SerialMode = serde_json::from_str(new_uninit_json).unwrap();
        assert_eq!(new_uninit_mode, SerialMode::Uninitialized);

        // New format: disabled
        let new_disabled_json = r#"{"status": "disabled"}"#;
        let new_disabled_mode: SerialMode = serde_json::from_str(new_disabled_json).unwrap();
        assert_eq!(new_disabled_mode, SerialMode::Disabled);

        // New format: enabled
        let new_enabled_json = r#"{"status": "enabled", "serial": "EM-B89E30F7F"}"#;
        let new_enabled_mode: SerialMode = serde_json::from_str(new_enabled_json).unwrap();
        assert_eq!(new_enabled_mode, SerialMode::Enabled("EM-B89E30F7F".to_string()));

        // New format: enabled but missing serial (should fail)
        let bad_enabled_json = r#"{"status": "enabled"}"#;
        let bad_res: std::result::Result<SerialMode, _> = serde_json::from_str(bad_enabled_json);
        assert!(bad_res.is_err());

        // Test struct deserialization with null serial_number
        #[derive(serde::Deserialize)]
        struct MockConfig {
            #[serde(default)]
            serial_number: SerialMode,
        }

        let config_null_json = r#"{"serial_number": null}"#;
        let config_null: MockConfig = serde_json::from_str(config_null_json).unwrap();
        assert_eq!(config_null.serial_number, SerialMode::Uninitialized);

        // Test struct deserialization with missing serial_number
        let config_missing_json = r#"{}"#;
        let config_missing: MockConfig = serde_json::from_str(config_missing_json).unwrap();
        assert_eq!(config_missing.serial_number, SerialMode::Uninitialized);

        // Round-trip test to ensure serialization format is correct (tag-adjacent)
        let mode = SerialMode::Enabled("EM-B89E30F7F".to_string());
        let serialized = serde_json::to_string(&mode).unwrap();
        assert_eq!(serialized, r#"{"status":"enabled","serial":"EM-B89E30F7F"}"#);
        let deserialized: SerialMode = serde_json::from_str(&serialized).unwrap();
        assert_eq!(deserialized, mode);

        let mode_disabled = SerialMode::Disabled;
        let serialized_disabled = serde_json::to_string(&mode_disabled).unwrap();
        assert_eq!(serialized_disabled, r#"{"status":"disabled"}"#);
        let deserialized_disabled: SerialMode = serde_json::from_str(&serialized_disabled).unwrap();
        assert_eq!(deserialized_disabled, mode_disabled);
    }
}
