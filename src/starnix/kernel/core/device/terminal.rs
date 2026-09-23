// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::fs::devpts::{DEVPTS_COUNT, get_device_type_for_pts};
use crate::mutable_state::{state_accessor, state_implementation};
use crate::task::{EventHandler, ProcessGroup, Session, WaitCanceler, WaitQueue, Waiter};
use crate::vfs::buffers::{InputBuffer, InputBufferExt as _, OutputBuffer};
use crate::vfs::{DirEntryHandle, FsString, Mounts};
use derivative::Derivative;
use line_discipline::{LineDiscipline, PendingSignals};
use macro_rules_attribute::apply;
use starnix_sync::{DeviceTerminalsLock, LockDepMutex, LockDepRwLock, PtsIdsSetLock};
use starnix_uapi::auth::FsCred;
use starnix_uapi::device_id::DeviceId;
use starnix_uapi::errors::Errno;
use starnix_uapi::vfs::FdEvents;
use starnix_uapi::{errno, error, uapi};
use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Weak};

/// Global state of the devpts filesystem.
pub struct TtyState {
    /// The terminal objects indexed by their identifier.
    pub terminals: LockDepRwLock<HashMap<u32, Weak<Terminal>>, DeviceTerminalsLock>,

    /// The set of allocated terminal identifiers.
    pts_ids_set: LockDepMutex<PtsIdsSet, PtsIdsSetLock>,
}

impl TtyState {
    /// Allocates a new terminal and returns it.
    pub fn get_next_terminal(
        self: &Arc<Self>,
        dev_pts_root: DirEntryHandle,
        creds: FsCred,
    ) -> Result<Arc<Terminal>, Errno> {
        let id = self.pts_ids_set.lock().acquire()?;
        let terminal = Terminal::new(self.clone(), dev_pts_root, creds, id);
        assert!(self.terminals.write().insert(id, Arc::downgrade(&terminal)).is_none());
        Ok(terminal)
    }

    /// Release the terminal identifier into the set of available identifier.
    pub fn release_terminal(&self, id: u32) -> Result<(), Errno> {
        // We need to remove this terminal id from the set of terminals before we release the
        // identifier. Otherwise, the id might be reused for a new terminal and we'll remove
        // the *new* terminal with that identifier instead of the old one.
        assert!(self.terminals.write().remove(&id).is_some());
        self.pts_ids_set.lock().release(id);
        Ok(())
    }
}

impl Default for TtyState {
    fn default() -> Self {
        Self { terminals: Default::default(), pts_ids_set: PtsIdsSet::new(DEVPTS_COUNT).into() }
    }
}

#[derive(Derivative)]
#[derivative(Default)]
#[derivative(Debug)]
pub struct TerminalMutableState {
    pub line_discipline: LineDiscipline,

    /// Wait queue for the main side of the terminal.
    main_wait_queue: WaitQueue,

    /// Wait queue for the replica side of the terminal.
    replica_wait_queue: WaitQueue,

    /// The controller for the terminal.
    pub controller: Option<TerminalController>,
}

/// State of a given terminal. This object handles both the main and the replica terminal.
#[derive(Derivative)]
#[derivative(Debug)]
pub struct Terminal {
    /// Weak self to allow cloning.
    weak_self: Weak<Self>,

    /// The global devpts state.
    #[derivative(Debug = "ignore")]
    state: Arc<TtyState>,

    /// The root of the devpts fs responsible for this terminal.
    pub dev_pts_root: DirEntryHandle,

    /// The owner of the terminal.
    pub fscred: FsCred,

    /// The identifier of the terminal.
    pub id: u32,

    /// The mutable state of the Terminal.
    mutable_state:
        starnix_sync::LockDepRwLock<TerminalMutableState, starnix_sync::TerminalMutableStateLock>,
}

impl Terminal {
    pub fn new(
        state: Arc<TtyState>,
        dev_pts_root: DirEntryHandle,
        fscred: FsCred,
        id: u32,
    ) -> Arc<Self> {
        Arc::new_cyclic(|weak_self| Self {
            weak_self: weak_self.clone(),
            state,
            dev_pts_root,
            fscred,
            id,
            mutable_state: Default::default(),
        })
    }

    pub fn to_owned(&self) -> Arc<Terminal> {
        self.weak_self.upgrade().expect("This should never be called while releasing the terminal")
    }

    /// Sets the terminal configuration.
    pub fn set_termios(&self, termios: uapi::termios2) {
        let signals = self.write().set_termios(termios);
        self.send_signals(signals);
    }

    pub fn flush(&self, is_main: bool, arg: u32) -> Result<(), Errno> {
        self.write().flush(is_main, arg)
    }

    /// `close` implementation of the main side of the terminal.
    pub fn main_close(&self) {
        // Remove the entry in the file system.
        let id = FsString::from(self.id.to_string());
        // The child is not a directory, the mount doesn't matter.
        self.dev_pts_root.remove_child(id.as_ref(), &Mounts::new());
        self.write().main_close();
    }

