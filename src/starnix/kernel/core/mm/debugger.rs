// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::task::CurrentTask;
use starnix_uapi::errors::Errno;
use starnix_uapi::from_status_like_fdio;

unsafe extern "C" {
    /// breakpoint_for_module_changes is a single breakpoint instruction that is used to notify
    /// the debugger about the module changes.
    fn breakpoint_for_module_changes();
}

/// Notifies the debugger, if one is attached, that the module list might have been changed.
///
/// For more information about the debugger protocol, see:
/// https://cs.opensource.google/fuchsia/fuchsia/+/master:src/developer/debug/debug_agent/process_handle.h;l=31
///
/// # Parameters:
/// - `current_task`: The task to set the property for. The register's of this task, the instruction
///                   pointer specifically, needs to be set to the value with which the task is
///                   expected to resume.
pub fn notify_debugger_of_module_list(current_task: &mut CurrentTask) -> Result<(), Errno> {
    let break_on_load = current_task
        .thread_group()
        .process
        .get_break_on_load()
        .map_err(|err| from_status_like_fdio!(err))?;

    // If break on load is 0, there is no debugger attached, so return before issuing the software
    // breakpoint.
    if break_on_load == 0 {
        return Ok(());
    }

    // For restricted executor, we only need to trigger the debug break on the current thread.
    let breakpoint_addr = breakpoint_for_module_changes as *const () as usize as u64;

    if breakpoint_addr != break_on_load {
        current_task
            .thread_group()
            .process
            .set_break_on_load(&breakpoint_addr)
            .map_err(|err| from_status_like_fdio!(err))?;
    }

    #[allow(
        clippy::undocumented_unsafe_blocks,
        reason = "Force documented unsafe blocks in Starnix"
    )]
    unsafe {
        breakpoint_for_module_changes();
    }

    Ok(())
}
