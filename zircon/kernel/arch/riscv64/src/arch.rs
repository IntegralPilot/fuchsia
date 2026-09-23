// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! RISC-V 64 architectural initialization, control registers, and CPU lifecycle management.

use debug::dprintf;

#[allow(dead_code)]
const LOCAL_TRACE: u32 = 0;

// RISC-V CSR constants.
const RISCV64_CSR_SMODE_BITS: u16 = 1 << 8;

const RISCV64_CSR_CYCLE: u16 = 0xc00;
pub const RISCV64_CSR_TIME: u16 = 0xc01;
const RISCV64_CSR_INSRET: u16 = 0xc02;
const RISCV64_CSR_CYCLEH: u16 = 0xc80;
const RISCV64_CSR_TIMEH: u16 = 0xc81;
const RISCV64_CSR_INSRETH: u16 = 0xc82;

pub const RISCV64_CSR_SSTATUS: u16 = RISCV64_CSR_SMODE_BITS;
pub const RISCV64_CSR_SIE: u16 = 0x004 | RISCV64_CSR_SMODE_BITS;
const RISCV64_CSR_STVEC: u16 = 0x005 | RISCV64_CSR_SMODE_BITS;
const RISCV64_CSR_SCOUNTEREN: u16 = 0x006 | RISCV64_CSR_SMODE_BITS;
const RISCV64_CSR_SENVCFG: u16 = 0x00a | RISCV64_CSR_SMODE_BITS;
const RISCV64_CSR_SSCRATCH: u16 = 0x040 | RISCV64_CSR_SMODE_BITS;
const RISCV64_CSR_SEPC: u16 = 0x041 | RISCV64_CSR_SMODE_BITS;
const RISCV64_CSR_SCAUSE: u16 = 0x042 | RISCV64_CSR_SMODE_BITS;
pub const RISCV64_CSR_STVAL: u16 = 0x043 | RISCV64_CSR_SMODE_BITS;
const RISCV64_CSR_SIP: u16 = 0x044 | RISCV64_CSR_SMODE_BITS;
pub const RISCV64_CSR_STIMECMP: u16 = 0x04d | RISCV64_CSR_SMODE_BITS;
pub const RISCV64_CSR_SATP: u16 = 0x080 | RISCV64_CSR_SMODE_BITS;
pub const RISCV64_CSR_VLENB: u16 = 0xc22;

const RISCV64_CSR_SSTATUS_SIE: u64 = 1 << 1;
pub const RISCV64_CSR_SSTATUS_SPIE: u64 = 1 << 5;
const RISCV64_CSR_SSTATUS_UBE: u64 = 1 << 6;
pub const RISCV64_CSR_SSTATUS_SPP: u64 = 1 << 8;
pub const RISCV64_CSR_SSTATUS_VS_SHIFT: u64 = 9;
pub const RISCV64_CSR_SSTATUS_VS_MASK: u64 = 3 << 9;
const RISCV64_CSR_SSTATUS_VS_OFF: u64 = 0;
pub const RISCV64_CSR_SSTATUS_VS_INITIAL: u64 = 1 << 9;
pub const RISCV64_CSR_SSTATUS_VS_CLEAN: u64 = 2 << 9;
const RISCV64_CSR_SSTATUS_VS_DIRTY: u64 = 3 << 9;
pub const RISCV64_CSR_SSTATUS_FS_SHIFT: u64 = 13;
pub const RISCV64_CSR_SSTATUS_FS_MASK: u64 = 3 << 13;
const RISCV64_CSR_SSTATUS_FS_OFF: u64 = 0;
pub const RISCV64_CSR_SSTATUS_FS_INITIAL: u64 = 1 << 13;
pub const RISCV64_CSR_SSTATUS_FS_CLEAN: u64 = 2 << 13;
const RISCV64_CSR_SSTATUS_FS_DIRTY: u64 = 3 << 13;
const RISCV64_CSR_SSTATUS_SUM: u64 = 1 << 18;
const RISCV64_CSR_SSTATUS_MXR: u64 = 1 << 19;
const RISCV64_CSR_SSTATUS_UXL_MASK: u64 = 3 << 32;
const RISCV64_CSR_SSTATUS_UXL_32BIT: u64 = 1 << 32;
pub const RISCV64_CSR_SSTATUS_UXL_64BIT: u64 = 2 << 32;
const RISCV64_CSR_SSTATUS_UXL_128BIT: u64 = 3 << 32;
pub const RISCV64_CSR_SSTATUS_SD: u64 = 1 << 63;

