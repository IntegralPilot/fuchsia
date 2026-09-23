// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! RISC-V Supervisor Binary Interface (SBI) wrapper and extension discovery.

use core::sync::atomic::{AtomicU32, Ordering};
use debug::dprintf;
use zx_status::Status;

pub use riscv_sbi_bindings::{
    RiscvSbiBase, RiscvSbiEid, RiscvSbiError, RiscvSbiHart, RiscvSbiHartState, RiscvSbiIpi,
    RiscvSbiResetReason, RiscvSbiResetType, RiscvSbiRet, RiscvSbiRfence, RiscvSbiSystemReset,
    RiscvSbiTimer,
};

const _: () = {
    assert!(core::mem::size_of::<RiscvSbiRet>() == 16);
    assert!(core::mem::align_of::<RiscvSbiRet>() == 8);
};

// SBI Extension IDs (EIDs)
const SBI_EID_BASE: u64 = RiscvSbiEid::Base as u64;
const SBI_EID_TIMER: u64 = RiscvSbiEid::Timer as u64;
const SBI_EID_IPI: u64 = RiscvSbiEid::Ipi as u64;
const SBI_EID_RFENCE: u64 = RiscvSbiEid::Rfence as u64;
const SBI_EID_HART: u64 = RiscvSbiEid::Hart as u64;
const SBI_EID_SYSTEM_RESET: u64 = RiscvSbiEid::SystemReset as u64;
const SBI_EID_PMU: u64 = RiscvSbiEid::Pmu as u64;
const SBI_EID_DBCN: u64 = RiscvSbiEid::Dbcn as u64;
const SBI_EID_SUSP: u64 = RiscvSbiEid::Susp as u64;
const SBI_EID_CPPC: u64 = RiscvSbiEid::Cppc as u64;

// Base Extension Function IDs
const SBI_BASE_GET_SPEC_VERSION: u64 = RiscvSbiBase::GetSpecVersion as u64;
const SBI_BASE_GET_IMPL_ID: u64 = RiscvSbiBase::GetImplId as u64;
const SBI_BASE_GET_IMPL_VERSION: u64 = RiscvSbiBase::GetImplVersion as u64;
const SBI_BASE_PROBE_EXTENSION: u64 = RiscvSbiBase::ProbeExtension as u64;
const SBI_BASE_GET_MVENDORID: u64 = RiscvSbiBase::GetMvendorid as u64;
const SBI_BASE_GET_MARCHID: u64 = RiscvSbiBase::GetMarchid as u64;
const SBI_BASE_GET_MIMPID: u64 = RiscvSbiBase::GetMimpid as u64;

// Timer Extension Function IDs
const SBI_TIMER_SET_TIMER: u64 = RiscvSbiTimer::SetTimer as u64;

// IPI Extension Function IDs
const SBI_IPI_SEND_IPI: u64 = RiscvSbiIpi::SendIpi as u64;

// RFence Extension Function IDs
const SBI_RFENCE_FENCE_I: u64 = RiscvSbiRfence::FenceI as u64;
const SBI_RFENCE_SFENCE_VMA: u64 = RiscvSbiRfence::SfenceVma as u64;
const SBI_RFENCE_SFENCE_VMA_ASID: u64 = RiscvSbiRfence::SfenceVmaAsid as u64;

// HSM (Hart State Management) Function IDs
const SBI_HART_START: u64 = RiscvSbiHart::Start as u64;
const SBI_HART_STOP: u64 = RiscvSbiHart::Stop as u64;
const SBI_HART_GET_STATUS: u64 = RiscvSbiHart::GetStatus as u64;
const SBI_HART_SUSPEND: u64 = RiscvSbiHart::Suspend as u64;

// System Reset Extension Function IDs & Constants
const SBI_SRST_SYSTEM_RESET: u64 = RiscvSbiSystemReset::SystemReset as u64;
const SBI_RESET_TYPE_SHUTDOWN: u64 = RiscvSbiResetType::Shutdown as u64;
const SBI_RESET_TYPE_WARM_REBOOT: u64 = RiscvSbiResetType::WarmReboot as u64;
const SBI_RESET_REASON_NONE: u64 = RiscvSbiResetReason::None as u64;

