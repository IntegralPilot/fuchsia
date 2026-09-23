// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{HeapRegs, RegisterStorage, RegisterStorageEnum};
use starnix_uapi::errors::Errno;
use starnix_uapi::user_address::{ArchSpecific, LongPtr};
use starnix_uapi::{error, uapi, user_regs_struct};

/// The size of the syscall instruction in bytes in aarch64 and arm mode.
const SYSCALL_ARM_INSTRUCTION_SIZE_BYTES: u64 = 4;
/// The size of the syscall instruction in bytes in aarch32 thumb mode.
const SYSCALL_THUMBS_INSTRUCTION_SIZE_BYTES: u64 = 2;

/// The state of the task's registers when the thread of execution entered the kernel.
/// This is a thin wrapper around [`zx::sys::zx_restricted_state_t`].
///
/// Implements [`std::ops::Deref`] and [`std::ops::DerefMut`] as a way to get at the underlying
/// [`zx::sys::zx_restricted_state_t`] that this type wraps.
#[derive(Default, Clone, Eq, PartialEq)]
pub struct RegisterState<T: RegisterStorage> {
    pub real_registers: T,

    /// A copy of the aarch64 `x0` register at the time of the `syscall` instruction. This is
    /// important to store, as the return value of a syscall overwrites `x0`, making it impossible
    /// to recover the original `x0` value in the case of syscall restart and strace output.
    pub orig_x0: u64,

    /// The contents of the Exception Link Register. This register is used to jump to a code
    /// location in restricted mode, as arm64 does not allow the PC to be set directly.
    pub elr: u64,
}

impl<T: RegisterStorage> ArchSpecific for RegisterState<T> {
    fn is_arch32(&self) -> bool {
        (self.cpsr as u64) & zx::sys::ZX_REG_CPSR_ARCH_32_MASK == zx::sys::ZX_REG_CPSR_ARCH_32_MASK
    }
}

impl<T: RegisterStorage> RegisterState<T> {
    pub fn to_user_regs_struct(&self) -> user_regs_struct {
        user_regs_struct {
            regs: std::array::from_fn(|i| self.r[i]),
            sp: self.sp,
            pc: self.pc,
            pstate: self.cpsr as u64,
        }
    }

    pub fn to_user_regs_struct_arch32(&self) -> starnix_uapi::arch32::user_regs_struct {
        let mut regs = [0u32; 18];
        for i in 0..13 {
            regs[i] = self.r[i] as u32;
        }
        regs[13] = self.sp as u32;
        regs[14] = self.r[30] as u32;
        regs[15] = self.pc as u32;
        regs[16] = self.cpsr;
        regs[17] = self.orig_x0 as u32;
        starnix_uapi::arch32::user_regs_struct { regs }
    }

    pub fn from_user_regs_struct(&mut self, regs: &user_regs_struct) {
        debug_assert!(!self.is_arch32());
        for i in 0..31 {
            self.r[i] = regs.regs[i];
        }
        self.sp = regs.sp;
        self.pc = regs.pc;
        self.cpsr = regs.pstate as u32;
    }

    pub fn from_user_regs_struct_arch32(&mut self, regs: &starnix_uapi::arch32::user_regs_struct) {
        debug_assert!(self.is_arch32());
        for i in 0..13 {
            self.r[i] = regs.regs[i] as u64;
        }
        self.sp = regs.regs[13] as u64;
        self.r[13] = self.sp;
        self.r[30] = regs.regs[14] as u64;
        self.r[14] = self.r[30];
        self.pc = regs.regs[15] as u64;
        self.r[15] = self.pc;
        self.cpsr = regs.regs[16];
        self.orig_x0 = regs.regs[17] as u64;
    }

    pub fn is_thumb(&self) -> bool {
        const IS_THUMB_MASK: u64 =
            zx::sys::ZX_REG_CPSR_ARCH_32_MASK | zx::sys::ZX_REG_CPSR_THUMB_MASK;
        (self.cpsr as u64) & IS_THUMB_MASK == IS_THUMB_MASK
    }

    /// Saves any register state required to restart `syscall`.
    pub fn save_registers_for_restart(&mut self, _syscall_number: u64) {
        // The x0 register may be clobbered during syscall handling (for the return value), but is
        // needed when restarting a syscall.
        self.orig_x0 = self.r[0];
    }

    /// Custom restart, invoke restart_syscall instead of the original syscall.
    pub fn prepare_for_custom_restart(&mut self) {
        if self.is_arch32() {
            self.r[7] = uapi::arch32::__NR_restart_syscall as u64;
        } else {
            self.r[8] = uapi::__NR_restart_syscall as u64;
        }
    }

    /// Restores x0 to match its value before restarting. This needs to be done when restarting
    /// syscalls because x0 may have been overwritten in the syscall dispatch loop.
    pub fn restore_original_return_register(&mut self) {
        self.r[0] = self.orig_x0;
    }

    /// Returns the register that indicates the single-machine-word return value from a
    /// function call.
    pub fn instruction_pointer_register(&self) -> u64 {
        self.pc
    }

    /// Sets the register that indicates the single-machine-word return value from a
    /// function call.
    pub fn set_instruction_pointer_register(&mut self, mut new_ip: u64) {
        if self.is_arch32() {
            let is_thumb = new_ip & 1 == 1;
            if is_thumb {
                new_ip -= 1;
                self.cpsr = self.cpsr | zx::sys::ZX_REG_CPSR_THUMB_MASK as u32;
            } else {
                self.cpsr = self.cpsr & !zx::sys::ZX_REG_CPSR_THUMB_MASK as u32;
            }
            self.r[15] = new_ip;
        }
        self.pc = new_ip;
    }

