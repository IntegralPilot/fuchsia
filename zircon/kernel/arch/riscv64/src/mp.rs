// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Multi-Processing (MP) and CPU topology support for RISC-V 64.

use crate::kernel::mp::{MpIpi, MpIpiTarget, MpUnplugEvent, SMP_MAX_CPUS};
use crate::kernel::types::{cpu_mask_t, cpu_num_t};
use core::sync::atomic::{AtomicU32, Ordering};
use zx_status::Status;

/// A physical RISC-V hart id, as opposed to a logical `cpu_num_t`.
#[allow(non_camel_case_types)]
type hart_id_t = u32;

unsafe extern "C" {
    pub static mut riscv64_num_cpus: u32;
    fn cpp_mp_get_online_mask() -> cpu_mask_t;
    fn cpp_interrupt_send_ipi(cpu_mask: cpu_mask_t, ipi: MpIpi) -> Result<(), Status>;
    fn cpp_interrupt_init_percpu();
    fn cpp_mp_mbx_reschedule_irq();
    fn cpp_mp_mbx_generic_irq();
    fn cpp_mp_mbx_interrupt_irq();
    fn cpp_mp_is_cpu_online(cpu_id: cpu_num_t) -> bool;
    fn cpp_riscv64_start_cpu(cpu_id: cpu_num_t, hart_id: hart_id_t) -> Result<(), Status>;
    fn cpp_platform_halt_cpu();
}

/// Per-CPU structure aligned on the architectural cache line boundary (64 bytes).
type Riscv64Percpu = riscv64_mp_bindings::riscv64_percpu;
// Ensure Riscv64Percpu layout matches the C++ struct layout.
const _: () = {
    assert!(core::mem::size_of::<Riscv64Percpu>() == 128);
    assert!(core::mem::align_of::<Riscv64Percpu>() == 64);
    assert!(core::mem::offset_of!(Riscv64Percpu, in_restricted_mode) == 24);
    assert!(core::mem::offset_of!(Riscv64Percpu, ipi_data) == 64);
};

const CPU_NUM_OFFSET: usize = core::mem::offset_of!(Riscv64Percpu, cpu_num);
const HART_ID_OFFSET: usize = core::mem::offset_of!(Riscv64Percpu, hart_id);
const NUM_SPINLOCKS_OFFSET: usize = core::mem::offset_of!(Riscv64Percpu, num_spinlocks);
const BLOCKING_DISALLOWED_OFFSET: usize = core::mem::offset_of!(Riscv64Percpu, blocking_disallowed);
const IN_RESTRICTED_MODE_OFFSET: usize = core::mem::offset_of!(Riscv64Percpu, in_restricted_mode);

/// Mapping from logical CPU number to physical Hart ID.
///
/// Kept separate from the percpu array for speed purposes.
static CPU_TO_HART_MAP: [AtomicU32; SMP_MAX_CPUS] = [const { AtomicU32::new(0) }; SMP_MAX_CPUS];

/// Global array of per-CPU structures.
///
/// Each CPU points to its own entry using the fixed register (`s11`).
pub static mut RISCV64_PERCPU_ARRAY: [Riscv64Percpu; SMP_MAX_CPUS] = [const {
    Riscv64Percpu {
        cpu_num: 0,
        hart_id: 0,
        blocking_disallowed: 0,
        num_spinlocks: 0,
        high_level_percpu: core::ptr::null_mut(),
        in_restricted_mode: 0,
        __bindgen_padding_0: [0; 9],
        ipi_data: AtomicU32::new(0),
    }
}; SMP_MAX_CPUS];

/// Set the per-CPU pointer register (s11 / x27).
#[inline(always)]
fn riscv64_set_percpu(ptr: *mut Riscv64Percpu) {
    // SAFETY: Assembly instruction mv sets s11 to the percpu pointer.
    // Omit `nomem` so prior stores to initialize percpu are ordered before s11 publication.
    unsafe {
        core::arch::asm!("mv s11, {}", in(reg) ptr, options(nostack, preserves_flags));
    }
}

