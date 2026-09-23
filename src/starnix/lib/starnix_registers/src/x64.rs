// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{HeapRegs, RegisterStorage, RegisterStorageEnum};
use starnix_uapi::errors::Errno;
use starnix_uapi::{__NR_restart_syscall, error, user_regs_struct};

/// The size of the syscall instruction in bytes.
const SYSCALL_INSTRUCTION_SIZE_BYTES: u64 = 2;

/// The state of the task's registers when the thread of execution entered the kernel.
/// This is a thin wrapper around [`zx::sys::zx_restricted_state_t`].
///
/// Implements [`std::ops::Deref`] and [`std::ops::DerefMut`] as a way to get at the underlying
/// [`zx::sys::zx_restricted_state_t`] that this type wraps.
#[derive(Default, Clone, Eq, PartialEq)]
pub struct RegisterState<T: RegisterStorage> {
    pub real_registers: T,

    /// A copy of the x64 `rax` register at the time of the `syscall` instruction. This is important
    /// to store, as the return value of a syscall overwrites `rax`, making it impossible to recover
    /// the original syscall number in the case of syscall restart and strace output.
    pub orig_rax: u64,
}

impl<T: RegisterStorage> RegisterState<T> {
    pub fn to_user_regs_struct(&self) -> user_regs_struct {
        user_regs_struct {
            r15: self.r15,
            r14: self.r14,
            r13: self.r13,
            r12: self.r12,
            rbp: self.rbp,
            rbx: self.rbx,
            r11: self.r11,
            r10: self.r10,
            r9: self.r9,
            r8: self.r8,
            rax: self.rax,
            rcx: self.rcx,
            rdx: self.rdx,
            rsi: self.rsi,
            rdi: self.rdi,
            orig_rax: self.orig_rax,
            rip: self.ip,
            cs: 0,
            eflags: self.flags,
            rsp: self.rsp,
            ss: 0,
            fs_base: self.fs_base,
            gs_base: self.gs_base,
            ds: 0,
            es: 0,
            fs: 0,
            gs: 0,
        }
    }

    pub fn to_user_regs_struct_arch32(&self) -> starnix_uapi::arch32::user_regs_struct {
        unreachable!("arch32 not supported on x64")
    }

    pub fn from_user_regs_struct(&mut self, regs: &user_regs_struct) {
        self.r15 = regs.r15;
        self.r14 = regs.r14;
        self.r13 = regs.r13;
        self.r12 = regs.r12;
        self.rbp = regs.rbp;
        self.rbx = regs.rbx;
        self.r11 = regs.r11;
        self.r10 = regs.r10;
        self.r9 = regs.r9;
        self.r8 = regs.r8;
        self.rax = regs.rax;
        self.rcx = regs.rcx;
        self.rdx = regs.rdx;
        self.rsi = regs.rsi;
        self.rdi = regs.rdi;
        self.orig_rax = regs.orig_rax;
        self.ip = regs.rip;
        self.flags = regs.eflags;
        self.rsp = regs.rsp;
        self.fs_base = regs.fs_base;
        self.gs_base = regs.gs_base;
    }

    pub fn from_user_regs_struct_arch32(&mut self, _regs: &starnix_uapi::arch32::user_regs_struct) {
        unreachable!("arch32 not supported on x64")
    }

    /// Saves any register state required to restart `syscall_number`.
    pub fn save_registers_for_restart(&mut self, syscall_number: u64) {
        // The `rax` register read from the thread's state is clobbered by
        // zircon with ZX_ERR_BAD_SYSCALL.  Similarly, Linux sets it to ENOSYS
        // until it has determined the correct return value for the syscall; we
        // emulate this behavior because ptrace callers expect it.
        self.rax = -(starnix_uapi::ENOSYS as i64) as u64;

        // `orig_rax` should hold the original value loaded into `rax` by the userspace process.
        self.orig_rax = syscall_number;
    }

    /// Custom restart, invoke restart_syscall instead of the original syscall.
    pub fn prepare_for_custom_restart(&mut self) {
        self.rax = __NR_restart_syscall as u64;
    }

    /// Restores `rax` to match its value before restarting.
    pub fn restore_original_return_register(&mut self) {
        self.rax = self.orig_rax;
    }

    /// Returns the register that indicates the single-machine-word return value from a
    /// function call.
    pub fn instruction_pointer_register(&self) -> u64 {
        self.ip
    }

    /// Sets the register that indicates the single-machine-word return value from a
    /// function call.
    pub fn set_instruction_pointer_register(&mut self, new_ip: u64) {
        self.ip = new_ip;
    }

    /// Rewind the the register that indicates the instruction pointer by one syscall instruction.
    pub fn rewind_syscall_instruction(&mut self) {
        self.ip -= SYSCALL_INSTRUCTION_SIZE_BYTES;
    }

    /// Returns the register that indicates the single-machine-word return value from a
    /// function call.
    pub fn return_register(&self) -> u64 {
        self.rax
    }

    /// Sets the register that indicates the single-machine-word return value from a
    /// function call.
    pub fn set_return_register(&mut self, return_value: u64) {
        self.rax = return_value;
    }

    /// Gets the register that indicates the current stack pointer.
    pub fn stack_pointer_register(&self) -> u64 {
        self.rsp
    }

    /// Sets the register that indicates the current stack pointer.
    pub fn set_stack_pointer_register(&mut self, sp: u64) {
        self.rsp = sp;
    }

    /// Sets the register that indicates the TLS.
    pub fn set_thread_pointer_register(&mut self, tp: u64) {
        self.fs_base = tp;
    }

