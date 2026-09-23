// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::device::terminal::{Terminal, TerminalController};
use crate::mutable_state::{state_accessor, state_implementation};
use crate::ptrace::{
    AtomicStopState, PtraceAllowedPtracers, PtraceEvent, PtraceOptions, PtraceStatus, PtraceTracer,
    StopState, ZombiePtracees, ptrace_detach,
};
use crate::security;
use crate::signals::syscalls::WaitingOptions;
use crate::signals::{
    DeliveryAction, IntoSignalInfoOptions, QueuedSignals, SignalActions, SignalDetail, SignalInfo,
    UncheckedSignalInfo, action_for_signal, send_standard_signal,
};
use crate::task::memory_attribution::MemoryAttributionLifecycleEvent;
use crate::task::{
    ControllingTerminal, CurrentTask, ExitStatus, Kernel, Pid, PidTable, PidTableGuard,
    ProcessGroup, Session, SessionDisassociation, Task, TaskMutableState, TaskPersistentInfo,
    TypedWaitQueue,
};
use crate::time::{IntervalTimerHandle, TimerTable};
use itertools::Itertools;
use macro_rules_attribute::apply;
use starnix_lifecycle::{AtomicCounter, DropNotifier};
use starnix_logging::{log_debug, log_error, log_info, log_warn, track_stub};
use starnix_sync::{
    LockDepMutex, LockDepRwLock, ThreadGroupLimits, ThreadGroupMutableStateLock,
    ThreadGroupPendingSignalsLock, ThreadGroupPtraceesLock, allow_subclass, ordered_write_lock,
};
use starnix_task_command::TaskCommand;
use starnix_types::ownership::{OwnedRef, Releasable};
use starnix_types::stats::TaskTimeStats;
use starnix_types::time::{itimerspec_from_itimerval, timeval_from_duration};
use starnix_uapi::auth::{CAP_SYS_ADMIN, CAP_SYS_RESOURCE, Credentials};
use starnix_uapi::errors::Errno;
use starnix_uapi::personality::PersonalityFlags;
use starnix_uapi::resource_limits::{Resource, ResourceLimits};
use starnix_uapi::signals::{
    SIGCHLD, SIGCONT, SIGHUP, SIGKILL, SIGTERM, SIGTTOU, SigSet, Signal, UncheckedSignal,
};
use starnix_uapi::user_address::UserAddress;
use starnix_uapi::{
    ITIMER_PROF, ITIMER_REAL, ITIMER_VIRTUAL, SA_NOCLDWAIT, SI_TKILL, SI_USER, SIG_IGN, errno,
    error, itimerval, pid_t, rlimit, tid_t, uid_t,
};
use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use zx::{Koid, Status};

#[derive(Debug)]
pub struct ZirconProcess {
    process: zx::Process,
    koid: Result<Koid, Status>,
}

impl ZirconProcess {
    pub fn new(process: zx::Process) -> Self {
        let koid = process.koid();
        Self { process, koid }
    }

    pub fn koid(&self) -> Result<Koid, Status> {
        self.koid
    }
}

impl std::ops::Deref for ZirconProcess {
    type Target = zx::Process;
    fn deref(&self) -> &Self::Target {
        &self.process
    }
}

/// Values used for waiting on the [ThreadGroup] lifecycle wait queue.
#[repr(u64)]
pub enum ThreadGroupLifecycleWaitValue {
    /// Wait for updates to the WaitResults of tasks in the group.
    ChildStatus,
    /// Wait for updates to `stopped`.
    Stopped,
}

impl Into<u64> for ThreadGroupLifecycleWaitValue {
    fn into(self) -> u64 {
        self as u64
    }
}

/// Child process that have exited, but the zombie ptrace needs to be consumed
/// before they can be waited for.
#[derive(Clone, Debug)]
pub struct DeferredZombiePTracer {
    /// Original tracer
    pub tracer_pid: Pid,
    /// Tracee tid
    pub tracee_tid: Pid,
    /// Tracee pgid
    pub tracee_pgid: Pid,
    /// Tracee thread group
    pub tracee_pid: Pid,
}

impl DeferredZombiePTracer {
    fn new(tracer: &ThreadGroup, tracee: &Task, tracee_pgid: Pid) -> Self {
        Self {
            tracer_pid: tracer.leader.clone(),
            tracee_tid: tracee.tid.clone(),
            tracee_pgid,
            tracee_pid: tracee.pid.clone(),
        }
    }
}

/// The mutable state of the ThreadGroup.
pub struct ThreadGroupMutableState {
    /// The parent thread group.
    ///
    /// The value needs to be writable so that it can be re-parent to the correct subreaper if the
    /// parent ends before the child.
    pub parent: Option<ThreadGroupParent>,

    /// The signal this process generates on exit.
    pub exit_signal: Option<Signal>,

    /// The tasks in the thread group.
    ///
    /// The references to Task is weak to prevent cycles as Task have a Arc reference to their
    /// thread group.
    /// It is still expected that these weak references are always valid, as tasks must unregister
    /// themselves before they are deleted.
    tasks: HashSet<TaskPersistentInfo>,

    /// The children of this thread group.
    ///
    /// The references to ThreadGroup is weak to prevent cycles as ThreadGroup have a Arc reference
    /// to their parent.
    /// It is still expected that these weak references are always valid, as thread groups must unregister
    /// themselves before they are deleted.
    pub children: BTreeMap<pid_t, Weak<ThreadGroup>>,

    /// Child tasks that have exited, but not yet been waited for.
    pub zombie_children: Vec<OwnedRef<ZombieProcess>>,

    /// ptracees of this process that have exited, but not yet been waited for.
    pub zombie_ptracees: ZombiePtracees,

    /// Child processes that have exited, but the zombie ptrace needs to be consumed
    /// before they can be waited for.
    pub deferred_zombie_ptracers: Vec<DeferredZombiePTracer>,

    /// Unified [WaitQueue] for all waited ThreadGroup events.
    pub lifecycle_waiters: TypedWaitQueue<ThreadGroupLifecycleWaitValue>,

    /// Whether this thread group will inherit from children of dying processes in its descendant
    /// tree.
    pub is_child_subreaper: bool,

    /// The IDs used to perform shell job control.
    pub process_group: Arc<ProcessGroup>,

    pub did_exec: bool,

    /// A signal that indicates whether the process is going to become waitable
    /// via waitid and waitpid for either WSTOPPED or WCONTINUED, depending on
    /// the value of `stopped`. If not None, contains the SignalInfo to return.
    pub last_signal: Option<SignalInfo>,

    /// Whether the `ThreadGroup` is running or not.
    ///
    /// For exited thread groups, this contains the exit status.
    run_state: ThreadGroupRunState,

    /// Time statistics accumulated from the children.
    pub children_time_stats: TaskTimeStats,

    /// Personality flags set with `sys_personality()`.
    pub personality: PersonalityFlags,

    /// Thread groups allowed to trace tasks in this this thread group.
    pub allowed_ptracers: PtraceAllowedPtracers,

    /// Channel to message when this thread group exits.
    exit_notifier: Option<futures::channel::oneshot::Sender<()>>,

    /// Notifier for name changes.
    pub notifier: Option<std::sync::mpsc::Sender<MemoryAttributionLifecycleEvent>>,
}

/// A collection of `Task` objects that roughly correspond to a "process".
///
/// Userspace programmers often think about "threads" and "process", but those concepts have no
/// clear analogs inside the kernel because tasks are typically created using `clone(2)`, which
/// takes a complex set of flags that describes how much state is shared between the original task
/// and the new task.
///
/// If a new task is created with the `CLONE_THREAD` flag, the new task will be placed in the same
/// `ThreadGroup` as the original task. Userspace typically uses this flag in conjunction with the
/// `CLONE_FILES`, `CLONE_VM`, and `CLONE_FS`, which corresponds to the userspace notion of a
/// "thread". For example, that's how `pthread_create` behaves. In that sense, a `ThreadGroup`
/// normally corresponds to the set of "threads" in a "process". However, this pattern is purely a
/// userspace convention, and nothing stops userspace from using `CLONE_THREAD` without
/// `CLONE_FILES`, for example.
///
/// In Starnix, a `ThreadGroup` corresponds to a Zircon process, which means we do not support the
/// `CLONE_THREAD` flag without the `CLONE_VM` flag. If we run into problems with this limitation,
/// we might need to revise this correspondence.
///
/// Each `Task` in a `ThreadGroup` has the same thread group ID (`tgid`). The task with the same
/// `pid` as the `tgid` is called the thread group leader.
///
/// Thread groups are destroyed when the last task in the group exits.
pub struct ThreadGroup {
    /// Weak reference to the `OwnedRef` of this `ThreadGroup`. This allows to retrieve the
    /// `TempRef` from a raw `ThreadGroup`.
    pub weak_self: Weak<ThreadGroup>,

    /// The kernel to which this thread group belongs.
    pub kernel: Arc<Kernel>,

    /// A handle to the underlying Zircon process object.
    ///
    /// Currently, we have a 1-to-1 mapping between thread groups and zx::process
    /// objects. This approach might break down if/when we implement CLONE_VM
    /// without CLONE_THREAD because that creates a situation where two thread
    /// groups share an address space. To implement that situation, we might
    /// need to break the 1-to-1 mapping between thread groups and zx::process
    /// or teach zx::process to share address spaces.
    pub process: ZirconProcess,

    /// A handle to the restricted address space for the Zircon process object.
    pub root_vmar: zx::Vmar,

    /// The lead task of this thread group.
    ///
    /// The lead task is typically the initial thread created in the thread group.
    pub leader: Pid,

    /// The signal actions that are registered for this process.
    pub signal_actions: Arc<SignalActions>,

    /// The timers for this thread group (from timer_create(), etc.).
    pub timers: TimerTable,

    /// A mechanism to be notified when this `ThreadGroup` is destroyed.
    pub drop_notifier: DropNotifier,

    /// Whether the process is currently stopped.
    ///
    /// Must only be set when the `mutable_state` write lock is held.
    stop_state: AtomicStopState,

    /// The mutable state of the ThreadGroup.
    mutable_state: LockDepRwLock<ThreadGroupMutableState, ThreadGroupMutableStateLock>,

    /// The resource limits for this thread group.  This is outside mutable_state
    /// to avoid deadlocks where the thread_group lock is held when acquiring
    /// the task lock, and vice versa.
    pub limits: LockDepMutex<ResourceLimits, ThreadGroupLimits>,

    /// The next unique identifier for a seccomp filter.  These are required to be
    /// able to distinguish identical seccomp filters, which are treated differently
    /// for the purposes of SECCOMP_FILTER_FLAG_TSYNC.  Inherited across clone because
    /// seccomp filters are also inherited across clone.
    pub next_seccomp_filter_id: AtomicCounter<u64>,

    /// Tasks ptraced by this process
    pub ptracees: LockDepMutex<HashSet<TaskPersistentInfo>, ThreadGroupPtraceesLock>,

    /// The signals that are currently pending for this thread group.
    pub pending_signals: LockDepMutex<QueuedSignals, ThreadGroupPendingSignalsLock>,

    /// Whether or not there are any pending signals available for tasks in this thread group.
    /// Used to avoid having to acquire the signal state lock in hot paths.
    pub has_pending_signals: AtomicBool,

