// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use starnix_core::security;
use starnix_core::task::CurrentTask;
use starnix_core::vfs::{FdFlags, FdNumber};
use starnix_logging::track_stub;

use starnix_uapi::auth::CAP_SYS_PTRACE;
use starnix_uapi::errors::Errno;
use starnix_uapi::open_flags::OpenFlags;
use starnix_uapi::{O_CLOEXEC, O_NONBLOCK, UFFD_USER_MODE_ONLY, error};

use crate::userfault_file::UserFaultFile;

pub fn sys_userfaultfd(current_task: &CurrentTask, raw_flags: u32) -> Result<FdNumber, Errno> {
    let unknown_flags = raw_flags & !(O_CLOEXEC | O_NONBLOCK | UFFD_USER_MODE_ONLY);
    if unknown_flags != 0 {
        return error!(EINVAL, format!("unknown flags provided: {unknown_flags:x?}"));
    }
    let mut open_flags = OpenFlags::empty();
    if raw_flags & O_NONBLOCK != 0 {
        open_flags |= OpenFlags::NONBLOCK;
    }
    if raw_flags & O_CLOEXEC != 0 {
        open_flags |= OpenFlags::CLOEXEC;
    }

    let fd_flags = if raw_flags & O_CLOEXEC != 0 {
        FdFlags::CLOEXEC
    } else {
        track_stub!(TODO("https://fxbug.dev/297375964"), "userfaultfds that survive exec()");
        return error!(ENOSYS);
    };

    let user_mode_only = raw_flags & UFFD_USER_MODE_ONLY != 0;
    if !user_mode_only {
        security::check_task_capable(current_task, CAP_SYS_PTRACE)?;
    }
    let uff_handle = UserFaultFile::new(current_task, open_flags, user_mode_only)?;
    current_task.add_file(uff_handle, fd_flags)
}

pub use sys_userfaultfd as sys_arch32_userfaultfd;
