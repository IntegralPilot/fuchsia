// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

pub mod apic;
pub mod fake_msr_access;
pub mod feature;
pub mod interrupts;
pub mod ioapic;
pub mod lapic;
pub mod platform_access;
pub mod pv;
pub mod registers;
pub mod suspend;
pub mod system_topology;
pub mod x86;

/// Architecture-specific saved normal mode state for x86_64.
///
/// Saves the normal mode `fs_base` and `gs_base` MSR values across restricted mode entry.
#[repr(C)]
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ArchSavedNormalState {
    pub normal_fs_base: u64,
    pub normal_gs_base: u64,
}

zr::static_assert!(core::mem::size_of::<ArchSavedNormalState>() == 16);
zr::static_assert!(core::mem::align_of::<ArchSavedNormalState>() == 8);

use debug::ltracef;
use zx_status::Status;
use zx_types::{zx_restricted_state_t, zx_status_t, zx_thread_state_general_regs_t};

const LOCAL_TRACE: u32 = 0;

unsafe extern "C" {
    fn cpp_x86_get_fsgsbase_ints_disabled(fsbase: *mut u64, gsbase: *mut u64);
    fn cpp_x86_get_fsgsbase_ints_enabled(fsbase: *mut u64, gsbase: *mut u64);
    fn cpp_x86_set_fsgsbase(fsbase: u64, gsbase: u64);
    fn cpp_x86_enter_uspace(iframe: *const Iframe) -> !;
    fn cpp_x86_get_general_regs(regs: *mut zx_thread_state_general_regs_t) -> i32;
    fn cpp_x86_set_general_regs(regs: *const zx_thread_state_general_regs_t) -> i32;
}

pub const MAX_CACHE_LINE: usize = 64;
#[repr(align(64))]
pub struct CpuAlignMarker;

/// The mask of all RFLAGS bits that user code is permitted to set or modify:
/// * CF (Carry Flag): bit 0 (`0x1`)
/// * PF (Parity Flag): bit 2 (`0x4`)
/// * AF (Auxiliary Carry Flag): bit 4 (`0x10`)
/// * ZF (Zero Flag): bit 6 (`0x40`)
/// * SF (Sign Flag): bit 7 (`0x80`)
/// * TF (Trap Flag): bit 8 (`0x100`)
/// * DF (Direction Flag): bit 10 (`0x400`)
/// * OF (Overflow Flag): bit 11 (`0x800`)
/// * NT (Nested Task Flag): bit 14 (`0x4000`)
/// * AC (Alignment Check Flag): bit 18 (`0x40000`)
/// * ID (Identification Flag): bit 21 (`0x200000`)
///
/// Combining these status, control, and system flags yields:
/// `0x1 | 0x4 | 0x10 | 0x40 | 0x80 | 0x100 | 0x400 | 0x800 | 0x4000 | 0x40000 | 0x200000 = 0x244dd5`.
///
/// See [intel/vol1]: Figure 3-8. EFLAGS Register, and
/// [amd/vol1]: Figure 3-7. RFLAGS Register.
const X86_FLAGS_USER: u64 = 0x244dd5;

/// RFLAGS Interrupt Enable Flag (IF), bit 9 (`1 << 9`).
///
/// See [intel/vol1]: Figure 3-8. EFLAGS Register, and
/// [amd/vol1]: Figure 3-7. RFLAGS Register.
const X86_FLAGS_IF: u64 = 1 << 9;

/// User 64-bit code segment selector (GDT index 6 with Requested Privilege Level 3: `(6 << 3) | 3 = 0x33`).
///
/// See [intel/vol3]: 3.4.2 Segment Selectors, and
/// [amd/vol2]: 4.5.1 Segment Selectors.
const USER_CODE_64_SELECTOR: u64 = 0x30 | 3;

/// User data segment selector (GDT index 5 with Requested Privilege Level 3: `(5 << 3) | 3 = 0x2b`).
///
/// See [intel/vol3]: 3.4.2 Segment Selectors, and
/// [amd/vol2]: 4.5.1 Segment Selectors.
const USER_DATA_SELECTOR: u64 = 0x28 | 3;

