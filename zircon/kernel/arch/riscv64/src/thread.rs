// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! RISC-V 64 thread initialization, context switching, and register management.

use super::arch::{
    RISCV64_CSR_SSTATUS, RISCV64_CSR_SSTATUS_FS_INITIAL, RISCV64_CSR_SSTATUS_FS_MASK,
    RISCV64_CSR_SSTATUS_FS_SHIFT, RISCV64_CSR_SSTATUS_SD, RISCV64_CSR_SSTATUS_SPIE,
    RISCV64_CSR_SSTATUS_UXL_64BIT, RISCV64_CSR_SSTATUS_VS_INITIAL, RISCV64_CSR_SSTATUS_VS_MASK,
    RISCV64_CSR_SSTATUS_VS_SHIFT, riscv64_csr_read,
};
use super::{Iframe, is_user_accessible};
use debug::{dprintf, ltracef};
use zx_types::zx_restricted_state_t;

#[allow(dead_code)]
const LOCAL_TRACE: u32 = 0;

const ZX_TLS_STACK_GUARD_OFFSET: isize = -0x10;
const ZX_TLS_UNSAFE_SP_OFFSET: isize = -0x8;

pub use arch_types_bindings::{GeneralRegsSource, UserEntryState};

/// Register state layout used by `riscv64_context_switch()`.
#[repr(C, align(16))]
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Riscv64ContextSwitchFrame {
    pub ra: u64, // return address (x1)
    pub tp: u64, // thread pointer
    pub gp: u64, // shadow-call-stack pointer (x3)
    pub s0: u64, // x8-x9
    pub s1: u64,
    pub s2: u64, // x18-x26
    pub s3: u64,
    pub s4: u64,
    pub s5: u64,
    pub s6: u64,
    pub s7: u64,
    pub s8: u64,
    pub s9: u64,
    pub s10: u64,
}

/// Double precision floating point state at context switch time.
pub type Riscv64FpuState = riscv64_thread_bindings::riscv64_fpu_state;

/// Vector register state at context switch time.
pub type Riscv64VectorState = riscv64_thread_bindings::riscv64_vector_state;

/// Architecture-specific thread state for RISC-V 64.
type ArchThread = riscv64_thread_bindings::arch_thread;

const _: () = {
    assert!(core::mem::size_of::<Riscv64ContextSwitchFrame>() == 112);
    assert!(core::mem::align_of::<Riscv64ContextSwitchFrame>() == 16);
    assert!(core::mem::offset_of!(Riscv64ContextSwitchFrame, ra) == 0);
    assert!(core::mem::offset_of!(Riscv64ContextSwitchFrame, tp) == 8);
    assert!(core::mem::offset_of!(Riscv64ContextSwitchFrame, gp) == 16);
    assert!(core::mem::offset_of!(Riscv64ContextSwitchFrame, s0) == 24);
    assert!(core::mem::offset_of!(Riscv64ContextSwitchFrame, s1) == 32);
    assert!(core::mem::offset_of!(Riscv64ContextSwitchFrame, s2) == 40);

    assert!(core::mem::size_of::<Riscv64FpuState>() == 264);
    assert!(core::mem::align_of::<Riscv64FpuState>() == 8);

    assert!(core::mem::size_of::<Riscv64VectorState>() == 544);
    assert!(core::mem::align_of::<Riscv64VectorState>() == 8);
    assert!(core::mem::offset_of!(Riscv64VectorState, v) == 0x0);
    assert!(core::mem::offset_of!(Riscv64VectorState, vcsr) == 0x200);
    assert!(core::mem::offset_of!(Riscv64VectorState, vstart) == 0x208);
    assert!(core::mem::offset_of!(Riscv64VectorState, vl) == 0x210);
    assert!(core::mem::offset_of!(Riscv64VectorState, vtype) == 0x218);

    assert!(core::mem::size_of::<ArchThread>() == 856);
    assert!(core::mem::align_of::<ArchThread>() == 8);
    assert!(core::mem::offset_of!(ArchThread, stack_guard) == 0);
    assert!(core::mem::offset_of!(ArchThread, unsafe_sp) == 8);
    assert!(core::mem::offset_of!(ArchThread, __bindgen_anon_1) == 16);
    assert!(core::mem::offset_of!(ArchThread, suspended_general_regs) == 24);
    assert!(core::mem::offset_of!(ArchThread, data_fault_resume) == 32);
    assert!(core::mem::offset_of!(ArchThread, fpu_dirty) == 40);
    assert!(core::mem::offset_of!(ArchThread, vector_dirty) == 41);
    assert!(core::mem::offset_of!(ArchThread, fpu_state) == 48);
    assert!(core::mem::offset_of!(ArchThread, vector_state) == 312);

    assert!(
        (core::mem::offset_of!(ArchThread, stack_guard) as isize
            - core::mem::offset_of!(ArchThread, __bindgen_anon_1) as isize)
            == ZX_TLS_STACK_GUARD_OFFSET
    );
    assert!(
        (core::mem::offset_of!(ArchThread, unsafe_sp) as isize
            - core::mem::offset_of!(ArchThread, __bindgen_anon_1) as isize)
            == ZX_TLS_UNSAFE_SP_OFFSET
    );
};

