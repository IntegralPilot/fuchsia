// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::security::selinux_hooks::{
    CommonFsNodePermission, FileClass, KernelPermission, PermissionCheck, ProcessPermission,
    build_permission_check, check_permission, check_self_permission, current_task_state,
    fs_node_effective_sid_and_class, fs_node_ensure_class, fs_node_set_label_with_task,
    has_file_permissions, is_internal_operation, permissions_from_flags, task_consistent_attrs,
};
use crate::security::{Arc, Auditable, ProcAttr, SecurityId, SecurityServer};
use crate::task::loader::ResolvedProgram;
use crate::task::{CurrentTask, Task};
use crate::vfs::{FsNode, FsStr, NamespaceNode};
use selinux::{
    Cap2Class, CapClass, CommonCap2Permission, CommonCapPermission, FilePermission, ForClass,
    InitialSid, KernelClass, NullessByteStr, PolicyCap, Process2Permission, SystemPermission,
    TaskAttrs,
};
use starnix_uapi::auth::{
    Credentials, PTRACE_MODE_ATTACH, PTRACE_MODE_NOAUDIT, PTRACE_MODE_READ, PtraceAccessMode,
};
use starnix_uapi::errors::Errno;
use starnix_uapi::mount_flags::MountFlags;
use starnix_uapi::resource_limits::Resource;
use starnix_uapi::signals::{SIGCHLD, SIGKILL, SIGSTOP, Signal};
use starnix_uapi::syslog::SyslogAction;
use starnix_uapi::{
    ITIMER_PROF, ITIMER_REAL, ITIMER_VIRTUAL, errno, error, itimerval, rlimit, timeval,
};

/// If the task SID is changing during `exec`, enforces permissions that relate to inheritance of
/// the calling task's file descriptor access and resource limits by the callee:
/// 1. Revoke access to any file descriptors that `current_task` is not permitted to access.
/// 2. Reset resource limits if `current_task` is not permitted to inherit rlimits.
///
/// Corresponds to the `bprm_committing_creds()` LSM hook.
pub(in crate::security) fn bprm_committing_creds(
    security_server: &Arc<SecurityServer>,
    current_task: &CurrentTask,
    resolved_program: &ResolvedProgram,
) {
    let new_sid = resolved_program.creds.security_state.current_sid;
    let previous_sid = resolved_program.creds.security_state.previous_sid;
    debug_assert!(previous_sid == current_task.current_creds().security_state.current_sid);
    if new_sid == previous_sid {
        return;
    }
    close_inaccessible_file_descriptors(security_server, current_task, new_sid);
    maybe_reset_rlimits(security_server, current_task, previous_sid, new_sid);
}

/// If the task SID is changing during `exec`, resets signal state if `current task` is not
/// permitted to inherit the parent task's signal state.
///
/// Corresponds to the `bprm_committed_creds()` LSM hook.
pub(in crate::security) fn bprm_committed_creds(
    security_server: &Arc<SecurityServer>,
    current_task: &CurrentTask,
) {
    let (previous_sid, new_sid) = {
        let state = current_task_state(current_task);
        (state.previous_sid, state.current_sid)
    };
    if new_sid == previous_sid {
        return;
    }

    maybe_reset_signal_state(security_server, current_task, previous_sid, new_sid);
}

/// "Closes" file descriptors that `current_task` does not have permission to access by remapping
/// those file descriptors to the null file in selinuxfs.
fn close_inaccessible_file_descriptors(
    security_server: &Arc<SecurityServer>,
    current_task: &CurrentTask,
    new_sid: SecurityId,
) {
    let kernel_state = current_task
        .kernel()
        .security_state
        .state
        .as_ref()
        .expect("kernel has security state because SELinux is enabled");

    let null_file_handle =
        kernel_state.selinuxfs_null.get().expect("selinuxfs_init_null() has been called").clone();

    let audit_context = current_task.into();
    let source_sid = new_sid;
    let permission_check = build_permission_check(&current_task, security_server);
    // Remap-to-null any fds that failed a check for allowing
    // `[child-process] [fd-from-child-fd-table]:fd { use }`,
    // or for any of the file permissions associated with the file mode and flags.
    current_task.files().remap(current_task, |file| {
        let permissions = permissions_from_flags(
            file.flags().into(),
            fs_node_effective_sid_and_class(file.node()).class,
        );
        let permission_result = has_file_permissions(
            &permission_check,
            current_task,
            source_sid,
            file,
            &permissions,
            audit_context,
        );
        permission_result.map_or_else(|_| Some(null_file_handle.clone()), |_| None)
    });
}

/// Checks the `rlimitinh` permission for the current task. If the permission is denied, resets
/// the current task's resource limits.
fn maybe_reset_rlimits(
    security_server: &Arc<SecurityServer>,
    current_task: &CurrentTask,
    previous_sid: SecurityId,
    new_sid: SecurityId,
) {
    let audit_context = current_task.into();
    let permission_check = build_permission_check(&current_task, security_server);
    if check_permission(
        &permission_check,
        current_task,
        previous_sid,
        new_sid,
        ProcessPermission::RlimitInh,
        audit_context,
    )
    .is_ok()
    {
        // Allow the resource limit inheritance that was applied when the current
        // task was created.
        return;
    }
    // Compute the new soft resource limits for the current task.
    // For each resource, the new soft limit is the minimum of the current task's hard limit
    // and the initial task's soft limit.
    let init_task = current_task.kernel().get_init_task().expect("get the initial task");
    let init_rlimits = { init_task.thread_group().limits.lock().clone() };
    let mut current_rlimits = current_task.thread_group().limits.lock();
    (Resource::ALL).iter().for_each(|resource| {
        let current = current_rlimits.get(*resource);
        let init = init_rlimits.get(*resource);
        current_rlimits.set(
            *resource,
            rlimit {
                rlim_cur: std::cmp::min(init.rlim_cur, current.rlim_max),
                rlim_max: current.rlim_max,
            },
        )
    });
}

/// Checks the `siginh` permission for the current task. If the permission is denied, resets
/// the current task's signal state.
fn maybe_reset_signal_state(
    security_server: &Arc<SecurityServer>,
    current_task: &CurrentTask,
    previous_sid: SecurityId,
    new_sid: SecurityId,
) {
    let audit_context = current_task.into();
    let permission_check = build_permission_check(&current_task, security_server);
    if check_permission(
        &permission_check,
        current_task,
        previous_sid,
        new_sid,
        ProcessPermission::SigInh,
        audit_context,
    )
    .is_ok()
    {
        // Allow the signal state inheritance that was applied when the current task
        // was created.
        return;
    }

    // Clear itimers.
    for timer in &[ITIMER_REAL, ITIMER_PROF, ITIMER_VIRTUAL] {
        current_task
            .thread_group()
            .set_itimer(
                &current_task,
                *timer,
                itimerval {
                    it_value: timeval { tv_sec: 0, tv_usec: 0 },
                    it_interval: timeval { tv_sec: 0, tv_usec: 0 },
                },
            )
            .unwrap_or_else(|_| panic!("unset itimer {}", timer));
    }

    // If another process dispatches a signal to this one mid-`exec()` then it is always possible
    // for it to be received before the credentials are committed (in which case it should be
    // cleared), or afterward (in which case it need not be cleared).  It's therefore acceptable to
    // take the signal queues' locks here, rather than holding them across the credentials commit.
    // Fatal signals (notably `SIGKILL`) should not be cleared on domain transitions, since the
    // process is already doomed and should still terminate rather than returning to userspace once
    // the `exec()` is complete.
    // TODO: https://fxbug.dev/509895244 - Preserve queued fatal signals.
    let mut task_mutable_state = current_task.write();
    let mut thread_group_signal_queue = current_task.thread_group().pending_signals.lock();

    // Clear the task-local signal state (except for pending internal Starnix signals).
    task_mutable_state.signals_mut().reset_to_default();

    // Clear the thread group's pending signals.
    thread_group_signal_queue.clear();

    // Reset signal dispositions.
    current_task.thread_group().signal_actions.reset_to_default();
}

/// Returns `TaskAttrs` for a new `Task` that will run in the specified `context`.
pub(in crate::security) fn task_alloc_from_context(
    security_server: &SecurityServer,
    context: &FsStr,
) -> Result<TaskAttrs, Errno> {
    const INITIAL_PREFIX: &[u8] = b"#";
    let sid = if context.starts_with(INITIAL_PREFIX) {
        let name = &*context[INITIAL_PREFIX.len()..];
        let initial_sid = InitialSid::all_variants().iter().find(|&x| x.name().as_bytes() == name);
        (*initial_sid.ok_or_else(|| errno!(EINVAL))?).into()
    } else {
        security_server
            .security_context_to_sid(context.into())
            .map_err(|e| errno!(EINVAL, format!("{:?}", e)))?
    };
    Ok(TaskAttrs::for_transition(sid, InitialSid::Kernel.into()))
}