    /// Called when a new reference to the main side of this terminal is made.
    pub fn main_open(&self) {
        self.write().main_open();
    }

    /// `wait_async` implementation of the main side of the terminal.
    pub fn main_wait_async(
        &self,
        waiter: &Waiter,
        events: FdEvents,
        handler: EventHandler,
    ) -> WaitCanceler {
        self.read().main_wait_async(waiter, events, handler)
    }

    /// `query_events` implementation of the main side of the terminal.
    pub fn main_query_events(&self) -> FdEvents {
        self.read().main_query_events()
    }

    /// `read` implementation of the main side of the terminal.
    pub fn main_read(&self, data: &mut dyn OutputBuffer) -> Result<usize, Errno> {
        self.write().main_read(data)
    }

    /// `write` implementation of the main side of the terminal.
    pub fn main_write(&self, data: &mut dyn InputBuffer) -> Result<usize, Errno> {
        let (bytes, signals) = self.write().main_write(data)?;
        self.send_signals(signals);
        Ok(bytes)
    }

    /// `close` implementation of the replica side of the terminal.
    pub fn replica_close(&self) {
        self.write().replica_close();
    }

    /// Called when a new reference to the replica side of this terminal is made.
    pub fn replica_open(&self) {
        self.write().replica_open();
    }

    /// `wait_async` implementation of the replica side of the terminal.
    pub fn replica_wait_async(
        &self,
        waiter: &Waiter,
        events: FdEvents,
        handler: EventHandler,
    ) -> WaitCanceler {
        self.read().replica_wait_async(waiter, events, handler)
    }

    /// `query_events` implementation of the replica side of the terminal.
    pub fn replica_query_events(&self) -> FdEvents {
        self.read().replica_query_events()
    }

    /// `read` implementation of the replica side of the terminal.
    pub fn replica_read(&self, data: &mut dyn OutputBuffer) -> Result<usize, Errno> {
        self.write().replica_read(data)
    }

    /// `write` implementation of the replica side of the terminal.
    pub fn replica_write(&self, data: &mut dyn InputBuffer) -> Result<usize, Errno> {
        self.write().replica_write(data)
    }

    /// Send the pending signals to the associated foreground process groups if they exist.
    fn send_signals(&self, signals: PendingSignals) {
        let signals = signals.signals();
        if !signals.is_empty() {
            let process_group = {
                let terminal_state = self.read();
                let Some(controller) = terminal_state.controller.as_ref() else {
                    return;
                };
                let Some(session) = controller.session.upgrade() else {
                    return;
                };
                let Ok(process_group) = session.read().get_foreground_process_group() else {
                    return;
                };
                process_group
            };
            process_group.send_signals(signals);
        }
    }

    pub fn device(&self) -> DeviceId {
        get_device_type_for_pts(self.id)
    }

    state_accessor!(Terminal, mutable_state);
}

struct InputBufferWrapper<'a>(&'a mut dyn crate::vfs::buffers::InputBuffer);

impl<'a> line_discipline::InputBuffer for InputBufferWrapper<'a> {
    fn available(&self) -> usize {
        self.0.available()
    }
    fn read_to_vec_exact(&mut self, size: usize) -> Result<Vec<u8>, Errno> {
        self.0.read_to_vec_exact(size)
    }
}

struct OutputBufferWrapper<'a>(&'a mut dyn crate::vfs::buffers::OutputBuffer);

impl<'a> line_discipline::OutputBuffer for OutputBufferWrapper<'a> {
    fn write(&mut self, data: &[u8]) -> Result<usize, Errno> {
        self.0.write(data)
    }
}

#[apply(state_implementation!)]
impl TerminalMutableState<Base = Terminal> {
    /// Returns the terminal configuration.
    pub fn termios(&self) -> &uapi::termios2 {
        self.line_discipline.termios()
    }

    pub fn set_packet_mode(&mut self, enabled: bool) {
        if enabled != self.line_discipline.is_packet_mode_enabled() {
            self.line_discipline.set_packet_mode(enabled);
            self.notify_waiters();
        }
    }

    /// Returns the number of available bytes to read from the side of the terminal described by
    /// `is_main`.
    pub fn get_available_read_size(&self, is_main: bool) -> usize {
        self.line_discipline.get_available_read_size(is_main)
    }

    /// Sets the terminal configuration.
    fn set_termios(&mut self, termios: uapi::termios2) -> PendingSignals {
        let old_canon_enabled = self.line_discipline.is_canon_enabled();
        let signals = self.line_discipline.set_termios(termios);
        let canon_disabled = old_canon_enabled && !self.line_discipline.is_canon_enabled();
        if canon_disabled || self.line_discipline.has_packet_mode_pending_events() {
            self.notify_waiters();
        }
        signals
    }

    pub fn flush(&mut self, is_main: bool, arg: u32) -> Result<(), Errno> {
        self.line_discipline.flush(is_main, arg)?;
        self.notify_waiters();
        Ok(())
    }