/// Read the per-CPU pointer register (s11 / x27).
#[inline(always)]
pub fn riscv64_read_percpu_ptr() -> *mut Riscv64Percpu {
    let ptr: *mut Riscv64Percpu;
    // SAFETY: Assembly instruction mv reads s11 into register.
    unsafe {
        core::arch::asm!("mv {}, s11", out(reg) ptr, options(nostack, preserves_flags));
    }
    ptr
}

/// Reads a 32-bit field directly from the `s11` per-CPU pointer register.
#[inline(always)]
unsafe fn read_percpu_u32<const OFFSET: usize>() -> u32 {
    let val: u32;
    // SAFETY: Single-instruction lwu relative to s11 percpu pointer.
    unsafe {
        core::arch::asm!(
            "lwu {val}, {offset}(s11)",
            val = out(reg) val,
            offset = const OFFSET,
            options(nostack, preserves_flags),
        );
    }
    val
}

/// Writes a 32-bit field directly to the `s11` per-CPU pointer register.
#[inline(always)]
unsafe fn write_percpu_u32<const OFFSET: usize>(val: u32) {
    // SAFETY: Single-instruction sw relative to s11 percpu pointer.
    unsafe {
        core::arch::asm!(
            "sw {val}, {offset}(s11)",
            val = in(reg) val,
            offset = const OFFSET,
            options(nostack, preserves_flags),
        );
    }
}

/// Get the maximum number of detected CPUs.
#[inline(always)]
fn arch_max_num_cpus() -> u32 {
    // SAFETY: `riscv64_num_cpus` is the C++ global defined in mp.cc. It is written
    // once during topology discovery on the boot CPU, before any secondary hart is
    // started, and only read afterwards -- the same unsynchronized access the C++
    // `arch_max_num_cpus`/`arch_set_num_cpus` inlines perform.
    unsafe { riscv64_num_cpus }
}

/// Set the number of active/detected CPUs.
#[inline(always)]
pub fn arch_set_num_cpus(cpu_count: u32) {
    // SAFETY: `riscv64_num_cpus` is the C++ global defined in mp.cc. It is written
    // once during topology discovery on the boot CPU, before any secondary hart is
    // started, and only read afterwards -- the same unsynchronized access the C++
    // `arch_max_num_cpus`/`arch_set_num_cpus` inlines perform.
    unsafe { riscv64_num_cpus = cpu_count };
}

/// Translate a logical CPU number to its physical Hart ID.
#[unsafe(no_mangle)]
pub extern "C" fn arch_cpu_num_to_hart_id(cpu_num: cpu_num_t) -> hart_id_t {
    if (cpu_num as usize) < SMP_MAX_CPUS {
        CPU_TO_HART_MAP[cpu_num as usize].load(Ordering::Relaxed)
    } else {
        0
    }
}

/// Get the boot CPU's physical Hart ID.
#[unsafe(no_mangle)]
pub extern "C" fn riscv64_boot_hart_id() -> hart_id_t {
    // SAFETY: Accesses the statically allocated, globally valid RISCV64_PERCPU_ARRAY.
    unsafe { RISCV64_PERCPU_ARRAY[0].hart_id }
}

/// Get the current logical CPU number from s11 percpu pointer.
#[inline(always)]
pub fn arch_curr_cpu_num() -> cpu_num_t {
    // SAFETY: Single-instruction read of cpu_num from s11 percpu struct.
    unsafe { read_percpu_u32::<CPU_NUM_OFFSET>() }
}

/// Get the current CPU's physical Hart ID.
#[unsafe(no_mangle)]
pub extern "C" fn riscv64_curr_hart_id() -> hart_id_t {
    // SAFETY: Single-instruction read of hart_id from s11 percpu struct.
    unsafe { read_percpu_u32::<HART_ID_OFFSET>() }
}