    /// The monotonic time at which the thread group started.
    pub start_time: zx::MonotonicInstant,

    /// Whether to log syscalls at INFO level for this thread group.
    log_syscalls_as_info: AtomicBool,
}

impl fmt::Debug for ThreadGroup {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}({})",
            self.process.get_name().unwrap_or(zx::Name::new_lossy("<unknown>")),
            self.leader
        )
    }
}

impl ThreadGroup {
    pub fn sync_syscall_log_level(&self) {
        let command = self.read().leader_command();
        let filters = self.kernel.syscall_log_filters.lock();
        let should_log = filters.iter().any(|f| f.matches(&command));
        let prev_should_log = self.log_syscalls_as_info.swap(should_log, Ordering::Relaxed);
        let change_str = match (should_log, prev_should_log) {
            (true, false) => Some("Enabled"),
            (false, true) => Some("Disabled"),
            _ => None,
        };
        if let Some(change_str) = change_str {
            log_info!(
                "{change_str} info syscall logs for thread group {} (command: {command})",
                self.leader
            );
        }
    }

    #[inline]
    pub fn syscall_log_level(&self) -> starnix_logging::Level {
        if self.log_syscalls_as_info.load(Ordering::Relaxed) {
            starnix_logging::Level::Info
        } else {
            starnix_logging::Level::Trace
        }
    }
}

impl PartialEq for ThreadGroup {
    fn eq(&self, other: &Self) -> bool {
        self.leader == other.leader
    }
}

impl Drop for ThreadGroup {
    fn drop(&mut self) {
        let state = self.mutable_state.get_mut();
        assert!(state.tasks.is_empty());
        assert!(state.children.is_empty());
        assert!(state.zombie_children.is_empty());
        assert!(state.zombie_ptracees.is_empty());
        #[cfg(any(test, debug_assertions))]
        assert!(
            state
                .parent
                .as_ref()
                .and_then(|p| p.0.upgrade().as_ref().map(|p| p
                    .read()
                    .children
                    .get(&self.leader.id)
                    .is_none()))
                .unwrap_or(true)
        );
    }
}

/// A wrapper around a `Weak<ThreadGroup>` that expects the underlying `Weak` to always be
/// valid. The wrapper will check this at runtime during creation and upgrade.
pub struct ThreadGroupParent(Weak<ThreadGroup>);

impl ThreadGroupParent {
    pub fn new(t: Weak<ThreadGroup>) -> Self {
        debug_assert!(t.upgrade().is_some());
        Self(t)
    }

    pub fn upgrade(&self) -> Arc<ThreadGroup> {
        self.0.upgrade().expect("ThreadGroupParent references must always be valid")
    }
}

