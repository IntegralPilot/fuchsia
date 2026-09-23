// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::task::idr::{Idr, IdrGuard};
use crate::task::memory_attribution::MemoryAttributionLifecycleEvent;
use crate::task::{ProcessGroup, Task, ThreadGroup};
use fuchsia_rcu::{RcuDroppable, RcuOptionBox, RcuReadScope, RcuWeak};
use starnix_uapi::errors::Errno;
use starnix_uapi::{errno, error, pid_t};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Weak};

#[derive(Debug, RcuDroppable)]
enum ProcessEntry {
    ThreadGroup(Weak<ThreadGroup>),
    Zombie,
}

impl ProcessEntry {
    fn thread_group(&self) -> Option<&Weak<ThreadGroup>> {
        match self {
            Self::ThreadGroup(group) => Some(group),
            _ => None,
        }
    }
}

/// Entities identified by a pid.
#[derive(Debug, RcuDroppable)]
pub struct PidEntry {
    pub id: pid_t,
    task: RcuWeak<Task>,
    process: RcuOptionBox<ProcessEntry>,
    process_group: RcuWeak<ProcessGroup>,
}

impl PidEntry {
    pub fn get_task(&self) -> Result<Arc<Task>, Errno> {
        self.task.upgrade().ok_or_else(|| errno!(ESRCH))
    }

    pub fn get_process(&self) -> Option<ProcessEntryRef> {
        let process = self.process.read()?;
        match &*process {
            ProcessEntry::ThreadGroup(thread_group) => {
                let thread_group = thread_group
                    .upgrade()
                    .expect("ThreadGroup was released, but not removed from PidTable");
                Some(ProcessEntryRef::Process(thread_group))
            }
            ProcessEntry::Zombie => Some(ProcessEntryRef::Zombie),
        }
    }

    pub fn get_thread_group(&self) -> Result<Arc<ThreadGroup>, Errno> {
        match self.get_process() {
            Some(ProcessEntryRef::Process(tg)) => Ok(tg),
            _ => error!(ESRCH),
        }
    }

    pub fn get_process_group(&self) -> Result<Arc<ProcessGroup>, Errno> {
        self.process_group.upgrade().ok_or_else(|| errno!(ESRCH))
    }

    #[cfg(test)]
    pub fn new_for_test(id: pid_t) -> Pid {
        Arc::new(Self::new(id))
    }

    fn new(id: pid_t) -> Self {
        Self {
            id,
            task: Default::default(),
            process: Default::default(),
            process_group: Default::default(),
        }
    }

    fn is_empty(&self, scope: &RcuReadScope) -> bool {
        self.task.strong_count(scope) == 0
            && self.process.is_none(scope)
            && self.process_group.strong_count(scope) == 0
    }
}

impl std::fmt::Display for PidEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.id)
    }
}

impl PartialEq for PidEntry {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self, other)
    }
}

impl Eq for PidEntry {}

impl PartialOrd for PidEntry {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PidEntry {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (self as *const Self).cmp(&(other as *const Self))
    }
}

impl std::hash::Hash for PidEntry {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        (self as *const Self).hash(state);
    }
}

pub enum ProcessEntryRef {
    Process(Arc<ThreadGroup>),
    Zombie,
}

pub type Pid = Arc<PidEntry>;

/// The number of reserved PIDs in Linux. When wrapping around, PID allocation restarts at this value.
pub const RESERVED_PIDS: u32 = 300;

/// The default maximal PID considered in Linux.
pub const PID_MAX_DEFAULT: pid_t = 1 << 15;

/// The maximal PID considered in Linux.
pub const PID_MAX_LIMIT: pid_t = 1 << 22;

/// The default number of PIDs per CPU used to scale pid_max.
pub const PIDS_PER_CPU_DEFAULT: pid_t = 1024;

/// Returns the actual PID limit given a requested limit, scaled by the system's CPU count.
fn actual_pid_limit(limit: pid_t) -> pid_t {
    actual_pid_limit_with_cpus(limit, zx::system_get_num_cpus())
}

/// Returns the actual PID limit given a requested limit and the number of CPUs.
fn actual_pid_limit_with_cpus(limit: pid_t, num_cpus: u32) -> pid_t {
    let cpu_limit = (num_cpus as pid_t).saturating_mul(PIDS_PER_CPU_DEFAULT);
    limit.max(cpu_limit).min(PID_MAX_LIMIT)
}

