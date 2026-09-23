// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::task::{CurrentTask, ThreadState};
use starnix_registers::RegisterStorage;
use starnix_syscalls::SyscallArg;
use starnix_syscalls::decls::{Syscall, SyscallDecl};

pub fn new_syscall_from_state<T: RegisterStorage>(
    syscall_decl: SyscallDecl,
    thread_state: &ThreadState<T>,
) -> Syscall {
    Syscall {
        decl: syscall_decl,
        arg0: SyscallArg::from_raw(thread_state.registers.rdi),
        arg1: SyscallArg::from_raw(thread_state.registers.rsi),
        arg2: SyscallArg::from_raw(thread_state.registers.rdx),
        arg3: SyscallArg::from_raw(thread_state.registers.r10),
        arg4: SyscallArg::from_raw(thread_state.registers.r8),
        arg5: SyscallArg::from_raw(thread_state.registers.r9),
    }
}

pub fn new_syscall(syscall_decl: SyscallDecl, current_task: &CurrentTask) -> Syscall {
    new_syscall_from_state(syscall_decl, &current_task.thread_state)
}
