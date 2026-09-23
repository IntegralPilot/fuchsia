// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! This module implements the YAMA LSM functionality, used to lock down ptrace access.

use std::borrow::Cow;
use std::sync::atomic::Ordering;

use crate::ptrace::PtraceAllowedPtracers;
use crate::security;
use crate::task::{CurrentTask, Task};
use crate::vfs::FsNodeOps;
use crate::vfs::pseudo::simple_file::{BytesFile, BytesFileOps, parse_unsigned_file};
use starnix_uapi::auth::{CAP_SYS_ADMIN, CAP_SYS_PTRACE, PtraceAccessMode};
use starnix_uapi::error;
use starnix_uapi::errors::Errno;

/// Scope definitions for Yama.  For full details, see ptrace(2).
/// 0 means classic ptrace checks, without additional restrictions.
/// This is the Starnix default (i.e. YAMA is not active).
pub const SCOPE_CLASSIC: u8 = 0;
/// 1 means tracer needs to have CAP_SYS_PTRACE or be a parent / child
/// process. This is the default with YAMA active.
pub const SCOPE_RESTRICTED: u8 = 1;
/// 2 means tracer needs to have CAP_SYS_PTRACE
pub const SCOPE_ADMIN_ONLY: u8 = 2;
/// 3 means no process can attach.
pub const SCOPE_NO_ATTACH: u8 = 3;

/// Corresponds to the `ptrace_access_check()` LSM hook.
pub(super) fn ptrace_access_check(
    current_task: &CurrentTask,
    tracee: &Task,
    mode: PtraceAccessMode,
) -> Result<(), Errno> {
    if !mode.contains(PtraceAccessMode::ATTACH) {
        // YAMA controls read access checks but not attach.
        return Ok(());
    }

    let ptrace_scope = current_task.kernel().ptrace_scope.load(Ordering::Relaxed);

    // From the `ptrace.2` man page description of YAMA's `ptrace_scope`:
    match ptrace_scope {
        // classic ptrace permissions:
        //
        // No additional restrictions on operations that perform
        // PTRACE_MODE_ATTACH checks (beyond those imposed by the
        // commoncap and other LSMs).
        //
        //
        // The use of PTRACE_TRACEME is unchanged.
        SCOPE_CLASSIC => Ok(()),

        // restricted ptrace: (the YAMA default)
        //
        // When performing an operation that requires a
        // PTRACE_MODE_ATTACH check, the calling process must either
        // have the CAP_SYS_PTRACE capability in the user namespace of
        // the target process or it must have a predefined
        // relationship with the target process.  By default, the
        // predefined relationship is that the target process must be
        // a descendant of the caller.
        //
        // A target process can employ the prctl(2) PR_SET_PTRACER
        // operation to declare an additional PID that is allowed to
        // perform PTRACE_MODE_ATTACH operations on the target.  See
        // the kernel source file
        // Documentation/admin-guide/LSM/Yama.rst (or
        // Documentation/security/Yama.txt before Linux 4.13) for
        // further details.
        //
        // The use of PTRACE_TRACEME is unchanged.
        SCOPE_RESTRICTED => {
            // This only allows us to attach to descendants and tasks that have
            // explicitly allowlisted us with PR_SET_PTRACER.
            let mut ttg = tracee.thread_group().read().parent.clone();
            let my_pid = &current_task.pid;
            while let Some(target) = ttg {
                let target = target.upgrade();
                if target.leader == *my_pid {
                    return Ok(());
                }
                ttg = target.read().parent.clone();
            }

            match &tracee.thread_group().read().allowed_ptracers {
                PtraceAllowedPtracers::None => (),
                PtraceAllowedPtracers::Some(pid) => {
                    if my_pid == pid {
                        return Ok(());
                    }
                }
                PtraceAllowedPtracers::Any => return Ok(()),
            }

            security::check_task_capable(current_task, CAP_SYS_PTRACE)
        }

        // admin-only attach:
        //
        // Only processes with the CAP_SYS_PTRACE capability in the
        // user namespace of the target process may perform
        // PTRACE_MODE_ATTACH operations or trace children that employ
        // PTRACE_TRACEME.
        SCOPE_ADMIN_ONLY => security::check_task_capable(current_task, CAP_SYS_PTRACE),

        // no attach:
        //
        // No process may perform PTRACE_MODE_ATTACH operations or
        // trace children that employ PTRACE_TRACEME.
        //
        // Once this value has been written to the file, it cannot be
        // changed.
        _ => error!(EPERM),
    }
}

/// Corresponds to the `ptrace_traceme()` LSM hook.
pub(super) fn ptrace_traceme(
    current_task: &CurrentTask,
    parent_tracer_task: &Task,
) -> Result<(), Errno> {
    let ptrace_scope = current_task.kernel().ptrace_scope.load(Ordering::Relaxed);

    match ptrace_scope {
        SCOPE_CLASSIC | SCOPE_RESTRICTED => Ok(()),
        SCOPE_ADMIN_ONLY => security::check_creds_capable(
            current_task,
            &parent_tracer_task.real_creds(),
            CAP_SYS_PTRACE,
        ),
        _ => error!(EPERM),
    }
}
pub struct PtraceScopeFile {}

impl PtraceScopeFile {
    pub fn new_node() -> impl FsNodeOps {
        BytesFile::new_node(Self {})
    }
}

impl BytesFileOps for PtraceScopeFile {
    fn write(&self, current_task: &CurrentTask, data: Vec<u8>) -> Result<(), Errno> {
        security::check_task_capable(current_task, CAP_SYS_ADMIN)?;

        let new_scope = parse_unsigned_file::<u8>(&data)?;
        if new_scope > SCOPE_NO_ATTACH {
            return error!(EINVAL);
        }

        let kernel = current_task.kernel();
        loop {
            let old_scope = kernel.ptrace_scope.load(Ordering::Relaxed);

            if old_scope == SCOPE_NO_ATTACH && new_scope != SCOPE_NO_ATTACH {
                return error!(EINVAL);
            }

            if kernel
                .ptrace_scope
                .compare_exchange(old_scope, new_scope, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                return Ok(());
            }
        }
    }

    fn read(&self, current_task: &CurrentTask) -> Result<Cow<'_, [u8]>, Errno> {
        let mut scope = current_task.kernel().ptrace_scope.load(Ordering::Relaxed).to_string();
        scope.push('\n');
        Ok(scope.into_bytes().into())
    }
}