    /// `close` implementation of the main side of the terminal.
    pub fn main_close(&mut self) {
        self.line_discipline.main_close();
        self.notify_waiters();
    }

    /// Called when a new reference to the main side of this terminal is made.
    pub fn main_open(&mut self) {
        self.line_discipline.main_open();
    }

    pub fn is_main_closed(&self) -> bool {
        self.line_discipline.is_main_closed()
    }

    /// `wait_async` implementation of the main side of the terminal.
    fn main_wait_async(
        &self,
        waiter: &Waiter,
        events: FdEvents,
        handler: EventHandler,
    ) -> WaitCanceler {
        self.main_wait_queue.wait_async_fd_events(waiter, events, handler)
    }

    /// `query_events` implementation of the main side of the terminal.
    fn main_query_events(&self) -> FdEvents {
        self.line_discipline.main_query_events()
    }

    /// `read` implementation of the main side of the terminal.
    fn main_read(&mut self, data: &mut dyn OutputBuffer) -> Result<usize, Errno> {
        let mut wrapper = OutputBufferWrapper(data);
        let result = self.line_discipline.main_read(&mut wrapper)?;
        self.notify_waiters();
        Ok(result)
    }

    /// `write` implementation of the main side of the terminal.
    fn main_write(&mut self, data: &mut dyn InputBuffer) -> Result<(usize, PendingSignals), Errno> {
        let mut wrapper = InputBufferWrapper(data);
        let (result, signals) = self.line_discipline.main_write(&mut wrapper)?;
        self.notify_waiters();
        Ok((result, signals))
    }

    /// `close` implementation of the replica side of the terminal.
    pub fn replica_close(&mut self) {
        self.line_discipline.replica_close();
        self.notify_waiters();
    }

    /// Called when a new reference to the replica side of this terminal is made.
    pub fn replica_open(&mut self) {
        self.line_discipline.replica_open();
    }

    /// `wait_async` implementation of the replica side of the terminal.
    fn replica_wait_async(
        &self,
        waiter: &Waiter,
        events: FdEvents,
        handler: EventHandler,
    ) -> WaitCanceler {
        self.replica_wait_queue.wait_async_fd_events(waiter, events, handler)
    }

    /// `query_events` implementation of the replica side of the terminal.
    fn replica_query_events(&self) -> FdEvents {
        self.line_discipline.replica_query_events()
    }

    /// `read` implementation of the replica side of the terminal.
    fn replica_read(&mut self, data: &mut dyn OutputBuffer) -> Result<usize, Errno> {
        let mut wrapper = OutputBufferWrapper(data);
        let result = self.line_discipline.replica_read(&mut wrapper)?;
        self.notify_waiters();
        Ok(result)
    }

    /// `write` implementation of the replica side of the terminal.
    fn replica_write(&mut self, data: &mut dyn InputBuffer) -> Result<usize, Errno> {
        let mut wrapper = InputBufferWrapper(data);
        let result = self.line_discipline.replica_write(&mut wrapper)?;
        self.notify_waiters();
        Ok(result)
    }

    /// Notify any waiters if the state of the terminal changes.
    fn notify_waiters(&mut self) {
        let main_events = self.line_discipline.main_query_events();
        if main_events.bits() != 0 {
            self.main_wait_queue.notify_fd_events(main_events);
        }
        let replica_events = self.line_discipline.replica_query_events();
        if replica_events.bits() != 0 {
            self.replica_wait_queue.notify_fd_events(replica_events);
        }
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        self.state.release_terminal(self.id).unwrap()
    }
}

/// The controlling session of a terminal. Is is associated to a single side of the terminal,
/// either main or replica.
#[derive(Debug)]
pub struct TerminalController {
    pub session: Weak<Session>,
}

impl TerminalController {
    pub fn new(session: &Arc<Session>) -> Option<Self> {
        Some(Self { session: Arc::downgrade(&session) })
    }

    pub fn get_foreground_process_group(&self) -> Result<Arc<ProcessGroup>, Errno> {
        let session = self.session.upgrade().ok_or_else(|| errno!(ESRCH))?;
        session.read().get_foreground_process_group()
    }
}

#[derive(Debug)]
struct PtsIdsSet {
    pts_count: u32,
    next_id: u32,
    reclaimed_ids: BTreeSet<u32>,
}

impl PtsIdsSet {
    fn new(pts_count: u32) -> Self {
        Self { pts_count, next_id: 0, reclaimed_ids: BTreeSet::new() }
    }

    fn release(&mut self, id: u32) {
        assert!(self.reclaimed_ids.insert(id))
    }

    fn acquire(&mut self) -> Result<u32, Errno> {
        match self.reclaimed_ids.iter().next() {
            Some(e) => {
                let value = *e;
                self.reclaimed_ids.remove(&value);
                Ok(value)
            }
            None => {
                if self.next_id < self.pts_count {
                    let id = self.next_id;
                    self.next_id += 1;
                    Ok(id)
                } else {
                    error!(ENOSPC)
                }
            }
        }
    }
}
