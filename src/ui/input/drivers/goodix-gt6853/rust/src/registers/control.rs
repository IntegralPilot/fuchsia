// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Low-level hardware control registers for Goodix GT6853.

use crate::data_types::traits::{AddressableRegister, WritableRegister};
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

/// Controls the execution state of the SS51 core.
//
// @cite(gt6853-hardware-description): sec=3.5.1 title="Core CPU and Safety Control Registers"
// @alias(gt6853-hardware-description): theirs="cpu_ctrl"
#[derive(Copy, Clone, Debug, PartialEq, Eq, IntoBytes, FromBytes, KnownLayout, Immutable)]
#[repr(transparent)]
pub struct CpuControl(pub u8);

impl AddressableRegister for CpuControl {
    const ADDRESS: u16 = 0x2180;
}

impl WritableRegister for CpuControl {}

impl CpuControl {
    /// Halts the core and sets the CPU to reset mode.
    pub const HOLD: Self = Self(0x24);

    /// Starts firmware execution if the CPU was previously on `HOLD` state.
    pub const RELEASE: Self = Self(0x00);
}

/// Controls power gating to the DSP and MCU subsystem.
//
// @cite(gt6853-hardware-description): sec=3.5.1 title="Core CPU and Safety Control Registers"
// @alias(gt6853-hardware-description): theirs="dsp_mcu_power"
#[derive(Copy, Clone, Debug, PartialEq, Eq, IntoBytes, FromBytes, KnownLayout, Immutable)]
#[repr(transparent)]
pub struct DspMcuPowerControl(pub u8);

impl AddressableRegister for DspMcuPowerControl {
    const ADDRESS: u16 = 0x2010;
}

impl WritableRegister for DspMcuPowerControl {}

impl DspMcuPowerControl {
    pub const ENABLE: Self = Self(0x00);
    pub const DISABLE: Self = Self(0x01);
}

/// Controls write access permissions for the SRAM Patch0 memory region.
//
// @cite(gt6853-hardware-description): sec=3.5.1 title="Core CPU and Safety Control Registers"
// @alias(gt6853-hardware-description): theirs="access_patch0"
#[derive(Copy, Clone, Debug, PartialEq, Eq, IntoBytes, FromBytes, KnownLayout, Immutable)]
#[repr(transparent)]
pub struct Patch0WriteAccessControl(pub u8);

impl AddressableRegister for Patch0WriteAccessControl {
    const ADDRESS: u16 = 0x204D;
}

impl WritableRegister for Patch0WriteAccessControl {}

impl Patch0WriteAccessControl {
    pub const ENABLE: Self = Self(0x01);
    pub const DISABLE: Self = Self(0x00);
}

/// Controls memory banking selection for SRAM access.
//
// @cite(gt6853-hardware-description): sec=3.5.1 title="Core CPU and Safety Control Registers"
// @alias(gt6853-hardware-description): theirs="bank_select"
#[derive(Copy, Clone, Debug, PartialEq, Eq, IntoBytes, FromBytes, KnownLayout, Immutable)]
#[repr(transparent)]
pub struct MemoryBankSelect(pub u8);

impl AddressableRegister for MemoryBankSelect {
    const ADDRESS: u16 = 0x2048;
}

impl WritableRegister for MemoryBankSelect {}

impl MemoryBankSelect {
    pub const BANK0: Self = Self(0x00);
}

/// Controls the instruction/data cache subsystem.
//
// @cite(gt6853-hardware-description): sec=3.5.1 title="Core CPU and Safety Control Registers"
// @alias(gt6853-hardware-description): theirs="cache"
#[derive(Copy, Clone, Debug, PartialEq, Eq, IntoBytes, FromBytes, KnownLayout, Immutable)]
#[repr(transparent)]
pub struct CacheControl(pub u8);

impl AddressableRegister for CacheControl {
    const ADDRESS: u16 = 0x204B;
}

impl WritableRegister for CacheControl {}

impl CacheControl {
    pub const DISABLE: Self = Self(0x00);
}

/// Controls the hardware watchdog timer.
//
// @cite(gt6853-hardware-description): sec=3.5.1 title="Core CPU and Safety Control Registers"
// @alias(gt6853-hardware-description): theirs="wtd_timer"
#[derive(Copy, Clone, Debug, PartialEq, Eq, IntoBytes, FromBytes, KnownLayout, Immutable)]
#[repr(transparent)]
pub struct WatchdogTimerControl(pub u8);

impl AddressableRegister for WatchdogTimerControl {
    const ADDRESS: u16 = 0x20B0;
}

impl WritableRegister for WatchdogTimerControl {}

impl WatchdogTimerControl {
    pub const DISABLE: Self = Self(0x00);
}

/// Controls hardware memory scrambling and encryption.
//
// @cite(gt6853-hardware-description): sec=3.5.1 title="Core CPU and Safety Control Registers"
// @alias(gt6853-hardware-description): theirs="scramble"
#[derive(Copy, Clone, Debug, PartialEq, Eq, IntoBytes, FromBytes, KnownLayout, Immutable)]
#[repr(transparent)]
pub struct MemoryScramblingEncryptionControl(pub u8);

impl AddressableRegister for MemoryScramblingEncryptionControl {
    const ADDRESS: u16 = 0x2218;
}

impl WritableRegister for MemoryScramblingEncryptionControl {}

impl MemoryScramblingEncryptionControl {
    pub const DISABLE: Self = Self(0x00);
}

/// Watchdog key register used to guard watchdog timer modifications.
//
// @cite(gt6853-hardware-description): sec=3.5.1 title="Core CPU and Safety Control Registers"
// @alias(gt6853-hardware-description): theirs="esd_key"
#[derive(Copy, Clone, Debug, PartialEq, Eq, IntoBytes, FromBytes, KnownLayout, Immutable)]
#[repr(transparent)]
pub struct WatchdogKeyRegister(pub u8);

impl AddressableRegister for WatchdogKeyRegister {
    const ADDRESS: u16 = 0x2318;
}

impl WritableRegister for WatchdogKeyRegister {}

impl WatchdogKeyRegister {
    /// Unlocks the watchdog timer register to allow configuration changes.
    pub const UNLOCK: Self = Self(0x95);

    /// Locks the watchdog timer register after modifications to restore lock protection.
    pub const LOCK: Self = Self(0x27);
}
