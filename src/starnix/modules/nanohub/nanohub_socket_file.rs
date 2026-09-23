// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use starnix_core::vfs::FileObjectState;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use starnix_core::task::{CurrentTask, EventHandler, WaitCanceler, Waiter};
use starnix_core::vfs::buffers::{InputBuffer, OutputBuffer};
use starnix_core::vfs::{FileObject, FileOps, fileops_impl_nonseekable, fileops_impl_noop_sync};

use starnix_syscalls::{SyscallArg, SyscallResult};

use starnix_uapi::errors::Errno;
use starnix_uapi::vfs::FdEvents;

pub struct NanohubSocketFile {
    socket_file: Box<dyn FileOps>,
    read_complete: Arc<AtomicBool>,
}

impl FileOps for NanohubSocketFile {
    fileops_impl_nonseekable!();
    fileops_impl_noop_sync!();

    fn read(
        &self,
        file: &FileObject,
        current_task: &CurrentTask,
        offset: usize,
        data: &mut dyn OutputBuffer,
    ) -> Result<usize, Errno> {
        debug_assert!(offset == 0);
        // Use the internal read implementation for the first call to read,
        // but return EOF for subsequent reads
        if self.read_complete.load(Ordering::Relaxed) {
            Ok(0)
        } else {
            self.read_complete.store(true, Ordering::Relaxed);
            self.socket_file.read(file, current_task, offset, data)
        }
    }

    fn write(
        &self,
        file: &FileObject,
        current_task: &CurrentTask,
        offset: usize,
        data: &mut dyn InputBuffer,
    ) -> Result<usize, Errno> {
        debug_assert!(offset == 0);

        // Sysfs routes expect to re-arm for reading after a write operation.
        self.read_complete.store(false, Ordering::Relaxed);

        self.socket_file.write(file, current_task, offset, data)
    }

    fn wait_async(
        &self,
        file: &FileObject,
        current_task: &CurrentTask,
        waiter: &Waiter,
        events: FdEvents,
        handler: EventHandler,
    ) -> Option<WaitCanceler> {
        self.socket_file.wait_async(file, current_task, waiter, events, handler)
    }

    fn query_events(
        &self,
        file: &FileObject,
        current_task: &CurrentTask,
    ) -> Result<FdEvents, Errno> {
        self.socket_file.query_events(file, current_task)
    }

    fn ioctl(
        &self,
        file: &FileObject,
        current_task: &CurrentTask,
        request: u32,
        arg: SyscallArg,
    ) -> Result<SyscallResult, Errno> {
        self.socket_file.ioctl(file, current_task, request, arg)
    }

    fn close(self: Box<Self>, file: &FileObjectState, current_task: &CurrentTask) {
        self.socket_file.close(file, current_task);
    }
}

impl NanohubSocketFile {
    pub fn new(socket_file: Box<dyn FileOps>) -> Box<Self> {
        Box::new(NanohubSocketFile { socket_file, read_complete: Arc::new(AtomicBool::new(false)) })
    }
}