/// Increments the count of spinlocks held on the current CPU.
#[inline(always)]
pub fn percpu_inc_num_spinlocks() {
    // SAFETY: Single-instruction read of cpu_num from s11 percpu struct.
    unsafe {
        write_percpu_u32::<NUM_SPINLOCKS_OFFSET>(
            read_percpu_u32::<NUM_SPINLOCKS_OFFSET>().wrapping_add(1),
        )
    }
}

/// Decrements the count of spinlocks held on the current CPU.
#[inline(always)]
pub fn percpu_dec_num_spinlocks() {
    // SAFETY: Single-instruction read of cpu_num from s11 percpu struct.
    unsafe {
        write_percpu_u32::<NUM_SPINLOCKS_OFFSET>(
            read_percpu_u32::<NUM_SPINLOCKS_OFFSET>().wrapping_sub(1),
        )
    }
}

/// Returns the number of spinlocks held on the current CPU.
#[inline(always)]
pub fn num_spinlocks_held() -> u32 {
    // SAFETY: Single-instruction read of cpu_num from s11 percpu struct.
    unsafe { read_percpu_u32::<NUM_SPINLOCKS_OFFSET>() }
}

/// Early initialization for a CPU's per-CPU structure and hart tracking.
///
/// Called once per cpu, sets up the percpu structure and tracks cpu number to
/// hart id.
#[unsafe(no_mangle)]
pub extern "C" fn riscv64_mp_early_init_percpu(hart_id: hart_id_t, cpu_num: cpu_num_t) {
    let cpu_idx = cpu_num as usize;
    if cpu_idx < SMP_MAX_CPUS {
        // SAFETY: Only written during early CPU init sequentially, array bounds guaranteed by SMP_MAX_CPUS check.
        // `riscv64_set_percpu` receives a valid pointer derived from the static array.
        unsafe {
            RISCV64_PERCPU_ARRAY[cpu_idx].cpu_num = cpu_num;
            RISCV64_PERCPU_ARRAY[cpu_idx].hart_id = hart_id;
            riscv64_set_percpu(&raw mut RISCV64_PERCPU_ARRAY[cpu_idx]);
        }
        CPU_TO_HART_MAP[cpu_idx].store(hart_id, Ordering::Release);
        core::sync::atomic::fence(Ordering::Release);
    }
}

/// Associate the high-level percpu structure with a CPU.
/// # Safety
/// Caller guarantees valid percpu pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn arch_setup_percpu(cpu_num: cpu_num_t, percpu: *mut core::ffi::c_void) {
    let cpu_idx = cpu_num as usize;
    if cpu_idx < SMP_MAX_CPUS {
        // SAFETY: Only written during early CPU init sequentially, array bounds guaranteed by SMP_MAX_CPUS check.
        // The pointer validity for `percpu` is deferred to the caller per the function's Safety docs.
        unsafe {
            RISCV64_PERCPU_ARRAY[cpu_idx].high_level_percpu = percpu as *mut _;
        }
    }
}

/// Translate a bitmap of CPU numbers to a bitmap of Hart IDs.
#[unsafe(no_mangle)]
pub extern "C" fn riscv64_cpu_mask_to_hart_mask(mut cmask: cpu_mask_t) -> u64 {
    let mut hmask = 0u64;
    let num_cpus = arch_max_num_cpus();
    let mut cpu = 0u32;
    while cpu < num_cpus && cmask != 0 {
        if (cmask & 1) != 0 {
            let hart = arch_cpu_num_to_hart_id(cpu);
            // set the bit in the hart mask
            hmask |= 1u64 << hart;
        }
        cmask >>= 1;
        cpu += 1;
    }
    hmask
}

/// Trigger task rescheduling on the CPUs specified by `mask`.
#[unsafe(no_mangle)]
pub extern "C" fn arch_mp_reschedule(mask: cpu_mask_t) {
    arch_mp_send_ipi(MpIpiTarget::Mask, mask, MpIpi::Reschedule);
}