const RISCV64_CSR_SIE_SSIE: u64 = 1 << 1;
pub const RISCV64_CSR_SIE_STIE: u64 = 1 << 5;
const RISCV64_CSR_SIE_SEIE: u64 = 1 << 9;

pub const RISCV64_CSR_SIP_SSIP: u64 = 1 << 1;
const RISCV64_CSR_SIP_STIP: u64 = 1 << 5;
const RISCV64_CSR_SIP_SEIP: u64 = 1 << 9;

const RISCV64_CSR_SCOUNTEREN_CY: u64 = 1 << 0;
const RISCV64_CSR_SCOUNTEREN_TM: u64 = 1 << 1;
const RISCV64_CSR_SCOUNTEREN_IR: u64 = 1 << 2;

const RISCV64_CSR_SENVCFG_FIOM: u64 = 1 << 0;
const RISCV64_CSR_SENVCFG_CBIE_MASK: u64 = 3 << 4;
const RISCV64_CSR_SENVCFG_CBIE_ILLEGAL: u64 = 0 << 4;
const RISCV64_CSR_SENVCFG_CBIE_FLUSH: u64 = 1 << 4;
const RISCV64_CSR_SENVCFG_CBIE_INVAL: u64 = 3 << 4;
const RISCV64_CSR_SENVCFG_CBCFE: u64 = 1 << 6;
const RISCV64_CSR_SENVCFG_CBZE: u64 = 1 << 7;

/// Opaque handle to the C++ `ArchPhysHandoff` handed off from physboot.
///
/// The C++ struct (`phys/arch/arch-handoff.h`) has `std::optional` members and so
/// cannot be described in Rust; its fields are read through the
/// `cpp_riscv64_handoff_*` accessors in `arch/riscv64/handoff.cc` instead of by
/// mirroring the layout here.  All of them are read exactly once at boot.
#[repr(C)]
pub struct ArchPhysHandoff {
    _opaque: [u8; 0],
}

unsafe extern "C" {
    pub fn cpp_riscv64_handoff_boot_hart_id(handoff: *const ArchPhysHandoff) -> u64;
    fn cpp_riscv64_mmu_early_init();
    fn cpp_riscv64_mmu_prevm_init();
    fn cpp_riscv64_mmu_init();
    fn cpp_mp_set_curr_cpu_online(online: bool);
    fn Riscv64ExceptionEntry();
}

/// Writes `val` into CSR `CSR`.
///
/// # Safety
/// Caller must ensure that writing `val` to `CSR` is valid for the current processor state.
#[inline(always)]
pub unsafe fn riscv64_csr_write<const CSR: u16>(val: u64) {
    // SAFETY: Assembly instruction csrw writes register val to CSR constant.
    // Omit `nomem` so this serves as a compiler barrier for control registers.
    unsafe {
        core::arch::asm!(
            "csrw {csr}, {val}",
            csr = const CSR,
            val = in(reg) val,
            options(nostack),
        );
    }
}

/// Reads the current value of CSR `CSR`.
///
/// # Safety
/// Caller must ensure reading `CSR` is valid for the current processor state.
#[inline(always)]
pub unsafe fn riscv64_csr_read<const CSR: u16>() -> u64 {
    let val: u64;
    // SAFETY: Assembly instruction csrr reads CSR constant into register val.
    // Omit `nomem` so CSR reads maintain ordering against memory operations.
    unsafe {
        core::arch::asm!(
            "csrr {val}, {csr}",
            csr = const CSR,
            val = out(reg) val,
            options(nostack),
        );
    }
    val
}

/// Sets bits specified by `val` in CSR `CSR`.
///
/// # Safety
/// Caller must ensure setting `val` in `CSR` is valid.
#[inline(always)]
pub unsafe fn riscv64_csr_set<const CSR: u16>(val: u64) {
    // SAFETY: Assembly instruction csrs sets bits in CSR constant.
    // Omit `nomem` so this serves as a compiler barrier.
    unsafe {
        core::arch::asm!(
            "csrs {csr}, {val}",
            csr = const CSR,
            val = in(reg) val,
            options(nostack),
        );
    }
}

