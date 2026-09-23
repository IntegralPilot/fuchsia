// Copyright 2016 The Fuchsia Authors
// Copyright (c) 2009 Corey Tabaka
// Copyright (c) 2016 Travis Geiselbrecht
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Remnants of a full i8042 keyboard driver, now just used to reboot the system as a fallback.

use crate::arch_rs::x86::x86::{inp, outp};

unsafe extern "C" {
    fn spin(usecs: u32);
}

// i8042 keyboard controller registers

/// i8042 Command Register I/O port.
pub const I8042_COMMAND_REG: u16 = 0x64;
/// i8042 Status Register I/O port.
pub const I8042_STATUS_REG: u16 = 0x64;

/// Timeout in milliseconds.
pub const I8042_CTL_TIMEOUT: usize = 500;

// status register bits

/// Status register: Parity error.
pub const I8042_STR_PARITY: u8 = 0x80;
/// Status register: Timeout error.
pub const I8042_STR_TIMEOUT: u8 = 0x40;
/// Status register: Auxiliary data (mouse).
pub const I8042_STR_AUXDATA: u8 = 0x20;
/// Status register: Keyboard locked.
pub const I8042_STR_KEYLOCK: u8 = 0x10;
/// Status register: Command/data bit (0 = data, 1 = command).
pub const I8042_STR_CMDDAT: u8 = 0x08;
/// Status register: Multiplexer error.
pub const I8042_STR_MUXERR: u8 = 0x04;
/// Status register: Input buffer full.
pub const I8042_STR_IBF: u8 = 0x02;
/// Status register: Output buffer full.
pub const I8042_STR_OBF: u8 = 0x01;

// control register bits

/// Control register: Keyboard interrupt enable.
pub const I8042_CTR_KBDINT: u8 = 0x01;
/// Control register: Auxiliary device (mouse) interrupt enable.
pub const I8042_CTR_AUXINT: u8 = 0x02;
/// Control register: Ignore keyboard lock.
pub const I8042_CTR_IGNKEYLK: u8 = 0x08;
/// Control register: Disable keyboard.
pub const I8042_CTR_KBDDIS: u8 = 0x10;
/// Control register: Disable auxiliary device (mouse).
pub const I8042_CTR_AUXDIS: u8 = 0x20;
/// Control register: Enable translation.
pub const I8042_CTR_XLATE: u8 = 0x40;

// commands

/// Command: Read control register.
pub const I8042_CMD_CTL_RCTR: u16 = 0x0120;
/// Command: Write control register.
pub const I8042_CMD_CTL_WCTR: u16 = 0x1060;
/// Command: Self test.
pub const I8042_CMD_CTL_TEST: u16 = 0x01aa;

/// Command: Disable keyboard interface.
pub const I8042_CMD_KBD_DIS: u16 = 0x00ad;
/// Command: Enable keyboard interface.
pub const I8042_CMD_KBD_EN: u16 = 0x00ae;
/// Command: Pulse output port / system reset.
pub const I8042_CMD_PULSE_RESET: u16 = 0x00fe;
/// Command: Test keyboard interface.
pub const I8042_CMD_KBD_TEST: u16 = 0x01ab;
/// Command: Set keyboard mode.
pub const I8042_CMD_KBD_MODE: u16 = 0x01f0;

#[inline]
fn i8042_read_status() -> u8 {
    // SAFETY: Reading status from standard i8042 keyboard controller status port (0x64).
    unsafe { inp(I8042_STATUS_REG) }
}

#[inline]
fn i8042_write_command(val: u8) {
    // SAFETY: Writing command byte to standard i8042 keyboard controller command port (0x64).
    unsafe { outp(I8042_COMMAND_REG, val) }
}

fn i8042_wait_write() -> Result<(), ()> {
    let mut i = 0;
    while (i8042_read_status() & I8042_STR_IBF) != 0 && (i < I8042_CTL_TIMEOUT) {
        // SAFETY: Spin delay is safe to call for busy waiting in kernel mode.
        unsafe { spin(10) };
        i += 1;
    }
    if i == I8042_CTL_TIMEOUT { Err(()) } else { Ok(()) }
}

/// Reboot the system via the keyboard, returns on failure.
#[unsafe(no_mangle)]
pub extern "C" fn pc_keyboard_reboot() {
    if i8042_wait_write().is_err() {
        return;
    }

    i8042_write_command(I8042_CMD_PULSE_RESET as u8);
    // Wait a second for the command to process before declaring failure
    // SAFETY: Spin delay is safe to call for busy waiting in kernel mode.
    unsafe { spin(1_000_000) };
}