/// Checks if creating a task is allowed.
pub(in crate::security) fn check_task_create_access(
    permission_check: &PermissionCheck<'_>,
    current_task: &CurrentTask,
) -> Result<(), Errno> {
    let audit_context = current_task.into();
    let task_sid = task_consistent_attrs(current_task).current_sid;
    check_self_permission(
        permission_check,
        current_task,
        task_sid,
        ProcessPermission::Fork,
        audit_context,
    )
}

/// Helper used by bprm_creds_for_exec to check unbounded transitions for no-new-privileges tasks
/// and no-SUID mounts.
fn check_nnp_nosuid_transition(
    permission_check: &PermissionCheck<'_>,
    new_sid: SecurityId,
    current_sid: SecurityId,
    executable: &NamespaceNode,
    current_task: &CurrentTask,
    audit_context: Auditable<'_>,
) -> Result<(), Errno> {
    // Always allow transitions into bounded (less privileged) domains.
    if permission_check.security_server().is_bounded_by(new_sid, current_sid) {
        return Ok(());
    }

    // Allow the transition unless one of no-new-privileges (NNP) or no-SUID are set.
    let no_new_privs = current_task.read().no_new_privs();
    let no_suid = executable.mount.flags().contains(MountFlags::NOSUID);
    if !no_new_privs && !no_suid {
        return Ok(());
    }

    // If the NNP / no-SUID permission checks are not enabled by policy then just deny the operation.
    if !permission_check.security_server().is_policycap_enabled(PolicyCap::NnpNosuidTransition) {
        if !permission_check.security_server().is_enforcing() {
            return Ok(());
        }
        return error!(EACCES);
    }

    if no_new_privs {
        check_permission(
            &permission_check,
            current_task,
            current_sid,
            new_sid,
            Process2Permission::NnpTransition,
            audit_context,
        )?;
    }
    if no_suid {
        check_permission(
            &permission_check,
            current_task,
            current_sid,
            new_sid,
            Process2Permission::NosuidTransition,
            audit_context,
        )?;
    }

    Ok(())
}

/// Checks SELinux permissions required for `execve`, updating [`ResolvedProgram`] with the
/// post-exec security context and [`ResolvedProgram::secure_exec`] state if allowed.
///
/// Corresponds to the `bprm_creds_for_exec()` LSM hook.
pub(in crate::security) fn bprm_creds_for_exec(
    security_server: &Arc<SecurityServer>,
    current_task: &CurrentTask,
    resolved_program: &mut ResolvedProgram,
) -> Result<(), Errno> {
    let permission_check = build_permission_check(&current_task, security_server);
    let TaskAttrs { current_sid, exec_sid, .. } = *task_consistent_attrs(current_task);

    let executable: &NamespaceNode = resolved_program.file.name();
    let executable_sid = fs_node_effective_sid_and_class(&executable.entry.node).sid;

    let new_sid = if let Some(exec_sid) = exec_sid {
        // Use the proc exec SID if set.
        exec_sid
    } else {
        permission_check
            .compute_create_sid(current_sid, executable_sid, KernelClass::Process, &[])
            .map_err(|_| errno!(EACCES))?
    };

    let task_audit_context = current_task.into();
    let executable_audit_context = [task_audit_context, executable.into()];
    if current_sid == new_sid {
        // To `exec()` a binary in the caller's domain, the caller must be granted
        // "execute_no_trans" permission to the binary.
        check_permission(
            &permission_check,
            current_task,
            current_sid,
            executable_sid,
            FilePermission::ExecuteNoTrans,
            (&executable_audit_context).into(),
        )?;
    } else {
        // Check that the domain transition is allowed.
        check_permission(
            &permission_check,
            current_task,
            current_sid,
            new_sid,
            ProcessPermission::Transition,
            task_audit_context,
        )?;

        check_nnp_nosuid_transition(
            &permission_check,
            new_sid,
            current_sid,
            executable,
            current_task,
            task_audit_context,
        )?;

        // Check that the executable file has an entry point into the new domain.
        check_permission(
            &permission_check,
            current_task,
            new_sid,
            executable_sid,
            FilePermission::Entrypoint,
            (&executable_audit_context).into(),
        )?;

        // Check that ptrace permission is allowed if the process is traced.
        if let Some(ptracer) = current_task.ptracer_task() {
            let tracer_sid = ptracer.real_creds().security_state.current_sid;
            // TODO: https://fxbug.dev/412581419 - SIGKILL the process on failure.
            check_permission(
                &permission_check,
                current_task,
                tracer_sid,
                new_sid,
                ProcessPermission::Ptrace,
                task_audit_context,
            )
            .map_err(|_| errno!(EPERM))?;
        }

        // If the process shares filesystem context with other processes (via `CLONE_FS`) then check
        // for the share permission to the new domain.
        if current_task.has_shared_fs() {
            check_permission(
                &permission_check,
                current_task,
                current_sid,
                new_sid,
                ProcessPermission::Share,
                task_audit_context,
            )?;
        }
    }

    // Check whether the executable should run in secure mode.
    let secure_exec = current_sid != new_sid
        && check_permission(
            &permission_check,
            current_task,
            current_sid,
            new_sid,
            ProcessPermission::NoAtSecure,
            task_audit_context,
        )
        .is_err();

    resolved_program.creds.security_state = TaskAttrs::for_transition(new_sid, current_sid);
    resolved_program.secure_exec |= secure_exec;

    Ok(())
}

/// Checks if source with `source_sid` may exercise the "getsched" permission on target with
/// `target_sid` according to SELinux server status `status` and permission checker
/// `permission`.
pub(in crate::security) fn check_getsched_access(
    permission_check: &PermissionCheck<'_>,
    current_task: &CurrentTask,
    target: &Task,
) -> Result<(), Errno> {
    let audit_context = current_task.into();
    let source_sid = current_task_state(current_task).current_sid;
    let target_sid = target.real_creds().security_state.current_sid;
    check_permission(
        permission_check,
        current_task,
        source_sid,
        target_sid,
        ProcessPermission::GetSched,
        audit_context,
    )
}

/// Checks if the task with `source_sid` is allowed to set scheduling parameters or nice for the task with
/// `target_sid`.
pub(in crate::security) fn check_setsched_access(
    permission_check: &PermissionCheck<'_>,
    current_task: &CurrentTask,
    target: &Task,
) -> Result<(), Errno> {
    let audit_context = current_task.into();
    let source_sid = current_task_state(current_task).current_sid;
    let target_sid = target.real_creds().security_state.current_sid;
    check_permission(
        permission_check,
        current_task,
        source_sid,
        target_sid,
        ProcessPermission::SetSched,
        audit_context,
    )
}

/// Checks if the task with `source_sid` is allowed to get the process group ID of the task with
/// `target_sid`.
pub(in crate::security) fn check_getpgid_access(
    permission_check: &PermissionCheck<'_>,
    current_task: &CurrentTask,
    target: &Task,
) -> Result<(), Errno> {
    let audit_context = current_task.into();
    let source_sid = current_task_state(current_task).current_sid;
    let target_sid = target.real_creds().security_state.current_sid;
    check_permission(
        permission_check,
        current_task,
        source_sid,
        target_sid,
        ProcessPermission::GetPgid,
        audit_context,
    )
}

/// Checks if the task with `source_sid` is allowed to set the process group ID of the task with
/// `target_sid`.
pub(in crate::security) fn check_setpgid_access(
    permission_check: &PermissionCheck<'_>,
    current_task: &CurrentTask,
    target: &Task,
) -> Result<(), Errno> {
    let audit_context = current_task.into();
    let source_sid = current_task_state(current_task).current_sid;
    let target_sid = target.real_creds().security_state.current_sid;
    check_permission(
        permission_check,
        current_task,
        source_sid,
        target_sid,
        ProcessPermission::SetPgid,
        audit_context,
    )
}

/// Checks if the task with `source_sid` has permission to read the session Id from a task with `target_sid`.
/// Corresponds to the `task_getsid` LSM hook.
pub(in crate::security) fn check_task_getsid(
    permission_check: &PermissionCheck<'_>,
    current_task: &CurrentTask,
    target: &Task,
) -> Result<(), Errno> {
    let audit_context = current_task.into();
    let source_sid = current_task_state(current_task).current_sid;
    let target_sid = target.real_creds().security_state.current_sid;
    check_permission(
        permission_check,
        current_task,
        source_sid,
        target_sid,
        ProcessPermission::GetSession,
        audit_context,
    )
}

