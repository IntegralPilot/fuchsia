// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::task::{Kernel, PidTable};
use fuchsia_rcu::RcuReadScope;
use starnix_logging::{log_debug, log_error, log_warn};
use starnix_sync::LockDepRwLock;
use starnix_uapi::{pid_t, tid_t};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use zx::Koid;

/// The Zircon koids backing one `Task`.
///
/// Throughout this module, "task" means `starnix_core::task::Task`, the Linux sense of
/// the word (one thread of a thread group), never Zircon's task abstraction
/// (job/process/thread).
///
/// Both koids are always present: a task is only recorded once its Zircon thread handle
/// is attached and its thread group has a valid backing process. Tasks for which either
/// half is unavailable (a task observed mid-creation by the initial seed, or a Starnix
/// kernel thread with no backing Zircon process) are not stored; mid-creation tasks
/// record themselves once their thread handle is attached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ZirconIdentity {
    /// The Zircon process koid.
    pub process: Koid,
    /// The Zircon thread koid.
    pub thread: Koid,
}

/// The Linux identity behind one Zircon koid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinuxIdentity {
    /// Identifies a specific Linux thread within a process.
    Thread { pid: pid_t, tid: tid_t },
    /// Identifies a Linux process.
    Process { pid: pid_t },
}

/// Bidirectional map between Linux pids/tids and Zircon koids.
///
/// Entries are only added while recording is active; nothing is removed when a thread or
/// process exits so that trace data can be resolved past a `Task`'s lifetime. The whole
/// map is released when the last recording session ends. Additional work may be needed to
/// handle pid reuse (https://fxbug.dev/322874557); currently new mapping information
/// overwrites existing thread mappings.
#[derive(Debug, Default)]
struct PidKoidMap {
    /// Forward table: Linux tid to the koids backing that task.
    tid_to_koid: HashMap<tid_t, ZirconIdentity>,
    /// Forward table: Linux pid to the backing Zircon process koid.
    pid_to_koid: HashMap<pid_t, Koid>,
    /// Reverse table: Zircon koid (thread or process) to its Linux identity.
    koid_to_pid: HashMap<Koid, LinuxIdentity>,
    /// Process-level negative cache: process koids known not to belong to this container.
    ///
    /// System-wide profiling produces many samples from native Fuchsia processes whose
    /// koids can never resolve; by Zircon's task hierarchy (a process that is not a
    /// Starnix process has no Starnix threads), one cached miss per process replaces
    /// two failed lookups per sample. Entries are evicted if a task later records for
    /// that process koid, and the whole set is released with the map.
    unmapped_processes: HashSet<Koid>,
}

impl PidKoidMap {
    fn insert(&mut self, pid: pid_t, tid: tid_t, identity: ZirconIdentity) {
        self.tid_to_koid.insert(tid, identity);
        self.pid_to_koid.entry(pid).or_insert(identity.process);
        self.koid_to_pid.insert(identity.thread, LinuxIdentity::Thread { pid, tid });
        // Threads of the same process all report the same process koid; keep the first
        // entry so the process continues to resolve to the same pid.
        self.koid_to_pid.entry(identity.process).or_insert(LinuxIdentity::Process { pid });
        // Evict a stale negative entry in case a sample was resolved before this task
        // recorded itself.
        self.unmapped_processes.remove(&identity.process);
    }

    fn get_zircon_identity(&self, tid: tid_t) -> Option<&ZirconIdentity> {
        self.tid_to_koid.get(&tid)
    }

    fn get_process_koid(&self, pid: pid_t) -> Option<Koid> {
        self.pid_to_koid.get(&pid).copied()
    }

    fn get_linux_identity(&self, koid: Koid) -> Option<LinuxIdentity> {
        self.koid_to_pid.get(&koid).copied()
    }

    /// Merges mappings from another map without overwriting existing entries, so a
    /// seeding snapshot cannot clobber a fresher entry recorded by a concurrently
    /// spawning task.
    fn extend_from(&mut self, other: PidKoidMap) {
        for (tid, identity) in other.tid_to_koid {
            self.unmapped_processes.remove(&identity.process);
            self.tid_to_koid.entry(tid).or_insert(identity);
        }
        for (pid, koid) in other.pid_to_koid {
            self.pid_to_koid.entry(pid).or_insert(koid);
        }
        for (koid, identity) in other.koid_to_pid {
            self.koid_to_pid.entry(koid).or_insert(identity);
        }
        self.unmapped_processes.extend(other.unmapped_processes);
    }
}

