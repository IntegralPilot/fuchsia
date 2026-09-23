// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::mm::MemoryManager;
use crate::task::{AbstractUnixSocketNamespace, AbstractVsockSocketNamespace, CurrentTask};
use crate::vfs::{FdTable, FsContext, FsNodeHandle, SharedFdTable};
use fuchsia_rcu::RcuArc;
use starnix_sync::{LockDepMutex, TaskFilesLock};
use starnix_uapi::errno;
use starnix_uapi::errors::Errno;
use std::ops::Deref;
use std::sync::{Arc, OnceLock};

/// The running state of a task.
///
/// This structure contains the state of a task that is only relevant while the task is running. It
/// is dropped when the task enters an exited state.
pub struct TaskRunningState {
    /// A handle to the underlying Zircon thread object.
    ///
    /// Some tasks lack an underlying Zircon thread. These tasks are used internally by the
    /// Starnix kernel to track background work, typically on a `kthread`.
    pub thread: OnceLock<ZirconThread>,

    /// The file descriptor table for this task.
    ///
    /// This table can be shared by many tasks.
    ///
    /// This is always `Some` while the task is running. It becomes `None` upon exit.
    pub files: LockDepMutex<Option<SharedFdTable>, TaskFilesLock>,

    /// The memory manager for this task.  This is `None` only for system tasks.
    pub mm: RcuArc<MemoryManager>,

    /// The file system for this task.
    pub fs: RcuArc<FsContext>,

    /// The namespace for abstract AF_UNIX sockets for this task.
    pub abstract_socket_namespace: Arc<AbstractUnixSocketNamespace>,

    /// The namespace for AF_VSOCK for this task.
    pub abstract_vsock_namespace: Arc<AbstractVsockSocketNamespace>,

    /// The pid directory, so it doesn't have to be generated and thrown away on every access.
    /// See https://fxbug.dev/291962828 for details.
    pub proc_pid_directory_cache: OnceLock<FsNodeHandle>,
}

impl TaskRunningState {
    #[track_caller]
    pub fn files(&self) -> Result<Arc<FdTable>, Errno> {
        self.files.lock().as_ref().map(|files| files.table.clone()).ok_or_else(|| errno!(ESRCH))
    }

    pub fn fork_files(&self) -> Option<SharedFdTable> {
        self.files.lock().as_ref().map(|files| SharedFdTable::new(files.fork()))
    }

    pub fn share_files(&self) -> Option<SharedFdTable> {
        self.files.lock().as_ref().map(|files| files.clone())
    }

    /// Unshares the file descriptor table for this task, if shared.
    ///
    /// Updates the [`Arc<FdTable>`] reference in both [`SharedFdTable`] and [`CurrentTask`].
    pub fn unshare_files(&self, current_task: &CurrentTask) {
        if let Some(ref mut files) = *self.files.lock() {
            files.unshare();
            *current_task.files.borrow_mut() = Some(files.table.clone());
        }
    }

    pub fn mm(&self) -> Result<Arc<MemoryManager>, Errno> {
        self.mm.upgrade().ok_or_else(|| errno!(EINVAL))
    }

    pub fn fs(&self) -> Arc<FsContext> {
        self.fs.upgrade().expect("TaskRunningState fs should never be None")
    }
}

/// A synchronized container for a Zircon thread and its cached KOID.
#[derive(Debug, Clone)]
pub struct ZirconThread {
    /// The underlying Zircon thread.
    ///
    /// # Thread Safety
    ///
    /// Blocking operations are unsafe while holding RCU read locks. However, references to this
    /// thread must be held across blocking operations (e.g., futex waits). The [`ZirconThread`]
    /// container as a whole is guarded by RCU because it is a member of the RCU-guarded
    /// [`TaskRunningState`]. This field is reference counted so it can be accessed outside of RCU
    /// locks through a strong reference.
    ///
    /// Holding a reference to the thread does not guarantee that the task to which it belongs will
    /// continue running. The task may exit at any time. The thread will continue to exist in memory
    /// until all references are dropped. When the task exits and execution stops, reference holders
    /// will observe the thread transition to [`zx::ThreadState::Dead`] normally.
    pub thread: Arc<zx::Thread>,
    pub koid: zx::Koid,
}

impl ZirconThread {
    pub fn new(thread: Arc<zx::Thread>) -> Self {
        let koid = thread.koid().expect("Failed to get thread koid");
        Self { thread, koid }
    }
}

impl Deref for ZirconThread {
    type Target = Arc<zx::Thread>;
    fn deref(&self) -> &Self::Target {
        &self.thread
    }
}
