// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::task::{CurrentTask, ExitStatus};
use std::sync::atomic::{AtomicPtr, Ordering};

/// The type of function that must be provided by the kernel binary to enter the syscall loop.
pub type SyscallLoopEntry = fn(&mut CurrentTask) -> ExitStatus;

// Need to make sure the function pointer is actually just a pointer to store it safely in atomic.
static_assertions::assert_eq_size!(SyscallLoopEntry, *const ());

static SYSCALL_LOOP_ENTRY: AtomicPtr<()> = AtomicPtr::new(std::ptr::null_mut());

/// Initialize the syscall loop entry function.
pub fn initialize_syscall_loop(enter_loop: SyscallLoopEntry) {
    SYSCALL_LOOP_ENTRY.store(enter_loop as *mut (), Ordering::Relaxed);
}

/// Enter the syscall loop on the calling thread.
///
/// Returns the final exit status of the task.
pub(crate) fn enter_syscall_loop(current_task: &mut CurrentTask) -> ExitStatus {
    let raw_entry: *mut () = SYSCALL_LOOP_ENTRY.load(Ordering::Relaxed);
    assert!(!raw_entry.is_null(), "must call initialize_syscall_loop() before executing tasks");
    // SAFETY: the static variable only has SyscallLoopEntry values stored into it.
    let entry: SyscallLoopEntry = unsafe {
        let raw_entry_ptr = &raw_entry as *const *mut ();
        let entry_ptr = raw_entry_ptr as *const SyscallLoopEntry;
        *entry_ptr
    };
    entry(current_task)
}