/// Checks if the task with `source_sid` is allowed to send `signal` to the task with `target_sid`.
pub(in crate::security) fn check_signal_access(
    permission_check: &PermissionCheck<'_>,
    current_task: &CurrentTask,
    target: &Task,
    signal: Signal,
) -> Result<(), Errno> {
    let audit_context = current_task.into();
    let source_sid = current_task_state(current_task).current_sid;
    let target_sid = target.real_creds().security_state.current_sid;
    match signal {
        // The `sigkill` permission is required for sending SIGKILL.
        SIGKILL => check_permission(
            permission_check,
            current_task,
            source_sid,
            target_sid,
            ProcessPermission::SigKill,
            audit_context,
        ),
        // The `sigstop` permission is required for sending SIGSTOP.
        SIGSTOP => check_permission(
            permission_check,
            current_task,
            source_sid,
            target_sid,
            ProcessPermission::SigStop,
            audit_context,
        ),
        // The `sigchld` permission is required for sending SIGCHLD.
        SIGCHLD => check_permission(
            permission_check,
            current_task,
            source_sid,
            target_sid,
            ProcessPermission::SigChld,
            audit_context,
        ),
        // The `signal` permission is required for sending any signal other than SIGKILL, SIGSTOP
        // or SIGCHLD.
        _ => check_permission(
            permission_check,
            current_task,
            source_sid,
            target_sid,
            ProcessPermission::Signal,
            audit_context,
        ),
    }
}

pub(in crate::security) fn check_syslog_access(
    permission_check: &PermissionCheck<'_>,
    current_task: &CurrentTask,
    action: SyslogAction,
) -> Result<(), Errno> {
    let sid = current_task_state(current_task).current_sid;
    let required_permission = match action {
        SyslogAction::ReadAll | SyslogAction::SizeBuffer => SystemPermission::SyslogRead,
        SyslogAction::ConsoleOff | SyslogAction::ConsoleOn | SyslogAction::ConsoleLevel => {
            SystemPermission::SyslogConsole
        }
        SyslogAction::Close
        | SyslogAction::Open
        | SyslogAction::Read
        | SyslogAction::ReadClear
        | SyslogAction::Clear
        | SyslogAction::SizeUnread => SystemPermission::SyslogMod,
    };
    check_permission(
        permission_check,
        current_task,
        sid,
        InitialSid::Kernel.into(),
        required_permission,
        current_task.into(),
    )
}

/// Checks if the `current_task` is allowed to query the Linux capabilities of the `target` task.
pub(in crate::security) fn check_getcap_access(
    permission_check: &PermissionCheck<'_>,
    current_task: &CurrentTask,
    target: &Task,
) -> Result<(), Errno> {
    let audit_context = current_task.into();
    let source_sid = current_task_state(current_task).current_sid;
    let target_sid = target.real_creds().security_state.current_sid;
    check_permission(
        permission_check,
        current_task,
        source_sid,
        target_sid,
        ProcessPermission::GetCap,
        audit_context,
    )
}

/// Checks if the `current_task` is allowed to set the Linux capabilities of the `target` task.
pub(in crate::security) fn check_setcap_access(
    permission_check: &PermissionCheck<'_>,
    current_task: &CurrentTask,
    target: &Task,
) -> Result<(), Errno> {
    let audit_context = current_task.into();
    let source_sid = current_task_state(current_task).current_sid;
    let target_sid = target.real_creds().security_state.current_sid;
    check_permission(
        permission_check,
        current_task,
        source_sid,
        target_sid,
        ProcessPermission::SetCap,
        audit_context,
    )
}

fn permission_from_capability(capability: starnix_uapi::auth::Capabilities) -> KernelPermission {
    // TODO: https://fxbug.dev/297313673 - CapClass::CapUserns will play a role here if-and-after
    // user namespaces are implemented in Starnix.
    match capability {
        // Mappings of capabilities to SELinux "cap" class permissions.
        starnix_uapi::auth::CAP_AUDIT_CONTROL => {
            CommonCapPermission::AuditControl.for_class(CapClass::Capability)
        }
        starnix_uapi::auth::CAP_AUDIT_WRITE => {
            CommonCapPermission::AuditWrite.for_class(CapClass::Capability)
        }
        starnix_uapi::auth::CAP_CHOWN => CommonCapPermission::Chown.for_class(CapClass::Capability),
        starnix_uapi::auth::CAP_DAC_OVERRIDE => {
            CommonCapPermission::DacOverride.for_class(CapClass::Capability)
        }
        starnix_uapi::auth::CAP_DAC_READ_SEARCH => {
            CommonCapPermission::DacReadSearch.for_class(CapClass::Capability)
        }
        starnix_uapi::auth::CAP_FOWNER => {
            CommonCapPermission::Fowner.for_class(CapClass::Capability)
        }
        starnix_uapi::auth::CAP_FSETID => {
            CommonCapPermission::Fsetid.for_class(CapClass::Capability)
        }
        starnix_uapi::auth::CAP_IPC_LOCK => {
            CommonCapPermission::IpcLock.for_class(CapClass::Capability)
        }
        starnix_uapi::auth::CAP_IPC_OWNER => {
            CommonCapPermission::IpcOwner.for_class(CapClass::Capability)
        }
        starnix_uapi::auth::CAP_KILL => CommonCapPermission::Kill.for_class(CapClass::Capability),
        starnix_uapi::auth::CAP_LEASE => CommonCapPermission::Lease.for_class(CapClass::Capability),
        starnix_uapi::auth::CAP_LINUX_IMMUTABLE => {
            CommonCapPermission::LinuxImmutable.for_class(CapClass::Capability)
        }
        starnix_uapi::auth::CAP_MKNOD => CommonCapPermission::Mknod.for_class(CapClass::Capability),
        starnix_uapi::auth::CAP_NET_ADMIN => {
            CommonCapPermission::NetAdmin.for_class(CapClass::Capability)
        }
        starnix_uapi::auth::CAP_NET_BIND_SERVICE => {
            CommonCapPermission::NetBindService.for_class(CapClass::Capability)
        }
        starnix_uapi::auth::CAP_NET_BROADCAST => {
            CommonCapPermission::NetBroadcast.for_class(CapClass::Capability)
        }
        starnix_uapi::auth::CAP_NET_RAW => {
            CommonCapPermission::NetRaw.for_class(CapClass::Capability)
        }
        starnix_uapi::auth::CAP_SETFCAP => {
            CommonCapPermission::Setfcap.for_class(CapClass::Capability)
        }
        starnix_uapi::auth::CAP_SETGID => {
            CommonCapPermission::Setgid.for_class(CapClass::Capability)
        }
        starnix_uapi::auth::CAP_SETPCAP => {
            CommonCapPermission::Setpcap.for_class(CapClass::Capability)
        }
        starnix_uapi::auth::CAP_SETUID => {
            CommonCapPermission::Setuid.for_class(CapClass::Capability)
        }
        starnix_uapi::auth::CAP_SYS_ADMIN => {
            CommonCapPermission::SysAdmin.for_class(CapClass::Capability)
        }
        starnix_uapi::auth::CAP_SYS_BOOT => {
            CommonCapPermission::SysBoot.for_class(CapClass::Capability)
        }
        starnix_uapi::auth::CAP_SYS_CHROOT => {
            CommonCapPermission::SysChroot.for_class(CapClass::Capability)
        }
        starnix_uapi::auth::CAP_SYS_MODULE => {
            CommonCapPermission::SysModule.for_class(CapClass::Capability)
        }
        starnix_uapi::auth::CAP_SYS_NICE => {
            CommonCapPermission::SysNice.for_class(CapClass::Capability)
        }
        starnix_uapi::auth::CAP_SYS_PACCT => {
            CommonCapPermission::SysPacct.for_class(CapClass::Capability)
        }
        starnix_uapi::auth::CAP_SYS_PTRACE => {
            CommonCapPermission::SysPtrace.for_class(CapClass::Capability)
        }
        starnix_uapi::auth::CAP_SYS_RAWIO => {
            CommonCapPermission::SysRawio.for_class(CapClass::Capability)
        }
        starnix_uapi::auth::CAP_SYS_RESOURCE => {
            CommonCapPermission::SysResource.for_class(CapClass::Capability)
        }
        starnix_uapi::auth::CAP_SYS_TIME => {
            CommonCapPermission::SysTime.for_class(CapClass::Capability)
        }
        starnix_uapi::auth::CAP_SYS_TTY_CONFIG => {
            CommonCapPermission::SysTtyConfig.for_class(CapClass::Capability)
        }

        // Mappings of capabilities to SELinux "cap2" class permissions.
        starnix_uapi::auth::CAP_AUDIT_READ => {
            CommonCap2Permission::AuditRead.for_class(Cap2Class::Capability2)
        }
        starnix_uapi::auth::CAP_BLOCK_SUSPEND => {
            CommonCap2Permission::BlockSuspend.for_class(Cap2Class::Capability2)
        }
        starnix_uapi::auth::CAP_BPF => CommonCap2Permission::Bpf.for_class(Cap2Class::Capability2),
        starnix_uapi::auth::CAP_MAC_ADMIN => {
            CommonCap2Permission::MacAdmin.for_class(Cap2Class::Capability2)
        }
        starnix_uapi::auth::CAP_MAC_OVERRIDE => {
            CommonCap2Permission::MacOverride.for_class(Cap2Class::Capability2)
        }
        starnix_uapi::auth::CAP_SYSLOG => {
            CommonCap2Permission::Syslog.for_class(Cap2Class::Capability2)
        }
        starnix_uapi::auth::CAP_WAKE_ALARM => {
            CommonCap2Permission::WakeAlarm.for_class(Cap2Class::Capability2)
        }
        starnix_uapi::auth::CAP_PERFMON => {
            CommonCap2Permission::Perfmon.for_class(Cap2Class::Capability2)
        }

        _ => {
            panic!("Unrecognized capabilities \"{:?}\" passed to check_capable!", capability)
        }
    }
}