pub struct PidTable {
    /// The most-recently allocated pid in this table.
    last_pid: AtomicI32,

    /// The tasks in this table, organized by pid_t using an IDR radix tree.
    idr: Idr<PidEntry>,

    /// Used to notify thread group changes.
    thread_group_notifier: RcuOptionBox<std::sync::mpsc::Sender<MemoryAttributionLifecycleEvent>>,
}

impl Default for PidTable {
    fn default() -> Self {
        let idr = Idr::new_cyclic(Some(RESERVED_PIDS));
        idr.set_max(actual_pid_limit(PID_MAX_DEFAULT) as u32);
        idr.lock().reserve_id(0);
        Self { last_pid: AtomicI32::new(0), idr, thread_group_notifier: Default::default() }
    }
}

impl std::fmt::Debug for PidTable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PidTable")
            .field("last_pid", &self.last_pid.load(Ordering::Relaxed))
            .finish()
    }
}

/// An RAII guard representing exclusive writer access to a [`PidTable`].
///
/// Holding this guard serializes mutations (allocations, reservations, removals)
/// to the PID table while allowing concurrent lock-free reads.
pub struct PidTableGuard<'a> {
    table: &'a PidTable,
    idr: IdrGuard<'a, PidEntry>,
}

impl<'a> std::ops::Deref for PidTableGuard<'a> {
    type Target = PidTable;

    fn deref(&self) -> &Self::Target {
        self.table
    }
}

impl<'a> PidTableGuard<'a> {
    /// Allocates a new PID, returning an error if the PID table is full.
    pub fn allocate_pid(&mut self) -> Result<Pid, Errno> {
        self.idr
            .alloc(|id| Arc::new(PidEntry::new(id as pid_t)))
            .map(|(_, pid)| pid)
            .ok_or_else(|| errno!(EAGAIN))
    }

    pub fn add_task(&mut self, task: Arc<Task>) {
        let scope = RcuReadScope::new();
        let entry =
            self.idr.lookup(task.tid.id as u32, &scope).expect("task.tid should be in pid table");
        assert_eq!(entry.task.strong_count(&scope), 0);
        if task.is_leader() {
            assert!(entry.process.is_none(&scope));
        }
        entry.task.update(Arc::downgrade(&task));

        // If not cloning a thread, add its thread group.
        if task.is_leader() {
            self.table.last_pid.store(task.tid.id, Ordering::Relaxed);
            entry
                .process
                .update(Some(ProcessEntry::ThreadGroup(Arc::downgrade(task.thread_group()))));

            // Notify thread group changes.
            if let Some(notifier) = self.table.thread_group_notifier.as_ref(&scope) {
                let mut tg_state = task.thread_group.write();
                let _ = notifier.send(MemoryAttributionLifecycleEvent::creation(task.tid.id));
                tg_state.notifier = Some(notifier.clone());
            }
        }
    }

    fn remove_item<F>(&mut self, pid: &Pid, do_remove: F)
    where
        F: FnOnce(&PidEntry),
    {
        let scope = RcuReadScope::new();
        debug_assert_eq!(self.idr.lookup(pid.id as u32, &scope).as_ref(), Some(pid));
        do_remove(pid);
        if pid.is_empty(&scope) {
            self.idr.remove(pid.id as u32);
        }
    }

    pub fn remove_task(&mut self, tid: &Pid) {
        self.remove_item(tid, |entry| {
            let scope = RcuReadScope::new();
            assert!(entry.task.strong_count(&scope) > 0);
            entry.task.update(Weak::new());
        });
    }

    /// Replace process with the specified `pid` with a zombie.
    pub fn kill_process(&mut self, pid: &Pid) {
        let scope = RcuReadScope::new();
        debug_assert_eq!(self.idr.lookup(pid.id as u32, &scope).as_ref(), Some(pid));
        assert!(matches!(pid.process.read().as_deref(), Some(ProcessEntry::ThreadGroup(_))));

        // All tasks from the process are expected to be cleared from the table before the process
        // becomes a zombie. Cannot verify this for all tasks here, check it just for the leader.
        assert_eq!(pid.task.strong_count(&scope), 0);

        pid.process.update(Some(ProcessEntry::Zombie));
    }

