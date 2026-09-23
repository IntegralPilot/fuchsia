// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::power::SuspendResumeManager;
use crate::task::{
    CurrentTask, EventHandler, SignalHandler, SignalHandlerInner, WaitCanceler, Waiter,
};
use crate::vfs::buffers::{InputBuffer, OutputBuffer};
use crate::vfs::pseudo::simple_file::{SimpleFileNode, parse_unsigned_file, serialize_for_file};
use crate::vfs::{FileObject, FileOps, FsNodeOps, fileops_impl_noop_sync, fileops_impl_seekless};
use starnix_uapi::error;
use starnix_uapi::errors::Errno;
use starnix_uapi::vfs::FdEvents;
use std::sync::Arc;

/// This file allows user space to put the system into a sleep state while taking into account the
/// concurrent arrival of wakeup events.
/// * Reading from it returns the current number of registered wakeup events and it blocks if some
/// wakeup events are being processed when the file is read from.
/// * Writing to it will only succeed if the current number of wakeup events is equal to the written
/// value and, if successful, will make the kernel abort a subsequent transition to a sleep state
// if any wakeup events are reported after the write has returned.
pub struct PowerWakeupCountFile {
    suspend_resume_manager: Arc<SuspendResumeManager>,
    blocking_event: zx::EventPair,
}

impl PowerWakeupCountFile {
    pub fn new_node(suspend_resume_manager: Arc<SuspendResumeManager>) -> impl FsNodeOps {
        SimpleFileNode::new(move |_| {
            Ok(Self {
                suspend_resume_manager: suspend_resume_manager.clone(),
                blocking_event: suspend_resume_manager.duplicate_lock_event(),
            })
        })
    }
}

impl FileOps for PowerWakeupCountFile {
    fileops_impl_seekless!();
    fileops_impl_noop_sync!();

    fn write(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        _offset: usize,
        data: &mut dyn InputBuffer,
    ) -> Result<usize, Errno> {
        let data = data.read_all()?;
        let expected_count: u64 = parse_unsigned_file(&data)?;
        let real_count = self.suspend_resume_manager.total_wakeup_events();
        if expected_count != real_count {
            return error!(EINVAL);
        }
        Ok(data.len())
    }

    fn read(
        &self,
        file: &FileObject,
        current_task: &CurrentTask,
        offset: usize,
        data: &mut dyn OutputBuffer,
    ) -> Result<usize, Errno> {
        file.blocking_op(current_task, FdEvents::POLLIN | FdEvents::POLLHUP, None, || {
            if !self.suspend_resume_manager.lock().can_suspend() {
                return error!(EAGAIN);
            }
            let wakeup_count = self.suspend_resume_manager.total_wakeup_events();
            let content = serialize_for_file(wakeup_count);
            if offset >= content.len() {
                return Ok(0);
            }
            data.write(&content[offset..])
        })
    }

    fn wait_async(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        waiter: &Waiter,
        events: FdEvents,
        handler: EventHandler,
    ) -> Option<WaitCanceler> {
        if events.contains(FdEvents::POLLIN) {
            let signal_handler = SignalHandler {
                inner: SignalHandlerInner::ZxHandle(|_signals| FdEvents::POLLIN),
                event_handler: handler,
                err_code: None,
            };
            return Some(WaitCanceler::new_port(
                waiter
                    .wake_on_zircon_signals(
                        &self.blocking_event,
                        zx::Signals::USER_0,
                        signal_handler,
                    )
                    .expect("Failed to wait on zircon signals"),
            ));
        }
        None
    }

    fn query_events(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
    ) -> Result<FdEvents, Errno> {
        let mut events = FdEvents::POLLOUT;
        if self.suspend_resume_manager.lock().can_suspend() {
            events |= FdEvents::POLLIN;
        }
        Ok(events)
    }
}