// SBI Error Codes
const SBI_SUCCESS: i64 = RiscvSbiError::Success as i64;
const SBI_ERR_FAILED: i64 = RiscvSbiError::Failed as i64;
const SBI_ERR_NOT_SUPPORTED: i64 = RiscvSbiError::NotSupported as i64;
const SBI_ERR_INVALID_PARAM: i64 = RiscvSbiError::InvalidParam as i64;
const SBI_ERR_DENIED: i64 = RiscvSbiError::Denied as i64;
const SBI_ERR_INVALID_ADDRESS: i64 = RiscvSbiError::InvalidAddress as i64;
const SBI_ERR_ALREADY_AVAILABLE: i64 = RiscvSbiError::AlreadyAvailable as i64;
const SBI_ERR_ALREADY_STARTED: i64 = RiscvSbiError::AlreadyStarted as i64;
const SBI_ERR_ALREADY_STOPPED: i64 = RiscvSbiError::AlreadyStopped as i64;
const SBI_ERR_NO_SHMEM: i64 = RiscvSbiError::NoShmem as i64;

// Hart States (from HSM extension)
const SBI_HART_STATE_STARTED: i64 = RiscvSbiHartState::Started as i64;
const SBI_HART_STATE_STOPPED: i64 = RiscvSbiHartState::Stopped as i64;
const SBI_HART_STATE_START_PENDING: i64 = RiscvSbiHartState::StartPending as i64;
const SBI_HART_STATE_STOP_PENDING: i64 = RiscvSbiHartState::StopPending as i64;
const SBI_HART_STATE_SUSPENDED: i64 = RiscvSbiHartState::Suspended as i64;
const SBI_HART_STATE_SUSPEND_PENDING: i64 = RiscvSbiHartState::SuspendPending as i64;
const SBI_HART_STATE_RESUME_PENDING: i64 = RiscvSbiHartState::ResumePending as i64;

// Power CPU states matching Zircon power_cpu_state enum (dev/pdev/power/power.rs)
const POWER_CPU_STATE_STARTED: u32 = 3;
pub const POWER_CPU_STATE_STOPPED: u32 = 4;
const POWER_CPU_STATE_START_PENDING: u32 = 5;
const POWER_CPU_STATE_STOP_PENDING: u32 = 6;
const POWER_CPU_STATE_SUSPENDED: u32 = 7;
const POWER_CPU_STATE_SUSPEND_PENDING: u32 = 8;
const POWER_CPU_STATE_RESUME_PENDING: u32 = 9;

/// SBI extensions tracked by Zircon.
#[repr(u8)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
enum SbiExtension {
    Base = 0,
    Timer = 1,
    Ipi = 2,
    Rfence = 3,
    Hart = 4,
    SystemReset = 5,
    Pmu = 6,
    Dbcn = 7,
    Susp = 8,
    Cppc = 9,
}

static SUPPORTED_EXTENSIONS: AtomicU32 = AtomicU32::new(0);

unsafe extern "C" {
    fn cpp_riscv64_cpu_mask_to_hart_mask(cmask: u32) -> u64;
    fn cpp_pdev_register_sbi_power();
}

/// Safely decode an SBI error code to `RiscvSbiError` without transmute UB.
#[inline(always)]
fn decode_sbi_error(err: i64) -> RiscvSbiError {
    match err {
        0 => RiscvSbiError::Success,
        -1 => RiscvSbiError::Failed,
        -2 => RiscvSbiError::NotSupported,
        -3 => RiscvSbiError::InvalidParam,
        -4 => RiscvSbiError::Denied,
        -5 => RiscvSbiError::InvalidAddress,
        -6 => RiscvSbiError::AlreadyAvailable,
        -7 => RiscvSbiError::AlreadyStarted,
        -8 => RiscvSbiError::AlreadyStopped,
        -9 => RiscvSbiError::NoShmem,
        _ => RiscvSbiError::Failed,
    }
}