    /// Sets the register that indicates the first argument to a function.
    pub fn set_arg0_register(&mut self, rdi: u64) {
        self.rdi = rdi;
    }

    /// Sets the register that indicates the second argument to a function.
    pub fn set_arg1_register(&mut self, rsi: u64) {
        self.rsi = rsi;
    }

    /// Sets the register that indicates the third argument to a function.
    pub fn set_arg2_register(&mut self, rdx: u64) {
        self.rdx = rdx;
    }

    /// Returns the register that contains the syscall number.
    pub fn syscall_register(&self) -> u64 {
        self.orig_rax
    }

    /// Resets the register that contains the application status flags.
    pub fn reset_flags(&mut self) {
        self.flags = 0;
    }

    /// Executes the given predicate on the register.
    pub fn apply_user_register(
        &mut self,
        offset: usize,
        f: &mut dyn FnMut(&mut u64),
    ) -> Result<(), Errno> {
        if offset == memoffset::offset_of!(user_regs_struct, r15) {
            f(&mut self.r15);
        } else if offset == memoffset::offset_of!(user_regs_struct, r14) {
            f(&mut self.r14);
        } else if offset == memoffset::offset_of!(user_regs_struct, r13) {
            f(&mut self.r13);
        } else if offset == memoffset::offset_of!(user_regs_struct, r12) {
            f(&mut self.r12);
        } else if offset == memoffset::offset_of!(user_regs_struct, rbp) {
            f(&mut self.rbp);
        } else if offset == memoffset::offset_of!(user_regs_struct, rbx) {
            f(&mut self.rbx);
        } else if offset == memoffset::offset_of!(user_regs_struct, r11) {
            f(&mut self.r11);
        } else if offset == memoffset::offset_of!(user_regs_struct, r10) {
            f(&mut self.r10);
        } else if offset == memoffset::offset_of!(user_regs_struct, r9) {
            f(&mut self.r9);
        } else if offset == memoffset::offset_of!(user_regs_struct, r8) {
            f(&mut self.r8);
        } else if offset == memoffset::offset_of!(user_regs_struct, rax) {
            f(&mut self.rax);
        } else if offset == memoffset::offset_of!(user_regs_struct, rcx) {
            f(&mut self.rcx);
        } else if offset == memoffset::offset_of!(user_regs_struct, rdx) {
            f(&mut self.rdx);
        } else if offset == memoffset::offset_of!(user_regs_struct, rsi) {
            f(&mut self.rsi);
        } else if offset == memoffset::offset_of!(user_regs_struct, rdi) {
            f(&mut self.rdi);
        } else if offset == memoffset::offset_of!(user_regs_struct, orig_rax) {
            f(&mut self.orig_rax);
        } else if offset == memoffset::offset_of!(user_regs_struct, rip) {
            f(&mut self.ip);
        } else if offset == memoffset::offset_of!(user_regs_struct, cs) {
            let mut val = 0;
            f(&mut val);
        } else if offset == memoffset::offset_of!(user_regs_struct, eflags) {
            f(&mut self.flags);
        } else if offset == memoffset::offset_of!(user_regs_struct, rsp) {
            f(&mut self.rsp);
        } else if offset == memoffset::offset_of!(user_regs_struct, ss) {
            let mut val = 0;
            f(&mut val);
        } else if offset == memoffset::offset_of!(user_regs_struct, fs_base) {
            f(&mut self.fs_base);
        } else if offset == memoffset::offset_of!(user_regs_struct, gs_base) {
            f(&mut self.gs_base);
        } else if offset == memoffset::offset_of!(user_regs_struct, ds) {
            let mut val = 0;
            f(&mut val);
        } else if offset == memoffset::offset_of!(user_regs_struct, es) {
            let mut val = 0;
            f(&mut val);
        } else if offset == memoffset::offset_of!(user_regs_struct, fs) {
            let mut val = 0;
            f(&mut val);
        } else if offset == memoffset::offset_of!(user_regs_struct, gs) {
            let mut val = 0;
            f(&mut val);
        } else {
            return error!(EINVAL);
        };
        Ok(())
    }

    pub fn load(&mut self, regs: zx::sys::zx_restricted_state_t) {
        *self.real_registers = regs;
        self.sync_stack_ptr();
    }

    pub fn sync_stack_ptr(&mut self) {
        self.orig_rax = self.rax;
    }
}

impl<T: RegisterStorage> std::fmt::Debug for RegisterState<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RegisterState")
            .field("real_registers", &self.real_registers)
            .field("orig_rax", &format_args!("{:#x}", &self.orig_rax))
            .finish()
    }
}

impl<T: RegisterStorage> std::ops::Deref for RegisterState<T> {
    type Target = zx::sys::zx_restricted_state_t;

    fn deref(&self) -> &Self::Target {
        &*self.real_registers
    }
}

impl<T: RegisterStorage> std::ops::DerefMut for RegisterState<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut *self.real_registers
    }
}

impl From<RegisterState<HeapRegs>> for RegisterState<RegisterStorageEnum> {
    fn from(regs: RegisterState<HeapRegs>) -> Self {
        Self { real_registers: regs.real_registers.into(), orig_rax: regs.orig_rax }
    }
}

impl From<RegisterState<RegisterStorageEnum>> for RegisterState<HeapRegs> {
    fn from(regs: RegisterState<RegisterStorageEnum>) -> Self {
        Self { real_registers: regs.real_registers.into(), orig_rax: regs.orig_rax }
    }
}