pub(in crate::security) fn is_task_capable_noaudit(
    permission_check: &PermissionCheck<'_>,
    current_task: &CurrentTask,
    capability: starnix_uapi::auth::Capabilities,
) -> bool {
    let sid = current_task_state(current_task).current_sid;
    let permission = permission_from_capability(capability);
    is_internal_operation(current_task)
        || permission_check.has_permission(sid, sid, permission).permit()
}

pub(in crate::security) fn check_creds_capable(
    permission_check: &PermissionCheck<'_>,
    current_task: &CurrentTask,
    creds: &Credentials,
    capability: starnix_uapi::auth::Capabilities,
) -> Result<(), Errno> {
    let sid = creds.security_state.current_sid;
    let permission = permission_from_capability(capability);
    check_self_permission(&permission_check, current_task, sid, permission, current_task.into())
        .map_err(|_| errno!(EPERM))
}

/// Checks if the task with `source_sid` has the permission to get and/or set limits on the task with `target_sid`.
pub(in crate::security) fn task_prlimit(
    permission_check: &PermissionCheck<'_>,
    current_task: &CurrentTask,
    target: &Task,
    check_get_rlimit: bool,
    check_set_rlimit: bool,
) -> Result<(), Errno> {
    let audit_context = current_task.into();
    let source_sid = current_task_state(current_task).current_sid;
    let target_sid = target.real_creds().security_state.current_sid;
    if check_get_rlimit {
        check_permission(
            permission_check,
            current_task,
            source_sid,
            target_sid,
            ProcessPermission::GetRlimit,
            audit_context,
        )?;
    }
    if check_set_rlimit {
        check_permission(
            permission_check,
            current_task,
            source_sid,
            target_sid,
            ProcessPermission::SetRlimit,
            audit_context,
        )?;
    }
    Ok(())
}

/// Check permission before setting the max resource limits of `target` from `old_limit` to `new_limit`.
pub(in crate::security) fn task_setrlimit(
    permission_check: &PermissionCheck<'_>,
    current_task: &CurrentTask,
    target: &Task,
    old_limit: rlimit,
    new_limit: rlimit,
) -> Result<(), Errno> {
    let audit_context = current_task.into();
    let source_sid = current_task_state(current_task).current_sid;
    let target_sid = target.real_creds().security_state.current_sid;
    if new_limit.rlim_max != old_limit.rlim_max {
        check_permission(
            permission_check,
            current_task,
            source_sid,
            target_sid,
            ProcessPermission::SetRlimit,
            audit_context,
        )?;
    }
    Ok(())
}

/// Checks if the `tracer` is allowed to trace the current task.
pub(in crate::security) fn ptrace_traceme(
    permission_check: &PermissionCheck<'_>,
    current_task: &CurrentTask,
    tracer: &Task,
) -> Result<(), Errno> {
    let audit_context = current_task.into();
    let tracer_sid = tracer.real_creds().security_state.current_sid;
    let tracee_sid = current_task_state(current_task).current_sid;
    check_permission(
        permission_check,
        current_task,
        tracer_sid,
        tracee_sid,
        ProcessPermission::Ptrace,
        audit_context,
    )
}

/// Checks if the `current_task` is permitted the to p-trace `target` with the specified `mode.
pub(in crate::security) fn ptrace_access_check(
    permission_check: &PermissionCheck<'_>,
    current_task: &CurrentTask,
    target: &Task,
    mode: PtraceAccessMode,
) -> Result<(), Errno> {
    let audit_context = current_task.into();
    let tracer_sid = current_task_state(current_task).current_sid;
    let tracee_sid = target.real_creds().security_state.current_sid;
    let permission: KernelPermission =
        if mode.contains(PTRACE_MODE_READ) && !mode.contains(PTRACE_MODE_ATTACH) {
            CommonFsNodePermission::Read.for_class(FileClass::File).into()
        } else {
            ProcessPermission::Ptrace.into()
        };
    if mode.contains(PTRACE_MODE_NOAUDIT) {
        let result = permission_check.has_permission(tracer_sid, tracee_sid, permission);
        return result.permit().then_some(()).ok_or_else(|| errno!(EACCES));
    }
    check_permission(
        permission_check,
        current_task,
        tracer_sid,
        tracee_sid,
        permission,
        audit_context,
    )
}

/// Returns the Security Context associated with the `name`ed entry for the specified `target` task.
/// `source` describes the calling task, `target` the state of the task for which to return the attribute.
pub(in crate::security) fn get_procattr(
    security_server: &SecurityServer,
    _current_task: &CurrentTask,
    task: &Task,
    attr: ProcAttr,
) -> Result<Vec<u8>, Errno> {
    let task_attrs = &task.real_creds().security_state;

    let sid = match attr {
        ProcAttr::Current => Some(task_attrs.current_sid),
        ProcAttr::Exec => task_attrs.exec_sid,
        ProcAttr::FsCreate => task_attrs.fscreate_sid,
        ProcAttr::KeyCreate => task_attrs.keycreate_sid,
        ProcAttr::Previous => Some(task_attrs.previous_sid),
        ProcAttr::SockCreate => task_attrs.sockcreate_sid,
    };

    // Convert it to a Security Context string.
    Ok(sid
        .and_then(|sid| security_server.sid_to_security_context_with_nul(sid))
        .unwrap_or_default())
}

/// Sets the Security Context associated with the `attr` entry in the task security state.
pub(in crate::security) fn set_procattr(
    security_server: &Arc<SecurityServer>,
    current_task: &CurrentTask,
    attr: ProcAttr,
    context: &[u8],
) -> Result<(), Errno> {
    // Attempt to convert the Security Context string to a SID.
    let context = NullessByteStr::from(context);
    let sid = match context.as_bytes() {
        b"\x0a" | b"" => None,
        _ => Some(security_server.security_context_to_sid(context).map_err(|_| errno!(EINVAL))?),
    };

    let audit_context = current_task.into();
    let permission_check = build_permission_check(&current_task, security_server);
    let current_sid = task_consistent_attrs(current_task).current_sid;
    let mut creds = Credentials::clone(&current_task.current_creds());

    match attr {
        ProcAttr::Current => {
            check_self_permission(
                &permission_check,
                current_task,
                current_sid,
                ProcessPermission::SetCurrent,
                audit_context,
            )?;

            // Permission to dynamically transition to the new Context is also required.
            let new_sid = sid.ok_or_else(|| errno!(EINVAL))?;
            check_permission(
                &permission_check,
                current_task,
                current_sid,
                new_sid,
                ProcessPermission::DynTransition,
                audit_context,
            )?;

            if current_task.thread_group().read().tasks_count() > 1 {
                // In multi-threaded programs dynamic transitions may only be used to down-scope
                // the capabilities available to the task. This is verified by requiring an explicit
                // "typebounds" relationship between the current and target domains, indicating that
                // the constraint on permissions of the bounded type has been verified by the policy
                // build tooling and/or will be enforced at run-time on permission checks.
                if !security_server.is_bounded_by(new_sid, current_sid) {
                    return error!(EACCES);
                }
            }

            // Check that ptrace permission is allowed if the process is traced.
            if let Some(ptracer) = current_task.ptracer_task() {
                let tracer_sid = ptracer.real_creds().security_state.current_sid;
                // TODO: https://fxbug.dev/412581419 - SIGKILL the process on failure.
                check_permission(
                    &permission_check,
                    current_task,
                    tracer_sid,
                    new_sid,
                    ProcessPermission::Ptrace,
                    audit_context,
                )?;
            }

            creds.security_state.current_sid = new_sid;
        }
        ProcAttr::Previous => {
            return error!(EINVAL);
        }
        ProcAttr::Exec => {
            check_self_permission(
                &permission_check,
                current_task,
                current_sid,
                ProcessPermission::SetExec,
                audit_context,
            )?;
            creds.security_state.exec_sid = sid;
        }
        ProcAttr::FsCreate => {
            check_self_permission(
                &permission_check,
                current_task,
                current_sid,
                ProcessPermission::SetFsCreate,
                audit_context,
            )?;
            creds.security_state.fscreate_sid = sid;
        }
        ProcAttr::KeyCreate => {
            check_self_permission(
                &permission_check,
                current_task,
                current_sid,
                ProcessPermission::SetKeyCreate,
                audit_context,
            )?;
            creds.security_state.keycreate_sid = sid;
        }
        ProcAttr::SockCreate => {
            check_self_permission(
                &permission_check,
                current_task,
                current_sid,
                ProcessPermission::SetSockCreate,
                audit_context,
            )?;
            creds.security_state.sockcreate_sid = sid;
        }
    };

    current_task.set_creds(creds);
    Ok(())
}

