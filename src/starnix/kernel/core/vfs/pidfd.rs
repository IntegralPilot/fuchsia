// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::task::{
    CurrentTask, EventHandler, Pid, ProcessEntryRef, SignalHandler, SignalHandlerInner,
    WaitCanceler, Waiter,
};
use crate::vfs::{
    Anon, FileHandle, FileObject, FileOps, fileops_impl_dataless, fileops_impl_nonseekable,
    fileops_impl_noop_sync,
};
use fuchsia_async as fasync;
use starnix_uapi::errors::Errno;
use starnix_uapi::open_flags::OpenFlags;
use starnix_uapi::vfs::FdEvents;
use starnix_uapi::{error, from_status_like_fdio};

pub struct PidFdFileObject {
    /// The process represented by this file.
    pid: Pid,

    /// Receives a notification when the tracked process terminates.
    ///
    /// The peer is held by the task monitoring the process, which drops it once the process has
    /// been fully released. Dropping this endpoint in turn tells that task to stop monitoring.
    ///
    /// `None` if the process was already terminated when the pidfd was created.
    terminated_event: Option<zx::EventPair>,
}

impl PidFdFileObject {
    fn get_signals_from_events(events: FdEvents) -> zx::Signals {
        if events.contains(FdEvents::POLLIN) {
            zx::Signals::EVENTPAIR_PEER_CLOSED
        } else {
            zx::Signals::NONE
        }
    }

    fn get_events_from_signals(signals: zx::Signals) -> FdEvents {
        let mut events = FdEvents::empty();

        if signals.contains(zx::Signals::EVENTPAIR_PEER_CLOSED) {
            events |= FdEvents::POLLIN;
        }

        events
    }
}

/// Returns an event that is signalled with `EVENTPAIR_PEER_CLOSED` when the current memory manager
/// of the process identified by `pid` is dropped.
///
/// Returns `None` if the process no longer has a reachable memory manager.
fn get_memory_manager_drop_event(pid: &Pid) -> Option<zx::EventPair> {
    let Some(ProcessEntryRef::Process(proc)) = pid.get_process() else {
        return None;
    };
    let task = pid.get_task().or_else(|_| proc.read().get_running_task());
    task.ok().and_then(|task| task.mm().ok()).map(|mm| mm.drop_notifier.event())
}

/// Waits until the process identified by `pid` has terminated and all of its resources have been
/// released.
///
/// The memory manager is monitored first.  Once no memory manager remains, the Zircon process is
/// monitored for termination.
async fn wait_for_process_release(
    pid: Pid,
    initial_mm_event: Option<zx::EventPair>,
    zx_process: zx::Process,
) {
    let mut mm_event = initial_mm_event;
    while let Some(event) = mm_event {
        if fasync::OnSignals::new(&event, zx::Signals::EVENTPAIR_PEER_CLOSED).await.is_err() {
            break;
        }

        // The memory manager has been dropped. If the process has a new one, it was replaced by
        // an `execve` and monitoring must continue with the new memory manager.
        mm_event = get_memory_manager_drop_event(&pid);
    }

    let _ = fasync::OnSignals::new(&zx_process, zx::Signals::PROCESS_TERMINATED).await;
}

/// Signals the pidfd holding the peer of `local_event` once the process identified by `pid` has
/// been fully released.
///
/// Stops early if the pidfd is closed first, which drops the peer and asserts
/// `EVENTPAIR_PEER_CLOSED` on `local_event`.
async fn monitor_pidfd(
    pid: Pid,
    initial_mm_event: Option<zx::EventPair>,
    zx_process: zx::Process,
    local_event: zx::EventPair,
) {
    let pidfd_closed =
        std::pin::pin!(fasync::OnSignals::new(&local_event, zx::Signals::EVENTPAIR_PEER_CLOSED));
    let released = std::pin::pin!(wait_for_process_release(pid, initial_mm_event, zx_process));
    let _ = futures::future::select(pidfd_closed, released).await;

    // Returning drops `local_event`, which signals `EVENTPAIR_PEER_CLOSED` on the peer, waking any
    // poller still waiting on the pidfd.
}

pub fn new_pidfd(
    current_task: &CurrentTask,
    pid: Pid,
    flags: OpenFlags,
) -> Result<FileHandle, Errno> {
    let terminated_event = match pid.get_process() {
        Some(ProcessEntryRef::Process(proc)) => {
            let zx_process = proc
                .process
                .duplicate_handle(zx::Rights::SAME_RIGHTS)
                .map_err(|status| from_status_like_fdio!(status))?;
            // Look up the memory manager here rather than in the monitoring task: the process is
            // known to be alive at this point, whereas it may already have been zombified, and
            // hence have an unreachable memory manager, by the time the task first runs.
            let initial_mm_event = get_memory_manager_drop_event(&pid);
            let (local_event, terminated_event) = zx::EventPair::create();
            let monitored_pid = pid.clone();

            current_task.kernel().kthreads.spawn_future(
                move || monitor_pidfd(monitored_pid, initial_mm_event, zx_process, local_event),
                "pidfd-monitor",
            );

            Some(terminated_event)
        }
        Some(ProcessEntryRef::Zombie) => None,
        None => {
            if pid.get_task().is_ok() {
                return error!(EINVAL);
            }
            return error!(ESRCH);
        }
    };

    Ok(Anon::new_private_file(
        current_task,
        Box::new(PidFdFileObject { pid, terminated_event }),
        flags,
        "[pidfd]",
    ))
}

impl FileOps for PidFdFileObject {
    fileops_impl_nonseekable!();
    fileops_impl_dataless!();
    fileops_impl_noop_sync!();

    fn as_pid(&self, _file: &FileObject) -> Result<Pid, Errno> {
        Ok(self.pid.clone())
    }

    fn wait_async(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        waiter: &Waiter,
        events: FdEvents,
        handler: EventHandler,
    ) -> Option<WaitCanceler> {
        let terminated_event = self.terminated_event.as_ref()?;
        let signal_handler = SignalHandler {
            inner: SignalHandlerInner::ZxHandle(PidFdFileObject::get_events_from_signals),
            event_handler: handler,
            err_code: None,
        };
        let canceler = waiter
            .wake_on_zircon_signals(
                terminated_event,
                PidFdFileObject::get_signals_from_events(events),
                signal_handler,
            )
            .unwrap(); // errors cannot happen unless the kernel is out of memory
        Some(WaitCanceler::new_port(canceler))
    }

    fn query_events(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
    ) -> Result<FdEvents, Errno> {
        let Some(terminated_event) = &self.terminated_event else {
            return Ok(FdEvents::POLLIN);
        };
        match terminated_event
            .wait_one(zx::Signals::EVENTPAIR_PEER_CLOSED, zx::MonotonicInstant::ZERO)
            .to_result()
        {
            Err(zx::Status::TIMED_OUT) => Ok(FdEvents::empty()),
            Ok(zx::Signals::EVENTPAIR_PEER_CLOSED) => Ok(FdEvents::POLLIN),
            result => unreachable!("unexpected result: {result:?}"),
        }
    }
}
