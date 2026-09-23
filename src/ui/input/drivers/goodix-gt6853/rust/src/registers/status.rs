// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Real-time status and hardware event registers for Goodix GT6853.

use crate::data_types::traits::{AddressableRegister, ReadableRegister, WritableRegister};
use bitfield::bitfield;
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

bitfield! {
    /// Hardware sensor ID register.
    //
    // @cite(gt6853-hardware-description): sec=3.1 title="Product Identification Registers"
    // @cite(gt6853-programming-guide): sec=3.1 title="Product Version"
    // @alias(gt6853-hardware-description): theirs="Sensor ID"
    // @alias(gt6853-programming-guide): theirs="Sensor ID"
    #[derive(Copy, Clone, PartialEq, Eq, IntoBytes, FromBytes, KnownLayout, Immutable)]
    #[repr(transparent)]
    pub struct SensorId(u8);
    impl Debug;

    pub u8, sensor_id, _: 3, 0;
}

impl AddressableRegister for SensorId {
    const ADDRESS: u16 = 0x4541;
}

impl ReadableRegister for SensorId {}

/// Target execution memory location after controller reset.
//
// @cite(gt6853-hardware-description): sec=3.5.2 title="ISP Bootloader Control Registers"
// @alias(gt6853-hardware-description): theirs="cpu_run_from"
#[derive(Copy, Clone, Debug, PartialEq, Eq, IntoBytes, FromBytes, KnownLayout, Immutable)]
#[repr(transparent)]
pub struct BootTarget(pub [u8; 8]);

impl AddressableRegister for BootTarget {
    const ADDRESS: u16 = 0x4506;
}

impl ReadableRegister for BootTarget {}
impl WritableRegister for BootTarget {}

impl BootTarget {
    /// Target execution from internal Flash storage for normal operation.
    pub const FLASH: Self = Self([0x00; 8]);

    /// Target execution from SRAM for ISP firmware downloading.
    pub const RAM: Self = Self([0x55; 8]);
}

/// Mailbox interrupt request event code communicated from the controller.
//
// @cite(gt6853-hardware-description): sec=3.2 title="Real-Time Command Registers"
// @cite(gt6853-programming-guide): sec=3.2 title="Real-Time Command Registers"
// @cite(gt6853-programming-guide): sec=4.6 title="Host Responding to \"INT Request\""
// @alias(gt6853-hardware-description): theirs="INT_Request"
// @alias(gt6853-programming-guide): theirs="INT_Request"
#[derive(Copy, Clone, Debug, PartialEq, Eq, IntoBytes, FromBytes, KnownLayout, Immutable)]
#[repr(transparent)]
pub struct InterruptRequestEvent(pub u8);

impl AddressableRegister for InterruptRequestEvent {
    const ADDRESS: u16 = 0x60D1;
}

impl ReadableRegister for InterruptRequestEvent {}
impl WritableRegister for InterruptRequestEvent {}

impl InterruptRequestEvent {
    pub const RELOAD_FLASH_CONFIG: Self = Self(0x01);
    pub const HARDWARE_RESET: Self = Self(0x03);
    pub const FIRMWARE_UPDATE: Self = Self(0x05);

    /// Host acknowledgment code written to clear pending mailbox request events.
    pub const ACKNOWLEDGE: Self = Self(0x00);

    /// Returns `true` if this matches a known request event code issued by the controller.
    pub const fn is_known(&self) -> bool {
        matches!(*self, Self::RELOAD_FLASH_CONFIG | Self::HARDWARE_RESET | Self::FIRMWARE_UPDATE)
    }
}

/// Firmware execution status register.
//
// @cite(gt6853-hardware-description): sec=3.2 title="Real-Time Command Registers"
// @cite(gt6853-programming-guide): sec=3.2 title="Real-Time Command Registers"
// @alias(gt6853-hardware-description): theirs="FW_Status"
// @alias(gt6853-programming-guide): theirs="FW_Status"
#[derive(Copy, Clone, Debug, PartialEq, Eq, IntoBytes, FromBytes, KnownLayout, Immutable)]
#[repr(transparent)]
pub struct FirmwareStatus(pub [u8; 4]);

impl AddressableRegister for FirmwareStatus {
    const ADDRESS: u16 = 0x60D3;
}

impl ReadableRegister for FirmwareStatus {}

impl FirmwareStatus {
    /// Healthy running firmware execution status value.
    pub const HEALTHY: Self = Self([0x08, 0x00, 0x00, 0x00]);

    pub const fn status_word(&self) -> u32 {
        u32::from_le_bytes(self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sensor_id_masking() {
        assert_eq!(SensorId(0x03).sensor_id(), 3);
        assert_eq!(SensorId(0xF3).sensor_id(), 3);
        assert_eq!(SensorId(0x00).sensor_id(), 0);
    }

    #[test]
    fn test_interrupt_request_event_is_known() {
        assert!(InterruptRequestEvent::RELOAD_FLASH_CONFIG.is_known());
        assert!(InterruptRequestEvent::HARDWARE_RESET.is_known());
        assert!(InterruptRequestEvent::FIRMWARE_UPDATE.is_known());
        assert!(!InterruptRequestEvent::ACKNOWLEDGE.is_known());
        assert!(!InterruptRequestEvent(0xFE).is_known());
    }

    #[test]
    fn test_firmware_status_endianness_conversion() {
        let status = FirmwareStatus([0x12, 0x34, 0x56, 0x78]);
        assert_eq!(status.status_word(), 0x78563412);
        assert_eq!(FirmwareStatus::HEALTHY.status_word(), 0x00000008);
    }
}