/// Sets the sid of `fs_node` to be that of `task`.
pub(in crate::security) fn fs_node_init_with_task(task: &Task, fs_node: &FsNode) {
    fs_node_ensure_class(fs_node).unwrap();
    fs_node_set_label_with_task(fs_node, &task.persistent_info);
}

#[cfg(test)]
mod tests {

    use super::*;

    use crate::security::selinux_hooks::testing::{
        create_test_executable, mutate_attrs_for_test, open_test_file,
    };
    use crate::security::selinux_hooks::{InitialSid, TaskAttrs, testing};
    use crate::signals::SignalInfo;
    use crate::testing::create_task_with_security_context;
    use starnix_uapi::auth::PTRACE_MODE_ATTACH;
    use starnix_uapi::signals::{SIGTERM, SigSet};
    use starnix_uapi::{CLONE_SIGHAND, CLONE_THREAD, CLONE_VM, error};
    use std::ffi::CString;
    use testing::spawn_kernel_with_selinux_hooks_test_policy_and_run;

    #[fuchsia::test]
    async fn task_create_allowed_for_allowed_type() {
        spawn_kernel_with_selinux_hooks_test_policy_and_run(|current_task, security_server| {
            mutate_attrs_for_test(&current_task, |attrs| {
                attrs.current_sid = security_server
                    .security_context_to_sid(b"u:object_r:fork_yes_t:s0".into())
                    .expect("invalid security context");
            });

            assert_eq!(
                check_task_create_access(
                    &build_permission_check(&current_task, &security_server),
                    &current_task
                ),
                Ok(())
            );
        })
        .await;
    }

    #[fuchsia::test]
    async fn task_create_denied_for_denied_type() {
        spawn_kernel_with_selinux_hooks_test_policy_and_run(|current_task, security_server| {
            mutate_attrs_for_test(&current_task, |attrs| {
                attrs.current_sid = security_server
                    .security_context_to_sid(b"u:object_r:fork_no_t:s0".into())
                    .expect("invalid security context");
            });

            assert_eq!(
                check_task_create_access(
                    &build_permission_check(&current_task, &security_server),
                    &current_task
                ),
                error!(EACCES)
            );
        })
        .await;
    }

    #[fuchsia::test]
    async fn exec_transition_allowed_for_allowed_transition_type() {
        spawn_kernel_with_selinux_hooks_test_policy_and_run(|current_task, security_server| {
            let current_sid = security_server
                .security_context_to_sid(b"u:object_r:exec_transition_source_t:s0".into())
                .expect("invalid security context");
            let exec_sid = security_server
                .security_context_to_sid(b"u:object_r:exec_transition_target_t:s0".into())
                .expect("invalid security context");

            let executable_security_context = b"u:object_r:executable_file_trans_t:s0";
            assert!(
                security_server.security_context_to_sid(executable_security_context.into()).is_ok()
            );
            let executable = create_test_executable(current_task, executable_security_context);

            mutate_attrs_for_test(&current_task, |attrs| {
                attrs.current_sid = current_sid;
                attrs.exec_sid = Some(exec_sid);
            });

            let file = open_test_file(current_task, &executable);
            let mut resolved_program = testing::make_resolved_program(current_task, file);
            assert_eq!(
                bprm_creds_for_exec(&security_server, &current_task, &mut resolved_program),
                Ok(())
            );
            assert_eq!(resolved_program.creds.security_state.current_sid, exec_sid);
            assert_eq!(resolved_program.secure_exec, true);
        })
        .await;
    }

    #[fuchsia::test]
    async fn exec_transition_noatsecure_allowed_for_allowed_transition_type() {
        spawn_kernel_with_selinux_hooks_test_policy_and_run(|current_task, security_server| {
            let current_sid = security_server
                .security_context_to_sid(
                    b"u:object_r:exec_transition_source_noatsecure_t:s0".into(),
                )
                .expect("invalid security context");
            let exec_sid = security_server
                .security_context_to_sid(b"u:object_r:exec_transition_target_t:s0".into())
                .expect("invalid security context");

            let executable_security_context = b"u:object_r:executable_file_trans_t:s0";
            assert!(
                security_server.security_context_to_sid(executable_security_context.into()).is_ok()
            );
            let executable = create_test_executable(current_task, executable_security_context);

            mutate_attrs_for_test(&current_task, |attrs| {
                attrs.current_sid = current_sid;
                attrs.exec_sid = Some(exec_sid);
            });

            let file = open_test_file(current_task, &executable);
            let mut resolved_program = testing::make_resolved_program(current_task, file);
            assert_eq!(
                bprm_creds_for_exec(&security_server, &current_task, &mut resolved_program),
                Ok(())
            );
            assert_eq!(resolved_program.creds.security_state.current_sid, exec_sid);
            assert_eq!(resolved_program.secure_exec, false);
        })
        .await;
    }

    #[fuchsia::test]
    async fn exec_transition_denied_for_transition_denied_type() {
        spawn_kernel_with_selinux_hooks_test_policy_and_run(|current_task, security_server| {
            let current_sid = security_server
                .security_context_to_sid(b"u:object_r:exec_transition_source_t:s0".into())
                .expect("invalid security context");
            let exec_sid = security_server
                .security_context_to_sid(b"u:object_r:exec_transition_denied_target_t:s0".into())
                .expect("invalid security context");

            let executable_security_context = b"u:object_r:executable_file_trans_t:s0";
            assert!(
                security_server.security_context_to_sid(executable_security_context.into()).is_ok()
            );
            let executable = create_test_executable(current_task, executable_security_context);

            mutate_attrs_for_test(&current_task, |attrs| {
                attrs.current_sid = current_sid;
                attrs.exec_sid = Some(exec_sid);
            });

            let file = open_test_file(current_task, &executable);
            let mut resolved_program = testing::make_resolved_program(current_task, file);
            assert_eq!(
                bprm_creds_for_exec(&security_server, &current_task, &mut resolved_program),
                error!(EACCES)
            );
        })
        .await;
    }

    // TODO(http://b/330904217): reenable test once filesystems are labeled and access is denied.
    #[ignore]
    #[fuchsia::test]
    async fn exec_transition_denied_for_executable_with_no_entrypoint_perm() {
        spawn_kernel_with_selinux_hooks_test_policy_and_run(|current_task, security_server| {
            let current_sid = security_server
                .security_context_to_sid(b"u:object_r:exec_transition_source_t:s0".into())
                .expect("invalid security context");
            let exec_sid = security_server
                .security_context_to_sid(b"u:object_r:exec_transition_target_t:s0".into())
                .expect("invalid security context");

            let executable_security_context =
                b"u:object_r:executable_file_trans_no_entrypoint_t:s0";
            assert!(
                security_server.security_context_to_sid(executable_security_context.into()).is_ok()
            );
            let executable = create_test_executable(current_task, executable_security_context);

            mutate_attrs_for_test(&current_task, |attrs| {
                attrs.current_sid = current_sid;
                attrs.exec_sid = Some(exec_sid);
            });

            let file = open_test_file(current_task, &executable);
            let mut resolved_program = testing::make_resolved_program(current_task, file);
            assert_eq!(
                bprm_creds_for_exec(&security_server, &current_task, &mut resolved_program),
                error!(EACCES)
            );
        })
        .await;
    }