/// Kernel-wide manager of pid/tid to koid mappings, shared by all recording clients
/// (system tracing and CPU profiling). Stored in `kernel.trace_event_manager`.
///
/// The manager lives for the lifetime of the `Kernel`; whether recording is active is
/// tracked by the session count inside it, not by its presence. Clients start recording
/// by obtaining a [`PidKoidSession`] from [`TracePerformanceEventManager::open`] and stop
/// by dropping it, so overlapping sessions from independent clients compose correctly:
/// the map is seeded when the first session opens and released when the last one drops,
/// and cleanup runs on every exit path because it is driven by `Drop`.
///
/// When no session is active, task creation fast-paths out on a single relaxed atomic
/// load with no locks and no allocations.
pub struct TracePerformanceEventManager {
    /// Weak reference to the enclosing `Kernel`, upgraded only on the 0 -> 1 session
    /// transition to seed the map from `kernel.pids`. Weak because the manager is a field
    /// of the `Kernel` itself (a strong reference would be a cycle).
    weak_kernel: Weak<Kernel>,

    /// Number of live [`PidKoidSession`]s. Maintained exclusively under [`state_lock`] by
    /// [`TracePerformanceEventManager::open`] and [`PidKoidSession::drop`].
    active_sessions: AtomicUsize,

    /// Serializes session lifecycle transitions (0 -> 1 initialization and 1 -> 0 cleanup).
    state_lock: Mutex<()>,

    /// The bidirectional mapping table. Readers take short read locks per lookup; the
    /// only writers are task spawns while recording (one insert each) and the session
    /// seed/release transitions.
    map: LockDepRwLock<PidKoidMap, starnix_sync::PidToKoidMapInnerLock>,
}

impl TracePerformanceEventManager {
    /// Creates a new manager holding a weak reference to the enclosing `Kernel`.
    pub fn new(weak_kernel: Weak<Kernel>) -> Self {
        Self {
            weak_kernel,
            active_sessions: AtomicUsize::new(0),
            state_lock: Mutex::new(()),
            map: LockDepRwLock::new(PidKoidMap::default()),
        }
    }

    /// Returns true if at least one session is actively recording mappings.
    pub fn is_recording(&self) -> bool {
        self.active_sessions.load(Ordering::Acquire) > 0
    }

    /// Opens a session. When the first session opens, the map is seeded with
    /// all currently running `Task`s; tasks created afterwards record themselves via
    /// `Task::record_pid_koid_mapping`. Recording continues until all active sessions
    /// have been dropped.
    pub fn open(self: &Arc<Self>) -> PidKoidSession {
        self.start_session_internal();
        PidKoidSession { manager: self.clone() }
    }

    /// Records the mapping for one `Task` if recording is active. Called from `Task`
    /// creation, after the task's Zircon thread handle is attached, so the identity is
    /// always complete.
    pub fn record(&self, pid: pid_t, tid: tid_t, identity: ZirconIdentity) {
        if self.active_sessions.load(Ordering::Acquire) == 0 {
            return;
        }
        self.map.write().insert(pid, tid, identity);
    }

    /// Looks up the Zircon koids recorded for a Linux tid.
    pub(crate) fn get_zircon_identity(&self, tid: tid_t) -> Option<ZirconIdentity> {
        self.map.read().get_zircon_identity(tid).copied()
    }

    /// Looks up the Zircon process koid recorded for a Linux pid.
    pub(crate) fn get_process_koid(&self, pid: pid_t) -> Option<Koid> {
        self.map.read().get_process_koid(pid)
    }

    /// Reverse resolution: maps sampled (process koid, thread koid) to Linux identity,
    /// with negative caching for native Fuchsia processes.
    fn resolve_koids(&self, pkoid: Koid, tkoid: Koid) -> Option<LinuxIdentity> {
        {
            let map = self.map.read();
            if map.unmapped_processes.contains(&pkoid) {
                return None;
            }
            if let Some(identity) = map.get_linux_identity(tkoid) {
                return Some(identity);
            }
        }

        // Native Fuchsia process or unmapped thread: record the process in the negative
        // cache under the write lock, re-checking first in case a concurrent record()
        // populated it meanwhile.
        let mut map = self.map.write();
        if let Some(identity) = map.get_linux_identity(tkoid) {
            return Some(identity);
        }
        if !map.koid_to_pid.contains_key(&pkoid) {
            map.unmapped_processes.insert(pkoid);
        }
        None
    }