#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
pub struct Iframe {
    pub rdi: u64,
    pub rsi: u64,
    pub rbp: u64,
    pub rbx: u64,
    pub rdx: u64,
    pub rcx: u64,
    pub rax: u64,
    pub r8: u64,
    pub r9: u64,
    pub r10: u64,
    pub r11: u64,
    pub r12: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,
    pub vector: u64,
    pub err_code: u64,
    pub ip: u64,
    pub cs: u64,
    pub flags: u64,
    pub user_sp: u64,
    pub user_ss: u64,
}

#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
pub struct SyscallRegs {
    pub rax: u64,
    pub rbx: u64,
    pub rcx: u64,
    pub rdx: u64,
    pub rsi: u64,
    pub rdi: u64,
    pub rbp: u64,
    pub r8: u64,
    pub r9: u64,
    pub r10: u64,
    pub r11: u64,
    pub r12: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,
    pub rip: u64,
    pub rflags: u64,
    pub rsp: u64,
}

zr::static_assert!(core::mem::size_of::<Iframe>() == 176);
zr::static_assert!(core::mem::align_of::<Iframe>() == 8);
zr::static_assert!(core::mem::offset_of!(Iframe, rdi) == 0);
zr::static_assert!(core::mem::offset_of!(Iframe, rsi) == 8);
zr::static_assert!(core::mem::offset_of!(Iframe, rbp) == 16);
zr::static_assert!(core::mem::offset_of!(Iframe, rbx) == 24);
zr::static_assert!(core::mem::offset_of!(Iframe, rdx) == 32);
zr::static_assert!(core::mem::offset_of!(Iframe, rcx) == 40);
zr::static_assert!(core::mem::offset_of!(Iframe, rax) == 48);
zr::static_assert!(core::mem::offset_of!(Iframe, r8) == 56);
zr::static_assert!(core::mem::offset_of!(Iframe, r9) == 64);
zr::static_assert!(core::mem::offset_of!(Iframe, r10) == 72);
zr::static_assert!(core::mem::offset_of!(Iframe, r11) == 80);
zr::static_assert!(core::mem::offset_of!(Iframe, r12) == 88);
zr::static_assert!(core::mem::offset_of!(Iframe, r13) == 96);
zr::static_assert!(core::mem::offset_of!(Iframe, r14) == 104);
zr::static_assert!(core::mem::offset_of!(Iframe, r15) == 112);
zr::static_assert!(core::mem::offset_of!(Iframe, vector) == 120);
zr::static_assert!(core::mem::offset_of!(Iframe, err_code) == 128);
zr::static_assert!(core::mem::offset_of!(Iframe, ip) == 136);
zr::static_assert!(core::mem::offset_of!(Iframe, cs) == 144);
zr::static_assert!(core::mem::offset_of!(Iframe, flags) == 152);
zr::static_assert!(core::mem::offset_of!(Iframe, user_sp) == 160);
zr::static_assert!(core::mem::offset_of!(Iframe, user_ss) == 168);

zr::static_assert!(core::mem::size_of::<SyscallRegs>() == 144);
zr::static_assert!(core::mem::align_of::<SyscallRegs>() == 8);
zr::static_assert!(core::mem::offset_of!(SyscallRegs, rax) == 0);
zr::static_assert!(core::mem::offset_of!(SyscallRegs, rbx) == 8);
zr::static_assert!(core::mem::offset_of!(SyscallRegs, rcx) == 16);
zr::static_assert!(core::mem::offset_of!(SyscallRegs, rdx) == 24);
zr::static_assert!(core::mem::offset_of!(SyscallRegs, rsi) == 32);
zr::static_assert!(core::mem::offset_of!(SyscallRegs, rdi) == 40);
zr::static_assert!(core::mem::offset_of!(SyscallRegs, rbp) == 48);
zr::static_assert!(core::mem::offset_of!(SyscallRegs, r8) == 56);
zr::static_assert!(core::mem::offset_of!(SyscallRegs, r9) == 64);
zr::static_assert!(core::mem::offset_of!(SyscallRegs, r10) == 72);
zr::static_assert!(core::mem::offset_of!(SyscallRegs, r11) == 80);
zr::static_assert!(core::mem::offset_of!(SyscallRegs, r12) == 88);
zr::static_assert!(core::mem::offset_of!(SyscallRegs, r13) == 96);
zr::static_assert!(core::mem::offset_of!(SyscallRegs, r14) == 104);
zr::static_assert!(core::mem::offset_of!(SyscallRegs, r15) == 112);
zr::static_assert!(core::mem::offset_of!(SyscallRegs, rip) == 120);
zr::static_assert!(core::mem::offset_of!(SyscallRegs, rflags) == 128);
zr::static_assert!(core::mem::offset_of!(SyscallRegs, rsp) == 136);

