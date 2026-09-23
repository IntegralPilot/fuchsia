// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// TODO(https://github.com/rust-lang/rust/issues/39371): remove
#![allow(non_upper_case_globals)]

use super::{
    BpfMapState, BpfProgState, build_permission_check, check_permission, check_self_permission,
    current_task_state,
};
use crate::security::PermissionFlags;
use crate::task::CurrentTask;
use selinux::{BpfPermission, SecurityId, SecurityServer};
use starnix_uapi::errors::Errno;
use starnix_uapi::{bpf_cmd, bpf_cmd_BPF_MAP_CREATE, bpf_cmd_BPF_PROG_LOAD, bpf_cmd_BPF_PROG_RUN};
use zerocopy::FromBytes;

/// Returns the security state to be assigned to a BPF map. This is defined as the security
/// context of the creating task.
pub(in crate::security) fn bpf_map_alloc(current_task: &CurrentTask) -> BpfMapState {
    BpfMapState { sid: current_task_state(current_task).current_sid }
}

/// Returns the security state to be assigned to a BPF program. This is defined as the
/// security context of the creating task.
pub(in crate::security) fn bpf_prog_alloc(current_task: &CurrentTask) -> BpfProgState {
    BpfProgState { sid: current_task_state(current_task).current_sid }
}

/// Returns whether `current_task` can perform the bpf `cmd`.
pub(in crate::security) fn check_bpf_access<Attr: FromBytes>(
    security_server: &SecurityServer,
    current_task: &CurrentTask,
    cmd: bpf_cmd,
    _attr: &Attr,
    _attr_size: u32,
) -> Result<(), Errno> {
    let audit_context = current_task.into();

    let sid: SecurityId = current_task_state(current_task).current_sid;
    let permission = match cmd {
        bpf_cmd_BPF_MAP_CREATE => BpfPermission::MapCreate,
        bpf_cmd_BPF_PROG_LOAD => BpfPermission::ProgLoad,
        bpf_cmd_BPF_PROG_RUN => BpfPermission::ProgRun,
        _ => return Ok(()),
    };
    check_self_permission(
        &build_permission_check(current_task, security_server),
        current_task,
        sid,
        permission,
        audit_context,
    )
}

/// Performs necessary checks when the kernel generates and returns a file descriptor for BPF
/// maps.
pub(in crate::security) fn check_bpf_map_access(
    security_server: &SecurityServer,
    current_task: &CurrentTask,
    subject_sid: SecurityId,
    bpf_map_state: &crate::security::BpfMapState,
    flags: PermissionFlags,
) -> Result<(), Errno> {
    let audit_context = current_task.into();

    if flags.contains(PermissionFlags::READ) {
        check_permission(
            &build_permission_check(current_task, security_server),
            current_task,
            subject_sid,
            bpf_map_state.state.sid,
            BpfPermission::MapRead,
            audit_context,
        )?;
    }
    if flags.contains(PermissionFlags::WRITE) {
        check_permission(
            &build_permission_check(current_task, security_server),
            current_task,
            subject_sid,
            bpf_map_state.state.sid,
            BpfPermission::MapWrite,
            audit_context,
        )?;
    }
    Ok(())
}

/// Performs necessary checks when the kernel generates and returns a file descriptor for BPF
/// programs.
pub(in crate::security) fn check_bpf_prog_access(
    security_server: &SecurityServer,
    current_task: &CurrentTask,
    subject_sid: SecurityId,
    bpf_program_state: &crate::security::BpfProgState,
) -> Result<(), Errno> {
    let audit_context = current_task.into();

    check_permission(
        &build_permission_check(current_task, security_server),
        current_task,
        subject_sid,
        bpf_program_state.state.sid,
        BpfPermission::ProgRun,
        audit_context,
    )
}