// Scratch word of memory to store into during context switches to wipe out any existing
// memory reservation in an LR/SC sequence.
#[repr(C, align(64))]
struct MemoryReservationScratch(u32);
static mut MEMORY_RESERVATION_SCRATCH: MemoryReservationScratch = MemoryReservationScratch(0);

unsafe extern "C" {
    pub fn riscv64_context_switch(old_sp: *mut usize, new_sp: usize);
    pub fn cpp_arch_set_current_thread(thread: *mut core::ffi::c_void);
    pub fn cpp_arch_set_restricted_flag(in_restricted: bool);
    pub fn cpp_with_frame_pointers() -> bool;
}

/// Returns a pointer to the RISC-V 64 architectural state embedded in `thread`.
///
/// This is a thin typed wrapper over the generic `kernel::thread::get_arch()` facade;
/// it performs pointer arithmetic only and never dereferences `thread`, so it is
/// usable during early boot when the thread is not yet fully constructed.
///
/// # Safety
/// Caller guarantees `thread` points to a C++ `Thread`.
#[inline(always)]
pub unsafe fn thread_arch(thread: *mut core::ffi::c_void) -> *mut ArchThread {
    // SAFETY: The caller guarantees `thread`; the facade only offsets the pointer.
    unsafe { crate::kernel::thread::get_arch(thread.cast()).cast() }
}

/// Initialize the architecture state of a thread.
/// # Safety
/// Caller guarantees `thread` is a valid pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn arch_thread_initialize(
    thread: *mut core::ffi::c_void,
    entry_point: usize,
) {
    debug_assert!(!thread.is_null());

    // SAFETY: The caller guarantees `thread` is valid and locked.
    let arch = unsafe { thread_arch(thread) };

    // zero out the entire arch state, including fpu state, which defaults to all zero
    // SAFETY: `arch` points at the thread's `arch_thread`, which is correctly sized
    // and aligned for `ArchThread`.
    unsafe { core::ptr::write(arch, ArchThread::default()) };

    // create a default stack frame on the stack
    // SAFETY: The caller guarantees `thread` is valid.
    let mut stack_top = unsafe { crate::kernel::thread::get_stack_top(thread.cast()) };

    // Always leave space at the very top for an iframe.
    stack_top -= core::mem::size_of::<Iframe>();
    debug_assert_eq!(stack_top % core::mem::align_of::<Iframe>(), 0);
    debug_assert_eq!(stack_top % 16, 0);

    // SAFETY: The thread's stack has room below `stack_top` for the context switch
    // frame, and `stack_top` is 16-byte aligned as asserted above.
    let frame = unsafe {
        let frame_ptr = (stack_top as *mut Riscv64ContextSwitchFrame).sub(1);

        // set the stack pointer
        (*arch).__bindgen_anon_1.sp = frame_ptr as usize;

        &mut *frame_ptr
    };

    // fill in the entry point
    frame.ra = entry_point as u64;

    // shadow call stack grows up
    // SAFETY: The caller guarantees `thread` is valid.
    frame.gp = unsafe { crate::kernel::thread::get_shadow_call_base(thread.cast()) } as u64;

    // set the thread pointer that will be restored on the first context switch
    // SAFETY: `arch` was initialized in place above; taking a field address reads nothing.
    frame.tp = unsafe { core::ptr::addr_of!((*arch).__bindgen_anon_1.sp) } as u64;
}