/// Generic low-level SBI ECALL invocations.
/// # Safety
/// What an `ecall` does is entirely determined by `eid`/`fid` and the arguments:
/// SBI functions can stop a hart, reset the machine, or fence another hart's
/// address space. The caller must pick a function whose effects are sound at the
/// call site. The trap itself is register-only, per the SBI calling convention.
#[inline(always)]
unsafe fn sbi_call_0(eid: u64, fid: u64) -> RiscvSbiRet {
    let mut a0: i64;
    let mut a1: i64;
    // SAFETY: `ecall` traps to the SEE, which per the SBI calling convention
    // reads a6/a7 and a0.. for arguments and returns in a0/a1. Every register
    // the convention touches is declared below, and the SEE preserves the
    // rest, so the trap does not disturb the compiler's view of memory or
    // of the stack. Whether the requested function is sound is the caller's
    // obligation, documented above.
    unsafe {
        core::arch::asm!(
            "ecall",
            in("a7") eid,
            in("a6") fid,
            lateout("a0") a0,
            lateout("a1") a1,
            options(nostack, preserves_flags),
        );
    }
    RiscvSbiRet { error: decode_sbi_error(a0), value: a1 as isize }
}

/// # Safety
/// What an `ecall` does is entirely determined by `eid`/`fid` and the arguments:
/// SBI functions can stop a hart, reset the machine, or fence another hart's
/// address space. The caller must pick a function whose effects are sound at the
/// call site. The trap itself is register-only, per the SBI calling convention.
#[inline(always)]
unsafe fn sbi_call_1(eid: u64, fid: u64, arg0: u64) -> RiscvSbiRet {
    let mut a0: i64;
    let mut a1: i64;
    // SAFETY: `ecall` traps to the SEE, which per the SBI calling convention
    // reads a6/a7 and a0.. for arguments and returns in a0/a1. Every register
    // the convention touches is declared below, and the SEE preserves the
    // rest, so the trap does not disturb the compiler's view of memory or
    // of the stack. Whether the requested function is sound is the caller's
    // obligation, documented above.
    unsafe {
        core::arch::asm!(
            "ecall",
            in("a7") eid,
            in("a6") fid,
            inout("a0") arg0 as i64 => a0,
            lateout("a1") a1,
            options(nostack, preserves_flags),
        );
    }
    RiscvSbiRet { error: decode_sbi_error(a0), value: a1 as isize }
}

/// # Safety
/// What an `ecall` does is entirely determined by `eid`/`fid` and the arguments:
/// SBI functions can stop a hart, reset the machine, or fence another hart's
/// address space. The caller must pick a function whose effects are sound at the
/// call site. The trap itself is register-only, per the SBI calling convention.
#[inline(always)]
unsafe fn sbi_call_2(eid: u64, fid: u64, arg0: u64, arg1: u64) -> RiscvSbiRet {
    let mut a0: i64;
    let mut a1: i64;
    // SAFETY: `ecall` traps to the SEE, which per the SBI calling convention
    // reads a6/a7 and a0.. for arguments and returns in a0/a1. Every register
    // the convention touches is declared below, and the SEE preserves the
    // rest, so the trap does not disturb the compiler's view of memory or
    // of the stack. Whether the requested function is sound is the caller's
    // obligation, documented above.
    unsafe {
        core::arch::asm!(
            "ecall",
            in("a7") eid,
            in("a6") fid,
            inout("a0") arg0 as i64 => a0,
            inout("a1") arg1 as i64 => a1,
            options(nostack, preserves_flags),
        );
    }
    RiscvSbiRet { error: decode_sbi_error(a0), value: a1 as isize }
}