    #[fuchsia::test]
    async fn exec_no_trans_allowed_for_executable() {
        spawn_kernel_with_selinux_hooks_test_policy_and_run(|current_task, security_server| {
            let current_sid = security_server
                .security_context_to_sid(b"u:object_r:exec_no_trans_source_t:s0".into())
                .expect("invalid security context");

            let executable_security_context = b"u:object_r:executable_file_no_trans_t:s0";
            assert!(
                security_server.security_context_to_sid(executable_security_context.into()).is_ok()
            );
            let executable = create_test_executable(current_task, executable_security_context);

            mutate_attrs_for_test(&current_task, |attrs| {
                attrs.current_sid = current_sid;
            });

            // Since the security domain is not changing, the `noatsecure` permission is not
            // checked and secure-mode exec is not required.
            let file = open_test_file(current_task, &executable);
            let mut resolved_program = testing::make_resolved_program(current_task, file);
            assert_eq!(
                bprm_creds_for_exec(&security_server, &current_task, &mut resolved_program),
                Ok(())
            );
            assert_eq!(resolved_program.creds.security_state.current_sid, current_sid);
            assert_eq!(resolved_program.secure_exec, false);
        })
        .await;
    }

    // TODO(http://b/330904217): reenable test once filesystems are labeled and access is denied.
    #[ignore]
    #[fuchsia::test]
    async fn exec_no_trans_denied_for_executable() {
        spawn_kernel_with_selinux_hooks_test_policy_and_run(|current_task, security_server| {
            let current_sid = security_server
                .security_context_to_sid(b"u:object_r:exec_transition_target_t:s0".into())
                .expect("invalid security context");

            let executable_security_context = b"u:object_r:executable_file_no_trans_t:s0";
            assert!(
                security_server.security_context_to_sid(executable_security_context.into()).is_ok()
            );
            let executable = create_test_executable(current_task, executable_security_context);

            mutate_attrs_for_test(&current_task, |attrs| {
                attrs.current_sid = current_sid;
            });

            // There is no `execute_no_trans` allow statement from `current_sid` to `executable_sid`,
            // expect access denied.
            let file = open_test_file(current_task, &executable);
            let mut resolved_program = testing::make_resolved_program(current_task, file);
            assert_eq!(
                bprm_creds_for_exec(&security_server, &current_task, &mut resolved_program),
                error!(EACCES)
            );
        })
        .await;
    }

    #[fuchsia::test]
    async fn security_state_is_updated_on_exec() {
        spawn_kernel_with_selinux_hooks_test_policy_and_run(|current_task, security_server| {
            let executable_security_context = b"u:object_r:executable_file_trans_t:s0";
            let executable =
                testing::create_test_executable(current_task, executable_security_context);

            let source_sid = security_server
                .security_context_to_sid(b"u:object_r:exec_transition_source_t:s0".into())
                .expect("invalid security context");
            let target_sid = security_server
                .security_context_to_sid(b"u:object_r:exec_transition_target_t:s0".into())
                .expect("invalid security context");

            let initial_state = {
                let mut attrs = current_task_state(current_task).clone();
                // Set previous SID to a different value from current, to allow verification
                // of the pre-exec "current" being moved into "previous".
                attrs.current_sid = source_sid;
                attrs.previous_sid = InitialSid::Unlabeled.into();

                // Set the other optional SIDs to a value, to verify that it is cleared on exec update.
                attrs.sockcreate_sid = Some(InitialSid::Unlabeled.into());
                attrs.fscreate_sid = Some(InitialSid::Unlabeled.into());
                attrs.keycreate_sid = Some(InitialSid::Unlabeled.into());

                // Set exec_sid to force a transition to target_sid.
                attrs.exec_sid = Some(target_sid);

                attrs
            };
            mutate_attrs_for_test(&current_task, |attrs| {
                *attrs = initial_state.clone();
            });

            let file = open_test_file(current_task, &executable);
            let mut resolved_program = testing::make_resolved_program(current_task, file);

            bprm_creds_for_exec(&security_server, &current_task, &mut resolved_program)
                .expect("bprm_creds_for_exec failed");

            assert_eq!(
                resolved_program.creds.security_state,
                TaskAttrs {
                    current_sid: target_sid,
                    exec_sid: None,
                    fscreate_sid: None,
                    keycreate_sid: None,
                    previous_sid: initial_state.current_sid,
                    sockcreate_sid: None,
                    internal_operation: false,
                }
            );
        })
        .await;
    }

    #[fuchsia::test]
    // The hooks_tests_policy denies the `rlimitinh` permission (implicitly, via `handle_unknown deny`)
    // for processes, so resource limits should be reset when the SID changes during exec.
    async fn handle_rlimitinh_on_exec() {
        spawn_kernel_with_selinux_hooks_test_policy_and_run(|current_task, security_server| {
            // In this testing context, `current_task` is the initial task.
            // Set its rlimits to some known values.
            assert_eq!(current_task.tid.id, 1);
            {
                let mut initial_limits = current_task.thread_group().limits.lock();
                (Resource::ALL).iter().for_each(|resource| {
                    initial_limits.set(*resource, rlimit { rlim_cur: 10, rlim_max: 20 });
                })
            }
            // Clone the initial task, then set the child task's rlimits to some new values.
            let child_task = current_task.clone_task_for_test(0, Some(SIGCHLD));
            {
                let mut child_limits = child_task.thread_group().limits.lock();
                (Resource::ALL).iter().for_each(|resource| {
                    child_limits.set(*resource, rlimit { rlim_cur: 30, rlim_max: 40 });
                })
            }

            // Clone the child task. Before exec, the grandchild task's rlimits should be equal
            // to its parent's.
            let grandchild_task = child_task.clone_task_for_test(0, Some(SIGCHLD));
            let parent_limits = { child_task.thread_group().limits.lock().clone() };
            let pre_exec_limits = { grandchild_task.thread_group().limits.lock().clone() };
            {
                (Resource::ALL).iter().for_each(|resource| {
                    let parent = parent_limits.get(*resource);
                    let pre_exec = pre_exec_limits.get(*resource);
                    assert_eq!(parent.rlim_cur, pre_exec.rlim_cur);
                    assert_eq!(parent.rlim_max, pre_exec.rlim_max);
                })
            }

            // Simulate exec of the grandchild task into a new domain.
            let previous_sid = { child_task.real_creds().security_state.current_sid };
            let new_sid = security_server
                .security_context_to_sid(b"u:object_r:test_valid_t:s0".into())
                .expect("invalid security context");

            assert_ne!(previous_sid, new_sid);

            let executable = testing::create_test_file(&grandchild_task);
            let file = open_test_file(&grandchild_task, &executable);
            let mut resolved_program = testing::make_resolved_program(&grandchild_task, file);
            resolved_program.creds.security_state = TaskAttrs::for_transition(
                new_sid,
                grandchild_task.real_creds().security_state.current_sid,
            );

            bprm_committing_creds(&security_server, &grandchild_task, &resolved_program);
            grandchild_task.set_creds(resolved_program.creds.clone());
            bprm_committed_creds(&security_server, &grandchild_task);

            let post_exec_limits = { grandchild_task.thread_group().limits.lock().clone() };
            {
                (Resource::ALL).iter().for_each(|resource| {
                    let pre_exec = pre_exec_limits.get(*resource);
                    let post_exec = post_exec_limits.get(*resource);
                    // Soft limits are reset to the minimum of the pre-exec hard limit and
                    // the initial task's soft limit.
                    assert_eq!(post_exec.rlim_cur, 10);
                    // Hard limits are unchanged.
                    assert_eq!(pre_exec.rlim_max, post_exec.rlim_max);
                })
            }

            // rlimits are not reset when the task SID does not change.
            let same_domain_task = child_task.clone_task_for_test(0, Some(SIGCHLD));

            let file = open_test_file(&same_domain_task, &executable);
            let mut resolved_program = testing::make_resolved_program(&same_domain_task, file);
            resolved_program.creds.security_state = TaskAttrs::for_transition(
                previous_sid,
                same_domain_task.real_creds().security_state.current_sid,
            );

            bprm_committing_creds(&security_server, &same_domain_task, &resolved_program);
            same_domain_task.set_creds(resolved_program.creds.clone());
            bprm_committed_creds(&security_server, &same_domain_task);

            let same_domain_limits = { same_domain_task.thread_group().limits.lock().clone() };
            {
                let parent_limits = { child_task.thread_group().limits.lock().clone() };
                (Resource::ALL).iter().for_each(|resource| {
                    let parent = parent_limits.get(*resource);
                    let same_domain = same_domain_limits.get(*resource);
                    assert_eq!(parent.rlim_cur, same_domain.rlim_cur);
                    assert_eq!(parent.rlim_max, same_domain.rlim_max);
                })
            }
        })
        .await;
    }