impl Clone for ThreadGroupParent {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

/// A selector that can match a process. Works as a representation of the pid argument to syscalls
/// like wait and kill.
#[derive(Debug, Clone)]
pub enum ProcessSelector {
    /// Matches any process at all.
    Any,
    /// Matches only the process with the specified pid
    Pid(Pid),
    /// Matches all the processes in the given process group
    Pgid(Pid),
}

impl ProcessSelector {
    pub fn match_tid(&self, tid: &Pid) -> bool {
        match self {
            ProcessSelector::Pid(pid) => {
                if pid == tid {
                    true
                } else if let Ok(task_ref) = tid.get_task() {
                    &task_ref.pid == pid
                } else {
                    false
                }
            }
            ProcessSelector::Any => true,
            ProcessSelector::Pgid(pgid) => {
                if let Ok(task_ref) = tid.get_task() {
                    &task_ref.thread_group().read().process_group.leader == pgid
                } else {
                    false
                }
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessExitInfo {
    pub status: ExitStatus,
    pub exit_signal: Option<Signal>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
enum ThreadGroupRunState {
    #[default]
    Running,
    Exiting(ExitStatus),
    Exited(ExitStatus),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WaitResult {
    pub pid: Pid,
    pub uid: uid_t,

    pub exit_info: ProcessExitInfo,

    /// Cumulative time stats for the process and its children.
    pub time_stats: TaskTimeStats,
}

impl WaitResult {
    // According to wait(2) man page, SignalInfo.signal needs to always be set to SIGCHLD
    pub fn as_signal_info(&self) -> SignalInfo {
        SignalInfo::with_detail(
            SIGCHLD,
            self.exit_info.status.signal_info_code(),
            SignalDetail::SIGCHLD {
                pid: self.pid.clone(),
                uid: self.uid,
                status: self.exit_info.status.signal_info_status(),
            },
        )
    }
}

#[derive(Debug)]
pub struct ZombieProcess {
    pub pid: Pid,
    pub pgid: Pid,
    pub uid: uid_t,

    pub exit_info: ProcessExitInfo,

    /// Cumulative time stats for the process and its children.
    pub time_stats: TaskTimeStats,

    /// Whether dropping this ZombieProcess should imply removing the pid from
    /// the PidTable
    pub is_canonical: bool,
}

impl PartialEq for ZombieProcess {
    fn eq(&self, other: &Self) -> bool {
        // We assume only one set of ZombieProcess data per process, so this should cover it.
        self.pid == other.pid && self.is_canonical == other.is_canonical
    }
}

impl Eq for ZombieProcess {}

impl PartialOrd for ZombieProcess {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ZombieProcess {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (&self.pid, self.is_canonical).cmp(&(&other.pid, other.is_canonical))
    }
}

impl ZombieProcess {
    pub fn new(
        thread_group: ThreadGroupStateRef<'_>,
        credentials: &Credentials,
        exit_info: ProcessExitInfo,
    ) -> OwnedRef<Self> {
        let time_stats = thread_group.base.time_stats() + thread_group.children_time_stats;
        OwnedRef::new(ZombieProcess {
            pid: thread_group.base.leader.clone(),
            pgid: thread_group.process_group.leader.clone(),
            uid: credentials.uid,
            exit_info,
            time_stats,
            is_canonical: true,
        })
    }

    pub fn pid(&self) -> pid_t {
        self.pid.id
    }

    pub fn pgid(&self) -> pid_t {
        self.pgid.id
    }

    pub fn to_wait_result(&self) -> WaitResult {
        WaitResult {
            pid: self.pid.clone(),
            uid: self.uid,
            exit_info: self.exit_info.clone(),
            time_stats: self.time_stats,
        }
    }

    pub fn as_artificial(&self) -> Self {
        ZombieProcess {
            pid: self.pid.clone(),
            pgid: self.pgid.clone(),
            uid: self.uid,
            exit_info: self.exit_info.clone(),
            time_stats: self.time_stats,
            is_canonical: false,
        }
    }

    pub fn matches_selector(&self, selector: &ProcessSelector) -> bool {
        match selector {
            ProcessSelector::Any => true,
            ProcessSelector::Pid(pid) => &self.pid == pid,
            ProcessSelector::Pgid(pgid) => &self.pgid == pgid,
        }
    }

    pub fn matches_selector_and_waiting_option(
        &self,
        selector: &ProcessSelector,
        options: &WaitingOptions,
    ) -> bool {
        if !self.matches_selector(selector) {
            return false;
        }

        if options.wait_for_all {
            true
        } else {
            // A "clone" zombie is one which has delivered no signal, or a
            // signal other than SIGCHLD to its parent upon termination.
            options.wait_for_clone == (self.exit_info.exit_signal != Some(SIGCHLD))
        }
    }
}

/// Trait for releasing a zombie process from the PID table.
///
/// This trait erases the lifetime parameter of [`PidTableGuard`] so that [`ZombieProcess`] can
/// implement [`Releasable`] without tying the mutable reference lifetime to the guard's lifetime
/// parameter, preserving variance and allowing reborrowing in loops and across sequential calls.
pub trait ZombieReleaser {
    fn remove_zombie(&mut self, pid: &Pid);
}

impl<'a> ZombieReleaser for PidTableGuard<'a> {
    fn remove_zombie(&mut self, pid: &Pid) {
        self.remove_zombie(pid);
    }
}

impl Releasable for ZombieProcess {
    type Context<'a> = &'a mut dyn ZombieReleaser;

    fn release<'a>(self, pids: &'a mut dyn ZombieReleaser) {
        if self.is_canonical {
            pids.remove_zombie(&self.pid);
        }
    }
}

/// A zombie process that is pending notification.
///
/// # Thread Safety
///
/// Notifications are generally produced in contexts in which a [`ThreadGroup`] state lock is held.
/// Any such lock must be released before notifications are delivered. The notification's
/// recipient thread group may be:
/// - The originating thread group, in which case delivery while locked would self-deadlock.
/// - One of this thread group's ancestors, in which case delivery while locked would invert the
///   parent-child ordering of [`ThreadGroup`] locks.
///
/// The [`PidTable`] lock must be held continuously between [`ZombieNotification`] production and
/// delivery to protect against concurrent exit races. Delivery requires releasing [`ThreadGroup`]
/// state locks. If the recipient thread group exits before the notification is delivered, subreaper
/// identification becomes impossible and the zombie must be reaped without notifying observers.
/// Holding the [`PidTable`] lock throughout notification ensures the recipient cannot concurrently
/// exit.
#[must_use = "Notifications must be explicitly delivered or discarded"]
pub struct ZombieNotification {
    /// The recipient [`ThreadGroup`], which is generally the zombie's parent.
    pub recipient: Weak<ThreadGroup>,

    /// The zombie process to notify the parent of.
    pub zombie: OwnedRef<ZombieProcess>,
}

impl ZombieNotification {
    pub fn new(recipient: Weak<ThreadGroup>, zombie: OwnedRef<ZombieProcess>) -> Self {
        Self { recipient, zombie }
    }

    /// Delivers the zombie notification to the parent.
    ///
    /// # Thread Safety
    ///
    /// Acquires [`ThreadGroup`] state locks.
    pub fn deliver(self, pids: &mut PidTableGuard<'_>) {
        if let Some(parent) = self.recipient.upgrade() {
            parent.do_zombie_notifications(self.zombie, pids);
        } else {
            log_warn!("Zombie {} reaped silently", self.zombie.pid());
            self.zombie.release(pids);
        }
    }

    /// Discards the zombie notification without delivering it.
    ///
    /// If the [`ZombieProcess`] has no other owners, it will be reaped.
    pub fn discard(self, pids: &mut PidTableGuard<'_>) {
        self.zombie.release(pids);
    }
}

impl ThreadGroup {
    /// Creates a ThreadGroup for a regular userspace process.
    pub fn new(
        kernel: Arc<Kernel>,
        process: zx::Process,
        root_vmar: zx::Vmar,
        parent: Option<ThreadGroupWriteGuard<'_>>,
        leader: Pid,
        exit_signal: Option<Signal>,
        process_group: Arc<ProcessGroup>,
        signal_actions: Arc<SignalActions>,
    ) -> Arc<ThreadGroup> {
        debug_assert!(!process.is_invalid());
        debug_assert!(!root_vmar.is_invalid());
        Self::new_internal(
            kernel,
            process,
            root_vmar,
            parent,
            leader,
            exit_signal,
            process_group,
            signal_actions,
        )
    }

    /// Creates a ThreadGroup for a kernel system task (e.g., kthreadd).
    pub fn for_system(
        kernel: Arc<Kernel>,
        leader: Pid,
        process_group: Arc<ProcessGroup>,
    ) -> Arc<ThreadGroup> {
        Self::new_internal(
            kernel,
            zx::Process::invalid(),
            zx::Vmar::invalid(),
            None,
            leader,
            Some(SIGCHLD),
            process_group,
            SignalActions::default(),
        )
    }

    /// Creates a ThreadGroup suitable for use in tests.
    ///
    /// This function performs the minimal setup necessary to produce a valid `ThreadGroup`
    /// instance. It uses an invalid handle for the root VMAR, sets no parent, and uses
    /// default signal actions with `SIGCHLD` as the exit signal.
    ///
    /// This should only be used in tests where a full process environment is not required.
    pub fn for_test(
        kernel: Arc<Kernel>,
        process: zx::Process,
        parent: ThreadGroupWriteGuard<'_>,
        leader: Pid,
        process_group: Arc<ProcessGroup>,
    ) -> Arc<ThreadGroup> {
        Self::new_internal(
            kernel,
            process,
            zx::Vmar::invalid(),
            Some(parent),
            leader,
            Some(SIGCHLD),
            process_group,
            SignalActions::default(),
        )
    }

    fn new_internal(
        kernel: Arc<Kernel>,
        process: zx::Process,
        root_vmar: zx::Vmar,
        parent: Option<ThreadGroupWriteGuard<'_>>,
        leader: Pid,
        exit_signal: Option<Signal>,
        process_group: Arc<ProcessGroup>,
        signal_actions: Arc<SignalActions>,
    ) -> Arc<ThreadGroup> {
        Arc::new_cyclic(|weak_self| {
            let process = ZirconProcess::new(process);
            let mut thread_group = ThreadGroup {
                weak_self: weak_self.clone(),
                kernel,
                process,
                root_vmar,
                leader,
                signal_actions,
                timers: Default::default(),
                drop_notifier: Default::default(),
                // A child process created via fork(2) inherits its parent's
                // resource limits.  Resource limits are preserved across execve(2).
                limits: LockDepMutex::new(
                    parent
                        .as_ref()
                        .map(|p| p.base.limits.lock().clone())
                        .unwrap_or(Default::default()),
                ),
                next_seccomp_filter_id: Default::default(),
                ptracees: Default::default(),
                stop_state: AtomicStopState::new(StopState::Awake),
                pending_signals: Default::default(),
                has_pending_signals: Default::default(),
                start_time: zx::MonotonicInstant::get(),
                mutable_state: ThreadGroupMutableState {
                    parent: parent
                        .as_ref()
                        .map(|p| ThreadGroupParent::new(p.base.weak_self.clone())),
                    exit_signal,
                    tasks: HashSet::new(),
                    children: BTreeMap::new(),
                    zombie_children: vec![],
                    zombie_ptracees: ZombiePtracees::new(),
                    deferred_zombie_ptracers: vec![],
                    lifecycle_waiters: TypedWaitQueue::<ThreadGroupLifecycleWaitValue>::default(),
                    is_child_subreaper: false,
                    process_group: Arc::clone(&process_group),
                    did_exec: false,
                    last_signal: None,
                    run_state: Default::default(),
                    children_time_stats: Default::default(),
                    personality: parent
                        .as_ref()
                        .map(|p| p.personality)
                        .unwrap_or(Default::default()),
                    allowed_ptracers: PtraceAllowedPtracers::None,
                    exit_notifier: None,
                    notifier: None,
                }
                .into(),
                log_syscalls_as_info: AtomicBool::new(false),
            };

            if let Some(mut parent) = parent {
                thread_group.next_seccomp_filter_id.reset(parent.base.next_seccomp_filter_id.get());
                parent.children.insert(thread_group.leader.id, weak_self.clone());
                process_group.insert(&thread_group);
            };
            thread_group
        })
    }

    state_accessor!(ThreadGroup, mutable_state);

    pub fn load_stopped(&self) -> StopState {
        self.stop_state.load(Ordering::Relaxed)
    }

    /// Causes the thread group to exit.
    ///
    /// This marks the thread group as exiting and sends [`SIGKILL`] to its tasks to initiate
    /// teardown. The thread group will not exist until the last task exits.
    ///
    /// If this is being called from a task that is part of the current thread group, the caller
    /// should pass `current_task`. If ownership issues prevent passing `current_task`, then callers
    /// should use [`CurrentTask::kill_thread_group()`] instead.
    pub fn kill(&self, exit_status: ExitStatus, mut current_task: Option<&mut CurrentTask>) {
        if let Some(ref mut current_task) = current_task {
            current_task
                .ptrace_event(PtraceOptions::TRACEEXIT, exit_status.signal_info_status() as u64);
        }
        let mut pids = self.kernel.pids.lock();
        let mut state = self.write();
        if !state.is_running() {
            return;
        }

        state.run_state = ThreadGroupRunState::Exiting(exit_status.clone());

        // Detach from any ptraced zombie tasks.
        let zombie_notifications = state.zombie_ptracees.detach_all(&mut pids);

        // Interrupt each task. Unlock the group because send_signal will lock the group in order
        // to call set_stopped.
        let tasks = state.tasks();
        drop(state);

        for notification in zombie_notifications {
            notification.deliver(&mut pids);
        }
        self.detach_ptracees(&mut pids);

        for task in tasks {
            task.write().set_exit_status(exit_status.clone());
            send_standard_signal(&task, SignalInfo::kernel(SIGKILL));
        }
    }

    pub fn add(&self, task: Arc<Task>) -> Result<(), Errno> {
        let mut state = self.write();
        if !state.is_running() {
            if state.tasks_count() == 0 {
                log_warn!(
                    "Task {} with leader {} not running while adding its first task, \
                not sending creation notification",
                    task.tid,
                    self.leader
                );
            }
            return error!(EINVAL);
        }
        state.tasks.insert(task.persistent_info.clone());

        Ok(())
    }

    /// Remove the task from the children of this ThreadGroup.
    ///
    /// It is important that the task is taken as an `Arc`. It ensures the tasks of the
    /// ThreadGroup are always valid as they are still valid when removed.
    pub fn remove(&self, mut pids: PidTableGuard<'_>, task: &Arc<Task>) {
        task.set_ptrace_zombie(&mut pids);
        pids.remove_task(&task.tid);

        let mut state = self.write();

        if !state.tasks.remove(&task.persistent_info) {
            // The task has never been added. The only expected case is that this thread group
            // is not running.
            debug_assert!(!state.is_running());
            return;
        }

        if state.tasks.is_empty() {
            let exit_status = if let ThreadGroupRunState::Exiting(exit_status) = &state.run_state {
                exit_status.clone()
            } else {
                let exit_status = task.exit_status().unwrap_or_else(|| {
                    log_error!("Exiting without an exit code.");
                    ExitStatus::Exit(u8::MAX)
                });
                state.set_exiting(exit_status.clone());
                exit_status
            };

            // Detach from any ptraced zombie tasks.
            let zombie_notifications = state.zombie_ptracees.detach_all(&mut pids);

            // Replace PID table entry with a zombie.
            let exit_info =
                ProcessExitInfo { status: exit_status, exit_signal: state.exit_signal.clone() };
            let zombie =
                ZombieProcess::new(state.as_ref(), &task.persistent_info.real_creds(), exit_info);
            pids.kill_process(&self.leader);

            let session = state.leave_process_group(&mut pids);

            // I have no idea if dropping the lock here is correct, and I don't want to think about
            // it. If problems do turn up with another thread observing an intermediate state of
            // this exit operation, the solution is to unify locks. It should be sensible and
            // possible for there to be a single lock that protects all (or nearly all) of the
            // data accessed by both exit and wait. In gvisor and linux this is the lock on the
            // equivalent of the PidTable. This is made more difficult by rust locks being
            // containers that only lock the data they contain, but see
            // https://docs.google.com/document/d/1YHrhBqNhU1WcrsYgGAu3JwwlVmFXPlwWHTJLAbwRebY/edit
            // for an idea.
            std::mem::drop(state);

            // `disassociate_controlling_terminal` can not be called while holding the
            // ThreadGroup state lock.
            session.disassociate_controlling_terminal();

            for notification in zombie_notifications {
                notification.deliver(&mut pids);
            }

            // Remove the process from the cgroup2 pid table after TG lock is dropped.
            // This function will hold the CgroupState lock which should be before the TG lock. See
            // more in lock_cgroup2_pid_table comments.
            self.kernel.cgroups.lock_cgroup2_pid_table().remove_process(&self.leader);

            self.detach_ptracees(&mut pids);

            // We will need the immediate parent and the reaper. Once we have them, we can make
            // sure to take the locks in the right order: parent before child.
            let parent = self.read().parent.clone();
            let reaper = self.find_reaper();

            {
                // Reparent the children.
                if let Some(reaper) = reaper {
                    let reaper = reaper.upgrade();
                    {
                        let mut reaper_state = reaper.write();
                        // This allow_subclass is safe because we lock the reaper (an ancestor)
                        // before locking `self` and its children. Lock ordering follows
                        // strictly top-down traversal in the process tree, avoiding cycles.
                        let _token = allow_subclass();
                        let mut state = self.write();
                        for (_pid, weak_child) in std::mem::take(&mut state.children) {
                            if let Some(child) = weak_child.upgrade() {
                                // This allow_subclass is safe because we lock the reaper (an
                                // ancestor) before locking `self` and its children. Lock ordering
                                // follows strictly top-down traversal in the process tree, avoiding
                                // cycles.
                                let _token = allow_subclass();
                                let mut child_state = child.write();

                                child_state.exit_signal = Some(SIGCHLD);
                                child_state.parent =
                                    Some(ThreadGroupParent::new(Arc::downgrade(&reaper)));
                                reaper_state.children.insert(child.leader.id, weak_child);
                            }
                        }
                        reaper_state.zombie_children.append(&mut state.zombie_children);
                    }
                    ZombiePtracees::reparent(self, &reaper);
                } else {
                    // If we don't have a reaper then just drop the zombies.
                    let mut state = self.write();
                    for zombie in state.zombie_children.drain(..) {
                        zombie.release(&mut pids);
                    }
                }
            }

            // Clear the `parent` reference now that children have been re-`parent`ed.
            self.write().parent = None;

            #[cfg(any(test, debug_assertions))]
            {
                let state = self.read();
                assert!(state.zombie_children.is_empty());
                assert!(state.zombie_ptracees.is_empty());
            }

            if let Some(ref parent) = parent {
                let parent = parent.upgrade();

                let tracer_tg = task
                    .read()
                    .ptrace
                    .as_ref()
                    .and_then(|ptrace| ptrace.core_state.thread_group.upgrade());

                let maybe_zombie = match tracer_tg {
                    Some(tracer_tg) => {
                        tracer_tg.maybe_notify_tracer(task, &mut pids, &parent, zombie)
                    }
                    None => Some(zombie),
                };

                if let Some(zombie) = maybe_zombie {
                    parent.do_zombie_notifications(zombie, &mut pids);
                }
            } else {
                zombie.release(&mut pids);
            }

            // TODO: Set the error_code on the Zircon process object. Currently missing a way
            // to do this in Zircon. Might be easier in the new execution model.

            // Once the last zircon thread stops, the zircon process will also stop executing.

            if let Some(parent) = parent {
                let parent = parent.upgrade();
                parent.check_orphans(&pids);
            }

            self.write().set_exited();
        }
    }

    /// Detach from any ptraced tasks, killing the ones that set `PTRACE_O_EXITKILL`.
    fn detach_ptracees(&self, pids: &mut PidTableGuard<'_>) {
        let tracee_tids = self.ptracees.lock().iter().map(|info| info.tid.clone()).collect_vec();
        for tracee_tid in tracee_tids {
            let Ok(tracee) = tracee_tid.get_task() else {
                continue;
            };

            let mut should_send_sigkill = false;
            if let Some(ptrace) = &tracee.read().ptrace {
                should_send_sigkill = ptrace.has_option(PtraceOptions::EXITKILL);
            }
            if should_send_sigkill {
                send_standard_signal(tracee.as_ref(), SignalInfo::kernel(SIGKILL));
            }

            let _ = ptrace_detach(
                pids,
                PtraceTracer::Exiting(self),
                tracee.as_ref(),
                &UserAddress::NULL,
            );
        }
    }

    pub fn do_zombie_notifications(
        &self,
        zombie: OwnedRef<ZombieProcess>,
        pids: &mut PidTableGuard<'_>,
    ) {
        let mut state = self.write();

        state.children.remove(&zombie.pid());
        state.deferred_zombie_ptracers.retain(|dzp| dzp.tracee_pid != zombie.pid);

        let exit_signal = zombie.exit_info.exit_signal;
        let mut signal_info = zombie.to_wait_result().as_signal_info();

        // From https://man7.org/linux/man-pages/man2/sigaction.2.html
        //
        // > SA_NOCLDWAIT (since Linux 2.6)
        // >
        // >     If signum is SIGCHLD, do not transform children into
        // >     zombies when they terminate.  See also waitpid(2).  This
        // >     flag is meaningful only when establishing a handler for
        // >     SIGCHLD, or when setting that signal's disposition to
        // >     SIG_DFL.
        let should_make_zombie = if exit_signal == Some(SIGCHLD) {
            let action = self.signal_actions.get(SIGCHLD);
            action.sa_handler != SIG_IGN && (action.sa_flags & SA_NOCLDWAIT as u64) == 0
        } else {
            true
        };
        if should_make_zombie {
            state.zombie_children.push(zombie);
        } else {
            state.reap_zombie(zombie, pids);
        }

        state.lifecycle_waiters.notify_value(ThreadGroupLifecycleWaitValue::ChildStatus);

        // Send signals
        if let Some(exit_signal) = exit_signal {
            signal_info.signal = exit_signal;
            state.send_signal(signal_info);
        }
    }

    /// Notifies the tracer if appropriate.  Returns Some(zombie) if caller
    /// needs to notify the parent, None otherwise.  The caller should probably
    /// invoke parent.do_zombie_notifications(zombie) on the result.
    fn maybe_notify_tracer(
        &self,
        tracee: &Task,
        pids: &mut PidTableGuard<'_>,
        parent: &ThreadGroup,
        zombie: OwnedRef<ZombieProcess>,
    ) -> Option<OwnedRef<ZombieProcess>> {
        let mut state = self.write();
        if state.zombie_ptracees.has_tracee(&tracee.tid) {
            if self == parent {
                // The tracer is the parent and has not consumed the
                // notification.  Detach to clean up ptrace state, then
                // notify the parent by discarding the notification
                // generated by the detach and returning Some(zombie).
                if let Some(zombie_notification) = state.zombie_ptracees.detach(pids, &tracee.tid) {
                    zombie_notification.discard(pids);
                }
                return Some(zombie);
            } else {
                // The tracer is not the parent and the tracer has not consumed
                // the notification.
                if !state.is_running() {
                    // The tracer exited concurrently. Notify the parent.
                    return Some(zombie);
                }

                // THREAD SAFETY: Release the tracer state lock before acquiring the parent state
                // lock to respect parent => child lock ordering.
                drop(state);
                {
                    // Tell the parent to expect a notification later.
                    let tracee_pgid = tracee.thread_group().read().process_group.leader.clone();
                    let mut parent_state = parent.write();
                    parent_state.deferred_zombie_ptracers.push(DeferredZombiePTracer::new(
                        self,
                        tracee,
                        tracee_pgid,
                    ));
                    parent_state.children.remove(&tracee.get_pid());
                }

                // Tell the tracer that there is a notification pending.
                // THREAD SAFETY: Checking for concurrent exit with is_running(), releasing the
                // tracer state lock, then reacquiring the lock introduces a TOCTOU race. This
                // hazard is safe because exit synchronizes on the PidTable lock, which is held
                // continuously.
                let mut state = self.write();
                state.zombie_ptracees.set_parent_of(&tracee.tid, Some(zombie), parent);
                tracee.write().notify_ptracers();
                return None;
            }
        } else if self == parent {
            // The tracer is the parent and has already consumed the parent
            // notification.  No further action required.
            parent.write().children.remove(&tracee.tid.id);
            zombie.release(pids);
            return None;
        }
        // The tracer is not the parent and has already consumed the parent
        // notification.  Notify the parent.
        Some(zombie)
    }

    /// Find the task which will adopt our children after we die.
    fn find_reaper(&self) -> Option<ThreadGroupParent> {
        let mut weak_parent = self.read().parent.clone()?;
        loop {
            weak_parent = {
                let parent = weak_parent.upgrade();
                let parent_state = parent.read();
                if parent_state.is_child_subreaper {
                    break;
                }
                match parent_state.parent {
                    Some(ref next_parent) => next_parent.clone(),
                    None => break,
                }
            };
        }
        Some(weak_parent)
    }

    pub fn setsid(&self) -> Result<(), Errno> {
        let mut pids = self.kernel.pids.lock();
        let pid = self.leader.clone();
        if pid.get_process_group().is_ok() {
            return error!(EPERM);
        }
        let process_group = ProcessGroup::new(pid, None);
        pids.add_process_group(&process_group);
        let session = self.write().set_process_group(process_group, &mut pids);
        session.disassociate_controlling_terminal();
        self.check_orphans(&pids);

        Ok(())
    }

    pub fn setpgid(
        &self,
        current_task: &CurrentTask,
        target: &Task,
        pgid: &Pid,
    ) -> Result<(), Errno> {
        let mut pids = self.kernel.pids.lock();

        {
            let current_process_group = Arc::clone(&self.read().process_group);

            // The target process must be either the current process of a child of the current process
            let mut target_thread_group = target.thread_group().write();
            let is_target_current_process_child = target_thread_group
                .parent
                .as_ref()
                .is_some_and(|tg| tg.upgrade().leader == self.leader);
            if target_thread_group.base.leader != self.leader && !is_target_current_process_child {
                return error!(ESRCH);
            }

            // If the target process is a child of the current task, it must not have executed one of the exec
            // function.
            if is_target_current_process_child && target_thread_group.did_exec {
                return error!(EACCES);
            }

            let new_process_group;
            {
                let target_process_group = &target_thread_group.process_group;

                // The target process must not be a session leader and must be in the same session as the current process.
                if target_thread_group.base.leader == target_process_group.session.leader
                    || current_process_group.session != target_process_group.session
                {
                    return error!(EPERM);
                }

                if *pgid == target_process_group.leader {
                    return Ok(());
                }

                // If the process group already exists, join it. Both process groups must be in the
                // same session.
                if let Ok(process_group) = pgid.get_process_group() {
                    if process_group.session != target_process_group.session {
                        return error!(EPERM);
                    }
                    security::check_setpgid_access(current_task, target)?;
                    new_process_group = process_group;
                } else if *pgid == target_thread_group.base.leader {
                    security::check_setpgid_access(current_task, target)?;
                    // Create a new process group.
                    new_process_group = ProcessGroup::new(
                        target_thread_group.base.leader.clone(),
                        Some(target_process_group.session.clone()),
                    );
                    pids.add_process_group(&new_process_group);
                } else {
                    return error!(EPERM);
                }
            }

            let session = target_thread_group.set_process_group(new_process_group, &mut pids);
            std::mem::drop(target_thread_group);
            // `disassociate_controlling_terminal` can not be called while holding the
            // ThreadGroup state lock.
            session.disassociate_controlling_terminal();
        }

        target.thread_group().check_orphans(&pids);

        Ok(())
    }

    fn itimer_real(&self) -> IntervalTimerHandle {
        self.timers.itimer_real()
    }

    pub fn set_itimer(
        &self,
        current_task: &CurrentTask,
        which: u32,
        value: itimerval,
    ) -> Result<itimerval, Errno> {
        if which == ITIMER_PROF || which == ITIMER_VIRTUAL {
            // We don't support setting these timers.
            // The gvisor test suite clears ITIMER_PROF as part of its test setup logic, so we support
            // clearing these values.
            if value.it_value.tv_sec == 0 && value.it_value.tv_usec == 0 {
                return Ok(itimerval::default());
            }
            track_stub!(TODO("https://fxbug.dev/322874521"), "Unsupported itimer type", which);
            return error!(ENOTSUP);
        }

        if which != ITIMER_REAL {
            return error!(EINVAL);
        }
        let itimer_real = self.itimer_real();
        let prev_remaining = itimer_real.time_remaining();
        if value.it_value.tv_sec != 0 || value.it_value.tv_usec != 0 {
            itimer_real.arm(current_task, itimerspec_from_itimerval(value), false)?;
        } else {
            itimer_real.disarm(current_task)?;
        }
        Ok(itimerval {
            it_value: timeval_from_duration(prev_remaining.remainder),
            it_interval: timeval_from_duration(prev_remaining.interval),
        })
    }

    pub fn get_itimer(&self, which: u32) -> Result<itimerval, Errno> {
        if which == ITIMER_PROF || which == ITIMER_VIRTUAL {
            // We don't support setting these timers, so we can accurately report that these are not set.
            return Ok(itimerval::default());
        }
        if which != ITIMER_REAL {
            return error!(EINVAL);
        }
        let remaining = self.itimer_real().time_remaining();
        Ok(itimerval {
            it_value: timeval_from_duration(remaining.remainder),
            it_interval: timeval_from_duration(remaining.interval),
        })
    }

    /// Check whether the stop state is compatible with `new_stopped`. If it is return it,
    /// otherwise, return None.
    fn check_stopped_state(
        &self,
        new_stopped: StopState,
        finalize_only: bool,
    ) -> Option<StopState> {
        let stopped = self.load_stopped();
        if finalize_only && !stopped.is_stopping_or_stopped() {
            return Some(stopped);
        }

        if stopped.is_illegal_transition(new_stopped) {
            return Some(stopped);
        }

        return None;
    }

    /// Set the stop status of the process.  If you pass |siginfo| of |None|,
    /// does not update the signal.  If |finalize_only| is set, will check that
    /// the set will be a finalize (Stopping -> Stopped or Stopped -> Stopped)
    /// before executing it.
    ///
    /// Returns the latest stop state after any changes.
    pub fn set_stopped(
        &self,
        new_stopped: StopState,
        siginfo: Option<SignalInfo>,
        finalize_only: bool,
    ) -> StopState {
        // Perform an early return check to see if we can avoid taking the lock.
        if let Some(stopped) = self.check_stopped_state(new_stopped, finalize_only) {
            return stopped;
        }

        self.write().set_stopped(new_stopped, siginfo, finalize_only)
    }

    /// Ensures |session| is the controlling session inside of |terminal_controller|, and returns a
    /// reference to the |TerminalController|.
    fn check_terminal_controller(
        session: &Arc<Session>,
        terminal_controller: &Option<TerminalController>,
    ) -> Result<(), Errno> {
        if let Some(terminal_controller) = terminal_controller {
            if let Some(terminal_session) = terminal_controller.session.upgrade() {
                if Arc::ptr_eq(session, &terminal_session) {
                    return Ok(());
                }
            }
        }
        error!(ENOTTY)
    }

    pub fn get_foreground_process_group(&self, terminal: &Terminal) -> Result<pid_t, Errno> {
        let state = self.read();
        let process_group = &state.process_group;
        let terminal_state = terminal.read();

        // "When fd does not refer to the controlling terminal of the calling
        // process, -1 is returned" - tcgetpgrp(3)
        Self::check_terminal_controller(&process_group.session, &terminal_state.controller)?;
        let pid = process_group.session.read().get_foreground_process_group_leader().id;
        Ok(pid)
    }

    pub fn set_foreground_process_group(
        &self,
        current_task: &CurrentTask,
        terminal: &Terminal,
        pgid: &Pid,
    ) -> Result<(), Errno> {
        let process_group;
        let send_ttou;
        {
            // Keep locks to ensure atomicity.
            let state = self.read();
            process_group = Arc::clone(&state.process_group);
            let terminal_state = terminal.read();
            Self::check_terminal_controller(&process_group.session, &terminal_state.controller)?;

            let new_process_group = pgid.get_process_group()?;
            if new_process_group.session != process_group.session {
                return error!(EPERM);
            }

            let mut session_state = process_group.session.write();
            // If the calling process is a member of a background group and not ignoring SIGTTOU, a
            // SIGTTOU signal is sent to all members of this background process group.
            send_ttou = &process_group.leader
                != session_state.get_foreground_process_group_leader()
                && !current_task.read().signal_mask().has_signal(SIGTTOU)
                && self.signal_actions.get(SIGTTOU).sa_handler != SIG_IGN;

            if !send_ttou {
                session_state.set_foreground_process_group(pgid);
            }
        }

        // Locks must not be held when sending signals.
        if send_ttou {
            process_group.send_signals(&[SIGTTOU]);
            return error!(EINTR);
        }

        Ok(())
    }

    pub fn set_controlling_terminal(
        &self,
        current_task: &CurrentTask,
        terminal: &Terminal,
        is_main: bool,
        steal: bool,
        is_readable: bool,
    ) -> Result<(), Errno> {
        // Keep locks to ensure atomicity.
        let state = self.read();
        let process_group = &state.process_group;
        let mut terminal_state = terminal.write();

        // It might be necessary to lock the existing session, to steal the terminal
        // for it. Because of ordering requirement, it must be locked now.
        let other_session = terminal_state.controller.as_ref().and_then(|cs| cs.session.upgrade());
        let (mut session_writer, other_session) =
            if let Some(other_session) = other_session.as_ref() {
                if *other_session == process_group.session {
                    (process_group.session.mutable_state.write(), None)
                } else {
                    let (session_writer, other_session_writer) = ordered_write_lock(
                        &process_group.session.mutable_state,
                        &other_session.mutable_state,
                    );
                    (session_writer, Some((other_session, other_session_writer)))
                }
            } else {
                (process_group.session.mutable_state.write(), None)
            };

        // "The calling process must be a session leader and not have a
        // controlling terminal already." - tty_ioctl(4)
        if process_group.session.leader != self.leader {
            return error!(EINVAL);
        }
        if let Some(ref current_ct) = session_writer.controlling_terminal {
            if current_ct.matches(terminal, is_main) {
                return Ok(());
            } else {
                return error!(EINVAL);
            }
        }

        let mut has_admin_capability_determined = false;

        // "If this terminal is already the controlling terminal of a different
        // session group, then the ioctl fails with EPERM, unless the caller
        // has the CAP_SYS_ADMIN capability and arg equals 1, in which case the
        // terminal is stolen, and all processes that had it as controlling
        // terminal lose it." - tty_ioctl(4)
        if let Some((other_session, mut other_session_writer)) = other_session {
            debug_assert!(*other_session != process_group.session);
            if !steal {
                return error!(EPERM);
            }
            security::check_task_capable(current_task, CAP_SYS_ADMIN)?;
            has_admin_capability_determined = true;

            // Steal the TTY away. Unlike TIOCNOTTY, don't send signals.
            other_session_writer.controlling_terminal = None;
        }

        if !is_readable && !has_admin_capability_determined {
            security::check_task_capable(current_task, CAP_SYS_ADMIN)?;
        }

        session_writer.controlling_terminal = Some(ControllingTerminal::new(terminal, is_main));
        terminal_state.controller = TerminalController::new(&process_group.session);
        Ok(())
    }

    pub fn release_controlling_terminal(
        &self,
        _current_task: &CurrentTask,
        terminal: &Terminal,
        is_main: bool,
    ) -> Result<(), Errno> {
        let process_group;
        {
            // Keep locks to ensure atomicity.
            let state = self.read();
            process_group = Arc::clone(&state.process_group);
            let mut terminal_state = terminal.write();
            let mut session_writer = process_group.session.write();

            // tty must be the controlling terminal.
            Self::check_terminal_controller(&process_group.session, &terminal_state.controller)?;
            if !session_writer
                .controlling_terminal
                .as_ref()
                .map_or(false, |ct| ct.matches(terminal, is_main))
            {
                return error!(ENOTTY);
            }

            // "If the process was session leader, then send SIGHUP and SIGCONT to the foreground
            // process group and all processes in the current session lose their controlling terminal."
            // - tty_ioctl(4)

            // Remove tty as the controlling tty for each process in the session, then
            // send them SIGHUP and SIGCONT.

            session_writer.controlling_terminal = None;
            terminal_state.controller = None;
        }

        if process_group.session.leader == self.leader {
            process_group.send_signals(&[SIGHUP, SIGCONT]);
        }

        Ok(())
    }

    fn check_orphans(&self, pids: &PidTable) {
        let mut thread_groups = self.read().children().collect::<Vec<_>>();
        let this = self.weak_self.upgrade().unwrap();
        thread_groups.push(this);
        let process_groups =
            thread_groups.iter().map(|tg| Arc::clone(&tg.read().process_group)).unique();
        for pg in process_groups {
            pg.check_orphaned(pids);
        }
    }

    pub fn get_rlimit(&self, resource: Resource) -> u64 {
        self.limits.lock().get(resource).rlim_cur
    }

    /// Adjusts the rlimits of the ThreadGroup to which `target_task` belongs to.
    pub fn adjust_rlimits(
        current_task: &CurrentTask,
        target_task: &Task,
        resource: Resource,
        maybe_new_limit: Option<rlimit>,
    ) -> Result<rlimit, Errno> {
        let thread_group = target_task.thread_group();
        let mut limit_state = thread_group.limits.lock();
        let old_limit = limit_state.get(resource);
        if let Some(new_limit) = maybe_new_limit {
            if new_limit.rlim_max > old_limit.rlim_max
                && !security::is_task_capable_noaudit(current_task, CAP_SYS_RESOURCE)
            {
                return error!(EPERM);
            }
            security::task_setrlimit(current_task, &target_task, old_limit, new_limit)?;
            limit_state.set(resource, new_limit)
        }
        Ok(old_limit)
    }

    pub fn time_stats(&self) -> TaskTimeStats {
        let process: &zx::Process = if self.process.as_handle_ref().is_invalid() {
            // `process` must be valid for all tasks, except `kthreads`. In that case get the
            // stats from starnix process.
            assert_eq!(
                self as *const ThreadGroup,
                Arc::as_ptr(&self.kernel.kthreads.system_thread_group())
            );
            &self.kernel.kthreads.starnix_process
        } else {
            &self.process
        };

        let info =
            zx::Task::get_runtime_info(process).expect("Failed to get starnix process stats");
        TaskTimeStats {
            user_time: zx::MonotonicDuration::from_nanos(info.cpu_time),
            // TODO(https://fxbug.dev/42078242): How can we calculate system time?
            system_time: zx::MonotonicDuration::default(),
        }
    }

    /// For each task traced by this thread_group that matches the given
    /// selector, acquire its TaskMutableState and ptracees lock and execute the
    /// given function.
    pub fn get_ptracees_and(
        &self,
        selector: &ProcessSelector,
        f: &mut dyn FnMut(&Task, &TaskMutableState),
    ) {
        for task_ref in self
            .ptracees
            .lock()
            .iter()
            .filter(|info| selector.match_tid(&info.tid))
            .filter_map(|info| info.tid.get_task().ok())
        {
            let task_state = task_ref.write();
            if task_state.ptrace.is_some() {
                f(&task_ref, &task_state);
            }
        }
    }

    /// Returns a tracee whose state has changed, so that waitpid can report on
    /// it. If this returns a value, and the pid is being traced, the tracer
    /// thread is deemed to have seen the tracee ptrace-stop for the purposes of
    /// PTRACE_LISTEN.
    pub fn get_waitable_ptracee(
        &self,
        selector: &ProcessSelector,
        options: &WaitingOptions,
        pids: &mut PidTableGuard<'_>,
    ) -> Option<WaitResult> {
        // This checks to see if the target is a zombie ptracee.
        let waitable_entry = self.write().zombie_ptracees.get_waitable_entry(selector, options);
        match waitable_entry {
            None => (),
            Some((zombie, None)) => return Some(zombie.to_wait_result()),
            Some((zombie, Some((tg, z)))) => {
                if let Some(tg) = tg.upgrade() {
                    if Arc::as_ptr(&tg) != self as *const Self {
                        tg.do_zombie_notifications(z, pids);
                    } else {
                        {
                            let mut state = tg.write();
                            state.children.remove(&z.pid());
                            state.deferred_zombie_ptracers.retain(|dzp| dzp.tracee_pid != z.pid);
                        }

                        z.release(pids);
                    };
                }
                return Some(zombie.to_wait_result());
            }
        }

        let mut tasks = vec![];

        // This checks to see if the target is a running ptracee.
        self.get_ptracees_and(selector, &mut |task: &Task, _| {
            tasks.push(task.weak_self.clone());
        });
        for task in tasks {
            let Some(task_ref) = task.upgrade() else {
                continue;
            };

            let process_state = &mut task_ref.thread_group().write();
            let mut task_state = task_ref.write();
            if task_state
                .ptrace
                .as_ref()
                .is_some_and(|ptrace| ptrace.is_waitable(task_ref.load_stopped(), options))
            {
                // We've identified a potential target.  Need to return either
                // the process's information (if we are in group-stop) or the
                // thread's information (if we are in a different stop).

                // The shared information:
                let info = process_state.tasks.iter().next().unwrap().clone();
                let uid = info.real_creds().uid;
                let mut exit_status = None;
                let exit_signal = process_state.exit_signal.clone();
                let time_stats =
                    process_state.base.time_stats() + process_state.children_time_stats;
                let task_stopped = task_ref.load_stopped();

                #[derive(PartialEq)]
                enum ExitType {
                    None,
                    Cont,
                    Stop,
                    Kill,
                }
                if process_state.is_waitable() {
                    let ptrace = &mut task_state.ptrace;
                    // The information for processes, if we were in group stop.
                    let process_stopped = process_state.base.load_stopped();
                    let mut fn_type = ExitType::None;
                    if process_stopped == StopState::Awake && options.wait_for_continued {
                        fn_type = ExitType::Cont;
                    }
                    let mut event = ptrace
                        .as_ref()
                        .map_or(PtraceEvent::None, |ptrace| {
                            ptrace.event_data.as_ref().map_or(PtraceEvent::None, |data| data.event)
                        })
                        .clone();
                    // Tasks that are ptrace'd always get stop notifications.
                    if process_stopped == StopState::GroupStopped
                        && (options.wait_for_stopped || ptrace.is_some())
                    {
                        fn_type = ExitType::Stop;
                    }
                    if fn_type != ExitType::None {
                        let siginfo = if options.keep_waitable_state {
                            process_state.last_signal.clone()
                        } else {
                            process_state.last_signal.take()
                        };
                        if let Some(mut siginfo) = siginfo {
                            if task_ref.thread_group().load_stopped() == StopState::GroupStopped
                                && ptrace.as_ref().is_some_and(|ptrace| ptrace.is_seized())
                            {
                                if event == PtraceEvent::None {
                                    event = PtraceEvent::Stop;
                                }
                                siginfo.code |= (PtraceEvent::Stop as i32) << 8;
                            }
                            if siginfo.signal == SIGKILL {
                                fn_type = ExitType::Kill;
                            }
                            exit_status = match fn_type {
                                ExitType::Stop => Some((
                                    ExitStatus::Stop(siginfo, event),
                                    process_state.base.leader.clone(),
                                )),
                                ExitType::Cont => Some((
                                    ExitStatus::Continue(siginfo, event),
                                    process_state.base.leader.clone(),
                                )),
                                ExitType::Kill => Some((
                                    ExitStatus::Kill(siginfo),
                                    process_state.base.leader.clone(),
                                )),
                                _ => None,
                            };
                        }
                        // Clear the wait status of the ptrace, because we're
                        // using the tg status instead.
                        ptrace
                            .as_mut()
                            .map(|ptrace| ptrace.get_last_signal(options.keep_waitable_state));
                    }
                }
                if exit_status.is_none() {
                    if let Some(ptrace) = task_state.ptrace.as_mut() {
                        // The information for the task, if we were in a non-group stop.
                        let mut fn_type = ExitType::None;
                        let event = ptrace
                            .event_data
                            .as_ref()
                            .map_or(PtraceEvent::None, |event| event.event);
                        if task_stopped == StopState::Awake {
                            fn_type = ExitType::Cont;
                        }
                        if task_stopped.is_stopping_or_stopped()
                            || ptrace.stop_status == PtraceStatus::Listening
                        {
                            fn_type = ExitType::Stop;
                        }
                        if fn_type != ExitType::None {
                            if let Some(siginfo) =
                                ptrace.get_last_signal(options.keep_waitable_state)
                            {
                                if siginfo.signal == SIGKILL {
                                    fn_type = ExitType::Kill;
                                }
                                exit_status = match fn_type {
                                    ExitType::Stop => Some((
                                        ExitStatus::Stop(siginfo, event),
                                        task_ref.tid.clone(),
                                    )),
                                    ExitType::Cont => Some((
                                        ExitStatus::Continue(siginfo, event),
                                        task_ref.tid.clone(),
                                    )),
                                    ExitType::Kill => {
                                        Some((ExitStatus::Kill(siginfo), task_ref.tid.clone()))
                                    }
                                    _ => None,
                                };
                            }
                        }
                    }
                }
                if let Some((exit_status, pid)) = exit_status {
                    return Some(WaitResult {
                        pid,
                        uid,
                        exit_info: ProcessExitInfo { status: exit_status, exit_signal },
                        time_stats,
                    });
                }
            }
        }
        None
    }

    /// Attempts to send an unchecked signal to this thread group.
    ///
    /// - `current_task`: The task that is sending the signal.
    /// - `unchecked_signal`: The signal that is to be sent. Unchecked, since `0` is a sentinel value
    /// where rights are to be checked but no signal is actually sent.
    ///
    /// # Returns
    /// Returns Ok(()) if the signal was sent, or the permission checks passed with a 0 signal, otherwise
    /// the error that was encountered.
    pub fn send_signal_unchecked(
        &self,
        current_task: &CurrentTask,
        unchecked_signal: UncheckedSignal,
    ) -> Result<(), Errno> {
        if let Some(signal) = self.check_signal_access(current_task, unchecked_signal)? {
            let signal_info = SignalInfo::with_detail(
                signal,
                SI_USER as i32,
                SignalDetail::Kill {
                    pid: current_task.pid.clone(),
                    uid: current_task.current_creds().uid,
                },
            );

            self.write().send_signal(signal_info);
        }

        Ok(())
    }

    /// Sends a signal to this thread_group without performing any access checks.
    ///
    /// # Safety
    /// This is unsafe, because it should only be called by tools and tests.
    pub unsafe fn send_signal_unchecked_debug(
        &self,
        current_task: &CurrentTask,
        unchecked_signal: UncheckedSignal,
    ) -> Result<(), Errno> {
        let signal = Signal::try_from(unchecked_signal)?;
        let signal_info = SignalInfo::with_detail(
            signal,
            SI_USER as i32,
            SignalDetail::Kill {
                pid: current_task.pid.clone(),
                uid: current_task.current_creds().uid,
            },
        );

        self.write().send_signal(signal_info);
        Ok(())
    }

    /// Attempts to send an unchecked signal to this thread group, with info read from
    /// `siginfo_ref`.
    ///
    /// - `current_task`: The task that is sending the signal.
    /// - `unchecked_signal`: The signal that is to be sent. Unchecked, since `0` is a sentinel value
    /// where rights are to be checked but no signal is actually sent.
    /// - `siginfo_ref`: The siginfo that will be enqueued.
    /// - `options`: Options for how to convert the siginfo into a signal info.
    ///
    /// # Returns
    /// Returns Ok(()) if the signal was sent, or the permission checks passed with a 0 signal, otherwise
    /// the error that was encountered.
    #[track_caller]
    pub fn send_signal_unchecked_with_info(
        &self,
        current_task: &CurrentTask,
        unchecked_signal: UncheckedSignal,
        siginfo_ref: UserAddress,
        options: IntoSignalInfoOptions,
    ) -> Result<(), Errno> {
        let siginfo = UncheckedSignalInfo::read_from_siginfo(current_task, siginfo_ref)?;
        if self.leader.id != current_task.get_pid()
            && (siginfo.code() >= 0 || siginfo.code() == SI_TKILL)
        {
            return error!(EPERM);
        }

        if matches!(options, IntoSignalInfoOptions::CheckSigno)
            && siginfo.signo() as u64 != unchecked_signal.raw()
        {
            return error!(EINVAL);
        }

        if let Some(signal) = self.check_signal_access(current_task, unchecked_signal)? {
            self.write().send_signal(siginfo.into_signal_info(signal, options)?);
        }

        Ok(())
    }

    /// Checks whether or not `current_task` can signal this thread group with `unchecked_signal`.
    ///
    /// Returns:
    ///   - `Ok(Some(Signal))` if the signal passed checks and should be sent.
    ///   - `Ok(None)` if the signal passed checks, but should not be sent. This is used by
    ///   userspace for permission checks.
    ///   - `Err(_)` if the permission checks failed.
    fn check_signal_access(
        &self,
        current_task: &CurrentTask,
        unchecked_signal: UncheckedSignal,
    ) -> Result<Option<Signal>, Errno> {
        // Pick an arbitrary task in thread_group to check permissions.
        //
        // Tasks can technically have different credentials, but in practice they are kept in sync.
        let Some(target_task) = self.read().get_signalable_task() else {
            // If there are no signalable tasks in the thread group, all tasks have exited and the
            // process is terminating or a zombie. A signal sent to a terminating or zombie process
            // is ignored, matching the behavior for Zombie processes in sys_kill.
            return Ok(None);
        };
        current_task.can_signal(&target_task, unchecked_signal)?;

        // 0 is a sentinel value used to do permission checks.
        if unchecked_signal.is_zero() {
            return Ok(None);
        }

        let signal = Signal::try_from(unchecked_signal)?;
        security::check_signal_access(current_task, &target_task, signal)?;

        Ok(Some(signal))
    }

    pub fn has_signal_queued(&self, signal: Signal) -> bool {
        self.pending_signals.lock().has_queued(signal)
    }

    pub fn num_signals_queued(&self) -> usize {
        self.pending_signals.lock().num_queued()
    }

    pub fn get_pending_signals(&self) -> SigSet {
        self.pending_signals.lock().pending()
    }

    pub fn is_any_signal_allowed_by_mask(&self, mask: SigSet) -> bool {
        self.pending_signals.lock().is_any_allowed_by_mask(mask)
    }

    pub fn take_next_signal_where<F>(&self, predicate: F) -> Option<SignalInfo>
    where
        F: Fn(&SignalInfo) -> bool,
    {
        let mut signals = self.pending_signals.lock();
        let r = signals.take_next_where(predicate);
        self.has_pending_signals.store(!signals.is_empty(), Ordering::Relaxed);
        r
    }

    /// Drive this `ThreadGroup` to exit, allowing it time to handle SIGTERM before sending SIGKILL.
    ///
    /// Returns once `ThreadGroup::exit()` has completed.
    ///
    /// Must be called from the system task.
    pub async fn shut_down(this: Weak<Self>) {
        const SHUTDOWN_SIGNAL_HANDLING_TIMEOUT: zx::MonotonicDuration =
            zx::MonotonicDuration::from_seconds(1);

        // Prepare for shutting down the thread group.
        let (tg_name, mut on_exited) = {
            // Nest this upgraded access so upgraded references aren't held across await-points.
            let Some(this) = this.upgrade() else {
                return;
            };

            let mut state = this.write();
            if state.is_exited() {
                // Do not set an exit notifier on an exited thread group. It will never be notified.
                return;
            }

            // Register a channel to be notified when exit() is complete.
            let (on_exited_send, on_exited) = futures::channel::oneshot::channel();
            state.exit_notifier = Some(on_exited_send);

            // We want to be able to log about this thread group without upgrading the `Weak`.
            let tg_name = format!("{this:?}");

            (tg_name, on_exited)
        };

        log_debug!(tg:% = tg_name; "shutting down thread group, sending SIGTERM");
        this.upgrade().map(|tg| tg.write().send_signal(SignalInfo::kernel(SIGTERM)));

        // Give thread groups some time to handle SIGTERM, proceeding early if they exit
        let timeout = fuchsia_async::Timer::new(SHUTDOWN_SIGNAL_HANDLING_TIMEOUT);
        futures::pin_mut!(timeout);

        // Use select_biased instead of on_timeout() so that we can await on on_exited later
        futures::select_biased! {
            _ = &mut on_exited => (),
            _ = timeout => {
                log_debug!(tg:% = tg_name; "sending SIGKILL");
                this.upgrade().map(|tg| tg.write().send_signal(SignalInfo::kernel(SIGKILL)));
            },
        };

        log_debug!(tg:% = tg_name; "waiting for exit");
        // It doesn't matter whether ThreadGroup::exit() was called or the process exited with
        // a return code and dropped the sender end of the channel.
        on_exited.await.ok();
        log_debug!(tg:% = tg_name; "thread group shutdown complete");
    }

    /// Returns the KOID of the process for this thread group.
    /// This method should be used to when mapping 32 bit linux process ids to KOIDs
    /// to avoid breaking the encapsulation of the zx::process within the ThreadGroup.
    /// This encapsulation is important since the relationship between the ThreadGroup
    /// and the Process may change over time. See [ThreadGroup::process] for more details.
    pub fn get_process_koid(&self) -> Result<Koid, Status> {
        self.process.koid()
    }
}

pub enum WaitableChildResult {
    ReadyNow(Box<WaitResult>),
    ShouldWait,
    NoneFound,
}

#[apply(state_implementation!)]
impl ThreadGroupMutableState<Base = ThreadGroup> {
    pub fn leader(&self) -> pid_t {
        self.base.leader.id
    }

    pub fn leader_command(&self) -> TaskCommand {
        self.get_task(self.leader())
            .map(|l| l.command())
            .unwrap_or_else(|| TaskCommand::new(b"<leader exited>"))
    }

    pub fn is_running(&self) -> bool {
        matches!(self.run_state, ThreadGroupRunState::Running)
    }

    pub fn is_exited(&self) -> bool {
        matches!(self.run_state, ThreadGroupRunState::Exited(_))
    }

    fn set_exiting(&mut self, exit_status: ExitStatus) {
        self.run_state = ThreadGroupRunState::Exiting(exit_status);
    }

    fn set_exited(&mut self) {
        let ThreadGroupRunState::Exiting(exit_status) = std::mem::take(&mut self.run_state) else {
            panic!("Must transition from Exiting to Exited");
        };
        self.run_state = ThreadGroupRunState::Exited(exit_status);

        if let Some(notifier) = self.exit_notifier.take() {
            let _ = notifier.send(());
        }
    }

    pub fn children(&self) -> impl Iterator<Item = Arc<ThreadGroup>> + '_ {
        self.children.values().map(|v| {
            v.upgrade().expect("Weak references to processes in ThreadGroup must always be valid")
        })
    }

    pub fn tasks(&self) -> Vec<Arc<Task>> {
        self.tasks.iter().flat_map(|info| info.tid.get_task().ok()).collect()
    }

    pub fn task_ids(&self) -> impl Iterator<Item = tid_t> + '_ {
        self.tasks.iter().map(|info| info.tid.id)
    }

    pub fn contains_task(&self, tid: tid_t) -> bool {
        self.tasks.iter().any(|info| info.tid.id == tid)
    }

    pub fn get_task(&self, tid: tid_t) -> Option<Arc<Task>> {
        self.tasks.iter().find(|info| info.tid.id == tid).and_then(|info| info.tid.get_task().ok())
    }

    pub fn tasks_count(&self) -> usize {
        self.tasks.len()
    }

    pub fn get_ppid(&self) -> pid_t {
        match &self.parent {
            Some(parent) => parent.upgrade().leader.id,
            None => 0,
        }
    }

    /// Changes the process group of the thread group.
    ///
    /// Returns a `SessionDisassociation`, which the caller must use to explicitly
    /// disassociate the controlling terminal if the thread group was previously a session
    /// leader.
    /// This must be done after the ThreadGroup state lock is released to avoid lock order
    /// violations.
    fn set_process_group(
        &mut self,
        process_group: Arc<ProcessGroup>,
        pids: &mut PidTableGuard<'_>,
    ) -> SessionDisassociation {
        if self.process_group == process_group {
            return SessionDisassociation::new(None);
        }
        let session = self.leave_process_group(pids);
        self.process_group = process_group;
        self.process_group.insert(self.base);
        session
    }

    /// Removes the thread group from its current process group.
    ///
    /// Returns a `SessionDisassociation`, which the caller must use to explicitly
    /// disassociate the controlling terminal if the thread group was previously a session
    /// leader.
    /// This must be done after the ThreadGroup state lock is released to avoid lock order
    /// violations.
    fn leave_process_group(&mut self, pids: &mut PidTableGuard<'_>) -> SessionDisassociation {
        let (is_empty, disassociation) = self.process_group.remove(self.base);
        if is_empty {
            self.process_group.session.write().remove(&self.process_group.leader);
            pids.remove_process_group(&self.process_group.leader);
        }
        disassociation
    }

    /// Reaps the given zombie, making its PID available for reuse.
    fn reap_zombie(&mut self, zombie: OwnedRef<ZombieProcess>, pids: &mut PidTableGuard<'_>) {
        self.children_time_stats += zombie.time_stats;
        zombie.release(pids);
    }

    /// Indicates whether the thread group is waitable via waitid and waitpid for
    /// either WSTOPPED or WCONTINUED.
    pub fn is_waitable(&self) -> bool {
        return self.last_signal.is_some() && !self.base.load_stopped().is_in_progress();
    }

    pub fn get_waitable_zombie(
        &mut self,
        zombie_list: &dyn Fn(&mut ThreadGroupMutableState) -> &mut Vec<OwnedRef<ZombieProcess>>,
        selector: &ProcessSelector,
        options: &WaitingOptions,
        pids: &mut PidTableGuard<'_>,
    ) -> Option<WaitResult> {
        // We look for the last zombie in the vector that matches pid selector and waiting options
        let selected_zombie_position = zombie_list(self)
            .iter()
            .rev()
            .position(|zombie| zombie.matches_selector_and_waiting_option(selector, options))
            .map(|position_starting_from_the_back| {
                zombie_list(self).len() - 1 - position_starting_from_the_back
            });

        selected_zombie_position.map(|position| {
            if options.keep_waitable_state {
                zombie_list(self)[position].to_wait_result()
            } else {
                let zombie = zombie_list(self).remove(position);
                let result = zombie.to_wait_result();
                self.reap_zombie(zombie, pids);
                result
            }
        })
    }

    pub fn is_correct_exit_signal(for_clone: bool, exit_code: Option<Signal>) -> bool {
        for_clone == (exit_code != Some(SIGCHLD))
    }

    fn get_waitable_running_children(
        &self,
        selector: &ProcessSelector,
        options: &WaitingOptions,
    ) -> WaitableChildResult {
        // The children whose pid matches the pid selector queried.
        let filter_children_by_pid_selector = |child: &ThreadGroup| match selector {
            ProcessSelector::Any => true,
            ProcessSelector::Pid(pid) => &child.leader == pid,
            ProcessSelector::Pgid(pgid) => {
                // This allow_subclass is safe because the lock is being acquired
                // in a strictly top-down traversal of the ThreadGroup tree (from parent
                // to child), so no lock ordering cycles can be formed.
                let _token = allow_subclass();
                &child.read().process_group.leader == pgid
            }
        };

        // The children whose exit signal matches the waiting options queried.
        let filter_children_by_waiting_options = |child: &ThreadGroup| {
            if options.wait_for_all {
                return true;
            }
            // This allow_subclass is safe because the lock is being acquired
            // in a strictly top-down traversal of the ThreadGroup tree (from parent
            // to child), so no lock ordering cycles can be formed.
            let _token = allow_subclass();
            Self::is_correct_exit_signal(options.wait_for_clone, child.read().exit_signal)
        };

        // If wait_for_exited flag is disabled or no exited children were found we look for running
        // children.
        let mut selected_children = self
            .children
            .values()
            .map(|t| t.upgrade().unwrap())
            .filter(|tg| filter_children_by_pid_selector(&tg))
            .filter(|tg| filter_children_by_waiting_options(&tg))
            .peekable();
        if selected_children.peek().is_none() {
            // There still might be a process that ptrace hasn't looked at yet.
            if self.deferred_zombie_ptracers.iter().any(|dzp| match selector {
                ProcessSelector::Any => true,
                ProcessSelector::Pid(pid) => &dzp.tracee_pid == pid,
                ProcessSelector::Pgid(pgid) => &dzp.tracee_pgid == pgid,
            }) {
                return WaitableChildResult::ShouldWait;
            }

            return WaitableChildResult::NoneFound;
        }
        for child in selected_children {
            // This allow_subclass is safe because the lock is being acquired
            // in a strictly top-down traversal of the ThreadGroup tree (from parent
            // to child), so no lock ordering cycles can be formed.
            let _token = allow_subclass();
            let child = child.write();
            if child.last_signal.is_some() {
                let build_wait_result = |mut child: ThreadGroupWriteGuard<'_>,
                                         exit_status: &dyn Fn(SignalInfo) -> ExitStatus|
                 -> WaitResult {
                    let siginfo = if options.keep_waitable_state {
                        child.last_signal.clone().unwrap()
                    } else {
                        child.last_signal.take().unwrap()
                    };
                    let exit_status = if siginfo.signal == SIGKILL {
                        // This overrides the stop/continue choice.
                        ExitStatus::Kill(siginfo)
                    } else {
                        exit_status(siginfo)
                    };
                    let info = child.tasks.iter().next().unwrap();
                    let uid = info.real_creds().uid;
                    WaitResult {
                        pid: child.base.leader.clone(),
                        uid,
                        exit_info: ProcessExitInfo {
                            status: exit_status,
                            exit_signal: child.exit_signal,
                        },
                        time_stats: child.base.time_stats() + child.children_time_stats,
                    }
                };
                let child_stopped = child.base.load_stopped();
                if child_stopped == StopState::Awake && options.wait_for_continued {
                    return WaitableChildResult::ReadyNow(Box::new(build_wait_result(
                        child,
                        &|siginfo| ExitStatus::Continue(siginfo, PtraceEvent::None),
                    )));
                }
                if child_stopped == StopState::GroupStopped && options.wait_for_stopped {
                    return WaitableChildResult::ReadyNow(Box::new(build_wait_result(
                        child,
                        &|siginfo| ExitStatus::Stop(siginfo, PtraceEvent::None),
                    )));
                }
            }
        }

        WaitableChildResult::ShouldWait
    }

    /// Returns any waitable child matching the given `selector` and `options`. Returns None if no
    /// child matching the selector is waitable. Returns ECHILD if no child matches the selector at
    /// all.
    ///
    /// Will remove the waitable status from the child depending on `options`.
    pub fn get_waitable_child(
        &mut self,
        selector: &ProcessSelector,
        options: &WaitingOptions,
        pids: &mut PidTableGuard<'_>,
    ) -> WaitableChildResult {
        if options.wait_for_exited {
            if let Some(waitable_zombie) = self.get_waitable_zombie(
                &|state: &mut ThreadGroupMutableState| &mut state.zombie_children,
                selector,
                options,
                pids,
            ) {
                return WaitableChildResult::ReadyNow(Box::new(waitable_zombie));
            }
        }

        self.get_waitable_running_children(selector, options)
    }

    /// Returns a running task in the current thread group.
    pub fn get_running_task(&self) -> Result<Arc<Task>, Errno> {
        self.tasks
            .iter()
            .find_map(|info| info.tid.get_task().ok().filter(|task| task.is_running()))
            .ok_or_else(|| errno!(ESRCH))
    }

    /// Returns a task representative of the [`ThreadGroup`] for signal access checks.
    ///
    /// Prefers a running task, but falls back to the first available non-running task.
    /// Returns `None` if the task list is empty or no tasks can be upgraded.
    fn get_signalable_task(&self) -> Option<Arc<Task>> {
        let mut non_running = if let Ok(task) = self.base.leader.get_task() {
            if task.is_running() {
                return Some(task);
            }
            Some(task)
        } else {
            None
        };
        for container in &self.tasks {
            if let Ok(task) = container.tid.get_task() {
                if task.is_running() {
                    return Some(task);
                }
                if non_running.is_none() {
                    non_running = Some(task);
                }
            }
        }
        non_running
    }

    /// Set the stop status of the process.  If you pass |siginfo| of |None|,
    /// does not update the signal.  If |finalize_only| is set, will check that
    /// the set will be a finalize (Stopping -> Stopped or Stopped -> Stopped)
    /// before executing it.
    ///
    /// Returns the latest stop state after any changes.
    pub fn set_stopped(
        mut self,
        new_stopped: StopState,
        siginfo: Option<SignalInfo>,
        finalize_only: bool,
    ) -> StopState {
        if let Some(stopped) = self.base.check_stopped_state(new_stopped, finalize_only) {
            return stopped;
        }

        // Thread groups don't transition to group stop if they are waking, because waking
        // means something told it to wake up (like a SIGCONT) but hasn't finished yet.
        if self.base.load_stopped() == StopState::Waking
            && (new_stopped == StopState::GroupStopping || new_stopped == StopState::GroupStopped)
        {
            return self.base.load_stopped();
        }

        // TODO(https://g-issues.fuchsia.dev/issues/306438676): When thread
        // group can be stopped inside user code, tasks/thread groups will
        // need to be either restarted or stopped here.
        self.store_stopped(new_stopped);
        if let Some(signal) = &siginfo {
            // We don't want waiters to think the process was unstopped
            // because of a sigkill.  They will get woken when the
            // process dies.
            if signal.signal != SIGKILL {
                self.last_signal = siginfo;
            }
        }
        if new_stopped == StopState::Waking || new_stopped == StopState::ForceWaking {
            self.lifecycle_waiters.notify_value(ThreadGroupLifecycleWaitValue::Stopped);
        };

        let parent = (!new_stopped.is_in_progress()).then(|| self.parent.clone()).flatten();

        // Drop the lock before locking the parent.
        std::mem::drop(self);
        if let Some(parent) = parent {
            let parent = parent.upgrade();
            parent
                .write()
                .lifecycle_waiters
                .notify_value(ThreadGroupLifecycleWaitValue::ChildStatus);
        }

        new_stopped
    }

    fn store_stopped(&mut self, state: StopState) {
        // We don't actually use the guard but we require it to enforce that the
        // caller holds the thread group's mutable state lock (identified by
        // mutable access to the thread group's mutable state).

        self.base.stop_state.store(state, Ordering::Relaxed)
    }

    /// Sends the signal `signal_info` to this thread group.
    #[allow(unused_mut, reason = "needed for some but not all macro outputs")]
    pub fn send_signal(mut self, signal_info: SignalInfo) {
        let sigaction = self.base.signal_actions.get(signal_info.signal);
        let action = action_for_signal(&signal_info, sigaction);

        {
            let mut pending_signals = self.base.pending_signals.lock();
            pending_signals.enqueue(signal_info.clone());
            self.base.has_pending_signals.store(true, Ordering::Relaxed);
        }
        let tasks: Vec<Pid> = self.tasks.iter().map(|info| info.tid.clone()).collect();

        // Set state to waking before interrupting any tasks.
        if signal_info.signal == SIGKILL {
            self.set_stopped(StopState::ForceWaking, Some(signal_info.clone()), false);
        } else if signal_info.signal == SIGCONT {
            self.set_stopped(StopState::Waking, Some(signal_info.clone()), false);
        }

        let mut has_interrupted_task = false;
        for task in tasks.iter().flat_map(|pid| pid.get_task().ok()) {
            if !task.is_running() {
                continue;
            }

            let mut task_state = task.write();

            if signal_info.signal == SIGKILL {
                task_state.thaw();
                task_state.set_stopped(StopState::ForceWaking, None, None, None);
            } else if signal_info.signal == SIGCONT {
                task_state.set_stopped(StopState::Waking, None, None, None);
            }

            let is_masked = task_state.is_signal_masked(signal_info.signal);
            let was_masked = task_state.is_signal_masked_by_saved_mask(signal_info.signal);

            let is_queued = action != DeliveryAction::Ignore
                || is_masked
                || was_masked
                || task_state.is_ptraced();

            if is_queued {
                task_state.notify_signal_waiters(&signal_info.signal);

                let is_fatal = signal_info.signal == SIGKILL
                    || (action == DeliveryAction::Terminate && !task_state.is_ptraced());

                if !is_masked
                    && action.must_interrupt(Some(sigaction))
                    && (!has_interrupted_task || is_fatal)
                {
                    // Interrupt every task if the action is fatal (such as SIGKILL),
                    // or only one task for catchable signals.
                    drop(task_state);
                    task.interrupt();
                    has_interrupted_task = true;
                }
            }
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::testing::*;

    #[::fuchsia::test]
    async fn test_setsid() {
        spawn_kernel_and_run(async |current_task| {
            fn get_process_group(task: &Task) -> Arc<ProcessGroup> {
                Arc::clone(&task.thread_group().read().process_group)
            }
            assert_eq!(current_task.thread_group().setsid(), error!(EPERM));

            let child_task = current_task.clone_task_for_test(0, Some(SIGCHLD));
            assert_eq!(get_process_group(&current_task), get_process_group(&child_task));

            let old_process_group = child_task.thread_group().read().process_group.clone();
            assert_eq!(child_task.thread_group().setsid(), Ok(()));
            assert_eq!(
                child_task.thread_group().read().process_group.session.leader,
                child_task.pid
            );
            assert!(!old_process_group.read().thread_groups().contains(child_task.thread_group()));
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_exit_status() {
        spawn_kernel_and_run(async |current_task| {
            let child = current_task.clone_task_for_test(0, Some(SIGCHLD));
            child.thread_group().kill(ExitStatus::Exit(42), None);
            std::mem::drop(child);
            assert_eq!(
                current_task.thread_group().read().zombie_children[0].exit_info.status,
                ExitStatus::Exit(42)
            );
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_setgpid() {
        spawn_kernel_and_run(async |current_task| {
            assert_eq!(current_task.thread_group().setsid(), error!(EPERM));

            let child_task1 = current_task.clone_task_for_test(0, Some(SIGCHLD));
            let child_task2 = current_task.clone_task_for_test(0, Some(SIGCHLD));
            let execd_child_task = current_task.clone_task_for_test(0, Some(SIGCHLD));
            execd_child_task.thread_group().write().did_exec = true;
            let other_session_child_task = current_task.clone_task_for_test(0, Some(SIGCHLD));
            assert_eq!(other_session_child_task.thread_group().setsid(), Ok(()));

            assert_eq!(
                child_task1.thread_group().setpgid(&current_task, &current_task, &current_task.pid),
                error!(ESRCH)
            );
            assert_eq!(
                current_task.thread_group().setpgid(
                    &current_task,
                    &execd_child_task,
                    &execd_child_task.pid
                ),
                error!(EACCES)
            );
            assert_eq!(
                current_task.thread_group().setpgid(
                    &current_task,
                    &current_task,
                    &current_task.pid
                ),
                error!(EPERM)
            );
            assert_eq!(
                current_task.thread_group().setpgid(
                    &current_task,
                    &other_session_child_task,
                    &other_session_child_task.pid
                ),
                error!(EPERM)
            );
            assert_eq!(
                current_task.thread_group().setpgid(&current_task, &child_task1, &child_task2.pid),
                error!(EPERM)
            );
            assert_eq!(
                current_task.thread_group().setpgid(
                    &current_task,
                    &child_task1,
                    &other_session_child_task.pid
                ),
                error!(EPERM)
            );

            assert_eq!(
                child_task1.thread_group().setpgid(&current_task, &child_task1, &child_task1.pid),
                Ok(())
            );
            assert_eq!(
                child_task1.thread_group().read().process_group.session.leader,
                current_task.tid
            );
            assert_eq!(child_task1.thread_group().read().process_group.leader, child_task1.tid);

            let old_process_group = child_task2.thread_group().read().process_group.clone();
            assert_eq!(
                current_task.thread_group().setpgid(&current_task, &child_task2, &child_task1.pid),
                Ok(())
            );
            assert_eq!(child_task2.thread_group().read().process_group.leader, child_task1.tid);
            assert!(!old_process_group.read().thread_groups().contains(child_task2.thread_group()));

            let child_task3 = current_task.clone_task_for_test(0, Some(SIGCHLD));
            assert_eq!(
                child_task3.thread_group().setpgid(&current_task, &child_task3, &child_task3.pid),
                Ok(())
            );
            // Move child_task1 to child_task3's process group.
            assert_eq!(
                current_task.thread_group().setpgid(&current_task, &child_task1, &child_task3.pid),
                Ok(())
            );
            assert_eq!(child_task1.thread_group().read().process_group.leader, child_task3.tid);

            // Rejoin child_task1's original process group (which still contains child_task2).
            assert_eq!(
                child_task1.thread_group().setpgid(&current_task, &child_task1, &child_task1.pid),
                Ok(())
            );
            assert_eq!(child_task1.thread_group().read().process_group.leader, child_task1.tid);
            let pg1 = child_task1.thread_group().read().process_group.clone();
            let pg2 = child_task2.thread_group().read().process_group.clone();
            assert_eq!(pg1, pg2);

            assert_eq!(
                crate::task::syscalls::sys_setpgid(&current_task, child_task1.pid.id, -1),
                error!(EINVAL)
            );
            assert_eq!(
                crate::task::syscalls::sys_setpgid(&current_task, child_task1.pid.id, 255),
                error!(EPERM)
            );
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_adopt_children() {
        spawn_kernel_and_run(async |current_task| {
            let task1 = current_task.clone_task_for_test(0, None);
            let task2 = task1.clone_task_for_test(0, None);
            let task3 = task2.clone_task_for_test(0, None);

            assert_eq!(task3.thread_group().read().get_ppid(), task2.tid.id);

            task2.thread_group().kill(ExitStatus::Exit(0), None);
            std::mem::drop(task2);

            // Task3 parent should be current_task.
            assert_eq!(task3.thread_group().read().get_ppid(), current_task.tid.id);
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_getppid_after_self_and_parent_exit() {
        spawn_kernel_and_run(async |current_task| {
            let task1 = current_task.clone_task_for_test(0, None);
            let task2 = task1.clone_task_for_test(0, None);

            // Take strong references to the ThreadGroups.
            let tg1 = task1.thread_group().clone();
            let tg2 = task2.thread_group().clone();

            assert_eq!(tg1.read().get_ppid(), current_task.tid.id);
            assert_eq!(tg2.read().get_ppid(), task1.tid.id);

            // Exit `task2` first, so that when `task1` exits, it will not be reparented to init.
            tg2.kill(ExitStatus::Exit(0), None);
            std::mem::drop(task2);

            // Exit `task1`, and drop the task and ThreadGroup.
            tg1.kill(ExitStatus::Exit(0), None);
            std::mem::drop(task1);
            std::mem::drop(tg1);

            // It should still be valid to call `get_ppid()` on `tg2`, though is parent ThreadGroup
            // no longer exists.
            let _ = tg2.read().get_ppid();
        })
        .await;
    }
}