/// # Safety
/// What an `ecall` does is entirely determined by `eid`/`fid` and the arguments:
/// SBI functions can stop a hart, reset the machine, or fence another hart's
/// address space. The caller must pick a function whose effects are sound at the
/// call site. The trap itself is register-only, per the SBI calling convention.
#[inline(always)]
unsafe fn sbi_call_3(eid: u64, fid: u64, arg0: u64, arg1: u64, arg2: u64) -> RiscvSbiRet {
    let mut a0: i64;
    let mut a1: i64;
    // SAFETY: `ecall` traps to the SEE, which per the SBI calling convention
    // reads a6/a7 and a0.. for arguments and returns in a0/a1. Every register
    // the convention touches is declared below, and the SEE preserves the
    // rest, so the trap does not disturb the compiler's view of memory or
    // of the stack. Whether the requested function is sound is the caller's
    // obligation, documented above.
    unsafe {
        core::arch::asm!(
            "ecall",
            in("a7") eid,
            in("a6") fid,
            inout("a0") arg0 as i64 => a0,
            inout("a1") arg1 as i64 => a1,
            in("a2") arg2,
            options(nostack, preserves_flags),
        );
    }
    RiscvSbiRet { error: decode_sbi_error(a0), value: a1 as isize }
}

/// # Safety
/// What an `ecall` does is entirely determined by `eid`/`fid` and the arguments:
/// SBI functions can stop a hart, reset the machine, or fence another hart's
/// address space. The caller must pick a function whose effects are sound at the
/// call site. The trap itself is register-only, per the SBI calling convention.
#[inline(always)]
unsafe fn sbi_call_4(
    eid: u64,
    fid: u64,
    arg0: u64,
    arg1: u64,
    arg2: u64,
    arg3: u64,
) -> RiscvSbiRet {
    let mut a0: i64;
    let mut a1: i64;
    // SAFETY: `ecall` traps to the SEE, which per the SBI calling convention
    // reads a6/a7 and a0.. for arguments and returns in a0/a1. Every register
    // the convention touches is declared below, and the SEE preserves the
    // rest, so the trap does not disturb the compiler's view of memory or
    // of the stack. Whether the requested function is sound is the caller's
    // obligation, documented above.
    unsafe {
        core::arch::asm!(
            "ecall",
            in("a7") eid,
            in("a6") fid,
            inout("a0") arg0 as i64 => a0,
            inout("a1") arg1 as i64 => a1,
            in("a2") arg2,
            in("a3") arg3,
            options(nostack, preserves_flags),
        );
    }
    RiscvSbiRet { error: decode_sbi_error(a0), value: a1 as isize }
}

/// Convert an SBI error code to `zx_status::Status`.
fn riscv_status_to_zx_status(error: RiscvSbiError) -> Result<(), Status> {
    match error {
        RiscvSbiError::Success => Ok(()),
        RiscvSbiError::Failed => Err(Status::INTERNAL),
        RiscvSbiError::NotSupported => Err(Status::NOT_SUPPORTED),
        RiscvSbiError::InvalidParam | RiscvSbiError::InvalidAddress => Err(Status::INVALID_ARGS),
        RiscvSbiError::Denied => Err(Status::ACCESS_DENIED),
        RiscvSbiError::NoShmem => Err(Status::NO_RESOURCES),
        _ => Err(Status::BAD_STATE),
    }
}

/// Query whether a specific SBI extension is present on the platform.
fn sbi_extension_present(ext: SbiExtension) -> bool {
    let mask = 1u32 << (ext as u8);
    (SUPPORTED_EXTENSIONS.load(Ordering::Relaxed) & mask) != 0
}