/// Entry state for a thread.
#[allow(unused_imports)]
pub use arch_types_bindings::{GeneralRegsSource, UserEntryState};

/// Checks if a virtual address is accessible to user mode on x86_64.
///
/// This address refers to userspace if it is in the lower half of the
/// canonical addresses (i.e., if all of the bits in the canonical address
/// mask are zero).
#[inline]
pub fn is_user_accessible(va: usize) -> bool {
    // See [intel/vol1]: 3.3.7.1 Canonical Addressing, and
    // [amd/vol1]: 2.1.3 Canonical Address Form.
    const X86_VADDR_BITS: usize = 48;
    const X86_CANONICAL_ADDRESS_MASK: usize = !((1usize << (X86_VADDR_BITS - 1)) - 1);
    (va & X86_CANONICAL_ADDRESS_MASK) == 0
}

/// Checks if a virtual address is in canonical form on x86_64.
///
/// An address is canonical if bits [N - 1, 63] are all either 0 (the low half of
/// canonical addresses) or all 1 (the high half of canonical addresses).
#[inline]
pub fn is_vaddr_canonical(va: u64) -> bool {
    // See [intel/vol1]: 3.3.7.1 Canonical Addressing, and
    // [amd/vol1]: 2.1.3 Canonical Address Form.
    const X86_VADDR_BITS: usize = 48;
    const X86_CANONICAL_ADDRESS_MASK: u64 = !((1u64 << (X86_VADDR_BITS - 1)) - 1);
    ((va & X86_CANONICAL_ADDRESS_MASK) == 0)
        || ((va & X86_CANONICAL_ADDRESS_MASK) == X86_CANONICAL_ADDRESS_MASK)
}

/// Base address of the kernel address space.
pub const KERNEL_ASPACE_BASE: usize = 0xffff_ff80_0000_0000;
/// Size of the kernel address space.
pub const KERNEL_ASPACE_SIZE: usize = 0x0000_0080_0000_0000;

/// Returns whether `va` is within the kernel address space.
#[inline]
pub fn is_kernel_address(va: usize) -> bool {
    va >= KERNEL_ASPACE_BASE && va.wrapping_sub(KERNEL_ASPACE_BASE) < KERNEL_ASPACE_SIZE
}

/// Userspace threads can only set an entry point to userspace addresses, or
/// the null pointer (for testing a thread that will always fail).
///
/// See docs/concepts/kernel/sysret_problem.md for more details.
#[inline]
pub fn is_valid_user_pc(pc: usize) -> bool {
    (pc == 0) || (is_user_accessible(pc) && is_vaddr_canonical(pc as u64))
}

/// Validates the x86_64 register state before entering restricted mode.
pub fn validate_state_pre_restricted_entry(state: &zx_restricted_state_t) -> Result<(), Status> {
    // validate that RIP is within user space
    if !is_user_accessible(state.ip as usize) {
        ltracef!("fail due to bad ip {:#x}\n", state.ip);
        return Err(Status::BAD_STATE);
    }

    // validate that the rflags saved only contain user settable flags
    if (state.flags & !X86_FLAGS_USER) != 0 {
        ltracef!("fail due to flags outside of X86_FLAGS_USER set ({:#x})\n", state.flags);
        return Err(Status::BAD_STATE);
    }

    // fs and gs base must be canonical
    if !is_vaddr_canonical(state.fs_base) {
        ltracef!("fail due to bad fs base {:#x}\n", state.fs_base);
        return Err(Status::BAD_STATE);
    }
    if !is_vaddr_canonical(state.gs_base) {
        ltracef!("fail due to bad gs base {:#x}\n", state.gs_base);
        return Err(Status::BAD_STATE);
    }

    // everything else can be whatever value it wants to be. worst case it immediately faults
    // in restricted mode and that's okay.
    Ok(())
}