/// Clears bits specified by `val` in CSR `CSR`.
///
/// # Safety
/// Caller must ensure clearing `val` in `CSR` is valid.
#[inline(always)]
pub unsafe fn riscv64_csr_clear<const CSR: u16>(val: u64) {
    // SAFETY: Assembly instruction csrc clears bits in CSR constant.
    // Omit `nomem` so this serves as a compiler barrier.
    unsafe {
        core::arch::asm!(
            "csrc {csr}, {val}",
            csr = const CSR,
            val = in(reg) val,
            options(nostack),
        );
    }
}

/// Hint to the processor that the CPU is in a spin-wait loop.
#[inline(always)]
pub fn arch_yield() {
    // SAFETY: Low-level RISC-V pause instruction.
    unsafe {
        core::arch::asm!("pause", options(nostack, preserves_flags));
    }
}

/// Query whether interrupts are disabled on the current CPU.
#[inline(always)]
pub fn arch_ints_disabled() -> bool {
    // SAFETY: Reading SSTATUS CSR to inspect the Supervisor Interrupt Enable (SIE) bit.
    unsafe { (riscv64_csr_read::<RISCV64_CSR_SSTATUS>() & RISCV64_CSR_SSTATUS_SIE) == 0 }
}

/// Disable supervisor interrupts on the current CPU.
#[inline(always)]
pub fn arch_disable_ints() {
    // SAFETY: Clears the Supervisor Interrupt Enable (SIE) bit in SSTATUS CSR.
    unsafe { riscv64_csr_clear::<RISCV64_CSR_SSTATUS>(RISCV64_CSR_SSTATUS_SIE) };
}

/// Enable supervisor interrupts on the current CPU.
#[inline(always)]
pub fn arch_enable_ints() {
    // SAFETY: Sets the Supervisor Interrupt Enable (SIE) bit in SSTATUS CSR.
    unsafe { riscv64_csr_set::<RISCV64_CSR_SSTATUS>(RISCV64_CSR_SSTATUS_SIE) };
}

/// First code to initialize each CPU.
#[unsafe(no_mangle)]
pub extern "C" fn riscv64_init_percpu() {
    // SAFETY: Sets up initial control registers (scratch, exception vector, sie, scounteren)
    // and zeros FPU state on the current CPU.
    unsafe {
        // set the top level exception handler
        riscv64_csr_write::<RISCV64_CSR_SSCRATCH>(0); // Handler expects zero.
        riscv64_csr_write::<RISCV64_CSR_STVEC>(
            (Riscv64ExceptionEntry as unsafe extern "C" fn()) as usize as u64,
        );

        // set up the default sstatus for the current cpu
        riscv64_csr_write::<RISCV64_CSR_SSTATUS>(0);

        // enable software interrupts and external interrupts, disable everything else
        riscv64_csr_write::<RISCV64_CSR_SIE>(RISCV64_CSR_SIE_SSIE | RISCV64_CSR_SIE_SEIE);

        // enable all of the counters
        riscv64_csr_write::<RISCV64_CSR_SCOUNTEREN>(
            RISCV64_CSR_SCOUNTEREN_CY | RISCV64_CSR_SCOUNTEREN_TM | RISCV64_CSR_SCOUNTEREN_IR,
        );

        // Zero out the fpu state, and set to initial
        super::fpu::riscv64_fpu_zero();
    }
}

/// Called in start.S prior to entering the main kernel.
/// Bootstraps the boot CPU as CPU 0 intrinsically, though it may have a nonzero hart.
/// # Safety
/// `arch_handoff` must point to a valid `ArchPhysHandoff` struct.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn riscv64_boot_cpu_init(arch_handoff: *const ArchPhysHandoff) {
    debug_assert!(!arch_handoff.is_null());
    // SAFETY: Caller guarantees `arch_handoff` is non-null and valid.
    let hart_id = unsafe { cpp_riscv64_handoff_boot_hart_id(arch_handoff) } as u32;
    riscv64_init_percpu();
    super::mp::riscv64_mp_early_init_percpu(hart_id, 0);
    // SAFETY: Caller guarantees `arch_handoff` is non-null and valid.
    unsafe { super::feature::riscv64_feature_early_init(arch_handoff) };
}

/// # Safety
/// `arch_handoff` must point to a valid `ArchPhysHandoff` struct.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ArchPostHandoffBootstrap(arch_handoff: *const ArchPhysHandoff) {
    // SAFETY: Forwarded directly from boot assembly with valid `arch_handoff`.
    unsafe { riscv64_boot_cpu_init(arch_handoff) };
}

