// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{HeapRegs, RegisterStorage, RegisterStorageEnum};
use starnix_uapi::errors::Errno;
use starnix_uapi::uapi::user_regs_struct;
use starnix_uapi::{__NR_restart_syscall, error};

/// The size of the syscall instruction in bytes. `ECALL` is not compressed, i.e. it always takes 4
/// bytes.
const SYSCALL_INSTRUCTION_SIZE_BYTES: u64 = 4;

/// The state of the task's registers when the thread of execution entered the kernel.
/// This is a thin wrapper around [`zx::sys::zx_restricted_state_t`].
///
/// Implements [`std::ops::Deref`] and [`std::ops::DerefMut`] as a way to get at the underlying
/// [`zx::sys::zx_restricted_state_t`] that this type wraps.
#[derive(Default, Clone, Eq, PartialEq)]
pub struct RegisterState<T: RegisterStorage> {
    pub real_registers: T,

    /// A copy of the `a0` register at the time of the `syscall` instruction. This is
    /// important to store, as the return value of a syscall overwrites `a0`, making it impossible
    /// to recover the original value in the case of syscall restart and strace output.
    pub orig_a0: u64,
}

impl<T: RegisterStorage> RegisterState<T> {
    /// Saves any register state required to restart `syscall`.
    pub fn save_registers_for_restart(&mut self, _syscall_number: u64) {
        // The x0 register may be clobbered during syscall handling (for the return value), but is
        // needed when restarting a syscall.
        self.orig_a0 = self.a0;
    }

    /// Custom restart, invoke restart_syscall instead of the original syscall.
    pub fn prepare_for_custom_restart(&mut self) {
        self.a7 = __NR_restart_syscall as u64;
    }

    /// Restores a0 to match its value before restarting. This needs to be done when restarting
    /// syscalls because a0 may have been overwritten in the syscall dispatch loop.
    pub fn restore_original_return_register(&mut self) {
        self.a0 = self.orig_a0;
    }

    /// Returns the register that indicates the single-machine-word return value from a
    /// function call.
    pub fn instruction_pointer_register(&self) -> u64 {
        self.pc
    }

    /// Sets the register that indicates the single-machine-word return value from a
    /// function call.
    pub fn set_instruction_pointer_register(&mut self, new_ip: u64) {
        self.pc = new_ip;
    }

    /// Rewind the the register that indicates the instruction pointer by one syscall instruction.
    pub fn rewind_syscall_instruction(&mut self) {
        self.pc -= SYSCALL_INSTRUCTION_SIZE_BYTES;
    }

    /// Returns the register that indicates the single-machine-word return value from a
    /// function call.
    pub fn return_register(&self) -> u64 {
        self.a0
    }

    /// Sets the register that indicates the single-machine-word return value from a
    /// function call.
    pub fn set_return_register(&mut self, return_value: u64) {
        self.a0 = return_value;
    }

    /// Gets the register that indicates the current stack pointer.
    pub fn stack_pointer_register(&self) -> u64 {
        self.sp
    }

    /// Sets the register that indicates the current stack pointer.
    pub fn set_stack_pointer_register(&mut self, sp: u64) {
        self.sp = sp;
    }

    /// Sets the register that indicates the TLS.
    pub fn set_thread_pointer_register(&mut self, tp: u64) {
        self.tp = tp;
    }

    /// Sets the register that indicates the first argument to a function.
    pub fn set_arg0_register(&mut self, x0: u64) {
        self.a0 = x0;
    }

    /// Sets the register that indicates the second argument to a function.
    pub fn set_arg1_register(&mut self, x1: u64) {
        self.a1 = x1;
    }

    /// Sets the register that indicates the third argument to a function.
    pub fn set_arg2_register(&mut self, x2: u64) {
        self.a2 = x2;
    }

    /// Returns the register that contains the syscall number.
    pub fn syscall_register(&self) -> u64 {
        self.a7
    }

    /// Resets the register that contains the application status flags.
    pub fn reset_flags(&mut self) {
        // No-op on RISC-V since there is no flags register.
    }

    pub fn to_user_regs_struct(&self) -> user_regs_struct {
        user_regs_struct {
            pc: self.pc,
            ra: self.ra,
            sp: self.sp,
            gp: self.gp,
            tp: self.tp,
            t0: self.t0,
            t1: self.t1,
            t2: self.t2,
            s0: self.s0,
            s1: self.s1,
            a0: self.a0,
            a1: self.a1,
            a2: self.a2,
            a3: self.a3,
            a4: self.a4,
            a5: self.a5,
            a6: self.a6,
            a7: self.a7,
            s2: self.s2,
            s3: self.s3,
            s4: self.s4,
            s5: self.s5,
            s6: self.s6,
            s7: self.s7,
            s8: self.s8,
            s9: self.s9,
            s10: self.s10,
            s11: self.s11,
            t3: self.t3,
            t4: self.t4,
            t5: self.t5,
            t6: self.t6,
        }
    }

    pub fn to_user_regs_struct_arch32(&self) -> starnix_uapi::arch32::user_regs_struct {
        unreachable!("arch32 not supported on riscv64")
    }