    /// Increments the session count, seeding the map from the kernel pid table when this
    /// is the first session.
    fn start_session_internal(&self) {
        let _guard = self.state_lock.lock().unwrap();
        let current = self.active_sessions.load(Ordering::Acquire);
        if current == 0 {
            if let Some(kernel) = self.weak_kernel.upgrade() {
                let snapshot = Self::snapshot_existing_tasks(&kernel.pids);
                self.map.write().extend_from(snapshot);
            } else {
                log_warn!("Kernel is shutting down, unable to snapshot running tasks");
            }
        }
        self.active_sessions.store(current + 1, Ordering::Release);
    }

    /// Decrements the session count, releasing the map when the last session drops.
    fn stop_session_internal(&self) {
        let _guard = self.state_lock.lock().unwrap();
        let current = self.active_sessions.load(Ordering::Acquire);
        if current == 0 {
            log_error!("session stopped without an active session");
            return;
        }
        if current == 1 {
            *self.map.write() = PidKoidMap::default();
        }
        self.active_sessions.store(current - 1, Ordering::Release);
    }

    /// Builds a map of all currently running `Task`s from the kernel pid table.
    ///
    /// A task is captured only if its identity is complete. A task observed mid-creation,
    /// before its Zircon thread handle is attached, is skipped: it records itself moments
    /// later (the executor attaches the thread handle and then calls
    /// `Task::record_pid_koid_mapping` while this session is already active), and during
    /// the gap its tid cannot appear in any trace data because its creator's syscall has
    /// not yet returned. Starnix kernel threads, which have no backing Zircon process,
    /// are intentionally excluded.
    fn snapshot_existing_tasks(pid_table: &PidTable) -> PidKoidMap {
        let mut pid_map = PidKoidMap::default();

        let scope = RcuReadScope::new();
        let mut count = 0;
        for pid in pid_table.running_task_ids(&scope) {
            count += 1;
            // Running `Task`s may exit at any time. Record one only if a snapshot of its
            // running state can be obtained.
            let Ok(task) = pid.get_task() else {
                continue;
            };
            if let Some(identity) = task.get_zircon_identity() {
                pid_map.insert(task.get_pid(), pid.id, identity);
            }
        }

        log_debug!("Initialized {} pid mappings. From {} ids", pid_map.tid_to_koid.len(), count);
        pid_map
    }
}

/// A reader's session and query interface for pid/koid mappings. Recording in the
/// kernel stays active while at least one session is held across any client; dropping
/// the session ends this client's interest. When all active sessions have been dropped,
/// the shared map is released and recording ceases.
///
/// Resolution queries are only reachable through a session, which enforces at compile
/// time that lookups happen while recording is active.
#[must_use = "Recording stops when this guard is dropped"]
pub struct PidKoidSession {
    manager: Arc<TracePerformanceEventManager>,
}

impl PidKoidSession {
    /// Forward resolution: maps a Linux tid to its thread koid, returning None on a miss.
    pub fn resolve_tid_to_koid(&self, tid: tid_t) -> Option<Koid> {
        if tid == 0 {
            return Some(Koid::from_raw(0));
        }
        self.manager.get_zircon_identity(tid).map(|id| id.thread)
    }

    /// Forward resolution: maps a Linux pid to its process koid, returning None on a miss.
    pub fn resolve_pid_to_koid(&self, pid: pid_t) -> Option<Koid> {
        if pid == 0 {
            return Some(Koid::from_raw(0));
        }
        self.manager.get_process_koid(pid)
    }

    /// Reverse resolution: maps sampled (process koid, thread koid) to Linux identity,
    /// with negative caching for native Fuchsia processes. See
    /// [`TracePerformanceEventManager::resolve_koids`].
    pub fn resolve_koids(&self, pkoid: Koid, tkoid: Koid) -> Option<LinuxIdentity> {
        self.manager.resolve_koids(pkoid, tkoid)
    }
}

