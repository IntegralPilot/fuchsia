// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::task::CurrentTask;
use extended_pstate::{ExtendedPstatePointer, ExtendedPstateState};
use starnix_registers::{HeapRegs, RegisterState, RegisterStorage, RegisterStorageEnum};
use starnix_syscalls::SyscallResult;
use starnix_types::arch::ArchWidth;
use starnix_uapi::errors::{Errno, ErrnoCode};
use starnix_uapi::user_address::ArchSpecific;

#[derive(Clone)]
pub enum ArchExtendedPstateStorage {
    // Storage for 64 bit restricted mode.
    State64(Box<ExtendedPstateState>),
    #[cfg(target_arch = "aarch64")]
    // Storage for 32 bit arm restricted mode.
    State32(Box<extended_pstate::ExtendedAarch32PstateState>),
}

impl ArchExtendedPstateStorage {
    /// Returns a type-erased pointer to the underlying storage currently in use.
    pub fn as_ptr(&mut self) -> ExtendedPstatePointer {
        match self {
            ArchExtendedPstateStorage::State64(state) => {
                ExtendedPstatePointer { extended_pstate: state.as_mut() as *mut _ }
            }
            #[cfg(target_arch = "aarch64")]
            ArchExtendedPstateStorage::State32(state) => {
                ExtendedPstatePointer { extended_aarch32_pstate: state.as_mut() as *mut _ }
            }
        }
    }

    pub fn reset(&mut self) {
        match self {
            ArchExtendedPstateStorage::State64(state) => state.reset(),
            #[cfg(target_arch = "aarch64")]
            ArchExtendedPstateStorage::State32(state) => state.reset(),
        }
    }

    fn with_arch(arch_width: ArchWidth) -> Self {
        #[cfg(target_arch = "aarch64")]
        if arch_width == ArchWidth::Arch32 {
            return ArchExtendedPstateStorage::State32(Box::new(
                extended_pstate::ExtendedAarch32PstateState::default(),
            ));
        }
        let _ = arch_width;
        ArchExtendedPstateStorage::State64(Box::new(ExtendedPstateState::default()))
    }
}

/// The thread related information of a `CurrentTask`. The information should never be used outside
/// of the thread owning the `CurrentTask`.
pub struct ThreadState<T: RegisterStorage> {
    /// A copy of the registers associated with the Zircon thread. Up-to-date values can be read
    /// from `self.handle.read_state_general_regs()`. To write these values back to the thread, call
    /// `self.handle.write_state_general_regs(self.thread_state.registers.into())`.
    pub registers: RegisterState<T>,

    /// Copy of the current extended processor state including floating point and vector registers.
    pub extended_pstate: ArchExtendedPstateStorage,

    /// The errno code (if any) that indicated this task should restart a syscall.
    pub restart_code: Option<ErrnoCode>,

    /// A custom function to resume a syscall that has been interrupted by SIGSTOP.
    /// To use, call set_syscall_restart_func and return ERESTART_RESTARTBLOCK. sys_restart_syscall
    /// will eventually call it.
    pub syscall_restart_func: Option<Box<SyscallRestartFunc>>,
}

impl<T: RegisterStorage + Default> Default for ThreadState<T> {
    // TODO(https://fxbug.dev/407084069): Implementing default doesn't make much
    // sense - we should only initialize thread state when we know the target
    // architecture and we should initialize for that target specifically.
    fn default() -> Self {
        let registers = RegisterState::<T>::default();
        let extended_pstate = ArchExtendedPstateStorage::with_arch(ArchWidth::Arch64);

        Self { registers, extended_pstate, restart_code: None, syscall_restart_func: None }
    }
}

impl<T: RegisterStorage> ThreadState<T> {
    pub fn arch_width(&self) -> ArchWidth {
        #[cfg(target_arch = "aarch64")]
        {
            return if self.registers.is_arch32() { ArchWidth::Arch32 } else { ArchWidth::Arch64 };
        }
        #[cfg(not(target_arch = "aarch64"))]
        ArchWidth::Arch64
    }

    /// Returns a new `ThreadState` with the same `registers` as this one.
    pub fn snapshot<R: RegisterStorage>(&self) -> ThreadState<R>
    where
        RegisterState<R>: From<RegisterState<T>>,
    {
        ThreadState::<R> {
            registers: self.registers.clone().into(),
            extended_pstate: self.extended_pstate.clone(),
            restart_code: self.restart_code,
            syscall_restart_func: None,
        }
    }

    pub fn extended_snapshot<R: RegisterStorage>(&self) -> ThreadState<R>
    where
        RegisterState<R>: From<RegisterState<T>>,
    {
        ThreadState::<R> {
            registers: self.registers.clone().into(),
            extended_pstate: self.extended_pstate.clone(),
            restart_code: self.restart_code,
            syscall_restart_func: None,
        }
    }

    pub fn replace_registers<O: RegisterStorage>(&mut self, other: &ThreadState<O>) {
        let self_arch = self.arch_width();
        let other_arch = other.arch_width();
        self.registers.load(*other.registers);
        // If we're switching between 32 and 64 bit mode, re-initialize the extended processor state.
        self.extended_pstate = if self_arch == other_arch {
            other.extended_pstate.clone()
        } else {
            ArchExtendedPstateStorage::with_arch(other_arch)
        };
    }

    pub fn get_user_register(&mut self, offset: usize) -> Result<usize, Errno> {
        let mut result: usize = 0;
        self.registers.apply_user_register(offset, &mut |register| result = *register as usize)?;
        Ok(result)
    }

    pub fn set_user_register(&mut self, offset: usize, value: usize) -> Result<(), Errno> {
        let self_arch = self.arch_width();
        let result =
            self.registers.apply_user_register(offset, &mut |register| *register = value as u64);
        // If setting the CPSR register to switch between 32 and 64 bit mode, re-initialize the extended processor state.
        if self_arch != self.arch_width() {
            self.extended_pstate = ArchExtendedPstateStorage::with_arch(self.arch_width());
        }
        result
    }
}

impl From<ThreadState<HeapRegs>> for ThreadState<RegisterStorageEnum> {
    fn from(value: ThreadState<HeapRegs>) -> Self {
        ThreadState {
            registers: value.registers.into(),
            extended_pstate: value.extended_pstate,
            restart_code: value.restart_code,
            syscall_restart_func: value.syscall_restart_func,
        }
    }
}

impl<T: RegisterStorage> ArchSpecific for ThreadState<T> {
    fn is_arch32(&self) -> bool {
        #[cfg(target_arch = "aarch64")]
        return self.registers.is_arch32();
        #[cfg(not(target_arch = "aarch64"))]
        false
    }
}

pub type SyscallRestartFunc =
    dyn FnOnce(&mut CurrentTask) -> Result<SyscallResult, Errno> + Send + Sync;
