// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! This module holds the common data types for emulator engines. These are implementation-agnostic
//! data types, not the engine-specific command types that each engine will define for itself. These
//! types will be directly deserializable from the PBM, and converted into engine-specific types at
//! runtime.

use emulator_instance::{EngineState, EngineType, FlagData, NetworkingMode, PortMapping};
use schemars::JsonSchema;
use sdk_metadata::{VirtualDeviceV1, display_impl};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt::Display;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EngineConsoleType {
    /// An emulator console for issuing commands to the emulation hypervisor.
    Command,

    /// An emulator console designed for machine-to-machine communication through a structured
    /// language such as JSON.
    Machine,

    /// An emulator console for communicating with the virtual serial port.
    Serial,

    /// A default value indicating none of the above.
    None,
}

impl Default for EngineConsoleType {
    fn default() -> Self {
        EngineConsoleType::None
    }
}

display_impl!(EngineConsoleType);

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct VirtualDeviceInfo {
    /// A unique name identifying the virtual device specification.
    pub name: String,

    /// An optional human readable description.
    pub description: Option<String>,

    /// Details of the Central Processing Unit (CPU).
    pub cpu: String,

    /// Count of Central Processing Units present.
    pub cpu_count: usize,

    /// Details about any audio devices included in the virtual device.
    pub audio: String,

    /// The size of the disk image for the virtual device, equivalent to virtual
    /// storage capacity.
    pub storage_bytes: u64,

    /// Details about any input devices, such as a mouse or touchscreen.
    pub pointing_device: String,

    /// Amount of memory in the virtual device.
    pub memory_bytes: u64,

    /// A map of names to port numbers. These are the ports that need to be
    /// available to the virtual device, though a given use case may not require
    /// all of them. When emulating with user-mode networking, these must be
    /// mapped to host-side ports to allow communication into the emulator from
    /// external tools (such as ssh and mDNS). When emulating with Tun/Tap mode
    /// networking port mapping is superfluous, so we expect this field to be
    /// ignored.
    pub ports: Option<HashMap<String, u16>>,

    /// The height of the virtual device's screen, measured in pixels.
    pub window_height: usize,

    /// The width of the virtual device's screen, measured in pixels.
    pub window_width: usize,

    /// The context identifier for the virtual machine.
    pub cid: u32,
}

impl From<VirtualDeviceV1> for VirtualDeviceInfo {
    fn from(value: VirtualDeviceV1) -> Self {
        VirtualDeviceInfo {
            name: value.name.clone(),
            description: value.description.clone(),
            cpu: value.hardware.cpu.arch.to_string(),
            cpu_count: value.hardware.cpu.count,
            audio: value.hardware.audio.model.to_string(),
            storage_bytes: value.hardware.storage.as_bytes().unwrap_or(0),
            pointing_device: value.hardware.inputs.pointing_device.to_string(),
            memory_bytes: value.hardware.memory.as_bytes().unwrap_or(0),
            window_height: value.hardware.window_size.height,
            window_width: value.hardware.window_size.width,
            ports: value.ports,
            cid: value.hardware.vsock.cid,
        }
    }
}

/// Indicates which details the "show" command should return.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub enum ShowDetail {
    All,
    Cmd {
        program: Option<String>,
        args: Option<Vec<String>>,
        env: Option<HashMap<String, String>>,
    },
    Config {
        flags: Option<FlagData>,
    },
    Device {
        device: Option<VirtualDeviceInfo>,
    },
    Net {
        mode: Option<NetworkingMode>,
        mac_address: Option<String>,
        upscript: Option<PathBuf>,
        ports: Option<HashMap<String, PortMapping>>,
    },
    Instance {
        name: Option<String>,
        engine_type: Option<EngineType>,
        engine_state: Option<EngineState>,
        pid: Option<u32>,
        emulator_binary: Option<PathBuf>,
        instance_directory: Option<PathBuf>,
    },
}

