// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::inotify::InotifyFileObject;
use starnix_core::task::CurrentTask;
use starnix_core::vfs::syscalls::{LookupFlags, lookup_at};
use starnix_core::vfs::{FdFlags, FdNumber, WdNumber};

use starnix_uapi::errors::Errno;
use starnix_uapi::inotify_mask::InotifyMask;
use starnix_uapi::user_address::UserCString;
use starnix_uapi::{IN_CLOEXEC, IN_NONBLOCK, errno, error};

pub fn sys_inotify_init1(current_task: &CurrentTask, flags: u32) -> Result<FdNumber, Errno> {
    if flags & !(IN_NONBLOCK | IN_CLOEXEC) != 0 {
        return error!(EINVAL);
    }
    let non_blocking = flags & IN_NONBLOCK != 0;
    let close_on_exec = flags & IN_CLOEXEC != 0;
    let inotify_file = InotifyFileObject::new_file(current_task, non_blocking);
    let fd_flags = if close_on_exec { FdFlags::CLOEXEC } else { FdFlags::empty() };
    current_task.add_file(inotify_file, fd_flags)
}

pub fn sys_inotify_init(current_task: &CurrentTask) -> Result<FdNumber, Errno> {
    sys_inotify_init1(current_task, 0)
}

pub fn sys_inotify_add_watch(
    current_task: &CurrentTask,
    fd: FdNumber,
    user_path: UserCString,
    mask: u32,
) -> Result<WdNumber, Errno> {
    let mask = InotifyMask::from_bits(mask).ok_or_else(|| errno!(EINVAL))?;
    if !mask.intersects(InotifyMask::ALL_EVENTS) {
        // Mask must include at least 1 event.
        return error!(EINVAL);
    }
    let file = current_task.files().get(fd)?;
    let inotify_file = file.downcast_file::<InotifyFileObject>().ok_or_else(|| errno!(EINVAL))?;
    let options = if mask.contains(InotifyMask::DONT_FOLLOW) {
        LookupFlags::no_follow()
    } else {
        LookupFlags::default()
    };
    let watched_node = lookup_at(current_task, FdNumber::AT_FDCWD, user_path, options)?;
    if mask.contains(InotifyMask::ONLYDIR) && !watched_node.entry.node.is_dir() {
        return error!(ENOTDIR);
    }
    inotify_file.add_watch(watched_node.entry, mask, &file)
}

pub fn sys_inotify_rm_watch(
    current_task: &CurrentTask,
    fd: FdNumber,
    watch_id: WdNumber,
) -> Result<(), Errno> {
    let file = current_task.files().get(fd)?;
    let inotify_file = file.downcast_file::<InotifyFileObject>().ok_or_else(|| errno!(EINVAL))?;
    inotify_file.remove_watch(watch_id, &file)
}

pub fn sys_arch32_inotify_init1(current_task: &CurrentTask, flags: u32) -> Result<FdNumber, Errno> {
    sys_inotify_init1(current_task, flags)
}

pub fn sys_arch32_inotify_init(current_task: &CurrentTask) -> Result<FdNumber, Errno> {
    sys_inotify_init1(current_task, 0)
}

pub fn sys_arch32_inotify_add_watch(
    current_task: &CurrentTask,
    fd: FdNumber,
    user_path: UserCString,
    mask: u32,
) -> Result<WdNumber, Errno> {
    sys_inotify_add_watch(current_task, fd, user_path, mask)
}

pub fn sys_arch32_inotify_rm_watch(
    current_task: &CurrentTask,
    fd: FdNumber,
    watch_id: WdNumber,
) -> Result<(), Errno> {
    sys_inotify_rm_watch(current_task, fd, watch_id)
}
