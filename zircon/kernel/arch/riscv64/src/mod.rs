// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

pub mod arch;
pub mod cache;
pub mod crashlog;
pub mod debugger;
pub mod exceptions;
pub mod feature;
pub mod fpu;
pub mod mp;
pub mod restricted;
pub mod sbi;
pub mod spinlock;
pub mod thread;
pub mod timer;
pub mod user_copy;
pub mod vector;

/// Base address of the kernel address space.
pub const KERNEL_ASPACE_BASE: usize = 0xffff_ffc0_0000_0000;
/// Size of the kernel address space.
pub const KERNEL_ASPACE_SIZE: usize = 1usize << 38;

/// Zic64b guarantees.
pub const MAX_CACHE_LINE: usize = 64;
#[repr(align(64))]
pub struct CpuAlignMarker;

/// Returns whether `va` is within the kernel address space.
#[inline]
pub fn is_kernel_address(va: usize) -> bool {
    va >= KERNEL_ASPACE_BASE && va.wrapping_sub(KERNEL_ASPACE_BASE) < KERNEL_ASPACE_SIZE
}

/// Userspace threads can only set an entry point to userspace addresses, or
/// the null pointer (for testing a thread that will always fail).
#[inline]
pub fn is_valid_user_pc(pc: usize) -> bool {
    (pc == 0) || (is_user_accessible(pc) && !is_kernel_address(pc))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_kernel_address() {
        assert!(is_kernel_address(KERNEL_ASPACE_BASE));
        assert!(is_kernel_address(KERNEL_ASPACE_BASE + 0x1000));
        assert!(is_kernel_address(KERNEL_ASPACE_BASE + KERNEL_ASPACE_SIZE - 1));
        assert!(!is_kernel_address(0));
        assert!(!is_kernel_address(0x1000));
        assert!(!is_kernel_address(0x0000_003f_ffff_ffff));
        assert!(!is_kernel_address(KERNEL_ASPACE_BASE - 1));
    }

    #[test]
    fn test_is_valid_user_pc() {
        // Null pointer is valid (used for threads intended to fault).
        assert!(is_valid_user_pc(0));
        // Valid userspace addresses.
        assert!(is_valid_user_pc(0x1000));
        assert!(is_valid_user_pc(0x0000_003f_ffff_0000));
        // Inaccessible user address (bit 38 set).
        assert!(!is_valid_user_pc(0x0000_0040_0000_0000));
        // Kernel address.
        assert!(!is_valid_user_pc(KERNEL_ASPACE_BASE));
        assert!(!is_valid_user_pc(0xffff_ffff_8000_0000));
    }
}

// Names the rest of the kernel resolves directly under `arch::riscv64`: the
// arch API contract checked by `assert_arch_signatures!` in
// //zircon/kernel/arch/src/api.rs, plus the few names other subsystems import
// by that path. Everything else stays behind its module, matching
// //zircon/kernel/arch/x86/src/mod.rs.
pub use arch::{
    arch_early_init, arch_enter_idle_state, arch_init, arch_late_init_percpu, arch_prevm_init,
};
pub use mp::arch_curr_cpu_num;
pub use restricted::{
    ArchSavedNormalState, Iframe, SyscallRegs, boot_hart_id, curr_hart_id, dump, enter_full,
    enter_restricted, redirect_restricted_exception_to_normal, save_restricted_exception_state,
    save_restricted_iframe_state, save_restricted_syscall_state, save_state_pre_restricted_entry,
    validate_state_pre_restricted_entry,
};
pub use thread::{
    arch_context_switch, arch_dump_thread, arch_enter_uspace, arch_prepare_uspace,
    arch_reset_suspended_general_regs, arch_restore_user_state, arch_save_user_state,
    arch_set_suspended_general_regs, arch_thread_construct_first, arch_thread_get_blocked_fp,
    arch_thread_initialize,
};
pub use user_copy::{
    arch_copy_from_user, arch_copy_from_user_capture_faults, arch_copy_to_user,
    arch_copy_to_user_capture_faults, is_user_accessible,
};
