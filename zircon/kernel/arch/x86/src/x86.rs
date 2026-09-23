// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use zx_status::Status;
use zx_types::zx_status_t;

/// # Safety
///
/// Caller must ensure that:
/// 1. The I/O port address `port` is valid and mapped on the system.
/// 2. Reading from the port does not cause side effects that violate memory safety or system stability.
pub unsafe fn inp(port: u16) -> u8 {
    let value: u8;
    // SAFETY: Input from CPU I/O port as requested by the caller.
    unsafe {
        core::arch::asm!(
            "in al, dx",
            in("dx") port,
            out("al") value,
            options(nostack, preserves_flags),
        );
    }
    value
}

/// # Safety
///
/// Caller must ensure that:
/// 1. The I/O port address `port` is valid and mapped on the system.
/// 2. Reading from the port does not cause side effects that violate memory safety or system stability.
pub unsafe fn inpw(port: u16) -> u16 {
    let value: u16;
    // SAFETY: Input from CPU I/O port as requested by the caller.
    unsafe {
        core::arch::asm!(
            "in ax, dx",
            in("dx") port,
            out("ax") value,
            options(nostack, preserves_flags),
        );
    }
    value
}

/// # Safety
///
/// Caller must ensure that:
/// 1. The I/O port address `port` is valid and mapped on the system.
/// 2. Reading from the port does not cause side effects that violate memory safety or system stability.
pub unsafe fn inpd(port: u16) -> u32 {
    let value: u32;
    // SAFETY: Input from CPU I/O port as requested by the caller.
    unsafe {
        core::arch::asm!(
            "in eax, dx",
            in("dx") port,
            out("eax") value,
            options(nostack, preserves_flags),
        );
    }
    value
}

/// # Safety
///
/// Caller must ensure that:
/// 1. The I/O port address `port` is valid and mapped on the system.
/// 2. Writing `value` to the port does not cause side effects that violate memory safety or system stability.
pub unsafe fn outp(port: u16, value: u8) {
    // SAFETY: Output to CPU I/O port as requested by the caller.
    unsafe {
        core::arch::asm!(
            "out dx, al",
            in("dx") port,
            in("al") value,
            options(nostack, preserves_flags),
        );
    }
}

/// # Safety
///
/// Caller must ensure that:
/// 1. The I/O port address `port` is valid and mapped on the system.
/// 2. Writing `value` to the port does not cause side effects that violate memory safety or system stability.
pub unsafe fn outpw(port: u16, value: u16) {
    // SAFETY: Output to CPU I/O port as requested by the caller.
    unsafe {
        core::arch::asm!(
            "out dx, ax",
            in("dx") port,
            in("ax") value,
            options(nostack, preserves_flags),
        );
    }
}

/// # Safety
///
/// Caller must ensure that:
/// 1. The I/O port address `port` is valid and mapped on the system.
/// 2. Writing `value` to the port does not cause side effects that violate memory safety or system stability.
pub unsafe fn outpd(port: u16, value: u32) {
    // SAFETY: Output to CPU I/O port as requested by the caller.
    unsafe {
        core::arch::asm!(
            "out dx, eax",
            in("dx") port,
            in("eax") value,
            options(nostack, preserves_flags),
        );
    }
}

/// Reads a 64-bit Model Specific Register (MSR).
///
/// # Safety
///
/// Caller must ensure that `msr_id` is a valid MSR supported by the current CPU.
/// Reading an unsupported MSR will cause a General Protection Fault (#GP).
pub unsafe fn read_msr(msr_id: u32) -> u64 {
    let low: u32;
    let high: u32;
    // SAFETY: rdmsr reads the MSR specified in ecx into edx:eax. Caller guarantees `msr_id` is valid.
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") msr_id,
            out("eax") low,
            out("edx") high,
            options(nostack, preserves_flags),
        );
    }
    ((high as u64) << 32) | (low as u64)
}

/// Reads the low 32 bits of a Model Specific Register (MSR).
///
/// # Safety
///
/// Caller must ensure that `msr_id` is a valid MSR supported by the current CPU.
/// Reading an unsupported MSR will cause a General Protection Fault (#GP).
pub unsafe fn read_msr32(msr_id: u32) -> u32 {
    let low: u32;
    // SAFETY: rdmsr reads the MSR specified in ecx into edx:eax. Caller guarantees `msr_id` is valid.
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") msr_id,
            out("eax") low,
            out("edx") _,
            options(nostack, preserves_flags),
        );
    }
    low
}

/// Writes a 64-bit value to a Model Specific Register (MSR).
///
/// # Safety
///
/// Caller must ensure that `msr_id` is a valid MSR supported by the current CPU and that `val`
/// contains valid configuration bits for that MSR. Writing invalid values or writing to an
/// unsupported MSR will cause a General Protection Fault (#GP) or undefined behavior.
pub unsafe fn write_msr(msr_id: u32, val: u64) {
    let low = val as u32;
    let high = (val >> 32) as u32;
    // SAFETY: wrmsr writes edx:eax to the MSR specified in ecx. Caller guarantees `msr_id` and `val` are valid.
    unsafe {
        core::arch::asm!(
            "wrmsr",
            in("ecx") msr_id,
            in("eax") low,
            in("edx") high,
            options(nostack, preserves_flags),
        );
    }
}

mod ffi {
    use super::*;

    // Implemented in assembly.
    unsafe extern "C" {
        pub(super) fn read_msr_safe(msr_id: u32, val: *mut u64) -> zx_status_t;
        // Write value |val| into MSR |msr_id|. Returns ZX_OK if the write was successful or
        // a non-Ok status if the write failed because we received a #GP fault.
        pub(super) fn write_msr_safe(msr_id: u32, val: u64) -> zx_status_t;
    }
}

/// Safely reads a 64-bit Model Specific Register (MSR) with fault recovery.
///
/// Returns `Ok(value)` if the read succeeded, or `Err(Status)` if a #GP fault occurred.
///
/// # Safety
///
/// While this catches #GP faults, caller must ensure that reading `msr_id` does not trigger
/// unwanted side effects.
pub unsafe fn read_msr_safe(msr_id: u32) -> Result<u64, Status> {
    let mut val: u64 = 0;
    // SAFETY: Calls assembly implementation with valid pointer to local variable.
    let status = unsafe { ffi::read_msr_safe(msr_id, &mut val) };
    Status::ok(status).map(|()| val)
}

/// Safely writes a 64-bit value to a Model Specific Register (MSR) with fault recovery.
///
/// Returns `Ok(())` if the write succeeded, or `Err(Status)` if a #GP fault occurred.
///
/// # Safety
///
/// While this catches #GP faults, caller must ensure that writing `val` to `msr_id` does not
/// violate system invariants or trigger unwanted hardware side effects.
pub unsafe fn write_msr_safe(msr_id: u32, val: u64) -> Result<(), Status> {
    // SAFETY: Calls assembly implementation which handles #GP faults safely.
    let status = unsafe { ffi::write_msr_safe(msr_id, val) };
    Status::ok(status)
}
