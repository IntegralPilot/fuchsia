// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::file::file_receive;
use super::{
    BinderConnectionState, build_permission_check, check_permission, check_self_permission,
    current_task_state,
};
use crate::task::{CurrentTask, Task};
use crate::vfs::FileObject;
use selinux::{BinderPermission, SecurityServer};
use starnix_uapi::errors::Errno;

/// Returns the security state to be assigned to a Binder connection. This is defined as the
/// security context of the creating task.
pub(in crate::security) fn binder_connection_alloc(
    current_task: &CurrentTask,
) -> BinderConnectionState {
    BinderConnectionState { sid: current_task_state(current_task).current_sid }
}

/// Returns the serialized Security Context associated with the given state.
pub(in crate::security) fn binder_get_context(
    security_server: &SecurityServer,
    state: &BinderConnectionState,
) -> Option<Vec<u8>> {
    security_server.sid_to_security_context_with_nul(state.sid)
}

/// Checks whether the given `current_task` can become the binder context manager.
pub(in crate::security) fn binder_set_context_mgr(
    security_server: &SecurityServer,
    current_task: &CurrentTask,
) -> Result<(), Errno> {
    let audit_context = current_task.into();
    let sid = current_task_state(current_task).current_sid;
    check_self_permission(
        &build_permission_check(current_task, security_server),
        current_task,
        sid,
        BinderPermission::SetContextMgr,
        audit_context,
    )
}

/// Checks whether the given `current_task` has permission to send a binder transaction
/// to the `target_task`.
pub(in crate::security) fn binder_transaction(
    security_server: &SecurityServer,
    connection_security_state: &BinderConnectionState,
    current_task: &CurrentTask,
    target_task: &Task,
) -> Result<(), Errno> {
    let audit_context = current_task.into();
    let source_sid = current_task_state(current_task).current_sid;
    let target_sid = target_task.real_creds().security_state.current_sid;
    if source_sid != connection_security_state.sid {
        check_permission(
            &build_permission_check(current_task, security_server),
            current_task,
            source_sid,
            connection_security_state.sid,
            BinderPermission::Impersonate,
            audit_context,
        )?;
    }
    check_permission(
        &build_permission_check(current_task, security_server),
        current_task,
        connection_security_state.sid,
        target_sid,
        BinderPermission::Call,
        audit_context,
    )?;
    Ok(())
}

/// Checks whether the given `current_task` has permission to transfer Binder objects
/// to the `target_task`.
pub(in crate::security) fn binder_transfer_binder(
    security_server: &SecurityServer,
    current_task: &CurrentTask,
    target_task: &Task,
) -> Result<(), Errno> {
    let audit_context = current_task.into();
    let source_sid = current_task_state(current_task).current_sid;
    let target_sid = target_task.real_creds().security_state.current_sid;
    check_permission(
        &build_permission_check(current_task, security_server),
        current_task,
        source_sid,
        target_sid,
        BinderPermission::Transfer,
        audit_context,
    )
}

/// Checks whether `task` has permission to receive `file` in a Binder transaction.
pub(in crate::security) fn binder_transfer_file(
    security_server: &SecurityServer,
    current_task: &CurrentTask,
    subject_task: &Task,
    file: &FileObject,
) -> Result<(), Errno> {
    let receiving_sid = subject_task.real_creds().security_state.current_sid;
    file_receive(security_server, current_task, receiving_sid, file)
}