/// Send an inter-processor interrupt to the target CPUs.
#[unsafe(no_mangle)]
pub extern "C" fn arch_mp_send_ipi(target: MpIpiTarget, mut cpu_mask: cpu_mask_t, ipi: MpIpi) {
    // translate the high level target + mask mechanism into just a mask
    match target {
        MpIpiTarget::All => {
            // SAFETY: Reads the C++ layer online CPU mask scalar.
            cpu_mask = unsafe { cpp_mp_get_online_mask() };
        }
        MpIpiTarget::AllButLocal => {
            let curr_cpu = arch_curr_cpu_num();
            // SAFETY: Reads the C++ layer online CPU mask scalar.
            cpu_mask = (unsafe { cpp_mp_get_online_mask() }) & !(1u32 << curr_cpu);
        }
        MpIpiTarget::Mask => {}
    }

    // no need to continue if the computed mask is 0
    if cpu_mask == 0 {
        return;
    }

    // Try the pdev based interrupt method first (e.g. AIA / PLIC), otherwise fall
    // back to SBI below.
    // SAFETY: FFI call to C++ interrupt setup. Bounds/masking handled inside C++.
    if unsafe { cpp_interrupt_send_ipi(cpu_mask, ipi) }.is_ok() {
        return;
    }

    // Translate the cpu mask to a list of harts: set the hart mask and set the
    // pending ipi bit in the per cpu struct.
    let mut hart_mask = 0u64;
    let mut cmask = cpu_mask;
    let num_cpus = arch_max_num_cpus();
    let mut cpu = 0u32;
    while cpu < num_cpus && cmask != 0 {
        if (cmask & 1) != 0 {
            let hart = arch_cpu_num_to_hart_id(cpu);
            // record a pending hart to notify
            hart_mask |= 1u64 << hart;
            // mark the pending ipi in the cpu
            // SAFETY: Bounds checked by `num_cpus` loop condition which is <= SMP_MAX_CPUS.
            unsafe {
                RISCV64_PERCPU_ARRAY[cpu as usize]
                    .ipi_data
                    .fetch_or(1u32 << (ipi as u8), Ordering::Relaxed);
            }
        }
        cmask >>= 1;
        cpu += 1;
    }

    core::sync::atomic::fence(Ordering::SeqCst);
    let ret = super::sbi::sbi_send_ipi(hart_mask, 0);
    debug_assert!(ret.error == super::sbi::RiscvSbiError::Success);
}

/// Handle a software-triggered supervisor interrupt (cross-CPU IPI).
///
/// Software triggered exceptions are used for cross-cpu calls.
#[unsafe(no_mangle)]
pub extern "C" fn riscv64_software_exception() {
    // Clear the IPI by clearing the pending software IPI bit.
    // SAFETY: Clear the pending supervisor software interrupt (SSIP) in sip CSR.
    unsafe {
        core::arch::asm!(
            "csrrc zero, sip, {ssip}",
            ssip = in(reg) 1u64 << 1, // RISCV64_CSR_SIP_SSIP
            options(nostack, preserves_flags)
        );
    }
    core::sync::atomic::fence(Ordering::Acquire);

    let percpu_ptr = riscv64_read_percpu_ptr();
    let mut reason = if !percpu_ptr.is_null() {
        // SAFETY: Pointer is validated to be non-null and corresponds to the active percpu block.
        unsafe { (*percpu_ptr).ipi_data.swap(0, Ordering::AcqRel) }
    } else {
        0
    };

    if (reason & (1u32 << (crate::kernel::mp::MpIpi::Reschedule as u8))) != 0 {
        // SAFETY: Delegating generic IPI callback processing to C++.
        unsafe { cpp_mp_mbx_reschedule_irq() };
        reason &= !(1u32 << (crate::kernel::mp::MpIpi::Reschedule as u8));
    }
    if (reason & (1u32 << (crate::kernel::mp::MpIpi::Generic as u8))) != 0 {
        // SAFETY: Delegating generic IPI callback processing to C++.
        unsafe { cpp_mp_mbx_generic_irq() };
        reason &= !(1u32 << (crate::kernel::mp::MpIpi::Generic as u8));
    }
    if (reason & (1u32 << (crate::kernel::mp::MpIpi::Interrupt as u8))) != 0 {
        // SAFETY: Delegating interrupt IPI callback processing to C++.
        unsafe { cpp_mp_mbx_interrupt_irq() };
        reason &= !(1u32 << (crate::kernel::mp::MpIpi::Interrupt as u8));
    }
    if (reason & (1u32 << (crate::kernel::mp::MpIpi::Halt as u8))) != 0 {
        super::arch::arch_disable_ints();
        core::sync::atomic::fence(Ordering::SeqCst);
        // Park this core in a WFI loop.
        loop {
            // SAFETY: `wfi` only parks the hart until an interrupt is pending. It
            // touches no memory and no registers, hence `nostack, preserves_flags`.
            unsafe { core::arch::asm!("wfi", options(nostack, preserves_flags)) };
        }
    }

    if reason != 0 {
        panic!("RISCV: unhandled ipi cause {:#x}", reason);
    }
}

