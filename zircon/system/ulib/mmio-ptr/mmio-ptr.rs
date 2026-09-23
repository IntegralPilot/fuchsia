// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![no_std]

unsafe extern "C" {
    fn mmio_read8(buffer: *const u8) -> u8;
    fn mmio_write8(data: u8, buffer: *mut u8);

    fn mmio_read16(buffer: *const u16) -> u16;
    fn mmio_write16(data: u16, buffer: *mut u16);

    fn mmio_read32(buffer: *const u32) -> u32;
    fn mmio_write32(data: u32, buffer: *mut u32);

    #[cfg(target_pointer_width = "64")]
    fn mmio_read64(buffer: *const u64) -> u64;
    #[cfg(target_pointer_width = "64")]
    fn mmio_write64(data: u64, buffer: *mut u64);
}

/// Reads an 8-bit value from MMIO memory.
///
/// # Safety
///
/// `buffer` must be a valid pointer to MMIO memory.
#[inline(always)]
pub unsafe fn read8(buffer: *const u8) -> u8 {
    // Safety: Justification deferred to caller.
    unsafe { mmio_read8(buffer) }
}

/// Writes an 8-bit value to MMIO memory.
///
/// # Safety
///
/// `buffer` must be a valid pointer to MMIO memory.
#[inline(always)]
pub unsafe fn write8(data: u8, buffer: *mut u8) {
    // Safety: Justification deferred to caller.
    unsafe { mmio_write8(data, buffer) }
}

/// Reads a 16-bit value from MMIO memory.
///
/// # Safety
///
/// `buffer` must be a valid pointer to MMIO memory.
#[inline(always)]
pub unsafe fn read16(buffer: *const u16) -> u16 {
    // Safety: Justification deferred to caller.
    unsafe { mmio_read16(buffer) }
}

/// Writes a 16-bit value to MMIO memory.
///
/// # Safety
///
/// `buffer` must be a valid pointer to MMIO memory.
#[inline(always)]
pub unsafe fn write16(data: u16, buffer: *mut u16) {
    // Safety: Justification deferred to caller.
    unsafe { mmio_write16(data, buffer) }
}

/// Reads a 32-bit value from MMIO memory.
///
/// # Safety
///
/// `buffer` must be a valid pointer to MMIO memory.
#[inline(always)]
pub unsafe fn read32(buffer: *const u32) -> u32 {
    // Safety: Justification deferred to caller.
    unsafe { mmio_read32(buffer) }
}

/// Writes a 32-bit value to MMIO memory.
///
/// # Safety
///
/// `buffer` must be a valid pointer to MMIO memory.
#[inline(always)]
pub unsafe fn write32(data: u32, buffer: *mut u32) {
    // Safety: Justification deferred to caller.
    unsafe { mmio_write32(data, buffer) }
}

/// Reads a 64-bit value from MMIO memory.
///
/// # Safety
///
/// `buffer` must be a valid pointer to MMIO memory.
#[inline(always)]
#[cfg(target_pointer_width = "64")]
pub unsafe fn read64(buffer: *const u64) -> u64 {
    // Safety: Justification deferred to caller.
    unsafe { mmio_read64(buffer) }
}

/// Writes a 64-bit value to MMIO memory.
///
/// # Safety
///
/// `buffer` must be a valid pointer to MMIO memory.
#[inline(always)]
#[cfg(target_pointer_width = "64")]
pub unsafe fn write64(data: u64, buffer: *mut u64) {
    // Safety: Justification deferred to caller.
    unsafe { mmio_write64(data, buffer) }
}