impl Drop for PidKoidSession {
    fn drop(&mut self) {
        self.manager.stop_session_internal();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::ZirconThread;
    use crate::testing::{create_task, spawn_kernel_and_run};
    use futures::channel::oneshot;

    impl PidKoidMap {
        fn is_empty(&self) -> bool {
            self.tid_to_koid.is_empty() && self.pid_to_koid.is_empty()
        }
    }

    impl TracePerformanceEventManager {
        fn new_for_testing() -> Self {
            Self::new(Weak::new())
        }
    }

    fn identity(process: u64, thread: u64) -> ZirconIdentity {
        ZirconIdentity { process: Koid::from_raw(process), thread: Koid::from_raw(thread) }
    }

    #[fuchsia::test]
    async fn test_snapshot_existing_tasks() {
        let (sender, receiver) = oneshot::channel();
        spawn_kernel_and_run(async move |current_task| {
            let kernel = current_task.kernel();
            let pid = current_task.task.get_pid();
            let tid = current_task.task.get_tid();
            let identity = current_task.task.get_zircon_identity().unwrap();

            // A task without an attached Zircon thread must be skipped by the seed.
            let _another_current = create_task(&kernel, "another-task");

            let session = kernel.trace_event_manager.open();

            assert_eq!(session.resolve_tid_to_koid(tid), Some(identity.thread));
            assert_eq!(session.resolve_pid_to_koid(pid), Some(identity.process));
            assert_eq!(kernel.trace_event_manager.map.read().tid_to_koid.len(), 1);
            assert_eq!(kernel.trace_event_manager.map.read().pid_to_koid.len(), 1);

            sender.send(()).unwrap();
        })
        .await;
        receiver.await.unwrap();
    }

    #[fuchsia::test]
    fn test_forward_resolution() {
        let manager = Arc::new(TracePerformanceEventManager::new_for_testing());
        let session = manager.open();

        manager.record(10, 10, identity(1001, 2001));
        manager.record(10, 11, identity(1001, 2002));

        // Worker thread whose leader (pid 20) never recorded a tid == 20 entry.
        manager.record(20, 201, identity(2001, 3001));

        // Zero resolves to zero.
        assert_eq!(session.resolve_tid_to_koid(0), Some(Koid::from_raw(0)));
        assert_eq!(session.resolve_pid_to_koid(0), Some(Koid::from_raw(0)));

        // Mapped values.
        assert_eq!(session.resolve_tid_to_koid(10), Some(Koid::from_raw(2001)));
        assert_eq!(session.resolve_tid_to_koid(11), Some(Koid::from_raw(2002)));
        assert_eq!(session.resolve_pid_to_koid(10), Some(Koid::from_raw(1001)));

        // Process PID resolves even when leader TID is not mapped.
        assert_eq!(session.resolve_pid_to_koid(20), Some(Koid::from_raw(2001)));
        assert_eq!(session.resolve_tid_to_koid(201), Some(Koid::from_raw(3001)));
        assert_eq!(session.resolve_tid_to_koid(20), None);

        // Unmapped values return None.
        assert_eq!(session.resolve_tid_to_koid(999), None);
        assert_eq!(session.resolve_pid_to_koid(999), None);
    }

    #[fuchsia::test]
    fn test_reverse_table_maintained() {
        let manager = Arc::new(TracePerformanceEventManager::new_for_testing());
        let _session = manager.open();
        manager.record(10, 100, identity(1000, 2000));

        assert_eq!(
            manager.map.read().get_linux_identity(Koid::from_raw(2000)),
            Some(LinuxIdentity::Thread { pid: 10, tid: 100 })
        );
        assert_eq!(
            manager.map.read().get_linux_identity(Koid::from_raw(1000)),
            Some(LinuxIdentity::Process { pid: 10 })
        );
        assert_eq!(manager.get_zircon_identity(100), Some(identity(1000, 2000)));
    }

    #[fuchsia::test]
    fn test_seed_does_not_overwrite_fresh_record() {
        let manager = Arc::new(TracePerformanceEventManager::new_for_testing());
        let _session = manager.open();

        // A task records a fresh entry, then a stale snapshot arrives with an outdated
        // entry for the same tid: the fresh entry must win.
        manager.record(1, 1, identity(101, 201));
        let mut stale = PidKoidMap::default();
        stale.insert(1, 1, identity(101, 999));
        manager.map.write().extend_from(stale);

        assert_eq!(manager.get_zircon_identity(1).map(|id| id.thread), Some(Koid::from_raw(201)));
    }

    #[fuchsia::test]
    fn test_reverse_resolution() {
        let manager = Arc::new(TracePerformanceEventManager::new_for_testing());
        let session = manager.open();
        manager.record(10, 100, identity(1000, 2000));

        // Known thread koid resolves to its pid/tid.
        assert_eq!(
            session.resolve_koids(Koid::from_raw(1000), Koid::from_raw(2000)),
            Some(LinuxIdentity::Thread { pid: 10, tid: 100 })
        );

        // Unknown thread koid under a known process returns None without negatively caching the process.
        assert_eq!(session.resolve_koids(Koid::from_raw(1000), Koid::from_raw(9999)), None);
        assert!(!manager.map.read().unmapped_processes.contains(&Koid::from_raw(1000)));

        // Native Fuchsia process: returns None and the process lands in the negative cache.
        assert_eq!(session.resolve_koids(Koid::from_raw(8), Koid::from_raw(7)), None);
        assert!(manager.map.read().unmapped_processes.contains(&Koid::from_raw(8)));

        // A task recording for a negatively cached process evicts the stale entry and
        // resolves afterwards.
        manager.record(30, 300, identity(8, 9));
        assert!(!manager.map.read().unmapped_processes.contains(&Koid::from_raw(8)));
        assert_eq!(
            session.resolve_koids(Koid::from_raw(8), Koid::from_raw(9)),
            Some(LinuxIdentity::Thread { pid: 30, tid: 300 })
        );

        // After the last session drops, the map and negative cache are released, so
        // resolution returns None.
        drop(session);
        assert_eq!(manager.resolve_koids(Koid::from_raw(1000), Koid::from_raw(2000)), None);
    }

    #[fuchsia::test]
    async fn test_overlapping_sessions() {
        spawn_kernel_and_run(async move |current_task| {
            let kernel = current_task.kernel();
            let manager = &kernel.trace_event_manager;
            assert!(!manager.is_recording());

            // Two independent clients open overlapping sessions.
            let session_a = manager.open();
            let session_b = manager.open();
            assert!(manager.is_recording());

            manager.record(99, 999, identity(9900, 9901));
            assert_eq!(session_b.resolve_tid_to_koid(999), Some(Koid::from_raw(9901)));

            // Dropping one session must not disturb the other.
            drop(session_a);
            assert!(manager.is_recording());
            assert_eq!(session_b.resolve_tid_to_koid(999), Some(Koid::from_raw(9901)));

            // Dropping the last session releases the map and stops recording; the
            // manager itself stays on the kernel.
            drop(session_b);
            assert!(!manager.is_recording());
            assert!(manager.map.read().is_empty());
        })
        .await;
    }

    #[fuchsia::test]
    async fn test_lifecycle() {
        let (sender, receiver) = oneshot::channel();
        spawn_kernel_and_run(async move |current_task| {
            let kernel = current_task.kernel();
            let session = kernel.trace_event_manager.open();

            assert_eq!(kernel.trace_event_manager.map.read().tid_to_koid.len(), 1);

            // Associate a thread with a new task.
            let another_current = create_task(&kernel, "another-task");
            let test_thread = another_current
                .thread_group()
                .process
                .create_thread(b"my-new-test-thread")
                .expect("test thread");

            {
                another_current
                    .running_state()
                    .thread
                    .set(ZirconThread::new(Arc::new(test_thread)))
                    .expect("test thread set");
            }

            assert_eq!(kernel.trace_event_manager.map.read().tid_to_koid.len(), 1);

            // This is called by the task when it is all ready to run.
            another_current.record_pid_koid_mapping();

            // Now expect 2 mappings.
            assert_eq!(kernel.trace_event_manager.map.read().tid_to_koid.len(), 2);
            assert!(
                kernel
                    .trace_event_manager
                    .get_zircon_identity(another_current.task.get_tid())
                    .is_some()
            );

            drop(session);

            // After the last session drops, record_pid_koid_mapping is a no-op.
            another_current.record_pid_koid_mapping();
            assert!(kernel.trace_event_manager.map.read().is_empty());
            sender.send(()).unwrap();
        })
        .await;
        receiver.await.unwrap();
    }
}
