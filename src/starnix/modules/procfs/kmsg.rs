// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use starnix_core::security;
use starnix_core::task::{CurrentTask, EventHandler, Syslog, SyslogAccess, WaitCanceler, Waiter};
use starnix_core::vfs::{
    AppendLockWriteGuard, CheckAccessReason, FileObject, FileOps, FileSystemHandle, FsNode,
    FsNodeHandle, FsNodeInfo, FsNodeOps, InputBuffer, OutputBuffer, fileops_impl_noop_sync,
    fileops_impl_seekless, fs_node_impl_not_dir,
};
use starnix_sync::DynamicLockDepRwLock;
use starnix_uapi::auth::FsCred;
use starnix_uapi::errors::Errno;
use starnix_uapi::open_flags::OpenFlags;
use starnix_uapi::syslog::SyslogAction;
use starnix_uapi::vfs::FdEvents;
use starnix_uapi::{error, mode};

pub fn kmsg_file(fs: &FileSystemHandle) -> FsNodeHandle {
    fs.create_node_and_allocate_node_id(
        KmsgNode,
        FsNodeInfo::new(mode!(IFREG, 0o400), FsCred::root()),
    )
}

struct KmsgNode;

impl FsNodeOps for KmsgNode {
    fs_node_impl_not_dir!();

    fn check_access(
        &self,
        node: &FsNode,
        current_task: &CurrentTask,
        permission_flags: security::PermissionFlags,
        info: &DynamicLockDepRwLock<FsNodeInfo>,
        reason: CheckAccessReason,
        audit_context: security::Auditable<'_>,
    ) -> Result<(), Errno> {
        node.default_check_access_impl(
            current_task,
            permission_flags,
            reason,
            info.read(),
            audit_context,
        )?;
        Syslog::validate_access(current_task, SyslogAccess::ProcKmsg(SyslogAction::Open))
    }

    fn create_file_ops(
        &self,
        _node: &FsNode,
        _current_task: &CurrentTask,
        _flags: OpenFlags,
    ) -> Result<Box<dyn FileOps>, Errno> {
        Ok(Box::new(KmsgFile))
    }

    fn truncate(
        &self,
        _guard: &AppendLockWriteGuard<'_>,
        _node: &FsNode,
        _current_task: &CurrentTask,
        _length: u64,
    ) -> Result<(), Errno> {
        // O_TRUNC succeeds on /proc/kmsg but does nothing.
        Ok(())
    }
}

struct KmsgFile;

impl FileOps for KmsgFile {
    fileops_impl_seekless!();
    fileops_impl_noop_sync!();

    fn wait_async(
        &self,
        _file: &FileObject,
        current_task: &CurrentTask,
        waiter: &Waiter,
        events: FdEvents,
        handler: EventHandler,
    ) -> Option<WaitCanceler> {
        let syslog = current_task
            .kernel()
            .syslog
            .access(current_task, SyslogAccess::ProcKmsg(SyslogAction::SizeUnread))
            .ok()?;
        Some(syslog.wait(waiter, events, handler))
    }

    fn query_events(
        &self,
        _file: &FileObject,
        current_task: &CurrentTask,
    ) -> Result<FdEvents, Errno> {
        let syslog = current_task
            .kernel()
            .syslog
            .access(current_task, SyslogAccess::ProcKmsg(SyslogAction::SizeUnread))?;
        let mut events = FdEvents::empty();
        if syslog.size_unread()? > 0 {
            events |= FdEvents::POLLIN;
        }
        Ok(events)
    }

    fn read(
        &self,
        file: &FileObject,
        current_task: &CurrentTask,
        _offset: usize,
        data: &mut dyn OutputBuffer,
    ) -> Result<usize, Errno> {
        let syslog = current_task
            .kernel()
            .syslog
            .access(current_task, SyslogAccess::ProcKmsg(SyslogAction::Read))?;
        file.blocking_op(current_task, FdEvents::POLLIN | FdEvents::POLLHUP, None, || {
            let bytes_written = syslog.read(data)?;
            Ok(bytes_written as usize)
        })
    }

    fn write(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        _offset: usize,
        _data: &mut dyn InputBuffer,
    ) -> Result<usize, Errno> {
        error!(EIO)
    }
}
