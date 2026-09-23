// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::sync::atomic::{AtomicU8, Ordering};

pub struct AtomicStopState {
    inner: AtomicU8,
}

impl AtomicStopState {
    pub fn new(state: StopState) -> Self {
        Self { inner: AtomicU8::new(state as u8) }
    }

    pub fn load(&self, ordering: Ordering) -> StopState {
        let v = self.inner.load(ordering);
        // SAFETY: we only ever store to the atomic a value originating
        // from a valid `StopState`.
        unsafe { std::mem::transmute(v) }
    }

    pub fn store(&self, state: StopState, ordering: Ordering) {
        self.inner.store(state as u8, ordering)
    }
}

/// This enum describes the state that a task or thread group can be in when being stopped.
/// The names are taken from ptrace(2).
#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(u8)]
pub enum StopState {
    /// In this state, the process has been told to wake up, but has not yet been woken.
    /// Individual threads may still be stopped.
    Waking,
    /// In this state, at least one thread is awake.
    Awake,
    /// Same as the above, but you are not allowed to make further transitions.  Used
    /// to kill the task / group.  These names are not in ptrace(2).
    ForceWaking,
    ForceAwake,

    /// In this state, the process has been told to stop via a signal, but has not yet stopped.
    GroupStopping,
    /// In this state, at least one thread of the process has stopped
    GroupStopped,
    /// In this state, the task has received a signal, and it is being traced, so it will
    /// stop at the next opportunity.
    SignalDeliveryStopping,
    /// Same as the last one, but has stopped.
    SignalDeliveryStopped,
    /// Stop for a ptrace event: a variety of events defined by ptrace and
    /// enabled with the use of various ptrace features, such as the
    /// PTRACE_O_TRACE_* options.  The parameter indicates the type of
    /// event. Examples include PTRACE_EVENT_FORK (the event is a fork),
    /// PTRACE_EVENT_EXEC (the event is exec), and other similar events.
    PtraceEventStopping,
    /// Same as the last one, but has stopped
    PtraceEventStopped,
    /// In this state, we have stopped before executing a syscall
    SyscallEnterStopping,
    SyscallEnterStopped,
    /// In this state, we have stopped after executing a syscall
    SyscallExitStopping,
    SyscallExitStopped,
}

impl StopState {
    /// This means a stop is either in progress or we've stopped.
    pub fn is_stopping_or_stopped(&self) -> bool {
        self.is_stopped() || self.is_stopping()
    }

    /// This means a stop is in progress.  Refers to any stop state ending in "ing".
    pub fn is_stopping(&self) -> bool {
        match *self {
            StopState::GroupStopping
            | StopState::SignalDeliveryStopping
            | StopState::PtraceEventStopping
            | StopState::SyscallEnterStopping
            | StopState::SyscallExitStopping => true,
            _ => false,
        }
    }

    /// This means task is stopped.
    pub fn is_stopped(&self) -> bool {
        match *self {
            StopState::GroupStopped
            | StopState::SignalDeliveryStopped
            | StopState::PtraceEventStopped
            | StopState::SyscallEnterStopped
            | StopState::SyscallExitStopped => true,
            _ => false,
        }
    }

    /// Returns the "ed" version of this StopState, if it is "ing".
    pub fn finalize(&self) -> Result<StopState, ()> {
        match *self {
            StopState::GroupStopping => Ok(StopState::GroupStopped),
            StopState::SignalDeliveryStopping => Ok(StopState::SignalDeliveryStopped),
            StopState::PtraceEventStopping => Ok(StopState::PtraceEventStopped),
            StopState::Waking => Ok(StopState::Awake),
            StopState::ForceWaking => Ok(StopState::ForceAwake),
            StopState::SyscallEnterStopping => Ok(StopState::SyscallEnterStopped),
            StopState::SyscallExitStopping => Ok(StopState::SyscallExitStopped),
            _ => Err(()),
        }
    }

    pub fn is_downgrade(&self, new_state: &StopState) -> bool {
        match *self {
            StopState::GroupStopped => *new_state == StopState::GroupStopping,
            StopState::SignalDeliveryStopped => *new_state == StopState::SignalDeliveryStopping,
            StopState::PtraceEventStopped => *new_state == StopState::PtraceEventStopping,
            StopState::SyscallEnterStopped => *new_state == StopState::SyscallEnterStopping,
            StopState::SyscallExitStopped => *new_state == StopState::SyscallExitStopping,
            StopState::Awake => *new_state == StopState::Waking,
            _ => false,
        }
    }

    pub fn is_waking_or_awake(&self) -> bool {
        *self == StopState::Waking
            || *self == StopState::Awake
            || *self == StopState::ForceWaking
            || *self == StopState::ForceAwake
    }

    /// Indicate if the transition to the stopped / awake state is not finished.  This
    /// function is typically used to determine when it is time to notify waiters.
    pub fn is_in_progress(&self) -> bool {
        *self == StopState::Waking
            || *self == StopState::ForceWaking
            || *self == StopState::GroupStopping
            || *self == StopState::SignalDeliveryStopping
            || *self == StopState::PtraceEventStopping
            || *self == StopState::SyscallEnterStopping
            || *self == StopState::SyscallExitStopping
    }

    pub fn ptrace_only(&self) -> bool {
        !self.is_waking_or_awake()
            && *self != StopState::GroupStopped
            && *self != StopState::GroupStopping
    }

    pub fn is_illegal_transition(&self, new_state: StopState) -> bool {
        *self == StopState::ForceAwake
            || (*self == StopState::ForceWaking && new_state != StopState::ForceAwake)
            || new_state == *self
            // Downgrades are generally a sign that something is screwed up, but
            // a SIGCONT can result in a downgrade from Awake to Waking, so we
            // allowlist it.
            || (self.is_downgrade(&new_state) && *self != StopState::Awake)
    }

    pub fn is_force(&self) -> bool {
        *self == StopState::ForceAwake || *self == StopState::ForceWaking
    }

    pub fn as_in_progress(&self) -> Result<StopState, ()> {
        match *self {
            StopState::GroupStopped => Ok(StopState::GroupStopping),
            StopState::SignalDeliveryStopped => Ok(StopState::SignalDeliveryStopping),
            StopState::PtraceEventStopped => Ok(StopState::PtraceEventStopping),
            StopState::Awake => Ok(StopState::Waking),
            StopState::ForceAwake => Ok(StopState::ForceWaking),
            StopState::SyscallEnterStopped => Ok(StopState::SyscallEnterStopping),
            StopState::SyscallExitStopped => Ok(StopState::SyscallExitStopping),
            _ => Ok(*self),
        }
    }
}