    pub fn remove_zombie(&mut self, pid: &Pid) {
        let scope = RcuReadScope::new();

        self.remove_item(pid, |entry| {
            assert!(matches!(entry.process.read().as_deref(), Some(ProcessEntry::Zombie)));
            entry.process.update(None);
        });

        // Notify thread group changes.
        if let Some(notifier) = self.table.thread_group_notifier.as_ref(&scope) {
            let _ = notifier.send(MemoryAttributionLifecycleEvent::destruction(pid.id));
        }
    }

    pub fn add_process_group(&mut self, process_group: &Arc<ProcessGroup>) {
        let scope = RcuReadScope::new();
        assert_eq!(process_group.leader.process_group.strong_count(&scope), 0);
        process_group.leader.process_group.update(Arc::downgrade(process_group));
    }

    pub fn remove_process_group(&mut self, leader: &Pid) {
        self.remove_item(leader, |entry| {
            let scope = RcuReadScope::new();
            assert!(entry.process_group.strong_count(&scope) > 0);
            entry.process_group.update(Weak::new());
        });
    }
}

impl PidTable {
    /// Acquires the lock for mutations, returning a [`PidTableGuard`].
    pub fn lock(&self) -> PidTableGuard<'_> {
        PidTableGuard { table: self, idr: self.idr.lock() }
    }

    pub fn get(&self, pid: pid_t) -> Result<Pid, Errno> {
        if pid <= 0 {
            return error!(ESRCH);
        }
        let scope = RcuReadScope::new();
        self.idr.lookup(pid as u32, &scope).ok_or_else(|| errno!(ESRCH))
    }

    pub fn set_thread_group_notifier(
        &self,
        notifier: std::sync::mpsc::Sender<MemoryAttributionLifecycleEvent>,
    ) {
        self.thread_group_notifier.update(Some(notifier));
    }

    pub fn get_thread_groups<'a>(
        &'a self,
        scope: &'a RcuReadScope,
    ) -> impl Iterator<Item = Arc<ThreadGroup>> + 'a {
        self.idr.iter(&scope).flat_map(move |(_, entry)| {
            entry
                .process
                .as_ref(&scope)
                .and_then(ProcessEntry::thread_group)
                .and_then(|g| g.upgrade())
        })
    }

    /// Returns the process ids for all processes, including zombies.
    pub fn process_ids(&self) -> Vec<pid_t> {
        let scope = RcuReadScope::new();
        self.idr
            .iter(&scope)
            .flat_map(|(_, entry)| entry.process.is_some(&scope).then_some(entry.id))
            .collect()
    }

    /// Returns an iterator over the [`Pid`]s for all the currently running tasks.
    pub fn running_task_ids<'a>(
        &'a self,
        scope: &'a RcuReadScope,
    ) -> impl Iterator<Item = &'a Pid> {
        self.idr
            .iter(scope)
            .map(|(_, entry)| entry)
            .filter(|entry| entry.task.strong_count(scope) > 0)
    }

    pub fn last_pid(&self) -> pid_t {
        self.last_pid.load(Ordering::Relaxed)
    }

    /// Returns the maximal PID value allowed for allocation.
    pub fn max(&self) -> pid_t {
        self.idr.max() as pid_t
    }

    /// Sets the maximal PID value allowed for allocation.
    pub fn set_max(&self, max: pid_t) {
        self.idr.set_max(actual_pid_limit(max) as u32);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pid_table_allocation() {
        let table = PidTable::default();
        let pid1 = table.lock().allocate_pid().unwrap();
        assert_eq!(pid1.id, 1);

        let pid2 = table.lock().allocate_pid().unwrap();
        assert_eq!(pid2.id, 2);

        assert_eq!(table.get(1).unwrap().id, 1);
        assert_eq!(table.get(2).unwrap().id, 2);
        assert!(table.get(0).is_err());
        assert!(table.get(-1).is_err());
        assert!(table.get(3).is_err());
    }

    #[test]
    fn test_pid_table_lock_guard() {
        let table = PidTable::default();
        let pid1 = {
            let mut guard = table.lock();
            let pid = guard.allocate_pid().unwrap();
            assert_eq!(pid.id, 1);
            pid
        };
        assert_eq!(table.get(1).unwrap(), pid1);
    }

    #[test]
    fn test_pid_table_empty_state() {
        let table = PidTable::default();
        assert_eq!(table.last_pid(), 0);
        assert_eq!(table.process_ids().len(), 0);
        let scope = RcuReadScope::new();
        assert_eq!(table.running_task_ids(&scope).count(), 0);
    }

    #[test]
    fn test_pid_table_max_and_wrap() {
        let table = PidTable::default();
        assert_eq!(table.max(), actual_pid_limit(PID_MAX_DEFAULT));

        // Allocate up to the max value (1..=max).
        let max = table.max();
        for expected in 1..=max {
            let pid = table.lock().allocate_pid().unwrap();
            assert_eq!(pid.id, expected);
        }

        // The table is full up to max. Allocation fails.
        assert_eq!(table.lock().allocate_pid().unwrap_err(), errno!(EAGAIN));

        // Free PID 2 (below RESERVED_PIDS) and PID 302 (at or above RESERVED_PIDS).
        table.idr.lock().remove(2);
        table.idr.lock().remove(302);

        // Next allocation wraps to RESERVED_PIDS (300) and allocates slot 302.
        // Slot 2 is skipped because wrapping only allocates IDs >= RESERVED_PIDS.
        let pid = table.lock().allocate_pid().unwrap();
        assert_eq!(pid.id, 302);

        // Now all slots >= RESERVED_PIDS are occupied, so allocation fails even though slot 2 is free.
        assert_eq!(table.lock().allocate_pid().unwrap_err(), errno!(EAGAIN));

        // Free PID 300. Allocation should reuse it.
        table.idr.lock().remove(300);
        let pid = table.lock().allocate_pid().unwrap();
        assert_eq!(pid.id, 300);

        // Increase max and verify new PIDs can be allocated.
        let new_max = max + 10;
        table.set_max(new_max);
        assert_eq!(table.max(), new_max);
        let pid = table.lock().allocate_pid().unwrap();
        assert_eq!(pid.id, max + 1);
    }

    #[test]
    fn test_actual_pid_limit() {
        // With 1 CPU, limit scales to at least 1024, but PID_MAX_DEFAULT is 32768.
        assert_eq!(actual_pid_limit_with_cpus(PID_MAX_DEFAULT, 1), PID_MAX_DEFAULT);
        assert_eq!(actual_pid_limit_with_cpus(500, 1), 1024);

        // With 64 CPUs, cpu_limit is 65536 > PID_MAX_DEFAULT.
        assert_eq!(actual_pid_limit_with_cpus(PID_MAX_DEFAULT, 64), 65536);
        assert_eq!(actual_pid_limit_with_cpus(100_000, 64), 100_000);

        // Capped at PID_MAX_LIMIT.
        assert_eq!(actual_pid_limit_with_cpus(PID_MAX_LIMIT + 1000, 1), PID_MAX_LIMIT);
        assert_eq!(actual_pid_limit_with_cpus(PID_MAX_DEFAULT, 10_000), PID_MAX_LIMIT);
    }

    #[::fuchsia::test]
    async fn test_pid_table_last_pid_on_thread_group() {
        use crate::testing::spawn_kernel_and_run;
        use starnix_uapi::signals::SIGCHLD;
        use starnix_uapi::{CLONE_SIGHAND, CLONE_THREAD, CLONE_VM};

        spawn_kernel_and_run(async |current_task| {
            let kernel = current_task.kernel();
            let initial_last_pid = kernel.pids.last_pid();

            // Allocating a PID directly must not update last_pid.
            let _allocated = kernel.pids.lock().allocate_pid().unwrap();
            assert_eq!(kernel.pids.last_pid(), initial_last_pid);

            // Cloning a thread in the same thread group must not update last_pid.
            let _thread = current_task.clone_task_for_test(
                (CLONE_THREAD | CLONE_VM | CLONE_SIGHAND) as u64,
                Some(SIGCHLD),
            );
            assert_eq!(kernel.pids.last_pid(), initial_last_pid);

            // Cloning a new process (thread group leader) must update last_pid.
            let child = current_task.clone_task_for_test(0, Some(SIGCHLD));
            assert_eq!(kernel.pids.last_pid(), child.get_pid());
        })
        .await;
    }
}