/// Construct the first thread on the current CPU.
/// # Safety
/// Caller guarantees `thread` is a valid pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn arch_thread_construct_first(thread: *mut core::ffi::c_void) {
    // Note: the C++ this replaced was annotated `__NO_SAFESTACK`, which suppresses both
    // safe-stack and shadow-call-stack.  Rust has no per-function equivalent, and kernel
    // Rust on riscv64 is built with `-Zsanitizer=shadow-call-stack`, so unlike the C++
    // this function does get a shadow call stack prologue.  That is harmless here: `gp`
    // is already a valid shadow call stack pointer by the time this is reached, the
    // push/pop is balanced within this frame, and the `tp` change made at the end is
    // never observed by it.  Safe stack is not enabled for the kernel at all.
    debug_assert!(!thread.is_null());

    // In the case of the boot CPU, `initial` doesn't actually point to real
    // memory yet; in the case of secondaries though, `initial` will already
    // be `t` and set at the thread pointer (during riscv64_secondary_start()).
    let initial: *mut core::ffi::c_void = crate::kernel::thread::current_get().cast();
    if initial == thread {
        return;
    }

    // In the case of the boot CPU, physboot handed off a temporary region of
    // memory covering the subset of `arch_thread` dealing in the thread ABI. So
    // `fake_thread` is indeed fake, but accessing its `stack_guard` and
    // `unsafe_sp` members is kosher.
    //
    // SAFETY: Only the two thread-ABI fields are touched; the handoff region covers
    // them, and no reference to the (partially fake) `arch_thread` is formed.
    unsafe {
        let fake_arch = thread_arch(initial);
        let arch = thread_arch(thread);

        // Copy over the thread ABI values from the temporary region into the first
        // thread.
        (*arch).stack_guard = (*fake_arch).stack_guard;
        (*arch).unsafe_sp = (*fake_arch).unsafe_sp;
    }

    // SAFETY: Installs `thread`, guaranteed valid by the caller, as this CPU's
    // current thread.
    unsafe { cpp_arch_set_current_thread(thread) };
}

/// Prepare an architectural iframe for entering userspace.
///
/// # Safety
/// Caller guarantees `out` points to storage that is valid for writes of an `Iframe`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn arch_prepare_uspace(state: &UserEntryState, out: *mut Iframe) {
    // Saved interrupt enable (so that interrupts are enabled when returning
    // to user space). Current interrupt enable state set to disabled, which
    // will matter when the arch_uspace_entry loads sstatus temporarily
    // before switching to user space. Set user space bitness to 64bit. Set
    // the FPU and vector registers to the initial state, with the implicit
    // assumption that the context switch routine would have defaulted the
    // FPU/vector state at the time this thread enters user space. All other
    // bits set to zero, default options.
    let mut status =
        RISCV64_CSR_SSTATUS_SPIE | RISCV64_CSR_SSTATUS_UXL_64BIT | RISCV64_CSR_SSTATUS_FS_INITIAL;
    let has_vec = super::feature::has_vector();
    if has_vec {
        status |= RISCV64_CSR_SSTATUS_VS_INITIAL;
    }
    // SAFETY: The caller guarantees `out` is valid for writes; every field of the
    // `Iframe` is initialized here, so no uninitialized state reaches user space.
    unsafe {
        core::ptr::write(
            out,
            Iframe {
                status,
                regs: zx_restricted_state_t {
                    pc: state.pc,
                    sp: state.sp,
                    gp: state.abi_reg,
                    tp: state.tp,
                    a0: state.arg1,
                    a1: state.arg2,
                    ..Default::default()
                },
            },
        )
    };
}