/// Early initialization of SBI: probe extensions and register with power driver.
#[unsafe(no_mangle)]
pub extern "C" fn riscv64_sbi_early_init() {
    // Probe to see what extensions are present
    let extensions: [(SbiExtension, u64); 10] = [
        (SbiExtension::Base, SBI_EID_BASE),
        (SbiExtension::Timer, SBI_EID_TIMER),
        (SbiExtension::Ipi, SBI_EID_IPI),
        (SbiExtension::Rfence, SBI_EID_RFENCE),
        (SbiExtension::Hart, SBI_EID_HART),
        (SbiExtension::SystemReset, SBI_EID_SYSTEM_RESET),
        (SbiExtension::Pmu, SBI_EID_PMU),
        (SbiExtension::Dbcn, SBI_EID_DBCN),
        (SbiExtension::Susp, SBI_EID_SUSP),
        (SbiExtension::Cppc, SBI_EID_CPPC),
    ];

    // Base extension is always present
    let mut bitmap = 1u32 << (SbiExtension::Base as u8);

    // Probe the extension
    for (ext, eid) in extensions.iter().skip(1) {
        // SAFETY: Invokes SBI base extension probe call.
        let ret = unsafe { sbi_call_1(SBI_EID_BASE, SBI_BASE_PROBE_EXTENSION, *eid) };
        // It shouldn't be legal for the base probe extension call to return anything but success,
        // but check here anyway.
        if ret.error == RiscvSbiError::Success && ret.value != 0 {
            bitmap |= 1u32 << (*ext as u8);
        }
    }

    SUPPORTED_EXTENSIONS.store(bitmap, Ordering::Relaxed);

    // Register with the pdev power driver.
    unsafe { cpp_pdev_register_sbi_power() };
}

/// Secondary initialization of SBI: dump specs and probed capabilities.
#[unsafe(no_mangle)]
pub extern "C" fn riscv64_sbi_init() {
    // Dump SBI version info and extensions found in early probing
    // SAFETY: Reads SBI machine IDs and spec versions.
    unsafe {
        let mvendorid = sbi_call_0(SBI_EID_BASE, SBI_BASE_GET_MVENDORID).value;
        let marchid = sbi_call_0(SBI_EID_BASE, SBI_BASE_GET_MARCHID).value;
        let mimpid = sbi_call_0(SBI_EID_BASE, SBI_BASE_GET_MIMPID).value;
        dprintf!(
            INFO,
            "RISCV: mvendorid {:#x} marchid {:#x} mimpid {:#x}\n",
            mvendorid,
            marchid,
            mimpid
        );

        let spec_version = sbi_call_0(SBI_EID_BASE, SBI_BASE_GET_SPEC_VERSION).value as u64;
        let impl_id = sbi_call_0(SBI_EID_BASE, SBI_BASE_GET_IMPL_ID).value;
        let impl_version = sbi_call_0(SBI_EID_BASE, SBI_BASE_GET_IMPL_VERSION).value;
        dprintf!(
            INFO,
            "RISCV: SBI spec version {}.{} impl id {:#x} version {:#x}\n",
            (spec_version >> 24) & 0x7f,
            spec_version & ((1 << 24) - 1),
            impl_id,
            impl_version
        );

        dprintf!(INFO, "RISCV: extensions: ");
        let names: [(&str, SbiExtension); 10] = [
            ("BASE", SbiExtension::Base),
            ("TIMER", SbiExtension::Timer),
            ("IPI", SbiExtension::Ipi),
            ("RFENCE", SbiExtension::Rfence),
            ("HSM", SbiExtension::Hart),
            ("SRST", SbiExtension::SystemReset),
            ("PMU", SbiExtension::Pmu),
            ("DBCN", SbiExtension::Dbcn),
            ("SUSP", SbiExtension::Susp),
            ("CPPC", SbiExtension::Cppc),
        ];
        for (name, ext) in names {
            if sbi_extension_present(ext) {
                dprintf!(INFO, "{} ", name);
            }
        }
        dprintf!(INFO, "\n");
    }
}

/// Set the next timer interrupt deadline via SBI Timer extension.
#[inline(always)]
pub fn sbi_set_timer(stime_value: u64) -> RiscvSbiRet {
    // SAFETY: SBI Timer extension call.
    unsafe { sbi_call_1(SBI_EID_TIMER, SBI_TIMER_SET_TIMER, stime_value) }
}