impl Default for ShowDetail {
    fn default() -> Self {
        ShowDetail::All
    }
}

impl Display for ShowDetail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ShowDetail::All => write!(f, "ShowDetail::All is a metavalue")?,
            ShowDetail::Cmd { program, args, env } => {
                writeln!(f, "Command:")?;
                writeln!(f, "\tProgram: {}", program.as_ref().map_or("", String::as_str))?;
                writeln!(
                    f,
                    "\tArguments: {}",
                    args.as_ref().map_or_else(|| "".to_owned(), |args| args.join(" "))
                )?;
                writeln!(f, "\tEnvironment:")?;
                if let Some(env_map) = env {
                    for (k, v) in env_map {
                        writeln!(f, "\t\t{k:32}{v}")?;
                    }
                } else {
                    writeln!(f, "\t\tNone")?;
                }
            }
            ShowDetail::Config { flags } => {
                writeln!(f, "Configuration:")?;
                if let Some(flag_data) = flags {
                    let s = serde_json::to_string_pretty(&flag_data)
                        .expect("serialization error for flagdata");
                    writeln!(f, "\t{s}")?;
                } else {
                    writeln!(f, "\tNone")?;
                }
            }
            ShowDetail::Device { device } => {
                writeln!(f, "Device:")?;
                if let Some(device_data) = device {
                    let s = serde_json::to_string_pretty(&device_data)
                        .expect("serialization error for flagdata");
                    writeln!(f, "\t{s}")?;
                } else {
                    writeln!(f, "\tNone")?;
                }
            }
            ShowDetail::Net { mode, mac_address, upscript, ports } => {
                writeln!(f, "Network:")?;
                if let Some(mode_data) = mode {
                    writeln!(f, "\tMode: {}", mode_data)?;
                } else {
                    writeln!(f, "\tMode: None")?;
                }
                if let Some(mac_data) = mac_address {
                    writeln!(f, "\tMAC: {}", mac_data)?;
                } else {
                    writeln!(f, "\tMAC: None")?;
                }
                if let Some(script_data) = upscript {
                    writeln!(f, "\tUpscript: {}", script_data.to_string_lossy())?;
                } else {
                    writeln!(f, "\tUpscript: None")?;
                }
                if let Some(portdata) = ports {
                    writeln!(f, "\tPort mappings:")?;
                    writeln!(f, "\t\t{:10}{:10}{:10}", "name", "guest", "host")?;
                    for (k, v) in portdata {
                        writeln!(f, "\t\t{:10}{:<10}{:<10}", k, v.guest, v.host.unwrap_or(0))?;
                    }
                }
            }
            ShowDetail::Instance {
                name,
                engine_type,
                engine_state,
                pid,
                emulator_binary,
                instance_directory,
            } => {
                writeln!(f, "Instance:")?;
                writeln!(f, "\tName: {}", name.as_deref().unwrap_or(""))?;
                if let Some(engine_type) = engine_type {
                    writeln!(f, "\tType: {}", engine_type)?;
                } else {
                    writeln!(f, "\tType: None")?;
                }
                if let Some(engine_state) = engine_state {
                    writeln!(f, "\tState: {}", engine_state)?;
                } else {
                    writeln!(f, "\tState: None")?;
                }
                if let Some(pid) = pid {
                    writeln!(f, "\tPID: {}", pid)?;
                } else {
                    writeln!(f, "\tPID: None")?;
                }
                if let Some(emulator_binary) = emulator_binary {
                    writeln!(f, "\tBinary: {}", emulator_binary.display())?;
                } else {
                    writeln!(f, "\tBinary: None")?;
                }
                if let Some(instance_directory) = instance_directory {
                    writeln!(f, "\tDirectory: {}", instance_directory.display())?;
                } else {
                    writeln!(f, "\tDirectory: None")?;
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_show_detail() {
        // Verify it returns a default.
        let _default = ShowDetail::default();
    }
}
