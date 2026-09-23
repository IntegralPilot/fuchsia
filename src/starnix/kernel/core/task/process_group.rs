// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::mutable_state::{state_accessor, state_implementation};
use crate::signals::SignalInfo;
use crate::task::{Pid, PidTable, Session, SessionDisassociation, ThreadGroup};
use macro_rules_attribute::apply;
use starnix_sync::{LockDepRwLock, ProcessGroupState};
use starnix_uapi::pid_t;
use starnix_uapi::signals::{SIGCONT, SIGHUP, Signal};
use std::collections::BTreeMap;
use std::sync::{Arc, Weak};

#[derive(Debug)]
pub struct ProcessGroupMutableState {
    /// The thread_groups in the process group.
    ///
    /// The references to ThreadGroup is weak to prevent cycles as ThreadGroup have a Arc reference to their process group.
    /// It is still expected that these weak references are always valid, as thread groups must unregister
    /// themselves before they are deleted.
    thread_groups: BTreeMap<pid_t, Weak<ThreadGroup>>,

    /// Whether this process group is orphaned and already notified its members.
    orphaned: bool,
}

#[derive(Debug)]
pub struct ProcessGroup {
    /// The session of the process group.
    pub session: Arc<Session>,

    /// The leader of the process group.
    pub leader: Pid,

    /// The mutable state of the ProcessGroup.
    mutable_state: LockDepRwLock<ProcessGroupMutableState, ProcessGroupState>,
}

impl PartialEq for ProcessGroup {
    fn eq(&self, other: &Self) -> bool {
        self.leader == other.leader
    }
}

impl Eq for ProcessGroup {}

impl std::hash::Hash for ProcessGroup {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.leader.hash(state);
    }
}

/// A process group is a set of processes that are considered to be a unit for the purposes of job
/// control and signal delivery. Each process in a process group has the same process group
/// ID (PGID). The process with the same PID as the PGID is called the process group leader.
///
/// When a signal is sent to a process group, it is delivered to all processes in the group,
/// including the process group leader. This allows a single signal to be used to control all
/// processes in a group, such as stopping or resuming them all.
///
/// Process groups are also used for job control. The foreground and background process groups of a
/// terminal are used to determine which processes can read from and write to the terminal. The
/// foreground process group is the only process group that can read from and write to the terminal
/// at any given time.
///
/// When a process forks from its parent, the child process inherits the parent's PGID. A process
/// can also explicitly change its own PGID using the setpgid() system call.
///
/// Process groups are destroyed when the last process in the group exits.
impl ProcessGroup {
    pub fn new(leader: Pid, session: Option<Arc<Session>>) -> Arc<ProcessGroup> {
        let session = session.unwrap_or_else(|| Session::new(leader.clone()));
        let process_group = Arc::new(ProcessGroup {
            session: session.clone(),
            leader,
            mutable_state: LockDepRwLock::new(ProcessGroupMutableState {
                thread_groups: BTreeMap::new(),
                orphaned: false,
            }),
        });
        session.write().insert(&process_group);
        process_group
    }

    state_accessor!(ProcessGroup, mutable_state);

    pub fn insert(&self, thread_group: &ThreadGroup) {
        self.write().thread_groups.insert(thread_group.leader.id, thread_group.weak_self.clone());
    }

    /// Removes the thread group from the process group.
    /// Returns whether the process group is empty and a `SessionDisassociation`, which
    /// the caller must use to explicitly disassociate the controlling terminal if the
    /// exiting thread group was the session leader.
    pub fn remove(&self, thread_group: &ThreadGroup) -> (bool, SessionDisassociation) {
        let is_session_leader = self.session.leader == thread_group.leader;
        let is_empty = self.write().remove(thread_group);
        let disassociation = if is_session_leader {
            SessionDisassociation::new(Some(self.session.clone()))
        } else {
            SessionDisassociation::new(None)
        };
        (is_empty, disassociation)
    }

    pub fn send_signals(&self, signals: &[Signal]) {
        let thread_groups = self.read().thread_groups().collect::<Vec<_>>();
        Self::send_signals_to_thread_groups(signals, thread_groups);
    }

    /// Check whether the process group became orphaned. If this is the case, send signals to its
    /// members if at least one is stopped.
    ///
    /// Takes a read lock on the PidTable to ensure the object cannot be removed while this method
    /// is running.
    pub fn check_orphaned(&self, _pids: &PidTable) {
        let thread_groups = {
            let state = self.read();
            if state.orphaned {
                return;
            }
            state.thread_groups().collect::<Vec<_>>()
        };
        for tg in thread_groups {
            let Some(parent) = tg.read().parent.clone() else {
                return;
            };
            let parent = parent.upgrade();
            let parent_state = parent.read();
            if parent_state.process_group.as_ref() != self
                && parent_state.process_group.session == self.session
            {
                return;
            }
        }
        let thread_groups = {
            let mut state = self.write();
            if state.orphaned {
                return;
            }
            state.orphaned = true;
            state.thread_groups().collect::<Vec<_>>()
        };
        if thread_groups.iter().any(|tg| tg.load_stopped().is_stopping_or_stopped()) {
            Self::send_signals_to_thread_groups(&[SIGHUP, SIGCONT], thread_groups);
        }
    }

    fn send_signals_to_thread_groups(
        signals: &[Signal],
        thread_groups: impl IntoIterator<Item = impl AsRef<ThreadGroup>>,
    ) {
        for thread_group in thread_groups.into_iter() {
            for &signal in signals {
                thread_group.as_ref().write().send_signal(SignalInfo::kernel(signal));
            }
        }
    }
}

#[apply(state_implementation!)]
impl ProcessGroupMutableState<Base = ProcessGroup> {
    pub fn thread_groups(&self) -> Box<dyn Iterator<Item = Arc<ThreadGroup>> + '_> {
        Box::new(self.thread_groups.values().map(|t| {
            t.upgrade()
                .expect("Weak references to thread_groups in ProcessGroup must always be valid")
        }))
    }

    /// Removes the thread group from the process group. Returns whether the process group is empty.
    fn remove(&mut self, thread_group: &ThreadGroup) -> bool {
        self.thread_groups.remove(&thread_group.leader.id);

        self.thread_groups.is_empty()
    }
}