/// Send an inter-processor interrupt to the harts specified by mask.
#[unsafe(no_mangle)]
pub extern "C" fn sbi_send_ipi(mask: u64, mask_base: u64) -> RiscvSbiRet {
    // SAFETY: SBI IPI extension call.
    unsafe { sbi_call_2(SBI_EID_IPI, SBI_IPI_SEND_IPI, mask, mask_base) }
}

/// Start execution on a secondary hart.
#[unsafe(no_mangle)]
extern "C" fn sbi_hart_start(hart_id: u64, start_addr: usize, priv_val: u64) -> Result<(), Status> {
    // SAFETY: SBI HSM extension call.
    let ret =
        unsafe { sbi_call_3(SBI_EID_HART, SBI_HART_START, hart_id, start_addr as u64, priv_val) };
    riscv_status_to_zx_status(ret.error)
}

/// Stop execution on the current local hart.
#[unsafe(no_mangle)]
extern "C" fn sbi_hart_stop() -> Result<(), Status> {
    // SAFETY: SBI HSM extension call.
    let ret = unsafe { sbi_call_0(SBI_EID_HART, SBI_HART_STOP) };
    riscv_status_to_zx_status(ret.error)
}

/// Get the execution state of a hart.
pub fn sbi_get_cpu_state_impl(hart_id: u64) -> Result<u32, Status> {
    // SAFETY: SBI HSM extension call.
    let ret = unsafe { sbi_call_1(SBI_EID_HART, SBI_HART_GET_STATUS, hart_id) };
    riscv_status_to_zx_status(ret.error)?;
    match ret.value as i64 {
        SBI_HART_STATE_STARTED => Ok(POWER_CPU_STATE_STARTED),
        SBI_HART_STATE_STOPPED => Ok(POWER_CPU_STATE_STOPPED),
        SBI_HART_STATE_START_PENDING => Ok(POWER_CPU_STATE_START_PENDING),
        SBI_HART_STATE_STOP_PENDING => Ok(POWER_CPU_STATE_STOP_PENDING),
        SBI_HART_STATE_SUSPENDED => Ok(POWER_CPU_STATE_SUSPENDED),
        SBI_HART_STATE_SUSPEND_PENDING => Ok(POWER_CPU_STATE_SUSPEND_PENDING),
        SBI_HART_STATE_RESUME_PENDING => Ok(POWER_CPU_STATE_RESUME_PENDING),
        _ => {
            // We should never reach here.
            Err(Status::INTERNAL)
        }
    }
}

/// # Safety
/// Caller guarantees valid out_state pointer.
#[unsafe(no_mangle)]
unsafe extern "C" fn sbi_get_cpu_state(hart_id: u64, out_state: *mut u32) -> Result<(), Status> {
    let state = sbi_get_cpu_state_impl(hart_id)?;
    unsafe { *out_state = state };
    Ok(())
}

/// # Safety
/// Caller guarantees valid out_state pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rust_sbi_get_cpu_state(
    hart_id: u64,
    out_state: *mut u32,
) -> Result<(), Status> {
    unsafe { sbi_get_cpu_state(hart_id, out_state) }
}

/// Instruct remote harts to execute `fence.i`.
#[unsafe(no_mangle)]
extern "C" fn sbi_remote_fencei(cpu_mask: u32) -> RiscvSbiRet {
    // SAFETY: Pure translation of a CPU mask to a HART mask; no preconditions.
    let hart_mask = unsafe { cpp_riscv64_cpu_mask_to_hart_mask(cpu_mask) };
    // SAFETY: SBI RFence extension call.
    unsafe { sbi_call_2(SBI_EID_RFENCE, SBI_RFENCE_FENCE_I, hart_mask, 0) }
}

