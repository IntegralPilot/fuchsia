// Copyright 2019 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use crate::arch_rs::x86::x86::outp;

/// CMOS RTC base I/O port.
pub const RTC_BASE_PORT: u16 = 0x70;

/// CMOS register offset for the boot byte.
pub const RTC_BOOT_BYTE: u8 = 48;

// flags and fields in RTC_BOOT_BYTE

/// Boot option: Normal boot.
pub const RTC_BOOT_NORMAL: u8 = 0x01;
/// Boot option: Recovery boot.
pub const RTC_BOOT_RECOVERY: u8 = 0x02;
/// Boot option: Bootloader boot.
pub const RTC_BOOT_BOOTLOADER: u8 = 0x04;
/// Mask for reboot counter field.
pub const RTC_BOOT_COUNT_MASK: u8 = 0xf0;
/// Shift amount for reboot counter field.
pub const RTC_BOOT_COUNT_SHIFT: u32 = 4;

/// Initial value for reboot counter.
pub const RTC_BOOT_COUNT_INITIAL: u8 = 3;

/// Writes a byte value `val` to the CMOS register at `addr`.
fn cmos_write(mut addr: u8, val: u8) {
    let ofs: u16 = if addr < 128 {
        0
    } else {
        addr -= 128;
        2
    };

    // SAFETY: Writing the target CMOS register address and data byte to the standard PC CMOS RTC I/O ports.
    unsafe {
        outp(RTC_BASE_PORT + ofs, addr);
        outp(RTC_BASE_PORT + ofs + 1, val);
    }
}

/// Sets the boot reason byte and reboot counter in CMOS RTC RAM.
///
/// If `reason` is <= 255, it is stored as the boot reason (masked with default boot attempts).
/// If `reason` > 255, it defaults to [`RTC_BOOT_NORMAL`].
#[unsafe(no_mangle)]
pub extern "C" fn bootbyte_set_reason(reason: u64) {
    // set boot reason, clamp to be in range of a uint8_t, default to RTC_BOOT_NORMAL
    let mut val: u8 = if reason <= 255 { reason as u8 } else { RTC_BOOT_NORMAL };

    // set default number of boot attempts
    val |= RTC_BOOT_COUNT_INITIAL << RTC_BOOT_COUNT_SHIFT;
    cmos_write(RTC_BOOT_BYTE, val); // boot_option and reboot_counter
}