    /// Rewind the the register that indicates the instruction pointer by one syscall instruction.
    pub fn rewind_syscall_instruction(&mut self) {
        let instruction_size = if self.is_thumb() {
            SYSCALL_THUMBS_INSTRUCTION_SIZE_BYTES
        } else {
            SYSCALL_ARM_INSTRUCTION_SIZE_BYTES
        };
        if self.is_arch32() {
            self.r[15] -= instruction_size;
        }
        self.pc -= instruction_size;
    }

    /// Returns the register that indicates the single-machine-word return value from a
    /// function call.
    pub fn return_register(&self) -> u64 {
        self.r[0]
    }

    /// Sets the register that indicates the single-machine-word return value from a
    /// function call.
    pub fn set_return_register(&mut self, return_value: u64) {
        self.r[0] = return_value;
    }

    /// Gets the register that indicates the current stack pointer.
    pub fn stack_pointer_register(&self) -> u64 {
        self.sp
    }

    /// Sets the register that indicates the current stack pointer.
    pub fn set_stack_pointer_register(&mut self, sp: u64) {
        self.sp = sp;
        if self.is_arch32() {
            self.r[13] = sp;
        }
    }

    /// Sets the register that indicates the TLS.
    pub fn set_thread_pointer_register(&mut self, tp: u64) {
        self.tpidr_el0 = tp;
    }

    /// Sets the register that indicates the first argument to a function.
    pub fn set_arg0_register(&mut self, x0: u64) {
        self.r[0] = x0;
    }

    /// Sets the register that indicates the second argument to a function.
    pub fn set_arg1_register(&mut self, x1: u64) {
        self.r[1] = x1;
    }

    /// Sets the register that indicates the third argument to a function.
    pub fn set_arg2_register(&mut self, x2: u64) {
        self.r[2] = x2;
    }

    /// Returns the register that contains the syscall number.
    pub fn syscall_register(&self) -> u64 {
        if self.is_arch32() { self.r[7] } else { self.r[8] }
    }

    /// Resets the register that contains the application status flags.
    pub fn reset_flags(&mut self) {
        // Reset all the flags except the aarch32 and thumb bits.
        self.cpsr = self.cpsr
            & (zx::sys::ZX_REG_CPSR_ARCH_32_MASK | zx::sys::ZX_REG_CPSR_THUMB_MASK) as u32;
    }

    /// Executes the given predicate on the register.
    pub fn apply_user_register(
        &mut self,
        offset: usize,
        f: &mut dyn FnMut(&mut u64),
    ) -> Result<(), Errno> {
        let is_arch32: bool = self.is_arch32();
        let reg_offset = |index: usize| -> usize {
            memoffset::offset_of!(user_regs_struct, regs)
                + index * LongPtr::size_of_object_for(self)
        };
        let mut final_f = |v: &mut u64| {
            if is_arch32 {
                *v = *v & (u32::MAX as u64);
                f(v);
                *v = *v & (u32::MAX as u64);
            } else {
                f(v)
            }
        };

        if offset >= std::mem::size_of::<user_regs_struct>() {
            return error!(EINVAL);
        }
        if offset == memoffset::offset_of!(user_regs_struct, sp)
            || (offset == reg_offset(13) && is_arch32)
        {
            final_f(&mut self.sp);
            // For arm, sp is register 13
            if is_arch32 {
                self.r[13] = self.sp;
            }
        } else if offset == memoffset::offset_of!(user_regs_struct, pc)
            || (offset == reg_offset(15) && is_arch32)
        {
            final_f(&mut self.pc);
            // For arm, pc is register 15
            if is_arch32 {
                self.r[15] = self.pc;
            }
        } else if offset == memoffset::offset_of!(user_regs_struct, pstate) {
            let mut cpsr = self.cpsr as u64;
            final_f(&mut cpsr);
            self.cpsr = cpsr as u32;
        } else if offset == reg_offset(30) || (offset == reg_offset(14) && is_arch32) {
            // The 30th register is stored as lr in self.real_registers
            final_f(&mut self.r[30]);
            if is_arch32 {
                // The 14th register is stored as lr in self.real_registers for
                // arm
                self.r[14] = self.r[30];
            }
        } else if offset % LongPtr::align_of_object_for(self) == 0 {
            let index = offset / LongPtr::size_of_object_for(self);
            final_f(&mut self.r[index])
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
        // We should synchronize the stack pointer with the aarch32 registers.
        if self.cpsr & zx::sys::ZX_REG_CPSR_ARCH_32_MASK as u32 != 0 {
            self.sp = self.r[13];
            self.r[30] = self.r[14];
            // The PC appears to advance properly and _not_ prefer r[15]
            // TODO(https://fxbug.dev/380402551): Make sure this isn't because of anything
            // done in zircon.
            self.r[15] = self.pc;
        }
        self.orig_x0 = self.r[0];
        self.elr = 0;
    }
}

impl<T: RegisterStorage> std::fmt::Debug for RegisterState<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RegisterState")
            .field("real_registers", &self.real_registers)
            .field("orig_x0", &format_args!("{:#x}", &self.orig_x0))
            .field("elr", &format_args!("{:#x}", &self.elr))
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
        Self { real_registers: regs.real_registers.into(), orig_x0: regs.orig_x0, elr: regs.elr }
    }
}

impl From<RegisterState<RegisterStorageEnum>> for RegisterState<HeapRegs> {
    fn from(regs: RegisterState<RegisterStorageEnum>) -> Self {
        Self { real_registers: regs.real_registers.into(), orig_x0: regs.orig_x0, elr: regs.elr }
    }
}