/// Switch to user mode, set the user stack pointer to user_stack_top, save the
/// top of the kernel stack pointer.
/// # Safety
/// Caller guarantees `iframe` is a valid pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn arch_enter_uspace(iframe: *const Iframe) -> ! {
    debug_assert!(!iframe.is_null());
    // SAFETY: Accesses thread stack top and jumps to user return routine.
    unsafe {
        let ct = crate::kernel::thread::current_get();
        let iframe_ref = &*iframe;
        let stack_top = crate::kernel::thread::get_stack_top(ct);

        ltracef!(
            "pc {:#x} sp {:#x} a0 {:#x} a1 {:#x}\n",
            iframe_ref.regs.pc,
            stack_top,
            iframe_ref.regs.a0,
            iframe_ref.regs.a1,
        );

        assert!(is_user_accessible(iframe_ref.regs.pc as usize));

        // arch_thread_initialize() left space so the base of the stack won't overlap
        // with anything currently in use. This function won't return, but instead
        // will abandon all the kernel register and stack state to start fresh at the
        // top of the machine stack and the base of the shadow call stack.
        let user_iframe = (stack_top as *mut Iframe).sub(1);
        core::ptr::copy_nonoverlapping(iframe, user_iframe, 1);

        let scsp = crate::kernel::thread::get_shadow_call_base(ct);

        // Disable interrupts and then warp into the stvec.S code as if just
        // returning from Riscv64UserException after entering the kernel for a user
        // mode exception. To that code, it looks just like this initial iframe was
        // the interrupted user state now being resumed.
        super::arch::arch_disable_ints();
        core::arch::asm!(
            "mv sp, {sp}",
            "mv gp, {gp}",
            "tail Riscv64ReturnToUser",
            "unimp",
            sp = in(reg) user_iframe,
            gp = in(reg) scsp,
            options(noreturn),
        );
    }
}

