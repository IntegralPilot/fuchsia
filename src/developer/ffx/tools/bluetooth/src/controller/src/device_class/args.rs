// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use fdomain_fuchsia_bluetooth::DeviceClass as FidlDeviceClass;
use std::str::FromStr;

#[derive(ArgsInfo, FromArgs, Debug, PartialEq, Clone)]
#[argh(
    subcommand,
    name = "device-class",
    description = "Interact with the active Bluetooth controller's device class.",
    example = "ffx bluetooth controller device-class"
)]
pub struct DeviceClassCommand {
    #[argh(subcommand)]
    pub subcommand: DeviceClassSubCommand,
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq, Clone)]
#[argh(subcommand)]
pub enum DeviceClassSubCommand {
    Set(SetCommand),
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq, Clone)]
#[argh(
    subcommand,
    name = "set",
    description = "Set the active Bluetooth controller's device class.",
    example = "To set the device's major class, specify `--major`. To specify the device's minor \
class, specify `--minor`. To specify the device's service classes, pass any number of them as \
space-separated arguments. See the Bluetooth Assigned Numbers specification for more information \
on device class values: https://www.bluetooth.com/specifications/assigned-numbers/baseband.

    $ ffx bluetooth controller device-class set --major computer --minor 1 networking audio"
)]
pub struct SetCommand {
    /// specify the device's major class. Allowed values are "miscellaneous", "computer", "phone",
    /// "lan", "audio-video", "peripheral", "imaging", "wearable", "toy", "health", and
    /// "uncategorized". Default value is "uncategorized"
    #[argh(option, long = "major")]
    pub major_class: Option<MajorClass>,

    /// specify the device's minor class (an integer between 0 and 63). The meaning of the minor
    /// class depends on the major class. Setting this option without specifying a major class
    /// will result in an error. Default value is 0. See the Bluetooth Assigned Numbers
    /// specification for more information:
    /// https://www.bluetooth.com/specifications/assigned-numbers/baseband
    // TODO(b/505211340): Add a `--preset` option with common major + minor class combinations.
    #[argh(option, long = "minor")]
    pub minor_class: Option<u8>,

    /// specify any number of service classes for the device. Allowed values are
    /// "limited-discoverable-mode", "le-audio", "positioning", "networking", "rendering",
    /// "capturing", "object-transfer", "audio", "telephony", and "information". If none are
    /// specified, no service classes will be set
    #[argh(positional)]
    pub service_classes: Vec<ServiceClass>,
}

/// specify the device's major class
#[derive(Debug, PartialEq, Clone, Copy)]
#[repr(u32)]
pub enum MajorClass {
    Miscellaneous = 0b00000,
    Computer = 0b00001,
    Phone = 0b00010,
    Lan = 0b00011,
    AudioVideo = 0b00100,
    Peripheral = 0b00101,
    Imaging = 0b00110,
    Wearable = 0b00111,
    Toy = 0b01000,
    Health = 0b01001,
    Uncategorized = 0b11111,
}

impl FromStr for MajorClass {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().replace("_", "").replace("-", "").as_str() {
            "miscellaneous" => Ok(MajorClass::Miscellaneous),
            "computer" => Ok(MajorClass::Computer),
            "phone" => Ok(MajorClass::Phone),
            "lan" => Ok(MajorClass::Lan),
            "audiovideo" => Ok(MajorClass::AudioVideo),
            "peripheral" => Ok(MajorClass::Peripheral),
            "imaging" => Ok(MajorClass::Imaging),
            "wearable" => Ok(MajorClass::Wearable),
            "toy" => Ok(MajorClass::Toy),
            "health" => Ok(MajorClass::Health),
            "uncategorized" => Ok(MajorClass::Uncategorized),
            _ => Err(format!("invalid major class: {s}")),
        }
    }
}

/// specify the device's service classes
#[derive(Debug, PartialEq, Clone, Copy)]
#[repr(u32)]
pub enum ServiceClass {
    LimitedDiscoverableMode = 1,
    LeAudio = 1 << 1,
    Positioning = 1 << 3,
    Networking = 1 << 4,
    Rendering = 1 << 5,
    Capturing = 1 << 6,
    ObjectTransfer = 1 << 7,
    Audio = 1 << 8,
    Telephony = 1 << 9,
    Information = 1 << 10,
}

impl FromStr for ServiceClass {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().replace("_", "").replace("-", "").as_str() {
            "limiteddiscoverablemode" => Ok(ServiceClass::LimitedDiscoverableMode),
            "leaudio" => Ok(ServiceClass::LeAudio),
            "positioning" => Ok(ServiceClass::Positioning),
            "networking" => Ok(ServiceClass::Networking),
            "rendering" => Ok(ServiceClass::Rendering),
            "capturing" => Ok(ServiceClass::Capturing),
            "objecttransfer" => Ok(ServiceClass::ObjectTransfer),
            "audio" => Ok(ServiceClass::Audio),
            "telephony" => Ok(ServiceClass::Telephony),
            "information" => Ok(ServiceClass::Information),
            _ => Err(format!("invalid service class: {s}")),
        }
    }
}

impl From<&SetCommand> for FidlDeviceClass {
    fn from(cmd: &SetCommand) -> Self {
        let major_val = cmd.major_class.unwrap_or(MajorClass::Uncategorized) as u32;
        let minor_val = cmd.minor_class.unwrap_or(0) as u32;
        let service_val =
            cmd.service_classes.iter().fold(0, |acc, service_class| acc | *service_class as u32);

        let device_val = (minor_val & 0b11_1111) << 2
            | ((major_val & 0b1_1111) << 8)
            | ((service_val & 0b111_1111_1111) << 13);
        FidlDeviceClass { value: device_val }
    }
}