    #[fuchsia::test]
    // The hooks_tests_policy denies the `siginh` permission for domain transitions from the initial
    // context to contexts with type `test_siginh_no_t`, so itimers should be cleared.
    async fn clear_itimers_on_exec_if_siginh_denied() {
        spawn_kernel_with_selinux_hooks_test_policy_and_run(|current_task, security_server| {
            let child_task = current_task.clone_task_for_test(0, Some(SIGCHLD));

            // Set the child task's ITIMER_REAL.
            let child_itimer_val = itimerval {
                it_value: timeval { tv_sec: 100000000, tv_usec: 0 },
                it_interval: timeval { tv_sec: 10, tv_usec: 0 },
            };
            child_task
                .thread_group()
                .set_itimer(&child_task, ITIMER_REAL, child_itimer_val)
                .expect("set ITIMER_REAL for child");
            let pre_exec_itimer_val =
                { child_task.thread_group().get_itimer(ITIMER_REAL).unwrap() };
            assert_ne!(pre_exec_itimer_val.it_value.tv_sec, 0);

            // Simulate exec of the child task into a context with type `test_siginh_no_t`.
            let old_sid = { current_task.real_creds().security_state.current_sid };
            let new_sid = security_server
                .security_context_to_sid(b"u:object_r:test_siginh_no_t:s0".into())
                .expect("invalid security context");
            assert_ne!(old_sid, new_sid);
            let executable = testing::create_test_file(&child_task);
            let file = open_test_file(&child_task, &executable);
            let mut resolved_program = testing::make_resolved_program(&child_task, file);
            resolved_program.creds.security_state = TaskAttrs::for_transition(
                new_sid,
                child_task.real_creds().security_state.current_sid,
            );

            bprm_committing_creds(&security_server, &child_task, &resolved_program);
            child_task.set_creds(resolved_program.creds.clone());
            bprm_committed_creds(&security_server, &child_task);

            // Check that the child task's ITIMER_REAL is now unset.
            let post_exec_itimer_val =
                { child_task.thread_group().get_itimer(ITIMER_REAL).unwrap() };
            assert_eq!(post_exec_itimer_val.it_value.tv_sec, 0);
        })
        .await;
    }

    #[fuchsia::test]
    // The hooks_tests_policy denies the `siginh` permission for domain transitions from the initial
    // context to contexts with type `test_siginh_no_t`, so pending signals and signal masks should
    // be cleared.
    async fn clear_signal_state_on_exec_if_siginh_denied() {
        spawn_kernel_with_selinux_hooks_test_policy_and_run(|current_task, security_server| {
            let child_task = current_task.clone_task_for_test(0, Some(SIGCHLD));

            assert_eq!(child_task.read().signal_mask(), SigSet(0));
            child_task.write().set_signal_mask(SigSet(starnix_uapi::SIGTERM.into()));

            assert_eq!(child_task.read().pending_signal_count(), 0);
            child_task.thread_group().write().send_signal(SignalInfo::kernel(SIGTERM));
            assert_eq!(child_task.read().pending_signal_count(), 1);

            // Simulate exec of the child task into a context with type `test_siginh_no_t`.
            let old_sid = { current_task.real_creds().security_state.current_sid };
            let new_sid = security_server
                .security_context_to_sid(b"u:object_r:test_siginh_no_t:s0".into())
                .expect("invalid security context");
            assert_ne!(old_sid, new_sid);
            let executable = testing::create_test_file(&child_task);
            let file = open_test_file(&child_task, &executable);
            let mut resolved_program = testing::make_resolved_program(&child_task, file);
            resolved_program.creds.security_state = TaskAttrs::for_transition(
                new_sid,
                child_task.real_creds().security_state.current_sid,
            );

            bprm_committing_creds(&security_server, &child_task, &resolved_program);
            child_task.set_creds(resolved_program.creds.clone());
            bprm_committed_creds(&security_server, &child_task);

            // Check that the previously pending signal has been cleared.
            assert_eq!(child_task.read().pending_signal_count(), 0);
            // Check that the signal mask is now empty.
            assert_eq!(child_task.read().signal_mask(), SigSet(0));
        })
        .await;
    }

    #[fuchsia::test]
    async fn setsched_access_allowed_for_allowed_type() {
        spawn_kernel_with_selinux_hooks_test_policy_and_run(|current_task, security_server| {
            let target_task = create_task_with_security_context(
                &current_task.kernel(),
                "target_task",
                &CString::new("u:object_r:test_setsched_target_t:s0").unwrap(),
            );

            mutate_attrs_for_test(&current_task, |attrs| {
                attrs.current_sid = security_server
                    .security_context_to_sid(b"u:object_r:test_setsched_yes_t:s0".into())
                    .unwrap();
            });
            assert_eq!(
                check_setsched_access(
                    &build_permission_check(&current_task, &security_server),
                    &current_task,
                    &target_task
                ),
                Ok(())
            );
        })
        .await;
    }

    #[fuchsia::test]
    async fn setsched_access_denied_for_denied_type() {
        spawn_kernel_with_selinux_hooks_test_policy_and_run(|current_task, security_server| {
            let target_task = create_task_with_security_context(
                &current_task.kernel(),
                "target_task",
                &CString::new("u:object_r:test_setsched_target_t:s0").unwrap(),
            );

            mutate_attrs_for_test(&current_task, |attrs| {
                attrs.current_sid = security_server
                    .security_context_to_sid(b"u:object_r:test_setsched_no_t:s0".into())
                    .unwrap();
            });

            assert_eq!(
                check_setsched_access(
                    &build_permission_check(&current_task, &security_server),
                    &current_task,
                    &target_task
                ),
                error!(EACCES)
            );
        })
        .await;
    }

    #[fuchsia::test]
    async fn getsched_access_allowed_for_allowed_type() {
        spawn_kernel_with_selinux_hooks_test_policy_and_run(|current_task, security_server| {
            let target_task = create_task_with_security_context(
                &current_task.kernel(),
                "target_task",
                &CString::new(b"u:object_r:test_getsched_target_t:s0").unwrap(),
            );

            mutate_attrs_for_test(&current_task, |attrs| {
                attrs.current_sid = security_server
                    .security_context_to_sid(b"u:object_r:test_getsched_yes_t:s0".into())
                    .unwrap();
            });

            assert_eq!(
                check_getsched_access(
                    &build_permission_check(&current_task, &security_server),
                    &current_task,
                    &target_task
                ),
                Ok(())
            );
        })
        .await;
    }

    #[fuchsia::test]
    async fn getsched_access_denied_for_denied_type() {
        spawn_kernel_with_selinux_hooks_test_policy_and_run(|current_task, security_server| {
            let target_task = create_task_with_security_context(
                &current_task.kernel(),
                "target_task",
                &CString::new(b"u:object_r:test_getsched_target_t:s0").unwrap(),
            );

            mutate_attrs_for_test(&current_task, |attrs| {
                attrs.current_sid = security_server
                    .security_context_to_sid(b"u:object_r:test_getsched_no_t:s0".into())
                    .unwrap();
            });

            assert_eq!(
                check_getsched_access(
                    &build_permission_check(&current_task, &security_server),
                    &current_task,
                    &target_task
                ),
                error!(EACCES)
            );
        })
        .await;
    }

    #[fuchsia::test]
    async fn getpgid_access_allowed_for_allowed_type() {
        spawn_kernel_with_selinux_hooks_test_policy_and_run(|current_task, security_server| {
            let target_task = create_task_with_security_context(
                &current_task.kernel(),
                "target_task",
                &CString::new(b"u:object_r:test_getpgid_target_t:s0").unwrap(),
            );

            mutate_attrs_for_test(&current_task, |attrs| {
                attrs.current_sid = security_server
                    .security_context_to_sid(b"u:object_r:test_getpgid_yes_t:s0".into())
                    .unwrap();
            });

            assert_eq!(
                check_getpgid_access(
                    &build_permission_check(&current_task, &security_server),
                    &current_task,
                    &target_task
                ),
                Ok(())
            );
        })
        .await;
    }

    #[fuchsia::test]
    async fn getpgid_access_denied_for_denied_type() {
        spawn_kernel_with_selinux_hooks_test_policy_and_run(|current_task, security_server| {
            let target_task = create_task_with_security_context(
                &current_task.kernel(),
                "target_task",
                &CString::new(b"u:object_r:test_getpgid_target_t:s0").unwrap(),
            );

            mutate_attrs_for_test(&current_task, |attrs| {
                attrs.current_sid = security_server
                    .security_context_to_sid(b"u:object_r:test_getpgid_no_t:s0".into())
                    .unwrap();
            });

            assert_eq!(
                check_getpgid_access(
                    &build_permission_check(&current_task, &security_server),
                    &current_task,
                    &target_task
                ),
                error!(EACCES)
            );
        })
        .await;
    }