/// Perform architecture context switch between threads.
/// # Safety
/// Caller guarantees `old_thread` and `new_thread` are valid pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn arch_context_switch(
    old_thread: *mut core::ffi::c_void,
    new_thread: *mut core::ffi::c_void,
) {
    debug_assert!(!old_thread.is_null());
    debug_assert!(!new_thread.is_null());
    debug_assert!(super::arch::arch_ints_disabled());

    if LOCAL_TRACE != 0 {
        // SAFETY: both threads are valid per this function's contract, so
        // `kernel::thread::name()` returns a NUL-terminated string that outlives
        // the borrow -- a thread's name is owned by the thread.
        unsafe {
            let old_name = crate::kernel::thread::name(old_thread.cast());
            let new_name = crate::kernel::thread::name(new_thread.cast());
            let old_name_str = core::ffi::CStr::from_ptr(old_name).to_str().unwrap_or("");
            let new_name_str = core::ffi::CStr::from_ptr(new_name).to_str().unwrap_or("");
            ltracef!(
                "old {:p} ({}), new {:p} ({})\n",
                old_thread,
                old_name_str,
                new_thread,
                new_name_str,
            );
        }
    }

    // Wipe out any LR/SC reservations this cpu may have.
    // SAFETY: a store-conditional to a dedicated scratch word. It clobbers nothing
    // but that word and `zero`, and its only effect is to break any outstanding
    // reservation on this hart.
    unsafe {
        core::arch::asm!(
            "sc.w zero, zero, ({scratch})",
            scratch = in(reg) core::ptr::addr_of_mut!(MEMORY_RESERVATION_SCRATCH),
            options(nostack),
        );
    }

    // FPU and vector context switch
    // Based on a combination of the current hardware state and whether or not the
    // threads have the dirty flags set, conditionally save and/or restore
    // hardware state.
    if LOCAL_TRACE != 0 {
        // SAFETY: `sstatus` is always readable in supervisor mode, and both threads
        // are valid per this function's contract, so `thread_arch()` -- which is
        // offset arithmetic only -- yields a live `ArchThread`.
        unsafe {
            let status = riscv64_csr_read::<RISCV64_CSR_SSTATUS>();
            let fpu_status = status & RISCV64_CSR_SSTATUS_FS_MASK;
            let vector_status = status & RISCV64_CSR_SSTATUS_VS_MASK;
            let old_arch = thread_arch(old_thread);
            let new_arch = thread_arch(new_thread);
            ltracef!(
                "fpu: sstatus.fp {:#x}, sstatus.vs {:#x}, sd {}, old.dirty {}, new.dirty {}\n",
                fpu_status >> RISCV64_CSR_SSTATUS_FS_SHIFT,
                vector_status >> RISCV64_CSR_SSTATUS_VS_SHIFT,
                ((status & RISCV64_CSR_SSTATUS_SD) != 0) as u32,
                (*old_arch).fpu_dirty as u32,
                (*new_arch).fpu_dirty as u32,
            );
        }
    }

    let current_fpu_status = super::fpu::riscv64_fpu_status();
    let current_vector_status = super::vector::riscv64_vector_status();
    // SAFETY: `old_thread` is valid per this function's contract.
    if !unsafe { crate::kernel::thread::is_user_state_saved(old_thread.cast()) } {
        // Save the fpu and vector state for the old (current) thread, depending on
        // whether the fpu or vector hardware is currently in the initial state.
        debug_assert_eq!(old_thread, crate::kernel::thread::current_get().cast());
        // SAFETY: `old_thread` is valid, and it is the running thread -- asserted
        // just above -- so its hardware state is the state being saved.
        unsafe {
            super::fpu::riscv64_thread_fpu_save(old_thread, current_fpu_status);
            if super::feature::has_vector() {
                super::vector::riscv64_thread_vector_save(old_thread, current_vector_status);
            }
        }
    }

    // Always restore the new thread's fpu and vector state even if it is
    // probably going to be restored by a higher layer later with a call to
    // arch_restore_user_state. Though it may be extra work in this case, it
    // avoids potential issues with state getting out of sync if the kernel
    // panicked or the higher layer forgot to restore.
    // SAFETY: `new_thread` is valid per this function's contract, and it is the
    // thread about to run, so its saved state is what the hardware should hold.
    unsafe {
        super::fpu::riscv64_thread_fpu_restore(new_thread, current_fpu_status);
        if super::feature::has_vector() {
            super::vector::riscv64_thread_vector_restore(new_thread, current_vector_status);
        }
    }

    // Set the percpu in_restricted_mode field.
    // SAFETY: `new_thread` is valid per this function's contract, and
    // `cpp_arch_set_restricted_flag` only writes the per-CPU flag word.
    unsafe {
        let in_restricted = crate::kernel::thread::is_in_restricted_mode(new_thread.cast());
        cpp_arch_set_restricted_flag(in_restricted);
    }

    // Regular integer context switch.
    // SAFETY: both threads are valid per this function's contract, so `thread_arch()`
    // yields live `ArchThread`s; `riscv64_context_switch` saves the callee-saved
    // registers to `old_arch.sp` and resumes from `new_arch.sp`, which is what every
    // `ArchThread` is initialized to hold.
    unsafe {
        let old_arch = thread_arch(old_thread);
        let new_arch = thread_arch(new_thread);
        riscv64_context_switch(
            core::ptr::addr_of_mut!((*old_arch).__bindgen_anon_1.sp),
            (*new_arch).__bindgen_anon_1.sp,
        );
    }
}

/// Dump architecture thread info to console.
/// # Safety
/// Caller guarantees `thread` is a valid pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn arch_dump_thread(thread: *const core::ffi::c_void) {
    debug_assert!(!thread.is_null());
    // SAFETY: Reads thread running status and arch sp.
    unsafe {
        if !crate::kernel::thread::is_running(thread.cast()) {
            let arch = thread_arch(thread as *mut _);
            dprintf!(INFO, "\tarch: ");
            dprintf!(INFO, "sp {:#x}\n", (*arch).__bindgen_anon_1.sp);
        }
    }
}

/// Get the blocked frame pointer of a thread.
/// # Safety
/// Caller guarantees `thread` is a valid pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn arch_thread_get_blocked_fp(thread: *mut core::ffi::c_void) -> usize {
    debug_assert!(!thread.is_null());
    // SAFETY: Reads thread frame pointer if frame pointers enabled.
    unsafe {
        if !cpp_with_frame_pointers() {
            return 0;
        }
        let arch = thread_arch(thread);
        let sp = (*arch).__bindgen_anon_1.sp;
        debug_assert!(sp != 0);
        if sp == 0 {
            return 0;
        }
        let frame = &*(sp as *const Riscv64ContextSwitchFrame);
        frame.s0 as usize
    }
}