/// Initialize per-CPU interrupts.
#[unsafe(no_mangle)]
pub extern "C" fn arch_mp_init_percpu() {
    // SAFETY: takes no arguments; initializes the calling CPU's interrupt controller state.
    unsafe { cpp_interrupt_init_percpu() };
}

/// Flush CPU state and halt execution on CPU unplug.
///
/// # Safety
/// `flush_done` must point at a live `MpUnplugEvent` that stays alive until the
/// unplugging CPU observes the signal. The generic mp layer owns it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn arch_flush_state_and_halt(flush_done: *mut MpUnplugEvent) -> ! {
    debug_assert!(super::arch::arch_ints_disabled());
    crate::kernel::thread::preempt_disable();
    // SAFETY: the caller guarantees `flush_done` points at a live `MpUnplugEvent`;
    // signalling it is the last thing this CPU does before halting, and
    // `cpp_platform_halt_cpu` takes no arguments and does not return.
    unsafe {
        crate::kernel::mp::unplug_event_signal(flush_done);
        cpp_platform_halt_cpu();
    }
    panic!("control should never reach here");
}

/// Prepare for CPU unplug.
#[unsafe(no_mangle)]
pub extern "C" fn arch_mp_prep_cpu_unplug(cpu_id: cpu_num_t) -> Result<(), Status> {
    // we do not allow unplugging the bootstrap processor
    if cpu_id == 0 || cpu_id >= arch_max_num_cpus() {
        return Err(Status::INVALID_ARGS);
    }
    Ok(())
}

/// Unplug a secondary CPU.
#[unsafe(no_mangle)]
pub extern "C" fn arch_mp_cpu_unplug(cpu_id: u32) -> Result<(), Status> {
    // we do not allow unplugging the bootstrap processor
    if cpu_id == 0 || cpu_id >= arch_max_num_cpus() {
        return Err(Status::INVALID_ARGS);
    }
    Ok(())
}

/// Hotplug a secondary CPU.
#[unsafe(no_mangle)]
pub extern "C" fn arch_mp_cpu_hotplug(cpu_id: u32) -> Result<(), Status> {
    if cpu_id == 0 || cpu_id >= arch_max_num_cpus() {
        return Err(Status::INVALID_ARGS);
    }
    // SAFETY: takes a CPU number by value and queries the generic online mask.
    if unsafe { cpp_mp_is_cpu_online(cpu_id) } {
        return Err(Status::BAD_STATE);
    }
    let hart_id = arch_cpu_num_to_hart_id(cpu_id);
    // SAFETY: both arguments are plain integers, and `cpu_id` was bounds-checked
    // against `arch_max_num_cpus()` above.
    unsafe { cpp_riscv64_start_cpu(cpu_id, hart_id) }
}