pub fn dump(state: &zx_restricted_state_t) {
    use core::fmt::Write;
    use debug::ltrace::KernelConsoleWriter;
    let mut w = KernelConsoleWriter;
    let _ = write!(
        w,
        " RIP: {:#18x}  FL: {:#18x}\n RAX: {:#18x} RBX: {:#18x} RCX: {:#18x} RDX: {:#18x}\n RSI: {:#18x} RDI: {:#18x} RBP: {:#18x} RSP: {:#18x}\n  R8: {:#18x}  R9: {:#18x} R10: {:#18x} R11: {:#18x}\n R12: {:#18x} R13: {:#18x} R14: {:#18x} R15: {:#18x}\nfs base {:#18x} gs base {:#18x}\n",
        state.ip,
        state.flags,
        state.rax,
        state.rbx,
        state.rcx,
        state.rdx,
        state.rsi,
        state.rdi,
        state.rbp,
        state.rsp,
        state.r8,
        state.r9,
        state.r10,
        state.r11,
        state.r12,
        state.r13,
        state.r14,
        state.r15,
        state.fs_base,
        state.gs_base,
    );
}

pub fn save_state_pre_restricted_entry(state: &mut ArchSavedNormalState) {
    // SAFETY: Reads fs/gs base registers for the current thread.
    unsafe {
        cpp_x86_get_fsgsbase_ints_disabled(&mut state.normal_fs_base, &mut state.normal_gs_base);
    }
}

pub fn enter_restricted(state: &zx_restricted_state_t) -> ! {
    // SAFETY: Sets fs/gs base registers for the current thread.
    unsafe {
        cpp_x86_set_fsgsbase(state.fs_base, state.gs_base);
    }

    let iframe = Iframe {
        rdi: state.rdi,
        rsi: state.rsi,
        rbp: state.rbp,
        rbx: state.rbx,
        rdx: state.rdx,
        rcx: state.rcx,
        rax: state.rax,
        r8: state.r8,
        r9: state.r9,
        r10: state.r10,
        r11: state.r11,
        r12: state.r12,
        r13: state.r13,
        r14: state.r14,
        r15: state.r15,
        vector: 0,
        err_code: 0,
        ip: state.ip,
        cs: USER_CODE_64_SELECTOR,
        flags: state.flags | X86_FLAGS_IF,
        user_sp: state.rsp,
        user_ss: USER_DATA_SELECTOR,
    };

    // SAFETY: Enters user space using the constructed iframe. Does not return.
    unsafe {
        cpp_x86_enter_uspace(&iframe);
    }
}

pub fn save_restricted_syscall_state(state: &mut zx_restricted_state_t, regs: &SyscallRegs) {
    state.rdi = regs.rdi;
    state.rsi = regs.rsi;
    state.rbp = regs.rbp;
    state.rbx = regs.rbx;
    state.rdx = regs.rdx;
    state.rcx = regs.rcx;
    state.rax = regs.rax;
    state.rsp = regs.rsp;
    state.r8 = regs.r8;
    state.r9 = regs.r9;
    state.r10 = regs.r10;
    state.r11 = regs.r11;
    state.r12 = regs.r12;
    state.r13 = regs.r13;
    state.r14 = regs.r14;
    state.r15 = regs.r15;
    state.ip = regs.rip;
    state.flags = regs.rflags & X86_FLAGS_USER;

    // SAFETY: Reads fs/gs base registers for the current thread.
    unsafe {
        cpp_x86_get_fsgsbase_ints_disabled(&mut state.fs_base, &mut state.gs_base);
    }
}

pub fn save_restricted_iframe_state(state: &mut zx_restricted_state_t, frame: &Iframe) {
    state.rdi = frame.rdi;
    state.rsi = frame.rsi;
    state.rbp = frame.rbp;
    state.rbx = frame.rbx;
    state.rdx = frame.rdx;
    state.rcx = frame.rcx;
    state.rax = frame.rax;
    state.r8 = frame.r8;
    state.r9 = frame.r9;
    state.r10 = frame.r10;
    state.r11 = frame.r11;
    state.r12 = frame.r12;
    state.r13 = frame.r13;
    state.r14 = frame.r14;
    state.r15 = frame.r15;
    state.ip = frame.ip;
    state.flags = frame.flags & X86_FLAGS_USER;
    state.rsp = frame.user_sp;

    // SAFETY: Reads fs/gs base registers for the current thread.
    unsafe {
        cpp_x86_get_fsgsbase_ints_disabled(&mut state.fs_base, &mut state.gs_base);
    }
}