/// Save user register state (FPU/vector) for a thread.
/// # Safety
/// Caller guarantees `thread` is a valid pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn arch_save_user_state(thread: *mut core::ffi::c_void) {
    debug_assert!(!thread.is_null());
    // SAFETY: The caller guarantees `thread`; this saves the live FPU registers into it.
    unsafe { super::fpu::riscv64_thread_fpu_save(thread, super::fpu::riscv64_fpu_status()) };
    if super::feature::has_vector() {
        // SAFETY: As above, and only reached when the vector extension is present.
        unsafe {
            super::vector::riscv64_thread_vector_save(
                thread,
                super::vector::riscv64_vector_status(),
            )
        };
    }
    // Not saving debug state because there isn't any.
}

/// Restore user register state (FPU/vector) for a thread.
/// # Safety
/// Caller guarantees `thread` is a valid pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn arch_restore_user_state(thread: *mut core::ffi::c_void) {
    debug_assert!(!thread.is_null());
    // SAFETY: The caller guarantees `thread`; this restores the FPU registers from it.
    unsafe { super::fpu::riscv64_thread_fpu_restore(thread, super::fpu::riscv64_fpu_status()) };
    if super::feature::has_vector() {
        // SAFETY: As above, and only reached when the vector extension is present.
        unsafe {
            super::vector::riscv64_thread_vector_restore(
                thread,
                super::vector::riscv64_vector_status(),
            )
        };
    }
}

/// Set suspended general registers for debugger/exception access.
/// # Safety
/// Caller guarantees pointers are valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn arch_set_suspended_general_regs(
    thread: *mut core::ffi::c_void,
    source: GeneralRegsSource,
    iframe: *mut core::ffi::c_void,
) {
    debug_assert!(!thread.is_null());
    debug_assert!(!iframe.is_null());
    debug_assert_eq!(source, GeneralRegsSource::Iframe);
    // SAFETY: Sets suspended_general_regs on the thread arch state.
    unsafe {
        let arch = &mut *thread_arch(thread);
        debug_assert!(arch.suspended_general_regs.is_null());
        arch.suspended_general_regs = iframe.cast();
    }
}

/// Reset suspended general registers after resuming.
/// # Safety
/// Caller guarantees `thread` is a valid pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn arch_reset_suspended_general_regs(thread: *mut core::ffi::c_void) {
    debug_assert!(!thread.is_null());
    // SAFETY: Clears suspended_general_regs on thread arch state.
    unsafe {
        let arch = &mut *thread_arch(thread);
        arch.suspended_general_regs = core::ptr::null_mut();
    }
}

#[cfg(ktest)]
/// Tests for RISC-V 64 thread initialization, context switches, and register state.
#[unittest::suite(name = "riscv64_thread")]
mod tests {
    use unittest::{assert_eq, assert_true};

    /// Test UserEntryState translation in arch_prepare_uspace.
    #[test]
    fn test_user_entry_state_prepare_uspace() {
        let state = UserEntryState {
            pc: 0x1000_0000,
            sp: 0x2000_0000,
            arg1: 10,
            arg2: 20,
            tp: 0x3000_0000,
            abi_reg: 0x4000_0000,
        };
        let mut iframe = core::mem::MaybeUninit::<Iframe>::uninit();
        // SAFETY: `iframe` is valid storage for an `Iframe`, which
        // `arch_prepare_uspace` initializes in full.
        let iframe = unsafe {
            arch_prepare_uspace(&state, iframe.as_mut_ptr());
            iframe.assume_init()
        };
        assert_eq!(iframe.regs.pc, 0x1000_0000);
        assert_eq!(iframe.regs.sp, 0x2000_0000);
        assert_eq!(iframe.regs.a0, 10);
        assert_eq!(iframe.regs.a1, 20);
        assert_eq!(iframe.regs.tp, 0x3000_0000);
        assert_eq!(iframe.regs.gp, 0x4000_0000);
        assert_true!((iframe.status & RISCV64_CSR_SSTATUS_SPIE) != 0);
        assert_true!((iframe.status & RISCV64_CSR_SSTATUS_UXL_64BIT) != 0);
    }
}