/// Instruct remote harts to execute `sfence.vma` over an address range.
#[unsafe(no_mangle)]
extern "C" fn sbi_remote_sfence_vma(cpu_mask: u32, start: usize, size: usize) -> RiscvSbiRet {
    // SAFETY: Pure translation of a CPU mask to a HART mask; no preconditions.
    let hart_mask = unsafe { cpp_riscv64_cpu_mask_to_hart_mask(cpu_mask) };
    // SAFETY: SBI RFence extension call.
    unsafe {
        sbi_call_4(SBI_EID_RFENCE, SBI_RFENCE_SFENCE_VMA, hart_mask, 0, start as u64, size as u64)
    }
}

/// Instruct remote harts to execute `sfence.vma` over an address range for an ASID.
#[unsafe(no_mangle)]
extern "C" fn sbi_remote_sfence_vma_asid(
    cpu_mask: u32,
    start: usize,
    size: usize,
    asid: u64,
) -> RiscvSbiRet {
    // SAFETY: Pure translation of a CPU mask to a HART mask; no preconditions.
    let hart_mask = unsafe { cpp_riscv64_cpu_mask_to_hart_mask(cpu_mask) };
    // SAFETY: SBI RFence extension call.
    unsafe {
        let mut a0: i64;
        let mut a1: i64;
        core::arch::asm!(
            "ecall",
            in("a7") SBI_EID_RFENCE,
            in("a6") SBI_RFENCE_SFENCE_VMA_ASID,
            inout("a0") hart_mask as i64 => a0,
            inout("a1") 0i64 => a1,
            in("a2") start as u64,
            in("a3") size as u64,
            in("a4") asid,
            options(nostack, preserves_flags),
        );
        RiscvSbiRet { error: decode_sbi_error(a0), value: a1 as isize }
    }
}

/// Request system shutdown via SBI System Reset extension.
#[unsafe(no_mangle)]
extern "C" fn sbi_shutdown() -> Result<(), Status> {
    // SAFETY: SBI SRST extension call.
    let ret = unsafe {
        sbi_call_2(
            SBI_EID_SYSTEM_RESET,
            SBI_SRST_SYSTEM_RESET,
            SBI_RESET_TYPE_SHUTDOWN,
            SBI_RESET_REASON_NONE,
        )
    };
    riscv_status_to_zx_status(ret.error)
}

/// Request system warm reboot via SBI System Reset extension.
#[unsafe(no_mangle)]
extern "C" fn sbi_reset() -> Result<(), Status> {
    // SAFETY: SBI SRST extension call.
    let ret = unsafe {
        sbi_call_2(
            SBI_EID_SYSTEM_RESET,
            SBI_SRST_SYSTEM_RESET,
            SBI_RESET_TYPE_WARM_REBOOT,
            SBI_RESET_REASON_NONE,
        )
    };
    riscv_status_to_zx_status(ret.error)
}

#[cfg(ktest)]
/// Tests for RISC-V 64 SBI status code mapping and extension checks.
#[unittest::suite(name = "riscv64_sbi")]
mod tests {
    use super::{RiscvSbiError, riscv_status_to_zx_status};
    use unittest::assert_true;
    use zx_status::Status;

    /// Test SBI error code mapping to Zircon Status.
    #[test]
    fn test_sbi_error_mapping() {
        assert_true!(riscv_status_to_zx_status(RiscvSbiError::Success).is_ok());
        assert_true!(riscv_status_to_zx_status(RiscvSbiError::Failed) == Err(Status::INTERNAL));
        assert_true!(
            riscv_status_to_zx_status(RiscvSbiError::NotSupported) == Err(Status::NOT_SUPPORTED)
        );
        assert_true!(
            riscv_status_to_zx_status(RiscvSbiError::InvalidParam) == Err(Status::INVALID_ARGS)
        );
        assert_true!(
            riscv_status_to_zx_status(RiscvSbiError::Denied) == Err(Status::ACCESS_DENIED)
        );
        assert_true!(
            riscv_status_to_zx_status(RiscvSbiError::NoShmem) == Err(Status::NO_RESOURCES)
        );
    }
}