pub fn save_restricted_exception_state(state: &mut zx_restricted_state_t) {
    let mut regs = zx_thread_state_general_regs_t::default();
    // SAFETY: Gets general registers of the current thread.
    let status = unsafe { cpp_x86_get_general_regs(&mut regs) };
    assert_eq!(Status::ok(status), Ok(()));

    state.rdi = regs.rdi;
    state.rsi = regs.rsi;
    state.rbp = regs.rbp;
    state.rbx = regs.rbx;
    state.rdx = regs.rdx;
    state.rcx = regs.rcx;
    state.rax = regs.rax;
    state.rsp = regs.rsp;
    state.r8 = regs.r8;
    state.r9 = regs.r9;
    state.r10 = regs.r10;
    state.r11 = regs.r11;
    state.r12 = regs.r12;
    state.r13 = regs.r13;
    state.r14 = regs.r14;
    state.r15 = regs.r15;
    state.ip = regs.rip;
    state.flags = regs.rflags & X86_FLAGS_USER;

    // SAFETY: Reads fs/gs base registers for the current thread.
    unsafe {
        cpp_x86_get_fsgsbase_ints_enabled(&mut state.fs_base, &mut state.gs_base);
    }
}

pub fn redirect_restricted_exception_to_normal(
    arch_state: &ArchSavedNormalState,
    vector_table: usize,
    context: usize,
    reason: u64,
) {
    let regs = zx_thread_state_general_regs_t {
        rax: 0,
        rbx: 0,
        rcx: 0,
        rdx: 0,
        rsi: reason,
        rdi: context as u64,
        rbp: 0,
        rsp: 0,
        r8: 0,
        r9: 0,
        r10: 0,
        r11: 0,
        r12: 0,
        r13: 0,
        r14: 0,
        r15: 0,
        rip: vector_table as u64,
        rflags: X86_FLAGS_IF,
        fs_base: arch_state.normal_fs_base,
        gs_base: arch_state.normal_gs_base,
    };
    // SAFETY: Sets general registers on current thread.
    let status = unsafe { cpp_x86_set_general_regs(&regs) };
    assert_eq!(Status::ok(status), Ok(()));
}

pub fn enter_full(
    arch_state: &ArchSavedNormalState,
    vector_table: usize,
    context: usize,
    code: u64,
) -> ! {
    // SAFETY: Restores user fs/gs base from normal mode.
    unsafe {
        cpp_x86_set_fsgsbase(arch_state.normal_fs_base, arch_state.normal_gs_base);
    }

    let iframe = Iframe {
        rdi: context as u64,
        rsi: code,
        rbp: 0,
        rbx: 0,
        rdx: 0,
        rcx: 0,
        rax: 0,
        r8: 0,
        r9: 0,
        r10: 0,
        r11: 0,
        r12: 0,
        r13: 0,
        r14: 0,
        r15: 0,
        vector: 0,
        err_code: 0,
        ip: vector_table as u64,
        cs: USER_CODE_64_SELECTOR,
        flags: X86_FLAGS_IF,
        user_sp: 0,
        user_ss: USER_DATA_SELECTOR,
    };

    // SAFETY: Enters user space using constructed iframe. Does not return.
    unsafe {
        cpp_x86_enter_uspace(&iframe);
    }
}

/// # Safety
/// Caller guarantees `state` is a valid pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rust_arch_validate_state_pre_restricted_entry(
    state: *const zx_restricted_state_t,
) -> zx_status_t {
    // SAFETY: Caller guarantees `state` is a valid pointer.
    let state = unsafe { &*state };
    Status::result_into_raw(validate_state_pre_restricted_entry(state))
}

/// # Safety
/// Caller guarantees `state` is a valid pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rust_arch_save_state_pre_restricted_entry(
    state: *mut ArchSavedNormalState,
) {
    // SAFETY: Caller guarantees `state` is valid.
    let state = unsafe { &mut *state };
    save_state_pre_restricted_entry(state);
}

/// # Safety
/// Caller guarantees `state` is a valid pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rust_arch_enter_restricted(state: *const zx_restricted_state_t) -> ! {
    // SAFETY: Caller guarantees `state` is valid.
    let state = unsafe { &*state };
    enter_restricted(state);
}

