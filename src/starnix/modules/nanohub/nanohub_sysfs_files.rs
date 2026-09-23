// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::sysfs::{SysfsError, SysfsOps, try_get};
use crate::sysfs_errno;
use starnix_logging::log_error;
use starnix_uapi::errno;
use {fidl_fuchsia_hardware_google_nanohub as fnanohub, zx};

#[derive(Default)]
pub struct FirmwareNameSysFsOps {}

impl SysfsOps<fnanohub::DeviceSynchronousProxy> for FirmwareNameSysFsOps {
    fn show(&self, service: &fnanohub::DeviceSynchronousProxy) -> Result<String, SysfsError> {
        Ok(service.get_firmware_name(zx::MonotonicInstant::INFINITE)?)
    }
}

#[derive(Default)]
pub struct TimeSyncSysFsOps {}

impl TimeSyncSysFsOps {
    fn format_time_sync(ap: i64, mcu: i64) -> String {
        format!("ap time: {} mcu time: {}\n", ap, mcu)
    }
}

impl SysfsOps<fnanohub::DeviceSynchronousProxy> for TimeSyncSysFsOps {
    fn show(
        &self,
        service: &fidl_fuchsia_hardware_google_nanohub::DeviceSynchronousProxy,
    ) -> Result<String, SysfsError> {
        let response = service.get_time_sync(zx::MonotonicInstant::INFINITE)??;
        let ap = try_get(response.ap_boot_time)?;
        let mcu = try_get(response.mcu_boot_time)?;
        Ok(Self::format_time_sync(ap, mcu))
    }
}

trait FromBit {
    fn from_bit(bit: u8) -> Self;
}

impl FromBit for fnanohub::PinState {
    /// Construct an ISP pin state from an integer encoding.
    fn from_bit(bit: u8) -> Self {
        if bit == 0 { fnanohub::PinState::Low } else { fnanohub::PinState::High }
    }
}

trait FromBitfield {
    fn from_bitfield(bitfield: u8) -> Self;
}

impl FromBitfield for fnanohub::HardwareResetPinStates {
    /// Construct hardware reset pin states encoded in a bitfield.
    fn from_bitfield(bitfield: u8) -> Self {
        fnanohub::HardwareResetPinStates {
            isp_pin_0: fnanohub::PinState::from_bit((bitfield >> 0) & 0x1),
            isp_pin_1: fnanohub::PinState::from_bit((bitfield >> 1) & 0x1),
            isp_pin_2: fnanohub::PinState::from_bit((bitfield >> 2) & 0x1),
        }
    }
}

#[derive(Default)]
pub struct HardwareResetSysFsOps {}

impl HardwareResetSysFsOps {
    fn parse_hardware_reset_request(
        &self,
        request: &String,
    ) -> Result<fnanohub::HardwareResetPinStates, SysfsError> {
        request
            // Parse the string input into an integer...
            .trim()
            .parse::<u8>()
            .map_err(|e| {
                log_error!("Failed to parse hardware reset request: {e:?}");
                sysfs_errno!(EINVAL)
            })
            // ... then use the integer to decode the ISP pin states.
            .map(fnanohub::HardwareResetPinStates::from_bitfield)
    }
}

impl SysfsOps<fnanohub::DeviceSynchronousProxy> for HardwareResetSysFsOps {
    fn store(
        &self,
        service: &fnanohub::DeviceSynchronousProxy,
        value: String,
    ) -> Result<(), SysfsError> {
        let pin_states = self.parse_hardware_reset_request(&value)?;

        Ok(service.hardware_reset(
            pin_states.isp_pin_0,
            pin_states.isp_pin_1,
            pin_states.isp_pin_2,
            zx::MonotonicInstant::INFINITE,
        )??)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[::fuchsia::test]
    fn test_parse_hardware_reset_request_valid_integer() {
        let ops = HardwareResetSysFsOps::default();

        // Test all possible 3-bit values.
        for i in 0..=7 {
            let request = i.to_string();
            let pin_states = ops.parse_hardware_reset_request(&request).unwrap();

            assert_eq!(
                pin_states.isp_pin_0,
                if (i & 0x1) == 0 { fnanohub::PinState::Low } else { fnanohub::PinState::High }
            );

            assert_eq!(
                pin_states.isp_pin_1,
                if (i & 0x2) == 0 { fnanohub::PinState::Low } else { fnanohub::PinState::High }
            );

            assert_eq!(
                pin_states.isp_pin_2,
                if (i & 0x4) == 0 { fnanohub::PinState::Low } else { fnanohub::PinState::High }
            );
        }
    }

    #[::fuchsia::test]
    fn test_parse_hardware_reset_request_out_of_range_integer() {
        let ops = HardwareResetSysFsOps::default();
        let request = "8".to_string();
        let pin_states = ops.parse_hardware_reset_request(&request).unwrap();
        assert_eq!(pin_states.isp_pin_0, fnanohub::PinState::Low);
        assert_eq!(pin_states.isp_pin_1, fnanohub::PinState::Low);
        assert_eq!(pin_states.isp_pin_2, fnanohub::PinState::Low);
    }

    #[::fuchsia::test]
    fn test_parse_hardware_reset_request_invalid_string() {
        let ops = HardwareResetSysFsOps::default();
        let request = "foo".to_string();
        assert_eq!(ops.parse_hardware_reset_request(&request).is_err(), true);
    }

    #[::fuchsia::test]
    fn test_format_time_sync() {
        let ap_time: i64 = 123;
        let mcu_time: i64 = 456;
        let value = TimeSyncSysFsOps::format_time_sync(ap_time, mcu_time);
        assert_eq!(value, "ap time: 123 mcu time: 456\n");
    }
}
