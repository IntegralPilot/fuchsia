// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::ControllerTool;
use fdomain_fuchsia_bluetooth::DeviceClass as FidlDeviceClass;
use ffx_bluetooth_controller_args::device_class::{
    DeviceClassCommand, DeviceClassSubCommand, MajorClass,
};
use ffx_writer::{SimpleWriter, ToolIO as _};
use fho::Result;

pub async fn handle_device_class(
    tool: &ControllerTool,
    cmd: &DeviceClassCommand,
    writer: &mut SimpleWriter,
) -> Result<()> {
    match &cmd.subcommand {
        // ffx bluetooth controller device-class set
        DeviceClassSubCommand::Set(cmd) => {
            // Check for invalid args
            if cmd.minor_class.is_some() && cmd.major_class.is_none() {
                return Err(fho::Error::User(anyhow::anyhow!(
                    "Unable to set device class: Minor class specified without a major class"
                )));
            }

            tool.set_device_class(cmd.into()).await?;
            let major = cmd.major_class.unwrap_or(MajorClass::Uncategorized);
            let minor = cmd.minor_class.unwrap_or(0);
            writer.line(format!(
                "Set device class to:\n  Major: {:?}\n  Minor: {}\n  Services: {:?}",
                major, minor, cmd.service_classes
            ))?;
        }
    }
    Ok(())
}

impl ControllerTool {
    async fn set_device_class(&self, device_class: FidlDeviceClass) -> Result<()> {
        Ok(self
            .access_proxy
            .set_device_class(&device_class)
            .map_err(|err| fho::Error::Unexpected(anyhow::anyhow!("FIDL error: {err}")))?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ffx_bluetooth_controller_args::device_class::{ServiceClass, SetCommand};

    fn custom_set_command(
        major: MajorClass,
        minor: u8,
        service_classes: Vec<ServiceClass>,
    ) -> SetCommand {
        SetCommand { major_class: Some(major), minor_class: Some(minor), service_classes }
    }

    fn default_set_command() -> SetCommand {
        custom_set_command(MajorClass::Uncategorized, 0, vec![])
    }

    #[test]
    fn test_parse_major_class() {
        let cases = vec![
            ("Miscellaneous", Ok(MajorClass::Miscellaneous)),
            ("computer", Ok(MajorClass::Computer)),
            ("phone", Ok(MajorClass::Phone)),
            ("LAN", Ok(MajorClass::Lan)),
            ("audio-video", Ok(MajorClass::AudioVideo)),
            ("peripheral", Ok(MajorClass::Peripheral)),
            ("imaging", Ok(MajorClass::Imaging)),
            ("wearable", Ok(MajorClass::Wearable)),
            ("toy", Ok(MajorClass::Toy)),
            ("health", Ok(MajorClass::Health)),
            ("uncategorized", Ok(MajorClass::Uncategorized)),
            ("TEST", Err("invalid major class: TEST".to_string())),
        ];
        for (input_str, expected) in cases {
            assert_eq!(input_str.parse::<MajorClass>(), expected);
        }
    }

    #[test]
    fn test_parse_service_class() {
        let cases = vec![
            ("limited_discoverable_mode", Ok(ServiceClass::LimitedDiscoverableMode)),
            ("LE-audio", Ok(ServiceClass::LeAudio)),
            ("Positioning", Ok(ServiceClass::Positioning)),
            ("NETWORKING", Ok(ServiceClass::Networking)),
            ("rendering", Ok(ServiceClass::Rendering)),
            ("capturing", Ok(ServiceClass::Capturing)),
            ("object-transfer", Ok(ServiceClass::ObjectTransfer)),
            ("audio", Ok(ServiceClass::Audio)),
            ("telephony", Ok(ServiceClass::Telephony)),
            ("information", Ok(ServiceClass::Information)),
            ("TEST", Err("invalid service class: TEST".to_string())),
        ];
        for (input_str, expected) in cases {
            assert_eq!(input_str.parse::<ServiceClass>(), expected);
        }
    }

    #[test]
    fn test_convert_set_command_to_device_class() {
        let cases = vec![
            (default_set_command(), 0x1f00),
            (
                custom_set_command(
                    MajorClass::Computer,
                    1,
                    vec![ServiceClass::Networking, ServiceClass::Audio],
                ),
                0x220104,
            ),
            (
                custom_set_command(
                    MajorClass::Phone,
                    3,
                    vec![ServiceClass::Networking, ServiceClass::Telephony],
                ),
                0x42020c,
            ),
        ];
        for (cmd, expected) in cases {
            let fidl_class = FidlDeviceClass::from(&cmd);
            assert_eq!(fidl_class.value, expected);
        }
    }
}