/// # Safety
/// Caller guarantees `state` and `regs` are valid pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rust_arch_save_restricted_syscall_state(
    state: *mut zx_restricted_state_t,
    regs: *const SyscallRegs,
) {
    // SAFETY: Caller guarantees pointers are valid.
    let state = unsafe { &mut *state };
    let regs = unsafe { &*regs };
    save_restricted_syscall_state(state, regs);
}

/// # Safety
/// Caller guarantees `state` and `frame` are valid pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rust_arch_save_restricted_iframe_state(
    state: *mut zx_restricted_state_t,
    frame: *const Iframe,
) {
    // SAFETY: Caller guarantees pointers are valid.
    let state = unsafe { &mut *state };
    let frame = unsafe { &*frame };
    save_restricted_iframe_state(state, frame);
}

/// # Safety
/// Caller guarantees `state` is a valid pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rust_arch_save_restricted_exception_state(
    state: *mut zx_restricted_state_t,
) {
    // SAFETY: Caller guarantees pointer is valid.
    let state = unsafe { &mut *state };
    save_restricted_exception_state(state);
}

/// # Safety
/// Caller guarantees `arch_state` is a valid pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rust_arch_redirect_restricted_exception_to_normal(
    arch_state: *const ArchSavedNormalState,
    vector_table: usize,
    context: usize,
    reason: u64,
) {
    // SAFETY: Caller guarantees pointer is valid.
    let arch_state = unsafe { &*arch_state };
    redirect_restricted_exception_to_normal(arch_state, vector_table, context, reason);
}

/// # Safety
/// Caller guarantees `arch_state` is a valid pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rust_arch_enter_full(
    arch_state: *const ArchSavedNormalState,
    vector_table: usize,
    context: usize,
    code: u64,
) -> ! {
    // SAFETY: Caller guarantees pointer is valid.
    let arch_state = unsafe { &*arch_state };
    enter_full(arch_state, vector_table, context, code);
}

/// # Safety
/// Caller guarantees `state` is a valid pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rust_arch_dump(state: *const zx_restricted_state_t) {
    // SAFETY: Caller guarantees `state` is a valid pointer.
    let state = unsafe { &*state };
    dump(state);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_user_accessible() {
        assert!(is_user_accessible(0x00000000_00100000));
        assert!(is_user_accessible(0x00007fff_ffffffff));
        assert!(!is_user_accessible(0x00008000_00000000));
        assert!(!is_user_accessible(0xffff8000_00000000));
    }

    #[test]
    fn test_validate_state_pre_restricted_entry() {
        let mut state = zx_restricted_state_t::default();
        state.ip = 0x1000;
        assert_eq!(validate_state_pre_restricted_entry(&state), Ok(()));

        state.ip = 0xffff_8000_0000_0000;
        assert_eq!(validate_state_pre_restricted_entry(&state), Err(Status::BAD_STATE));
    }

    #[test]
    fn test_dump() {
        let state = zx_restricted_state_t::default();
        dump(&state);
    }

    #[test]
    fn test_is_kernel_address() {
        assert!(is_kernel_address(KERNEL_ASPACE_BASE));
        assert!(is_kernel_address(KERNEL_ASPACE_BASE + 0x1000));
        assert!(is_kernel_address(usize::MAX));
        assert!(!is_kernel_address(0));
        assert!(!is_kernel_address(0x1000));
        assert!(!is_kernel_address(0x0000_7fff_ffff_ffff));
        assert!(!is_kernel_address(KERNEL_ASPACE_BASE - 1));
    }

    #[test]
    fn test_is_valid_user_pc() {
        // Null pointer is valid (used for threads intended to fault).
        assert!(is_valid_user_pc(0));
        // Valid userspace addresses.
        assert!(is_valid_user_pc(0x1000));
        assert!(is_valid_user_pc(0x0000_7fff_ffff_0000));
        // Non-canonical address.
        assert!(!is_valid_user_pc(0x0000_8000_0000_0000));
        // Kernel address.
        assert!(!is_valid_user_pc(KERNEL_ASPACE_BASE));
        assert!(!is_valid_user_pc(0xffff_8000_0000_0000));
    }
}
