// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::perf::PerfEventFile;
use crate::security::selinux_hooks::{PerfEventState, check_permission};
use crate::security::{PerfEventType, TargetTaskType};
use crate::task::CurrentTask;
use linux_uapi::perf_event_attr;
use selinux::{PerfEventPermission, SecurityServer};
use starnix_uapi::errno;
use starnix_uapi::errors::Errno;

use super::{build_permission_check, check_self_permission, current_task_state};

/// Checks whether `current_task` has the necessary permissions to open a perf_event for the given
/// target task.
pub(in crate::security) fn check_perf_event_open_access(
    security_server: &SecurityServer,
    current_task: &CurrentTask,
    target_task_type: TargetTaskType<'_>,
    attr: &perf_event_attr,
    event_type: PerfEventType,
) -> Result<(), Errno> {
    let audit_context = current_task.into();
    let subject_sid = current_task_state(current_task).current_sid;
    // Always check `perf_event { open }` permission on the current task.
    check_self_permission(
        &build_permission_check(current_task, security_server),
        current_task,
        subject_sid,
        PerfEventPermission::Open,
        audit_context,
    )?;

    // Check capability `capability2 { perfmon }` first, and if it fails check
    // `capability { sys_admin }` instead.
    if crate::security::check_task_capable(current_task, starnix_uapi::auth::CAP_PERFMON).is_err() {
        if crate::security::check_task_capable(current_task, starnix_uapi::auth::CAP_SYS_ADMIN)
            .is_err()
        {
            // Exceptionally, if the event is a tracepoint perf event on the current task and while
            // excluding kernel, we allow it.
            if matches!(
                (event_type, attr.exclude_kernel(), &target_task_type),
                (PerfEventType::Tracepoint, 1, TargetTaskType::CurrentTask)
            ) {
                return Ok(());
            }
            return Err(errno!(EACCES));
        }
    }

    // Check `perf_event { kernel }` permission when `exclude_kernel` is 0.
    if attr.exclude_kernel() == 0 {
        check_self_permission(
            &build_permission_check(current_task, security_server),
            current_task,
            subject_sid,
            PerfEventPermission::Kernel,
            audit_context,
        )?;
    }

    // Check `perf_event { cpu }` permission when
    // - type is PERF_TYPE_SOFTWARE or
    // - type is PERF_TYPE_HARDWARE or
    // - type is in [CACHE, TRACEPOINT, BREAKPOINT, RAW) and pid == -1
    let check_cpu = match event_type {
        PerfEventType::Software | PerfEventType::Hardware => true,
        PerfEventType::HwCache
        | PerfEventType::Tracepoint
        | PerfEventType::Breakpoint
        | PerfEventType::Raw => matches!(target_task_type, TargetTaskType::AllTasks),
    };
    if check_cpu {
        check_self_permission(
            &build_permission_check(current_task, security_server),
            current_task,
            subject_sid,
            PerfEventPermission::Cpu,
            audit_context,
        )?;
    }

    // Don't check `perf_event { tracepoint }` permission, even if the type is PERF_TYPE_TRACEPOINT:
    // this has been the observed behavior in SELinux.

    Ok(())
}

/// Returns the SID to be used for a PerfEventFileState object upon creation.
pub(in crate::security) fn perf_event_alloc(current_task: &CurrentTask) -> PerfEventState {
    PerfEventState { sid: current_task_state(current_task).current_sid }
}

/// Checks whether `current_task` has the necessary permissions to read the given `perf_event_file`.
pub(in crate::security) fn check_perf_event_read_access(
    security_server: &SecurityServer,
    current_task: &CurrentTask,
    perf_event_file: &PerfEventFile,
) -> Result<(), Errno> {
    let audit_context = current_task.into();
    let subject_sid = current_task_state(current_task).current_sid;
    let target_sid = perf_event_file.security_state.state.sid;
    check_permission(
        &build_permission_check(current_task, security_server),
        current_task,
        subject_sid,
        target_sid,
        PerfEventPermission::Read,
        audit_context,
    )
}

/// Checks whether `current_task` has the necessary permissions to write to the given `perf_event_file`.
pub(in crate::security) fn check_perf_event_write_access(
    security_server: &SecurityServer,
    current_task: &CurrentTask,
    perf_event_file: &PerfEventFile,
) -> Result<(), Errno> {
    let audit_context = current_task.into();
    let subject_sid = current_task_state(current_task).current_sid;
    let target_sid = perf_event_file.security_state.state.sid;
    check_permission(
        &build_permission_check(current_task, security_server),
        current_task,
        subject_sid,
        target_sid,
        PerfEventPermission::Write,
        audit_context,
    )
}
