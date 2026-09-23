// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! SPMI Debug Register Override Fallback.

use anyhow::{Context, Result, anyhow};
use fidl::endpoints::ServiceMarker;
use fidl_fuchsia_hardware_spmi as fspmi;
use fuchsia_component::client::connect_to_protocol_at_path;
use std::fmt;
use std::str::FromStr;

const USB_IN_SUSPEND_REG: u16 = 0x2954;
const SUSPEND_USB_VALUE: u8 = 0x01;
const RESUME_USB_VALUE: u8 = 0x00;

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum PowerSource {
    Battery,
    Usb,
}

impl FromStr for PowerSource {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "battery" | "batt" => Ok(Self::Battery),
            "usb" => Ok(Self::Usb),
            _ => Err(format!("Invalid power source '{s}'. Supported: battery, usb")),
        }
    }
}

impl fmt::Display for PowerSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Battery => write!(f, "battery"),
            Self::Usb => write!(f, "usb"),
        }
    }
}

pub async fn set_spmi_power_source(source: PowerSource) -> Result<()> {
    let is_battery = matches!(source, PowerSource::Battery);

    let instance = crate::common::select_instance(fspmi::DebugServiceMarker::SERVICE_NAME)?;

    let controller_path =
        crate::common::append_member_suffix(instance, crate::common::MEMBER_DEVICE);
    let controller_path_str = controller_path.to_str().context("invalid UTF-8 path")?;
    let debug_proxy = connect_to_protocol_at_path::<fspmi::DebugMarker>(controller_path_str)
        .context("Failed to connect to SPMI Debug controller")?;

    let (device_proxy, device_server) = fidl::endpoints::create_proxy::<fspmi::DeviceMarker>();
    debug_proxy
        .connect_target(0, device_server)
        .await
        .context("connect_target on SPMI target 0 failed")?
        .map_err(|e| anyhow!("connect_target rejected with status: {:?}", e))?;

    let write_val = if is_battery { SUSPEND_USB_VALUE } else { RESUME_USB_VALUE };
    device_proxy
        .register_write(USB_IN_SUSPEND_REG, &[write_val])
        .await
        .context("SPMI register_write call failed")?
        .map_err(|e| anyhow!("register_write rejected with status: {:?}", e))?;

    println!(
        "Successfully set power source to {} via SPMI (wrote 0x{:02x} to 0x{:04x})",
        source, write_val, USB_IN_SUSPEND_REG
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_power_source() {
        assert_eq!("battery".parse::<PowerSource>().unwrap(), PowerSource::Battery);
        assert_eq!("batt".parse::<PowerSource>().unwrap(), PowerSource::Battery);
        assert_eq!("usb".parse::<PowerSource>().unwrap(), PowerSource::Usb);
        assert!("wall".parse::<PowerSource>().is_err());
    }

    #[test]
    fn test_display_power_source() {
        assert_eq!(PowerSource::Battery.to_string(), "battery");
        assert_eq!(PowerSource::Usb.to_string(), "usb");
    }
}
