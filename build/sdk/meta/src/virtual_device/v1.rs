// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Representation of the virtual_device metadata.

use crate::common::{
    AudioModel, CpuArchitecture, DataUnits, ElementType, Envelope, PointingDevice, ScreenUnits,
};
use crate::json::{JsonObject, schema};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Specifics for a CPU.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct Cpu {
    /// Target CPU architecture.
    pub arch: CpuArchitecture,
    /// Count of CPUs present. For backwards compatibility, defaults to 4 when deserializing old data.
    #[serde(default = "default_cpu_count")]
    pub count: usize,
}

fn default_cpu_count() -> usize {
    4
}

fn default_avx2_enabled() -> bool {
    true
}

/// Details of virtual input devices, such as mice.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct InputDevice {
    /// Pointing device for interacting with the target.
    pub pointing_device: PointingDevice,
}

/// Details of the virtual device's audio interface, if any.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct AudioDevice {
    /// The model of the emulated audio device, or None.
    pub model: AudioModel,
}

/// Screen dimensions for the virtual device, if any.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct Screen {
    pub height: usize,
    pub width: usize,
    pub units: ScreenUnits,
}

/// Details of the virtual device's vsock interface, if any.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct VsockDevice {
    /// Whether the vsock device is enabled.
    pub enabled: bool,

    /// The context id the kernel should associate with the vsock.
    pub cid: u32,
}

/// A generic data structure for indicating quantities of data.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct DataAmount {
    pub quantity: usize,
    pub units: DataUnits,
}

impl DataAmount {
    /// Returns None if the result would overflow.
    pub fn as_bytes(&self) -> Option<u64> {
        let quantity: u64 = self.quantity.try_into().ok()?;
        quantity.checked_mul(self.units.as_bytes())
    }
}

/// Specifics for a given platform.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct Hardware {
    /// Details of the Central Processing Unit (CPU).
    pub cpu: Cpu,

    /// Details about any audio devices included in the virtual device.
    pub audio: AudioDevice,

    /// The size of the disk image for the virtual device, equivalent to virtual
    /// storage capacity.
    pub storage: DataAmount,

    /// Details about any input devices, such as a mouse or touchscreen.
    pub inputs: InputDevice,

    /// Amount of memory in the virtual device.
    pub memory: DataAmount,

    /// The size of the virtual device's screen, measured in pixels.
    pub window_size: Screen,

    /// Details about the vsock device.
    pub vsock: VsockDevice,
}

/// Description of a virtual (rather than physical) hardware device.
///
/// This does not include the data "envelope", i.e. it begins within /data in
/// the source json file.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct VirtualDeviceV1 {
    /// A unique name identifying the virtual device specification.
    pub name: String,

    /// An optional human readable description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Always "virtual_device" for a VirtualDeviceV1. This is valuable for
    /// debugging or when writing this record to a json string.
    #[serde(rename = "type")]
    pub kind: ElementType,

    /// Details about the properties of the device.
    pub hardware: Hardware,

    /// Whether AVX2 (and AVX512) is enabled.
    #[serde(default = "default_avx2_enabled")]
    pub avx2_enabled: bool,

    /// A map of names to port numbers. These are the ports that need to be
    /// available to the virtual device, though a given use case may not require
    /// all of them. When emulating with user-mode networking, these must be
    /// mapped to host-side ports to allow communication into the emulator from
    /// external tools (such as ssh and mDNS). When emulating with Tun/Tap mode
    /// networking port mapping is superfluous, so we expect this field to be
    /// ignored.
    pub ports: Option<HashMap<String, u16>>,
}

impl VirtualDeviceV1 {
    pub fn new(name: impl ToString, hardware: Hardware) -> Self {
        Self {
            name: name.to_string(),
            description: None,
            kind: ElementType::VirtualDevice,
            hardware,
            avx2_enabled: true,
            ports: None,
        }
    }
}

impl JsonObject for Envelope<VirtualDeviceV1> {
    fn get_schema() -> &'static str {
        include_str!("../../virtual_device.json")
    }

    fn get_referenced_schemata() -> &'static [&'static str] {
        &[schema::COMMON, schema::HARDWARE_V1]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    test_validation! {
        name = test_validation,
        kind = Envelope::<VirtualDeviceV1>,
        data = r#"
        {
            "schema_id": "http://fuchsia.com/schemas/sdk/virtual_device.json",
            "data": {
                "name": "generic-x64",
                "type": "virtual_device",
                "hardware": {
                    "audio": {
                        "model": "hda"
                    },
                    "cpu": {
                        "arch": "x64"
                    },
                    "inputs": {
                        "pointing_device": "touch"
                    },
                    "window_size": {
                        "width": 640,
                        "height": 480,
                        "units": "pixels"
                    },
                    "memory": {
                        "quantity": 1,
                        "units": "gigabytes"
                    },
                    "storage": {
                        "quantity": 1,
                        "units": "gigabytes"
                    },
                    "vsock": {
                        "enabled": true,
                        "cid": 3
                    }
                },
                "start_up_args_template": "/path/to/args"
            }
        }
        "#,
        valid = true,
    }

    test_validation! {
        name = test_validation_invalid,
        kind = Envelope::<VirtualDeviceV1>,
        data = r#"
        {
            "schema_id": "http://fuchsia.com/schemas/sdk/virtual_device.json",
            "data": {
                "name": "generic-x64",
                "type": "cc_prebuilt_library",
                "hardware": {
                    "audio": {
                        "model": "hda"
                    },
                    "cpu": {
                        "arch": "x64"
                    },
                    "inputs": {
                        "pointing_device": "touch"
                    },
                    "window_size": {
                        "width": 640,
                        "height": 480,
                        "units": "pixels"
                    },
                    "memory": {
                        "quantity": 1,
                        "units": "gigabytes"
                    },
                    "storage": {
                        "quantity": 1,
                        "units": "gigabytes"
                    },
                    "vsock": {
                        "enabled": true,
                        "cid": 3
                    }
                },
                "start_up_args_template": "/path/to/args"
            }
        }
        "#,
        // Incorrect type
        valid = false,
    }
}