/// Architecture early initialization before MMU/heap.
#[unsafe(no_mangle)]
pub extern "C" fn arch_early_init() {
    super::sbi::riscv64_sbi_early_init();
    // SAFETY: Calls early init routines for MMU, then marks boot CPU online.
    unsafe {
        cpp_riscv64_mmu_early_init();

        // mark the boot cpu online
        cpp_mp_set_curr_cpu_online(true);
    }
}

/// Architecture pre-VM initialization.
#[unsafe(no_mangle)]
pub extern "C" fn arch_prevm_init() {
    // SAFETY: Calls MMU pre-VM init.
    unsafe {
        cpp_riscv64_mmu_prevm_init();
    }
}

/// Architecture main initialization after heap/MMU are available.
#[unsafe(no_mangle)]
pub extern "C" fn arch_init() {
    // print some arch info
    dprintf!(INFO, "RISCV: Boot HART ID {}\n", super::boot_hart_id());
    dprintf!(INFO, "RISCV: Supervisor mode\n");

    super::feature::riscv64_feature_init();
    super::sbi::riscv64_sbi_init();

    // SAFETY: Calls MMU initialization.
    unsafe {
        cpp_riscv64_mmu_init();
    }
}

/// Architecture late per-CPU initialization.
#[unsafe(no_mangle)]
pub extern "C" fn arch_late_init_percpu() {
    // While it would be nicer to zero out vector state - and set it to initial -
    // earlier and next to the call to do so for FPU state, that is too early for
    // vector feature bit to have been set.
    if super::feature::has_vector() {
        // SAFETY: Zeroes the vector registers on the current CPU; only reached once the
        // vector extension is known to be present.
        unsafe { super::vector::riscv64_vector_zero() };
    }

    if super::feature::has_zicbom() {
        // allow userspace to perform FLUSH and CLEAN operations, but forbid INVAL
        // SAFETY: Configures the SENVCFG cache-block bits on the current CPU.
        unsafe {
            riscv64_csr_set::<RISCV64_CSR_SENVCFG>(RISCV64_CSR_SENVCFG_CBCFE);
            riscv64_csr_clear::<RISCV64_CSR_SENVCFG>(RISCV64_CSR_SENVCFG_CBIE_MASK);
            riscv64_csr_set::<RISCV64_CSR_SENVCFG>(RISCV64_CSR_SENVCFG_CBIE_ILLEGAL);
        }
    }
    if super::feature::has_zicboz() {
        // Allow user space to perform zeroing
        // SAFETY: Configures the SENVCFG cache-block-zero enable on the current CPU.
        unsafe { riscv64_csr_set::<RISCV64_CSR_SENVCFG>(RISCV64_CSR_SENVCFG_CBZE) };
    }

    // per cpu on each secondary (and the boot cpu a second time)
    // SAFETY: Marks the current CPU online in the C++ MP layer.
    unsafe { cpp_mp_set_curr_cpu_online(true) };
}

/// Places the CPU in a low-power idle state waiting for an interrupt.
#[unsafe(no_mangle)]
pub extern "C" fn arch_enter_idle_state() {
    // SAFETY: WFI puts the CPU in low power wait-for-interrupt state.
    // Omit `nomem` so this serves as a compiler barrier against memory reordering across idle.
    unsafe {
        core::arch::asm!("wfi", options(nostack));
    }
}

#[cfg(ktest)]
/// Tests for RISC-V 64 architectural initialization and CSRs.
#[unittest::suite(name = "riscv64_arch")]
mod tests {
    use unittest::assert_eq;

    /// Test RISC-V CSR constant values.
    #[test]
    fn test_csr_constants() {
        assert_eq!(RISCV64_CSR_SSTATUS, 0x100);
        assert_eq!(RISCV64_CSR_SIE, 0x104);
        assert_eq!(RISCV64_CSR_STVEC, 0x105);
        assert_eq!(RISCV64_CSR_SCOUNTEREN, 0x106);
        assert_eq!(RISCV64_CSR_SENVCFG, 0x10a);
        assert_eq!(RISCV64_CSR_SSCRATCH, 0x140);
        assert_eq!(RISCV64_CSR_SEPC, 0x141);
        assert_eq!(RISCV64_CSR_SCAUSE, 0x142);
        assert_eq!(RISCV64_CSR_STVAL, 0x143);
        assert_eq!(RISCV64_CSR_SIP, 0x144);
        assert_eq!(RISCV64_CSR_STIMECMP, 0x14d);
        assert_eq!(RISCV64_CSR_SATP, 0x180);
    }
}