    pub fn from_user_regs_struct(&mut self, regs: &user_regs_struct) {
        self.pc = regs.pc;
        self.ra = regs.ra;
        self.sp = regs.sp;
        self.gp = regs.gp;
        self.tp = regs.tp;
        self.t0 = regs.t0;
        self.t1 = regs.t1;
        self.t2 = regs.t2;
        self.s0 = regs.s0;
        self.s1 = regs.s1;
        self.a0 = regs.a0;
        self.a1 = regs.a1;
        self.a2 = regs.a2;
        self.a3 = regs.a3;
        self.a4 = regs.a4;
        self.a5 = regs.a5;
        self.a6 = regs.a6;
        self.a7 = regs.a7;
        self.s2 = regs.s2;
        self.s3 = regs.s3;
        self.s4 = regs.s4;
        self.s5 = regs.s5;
        self.s6 = regs.s6;
        self.s7 = regs.s7;
        self.s8 = regs.s8;
        self.s9 = regs.s9;
        self.s10 = regs.s10;
        self.s11 = regs.s11;
        self.t3 = regs.t3;
        self.t4 = regs.t4;
        self.t5 = regs.t5;
        self.t6 = regs.t6;
    }

    pub fn from_user_regs_struct_arch32(&mut self, _regs: &starnix_uapi::arch32::user_regs_struct) {
        unreachable!("arch32 not supported on riscv64")
    }

    /// Executes the given predicate on the register.
    pub fn apply_user_register(
        &mut self,
        offset: usize,
        f: &mut dyn FnMut(&mut u64),
    ) -> Result<(), Errno> {
        if offset >= std::mem::size_of::<user_regs_struct>() {
            return error!(EINVAL);
        }

        if offset == memoffset::offset_of!(user_regs_struct, pc) {
            f(&mut self.pc);
        } else if offset == memoffset::offset_of!(user_regs_struct, ra) {
            f(&mut self.ra);
        } else if offset == memoffset::offset_of!(user_regs_struct, sp) {
            f(&mut self.sp);
        } else if offset == memoffset::offset_of!(user_regs_struct, gp) {
            f(&mut self.gp);
        } else if offset == memoffset::offset_of!(user_regs_struct, tp) {
            f(&mut self.tp);
        } else if offset == memoffset::offset_of!(user_regs_struct, t0) {
            f(&mut self.t0);
        } else if offset == memoffset::offset_of!(user_regs_struct, t1) {
            f(&mut self.t1);
        } else if offset == memoffset::offset_of!(user_regs_struct, t2) {
            f(&mut self.t2);
        } else if offset == memoffset::offset_of!(user_regs_struct, s0) {
            f(&mut self.s0);
        } else if offset == memoffset::offset_of!(user_regs_struct, s1) {
            f(&mut self.s1);
        } else if offset == memoffset::offset_of!(user_regs_struct, a0) {
            f(&mut self.a0);
        } else if offset == memoffset::offset_of!(user_regs_struct, a1) {
            f(&mut self.a1);
        } else if offset == memoffset::offset_of!(user_regs_struct, a2) {
            f(&mut self.a2);
        } else if offset == memoffset::offset_of!(user_regs_struct, a3) {
            f(&mut self.a3);
        } else if offset == memoffset::offset_of!(user_regs_struct, a4) {
            f(&mut self.a4);
        } else if offset == memoffset::offset_of!(user_regs_struct, a5) {
            f(&mut self.a5);
        } else if offset == memoffset::offset_of!(user_regs_struct, a6) {
            f(&mut self.a6);
        } else if offset == memoffset::offset_of!(user_regs_struct, a7) {
            f(&mut self.a7);
        } else if offset == memoffset::offset_of!(user_regs_struct, s2) {
            f(&mut self.s2);
        } else if offset == memoffset::offset_of!(user_regs_struct, s3) {
            f(&mut self.s3);
        } else if offset == memoffset::offset_of!(user_regs_struct, s4) {
            f(&mut self.s4);
        } else if offset == memoffset::offset_of!(user_regs_struct, s5) {
            f(&mut self.s5);
        } else if offset == memoffset::offset_of!(user_regs_struct, s6) {
            f(&mut self.s6);
        } else if offset == memoffset::offset_of!(user_regs_struct, s7) {
            f(&mut self.s7);
        } else if offset == memoffset::offset_of!(user_regs_struct, s8) {
            f(&mut self.s8);
        } else if offset == memoffset::offset_of!(user_regs_struct, s9) {
            f(&mut self.s9);
        } else if offset == memoffset::offset_of!(user_regs_struct, s10) {
            f(&mut self.s10);
        } else if offset == memoffset::offset_of!(user_regs_struct, s11) {
            f(&mut self.s11);
        } else if offset == memoffset::offset_of!(user_regs_struct, t3) {
            f(&mut self.t3);
        } else if offset == memoffset::offset_of!(user_regs_struct, t4) {
            f(&mut self.t4);
        } else if offset == memoffset::offset_of!(user_regs_struct, t5) {
            f(&mut self.t5);
        } else if offset == memoffset::offset_of!(user_regs_struct, t6) {
            f(&mut self.t6);
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
        self.orig_a0 = self.a0;
    }
}

impl<T: RegisterStorage> std::fmt::Debug for RegisterState<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RegisterState")
            .field("real_registers", &self.real_registers)
            .field("orig_a0", &format_args!("{:#x}", &self.orig_a0))
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
        Self { real_registers: regs.real_registers.into(), orig_a0: regs.orig_a0 }
    }
}

impl From<RegisterState<RegisterStorageEnum>> for RegisterState<HeapRegs> {
    fn from(regs: RegisterState<RegisterStorageEnum>) -> Self {
        Self { real_registers: regs.real_registers.into(), orig_a0: regs.orig_a0 }
    }
}