    #[fuchsia::test]
    async fn sigkill_access_allowed_for_allowed_type() {
        spawn_kernel_with_selinux_hooks_test_policy_and_run(|current_task, security_server| {
            let target_task = create_task_with_security_context(
                &current_task.kernel(),
                "target_task",
                &CString::new(b"u:object_r:test_kill_target_t:s0").unwrap(),
            );

            mutate_attrs_for_test(&current_task, |attrs| {
                attrs.current_sid = security_server
                    .security_context_to_sid(b"u:object_r:test_kill_sigkill_t:s0".into())
                    .unwrap();
            });

            assert_eq!(
                check_signal_access(
                    &build_permission_check(&current_task, &security_server),
                    &current_task,
                    &target_task,
                    SIGKILL,
                ),
                Ok(())
            );
        })
        .await;
    }

    #[fuchsia::test]
    async fn sigchld_access_allowed_for_allowed_type() {
        spawn_kernel_with_selinux_hooks_test_policy_and_run(|current_task, security_server| {
            let target_task = create_task_with_security_context(
                &current_task.kernel(),
                "target_task",
                &CString::new(b"u:object_r:test_kill_target_t:s0").unwrap(),
            );

            mutate_attrs_for_test(&current_task, |attrs| {
                attrs.current_sid = security_server
                    .security_context_to_sid(b"u:object_r:test_kill_sigchld_t:s0".into())
                    .unwrap();
            });

            assert_eq!(
                check_signal_access(
                    &build_permission_check(&current_task, &security_server),
                    &current_task,
                    &target_task,
                    SIGCHLD,
                ),
                Ok(())
            );
        })
        .await;
    }

    #[fuchsia::test]
    async fn sigstop_access_allowed_for_allowed_type() {
        spawn_kernel_with_selinux_hooks_test_policy_and_run(|current_task, security_server| {
            let target_task = create_task_with_security_context(
                &current_task.kernel(),
                "target_task",
                &CString::new(b"u:object_r:test_kill_target_t:s0").unwrap(),
            );

            mutate_attrs_for_test(&current_task, |attrs| {
                attrs.current_sid = security_server
                    .security_context_to_sid(b"u:object_r:test_kill_sigstop_t:s0".into())
                    .unwrap();
            });

            assert_eq!(
                check_signal_access(
                    &build_permission_check(&current_task, &security_server),
                    &current_task,
                    &target_task,
                    SIGSTOP,
                ),
                Ok(())
            );
        })
        .await;
    }

    #[fuchsia::test]
    async fn signal_access_allowed_for_allowed_type() {
        spawn_kernel_with_selinux_hooks_test_policy_and_run(|current_task, security_server| {
            let target_task = create_task_with_security_context(
                &current_task.kernel(),
                "target_task",
                &CString::new(b"u:object_r:test_kill_target_t:s0").unwrap(),
            );

            mutate_attrs_for_test(&current_task, |attrs| {
                attrs.current_sid = security_server
                    .security_context_to_sid(b"u:object_r:test_kill_signal_t:s0".into())
                    .unwrap();
            });

            // The `signal` permission allows signals other than SIGKILL, SIGCHLD, SIGSTOP.
            assert_eq!(
                check_signal_access(
                    &build_permission_check(&current_task, &security_server),
                    &current_task,
                    &target_task,
                    SIGTERM,
                ),
                Ok(())
            );
        })
        .await;
    }

    #[fuchsia::test]
    async fn signal_access_denied_for_denied_signals() {
        spawn_kernel_with_selinux_hooks_test_policy_and_run(|current_task, security_server| {
            let target_task = create_task_with_security_context(
                &current_task.kernel(),
                "target_task",
                &CString::new(b"u:object_r:test_kill_target_t:s0").unwrap(),
            );

            mutate_attrs_for_test(&current_task, |attrs| {
                attrs.current_sid = security_server
                    .security_context_to_sid(b"u:object_r:test_kill_signal_t:s0".into())
                    .unwrap();
            });

            // The `signal` permission does not allow SIGKILL, SIGCHLD or SIGSTOP.
            for signal in [SIGCHLD, SIGKILL, SIGSTOP] {
                assert_eq!(
                    check_signal_access(
                        &build_permission_check(&current_task, &security_server),
                        &current_task,
                        &target_task,
                        signal,
                    ),
                    error!(EACCES)
                );
            }
        })
        .await;
    }

    #[fuchsia::test]
    async fn ptrace_access_allowed_for_allowed_type_and_state_is_updated() {
        spawn_kernel_with_selinux_hooks_test_policy_and_run(|current_task, security_server| {
            let tracee_task = create_task_with_security_context(
                &current_task.kernel(),
                "target_task",
                &CString::new(b"u:object_r:test_ptrace_traced_t:s0").unwrap(),
            );

            mutate_attrs_for_test(&current_task, |attrs| {
                attrs.current_sid = security_server
                    .security_context_to_sid(b"u:object_r:test_ptrace_tracer_yes_t:s0".into())
                    .unwrap();
            });

            assert_eq!(
                ptrace_access_check(
                    &build_permission_check(&current_task, &security_server),
                    &current_task,
                    &tracee_task,
                    PTRACE_MODE_ATTACH
                ),
                Ok(())
            );
        })
        .await;
    }

    #[fuchsia::test]
    async fn ptrace_access_denied_for_denied_type_and_state_is_not_updated() {
        spawn_kernel_with_selinux_hooks_test_policy_and_run(|current_task, security_server| {
            let tracee_task = create_task_with_security_context(
                &current_task.kernel(),
                "target_task",
                &CString::new(b"u:object_r:test_ptrace_traced_t:s0").unwrap(),
            );

            mutate_attrs_for_test(&current_task, |attrs| {
                attrs.current_sid = security_server
                    .security_context_to_sid(b"u:object_r:test_ptrace_tracer_no_t:s0".into())
                    .unwrap();
            });

            assert_eq!(
                ptrace_access_check(
                    &build_permission_check(&current_task, &security_server),
                    &current_task,
                    &tracee_task,
                    PTRACE_MODE_ATTACH
                ),
                error!(EACCES)
            );
            // TODO: Verify that the tracer has not been set on `tracee_task`.
        })
        .await;
    }

    #[fuchsia::test]
    async fn setcurrent_bounds() {
        const BINARY_POLICY: &[u8] = include_bytes!(
            "../../../../lib/selinux/testdata/composite_policies/compiled/bounded_transition_policy"
        );
        const BOUNDED_CONTEXT: &[u8] = b"test_u:test_r:bounded_t:s0";
        const UNBOUNDED_CONTEXT: &[u8] = b"test_u:test_r:unbounded_t:s0";

        spawn_kernel_with_selinux_hooks_test_policy_and_run(|current_task, security_server| {
            security_server.load_policy(BINARY_POLICY.to_vec()).expect("policy load failed");

            mutate_attrs_for_test(&current_task, |attrs| {
                attrs.current_sid =
                    security_server.security_context_to_sid(UNBOUNDED_CONTEXT.into()).unwrap();
            });

            // Thread-group has a single task, so dynamic transitions are permitted, with "setcurrent"
            // and "dyntransition".
            assert_eq!(
                set_procattr(&security_server, &current_task, ProcAttr::Current, BOUNDED_CONTEXT),
                Ok(()),
                "Unbounded_t->bounded_t single-threaded"
            );

            // Note that this case requires that the both the bounded and bounding contexts have
            // the "setcurrent" permission, to pass the dynamic bounds check.
            assert_eq!(
                set_procattr(&security_server, &current_task, ProcAttr::Current, UNBOUNDED_CONTEXT),
                Ok(()),
                "Bounded_t->unbounded_t single-threaded"
            );

            // Create a second task in the same thread group.
            let _child_task = current_task
                .clone_task_for_test((CLONE_THREAD | CLONE_VM | CLONE_SIGHAND) as u64, None);

            // Thread-group has a multiple tasks, so dynamic transitions to are only allowed to bounded
            // domains.
            assert_eq!(
                set_procattr(&security_server, &current_task, ProcAttr::Current, BOUNDED_CONTEXT),
                Ok(()),
                "Unbounded_t->bounded_t multi-threaded"
            );
            assert_eq!(
                set_procattr(&security_server, &current_task, ProcAttr::Current, UNBOUNDED_CONTEXT),
                error!(EACCES),
                "Bounded_t->unbounded_t multi-threaded"
            );
        })
        .await;
    }
}
