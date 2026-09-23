// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::mm::{MemoryAccessor, MemoryAccessorExt, MemoryManager, TaskMemoryAccessor};
use crate::mutable_state::{state_accessor, state_implementation};
use crate::ptrace::{
    AtomicStopState, PtraceEvent, PtraceEventData, PtraceState, PtraceStatus, StopState,
};
use crate::signals::{KernelSignal, SignalDetail, SignalInfo, SignalState};
use crate::task::memory_attribution::MemoryAttributionLifecycleEvent;
use crate::task::run_state::RunState;
use crate::task::tracing::ZirconIdentity;
use crate::task::{
    AbstractUnixSocketNamespace, AbstractVsockSocketNamespace, CurrentCreds, CurrentTask,
    EventHandler, Kernel, NormalPriority, Pid, ProcessExitInfo, RealtimePriority, SchedulerState,
    SchedulingPolicy, SeccompFilterContainer, SeccompState, SeccompStateValue, TaskRunningState,
    ThreadGroup, ThreadState, UtsNamespaceHandle, WaitCanceler, Waiter, ZombieProcess,
};
use crate::vfs::{FdTable, FsContext, FsString, SharedFdTable};
use atomic_bitflags::atomic_bitflags;
use fuchsia_rcu::{RcuArc, RcuDroppable, RcuDroppableArc, RcuReadGuard, RcuReadScope};
use macro_rules_attribute::apply;
use starnix_logging::{log_warn, set_zx_name};
use starnix_registers::HeapRegs;
use starnix_sync::{
    LockDepGuard, LockDepMutex, LockDepReadGuard, LockDepRwLock, LockDepWriteGuard,
    TaskCommandLevel, TaskCredsLock,
};
use starnix_task_command::TaskCommand;
use starnix_types::arch::ArchWidth;
use starnix_types::stats::TaskTimeStats;
use starnix_uapi::auth::{Credentials, FsCred};
use starnix_uapi::errors::Errno;
use starnix_uapi::signals::{SIGCHLD, SigSet, Signal, sigaltstack_contains_pointer};
use starnix_uapi::user_address::{
    ArchSpecific, MappingMultiArchUserRef, UserAddress, UserCString, UserRef,
};
use starnix_uapi::{
    CLD_CONTINUED, CLD_DUMPED, CLD_EXITED, CLD_KILLED, CLD_STOPPED, CLD_TRAPPED,
    FUTEX_BITSET_MATCH_ANY, errno, error, from_status_like_fdio, pid_t, sigaction_t, sigaltstack,
    tid_t, uapi,
};
use std::collections::VecDeque;
use std::mem::MaybeUninit;
use std::ops::Deref;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::{cmp, fmt};
use zx::{Signals, Task as _};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExitStatus {
    Exit(u8),
    Kill(SignalInfo),
    CoreDump(SignalInfo),
    // The second field for Stop and Continue contains the type of ptrace stop
    // event that made it stop / continue, if applicable (PTRACE_EVENT_STOP,
    // PTRACE_EVENT_FORK, etc)
    Stop(SignalInfo, PtraceEvent),
    Continue(SignalInfo, PtraceEvent),
}
impl ExitStatus {
    /// Converts the given exit status to a status code suitable for returning from wait syscalls.
    pub fn wait_status(&self) -> i32 {
        match self {
            ExitStatus::Exit(status) => (*status as i32) << 8,
            ExitStatus::Kill(siginfo) => siginfo.signal.number() as i32,
            ExitStatus::CoreDump(siginfo) => (siginfo.signal.number() as i32) | 0x80,
            ExitStatus::Continue(siginfo, trace_event) => {
                let trace_event_val = *trace_event as u32;
                if trace_event_val != 0 {
                    (siginfo.signal.number() as i32) | (trace_event_val << 16) as i32
                } else {
                    0xffff
                }
            }
            ExitStatus::Stop(siginfo, trace_event) => {
                let trace_event_val = *trace_event as u32;
                (0x7f + ((siginfo.signal.number() as i32) << 8)) | (trace_event_val << 16) as i32
            }
        }
    }

    pub fn signal_info_code(&self) -> i32 {
        match self {
            ExitStatus::Exit(_) => CLD_EXITED as i32,
            ExitStatus::Kill(_) => CLD_KILLED as i32,
            ExitStatus::CoreDump(_) => CLD_DUMPED as i32,
            ExitStatus::Stop(_, _) => CLD_STOPPED as i32,
            ExitStatus::Continue(_, _) => CLD_CONTINUED as i32,
        }
    }

    pub fn signal_info_status(&self) -> i32 {
        match self {
            ExitStatus::Exit(status) => *status as i32,
            ExitStatus::Kill(siginfo)
            | ExitStatus::CoreDump(siginfo)
            | ExitStatus::Continue(siginfo, _)
            | ExitStatus::Stop(siginfo, _) => siginfo.signal.number() as i32,
        }
    }
}

atomic_bitflags! {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct TaskFlags: u8 {
        const EXITED                   = 1 << 0;
        const SIGNALS_AVAILABLE        = 1 << 1;
        const TEMPORARY_SIGNAL_MASK    = 1 << 2;
        /// Whether the executor should dump the stack of this task when it exits.
        /// Currently used to implement ExitStatus::CoreDump.
        const DUMP_ON_EXIT             = 1 << 3;
        const KERNEL_SIGNALS_AVAILABLE = 1 << 4;
        /// Whether the executor has successfully spawned a thread for this task.
        const SPAWNED                  = 1 << 5;
    }
}

/// This contains thread state that tracers can inspect and modify.  It is
/// captured when a thread stops, and optionally copied back (if dirty) when a
/// thread starts again.  An alternative implementation would involve the
/// tracers acting on thread state directly; however, this would involve sharing
/// CurrentTask structures across multiple threads, which goes against the
/// intent of the design of CurrentTask.
pub struct CapturedThreadState {
    /// The thread state of the traced task.  This is copied out when the thread
    /// stops.
    pub thread_state: ThreadState<HeapRegs>,

    /// Indicates that the last ptrace operation changed the thread state, so it
    /// should be written back to the original thread.
    pub dirty: bool,
}

impl ArchSpecific for CapturedThreadState {
    fn is_arch32(&self) -> bool {
        self.thread_state.is_arch32()
    }
}

#[derive(Debug)]
pub struct RobustList {
    pub next: RobustListPtr,
}

pub type RobustListPtr =
    MappingMultiArchUserRef<RobustList, uapi::robust_list, uapi::arch32::robust_list>;

impl From<uapi::robust_list> for RobustList {
    fn from(robust_list: uapi::robust_list) -> Self {
        Self { next: RobustListPtr::from(robust_list.next) }
    }
}

#[cfg(target_arch = "aarch64")]
impl From<uapi::arch32::robust_list> for RobustList {
    fn from(robust_list: uapi::arch32::robust_list) -> Self {
        Self { next: RobustListPtr::from(robust_list.next) }
    }
}

#[derive(Debug)]
pub struct RobustListHead {
    pub list: RobustList,
    pub futex_offset: isize,
}

pub type RobustListHeadPtr =
    MappingMultiArchUserRef<RobustListHead, uapi::robust_list_head, uapi::arch32::robust_list_head>;

impl From<uapi::robust_list_head> for RobustListHead {
    fn from(robust_list_head: uapi::robust_list_head) -> Self {
        Self {
            list: robust_list_head.list.into(),
            futex_offset: robust_list_head.futex_offset as isize,
        }
    }
}

#[cfg(target_arch = "aarch64")]
impl From<uapi::arch32::robust_list_head> for RobustListHead {
    fn from(robust_list_head: uapi::arch32::robust_list_head) -> Self {
        Self {
            list: robust_list_head.list.into(),
            futex_offset: robust_list_head.futex_offset as isize,
        }
    }
}

pub struct TaskMutableState {
    // See https://man7.org/linux/man-pages/man2/set_tid_address.2.html
    pub clear_child_tid: UserRef<tid_t>,

    /// Signal handler related state. This is grouped together for when atomicity is needed during
    /// signal sending and delivery.
    signals: SignalState,

    /// The current run state of the task.
    pub run_state: RunState,

    /// Internal signals that have a higher priority than a regular signal.
    ///
    /// Storing in a separate queue outside of `SignalState` ensures the internal signals will
    /// never be ignored or masked when dequeuing. Higher priority ensures that no user signals
    /// will jump the queue, e.g. ptrace, which delays the delivery.
    ///
    /// This design is not about observable consequence, but about convenient implementation.
    kernel_signals: VecDeque<KernelSignal>,

    /// The exit status that this task exited with.
    exit_status: Option<ExitStatus>,

    /// Desired scheduler state for the task.
    pub scheduler_state: SchedulerState,

    /// The UTS namespace assigned to this thread.
    ///
    /// This field is kept in the mutable state because the UTS namespace of a thread
    /// can be forked using `clone()` or `unshare()` syscalls.
    ///
    /// We use UtsNamespaceHandle because the UTS properties can be modified
    /// by any other thread that shares this namespace.
    pub uts_ns: UtsNamespaceHandle,

    /// Bit that determines whether a newly started program can have privileges its parent does
    /// not have.  See Documentation/prctl/no_new_privs.txt in the Linux kernel for details.
    /// Note that Starnix does not currently implement the relevant privileges (e.g.,
    /// setuid/setgid binaries).  So, you can set this, but it does nothing other than get
    /// propagated to children.
    ///
    /// The documentation indicates that this can only ever be set to
    /// true, and it cannot be reverted to false.  Accessor methods
    /// for this field ensure this property.
    no_new_privs: bool,

    /// Userspace hint about how to adjust the OOM score for this process.
    pub oom_score_adj: i32,

    /// List of currently installed seccomp_filters
    pub seccomp_filters: SeccompFilterContainer,

    /// A pointer to the head of the robust futex list of this thread in
    /// userspace. See get_robust_list(2)
    pub robust_list_head: RobustListHeadPtr,

    /// The timer slack used to group timer expirations for the calling thread.
    ///
    /// Timers may expire up to `timerslack_ns` late, but never early.
    ///
    /// If this value is 0, the task's default timerslack is used.
    pub timerslack_ns: u64,

    /// The default value for `timerslack_ns`. This value cannot change during the lifetime of a
    /// task.
    ///
    /// This value is set to the `timerslack_ns` of the creating thread, and thus is not constant
    /// across tasks.
    pub default_timerslack_ns: u64,

    /// Information that a tracer needs to communicate with this process, if it
    /// is being traced.
    pub ptrace: Option<Box<PtraceState>>,

    /// Information that a tracer needs to inspect this process.
    pub captured_thread_state: Option<Box<CapturedThreadState>>,

    /// The last applied scheduler role name.
    last_applied_role: Option<String>,

    /// The cpuset cgroup path of this task.
    pub cpuset_path: String,
}

impl TaskMutableState {
    pub fn no_new_privs(&self) -> bool {
        self.no_new_privs
    }

    /// Sets the value of no_new_privs to true.  It is an error to set
    /// it to anything else.
    pub fn enable_no_new_privs(&mut self) {
        self.no_new_privs = true;
    }

    pub fn get_timerslack<T: zx::Timeline>(&self) -> zx::Duration<T> {
        zx::Duration::from_nanos(self.timerslack_ns as i64)
    }

    /// Sets the current timerslack of the task to `ns`.
    ///
    /// If `ns` is zero, the current timerslack gets reset to the task's default timerslack.
    pub fn set_timerslack_ns(&mut self, ns: u64) {
        if ns == 0 {
            self.timerslack_ns = self.default_timerslack_ns;
        } else {
            self.timerslack_ns = ns;
        }
    }

    pub fn is_ptraced(&self) -> bool {
        self.ptrace.is_some()
    }

    pub fn is_ptrace_listening(&self) -> bool {
        self.ptrace.as_ref().is_some_and(|ptrace| ptrace.stop_status == PtraceStatus::Listening)
    }

    pub fn ptrace_on_signal_consume(&mut self) -> bool {
        self.ptrace.as_mut().is_some_and(|ptrace: &mut Box<PtraceState>| {
            if ptrace.stop_status.is_continuing() {
                ptrace.stop_status = PtraceStatus::Default;
                false
            } else {
                true
            }
        })
    }

    pub fn notify_ptracers(&mut self) {
        if let Some(ptrace) = &self.ptrace {
            ptrace.tracer_waiters().notify_all();
        }
    }

    pub fn wait_on_ptracer(&self, waiter: &Waiter) {
        if let Some(ptrace) = &self.ptrace {
            ptrace.tracee_waiters.wait_async(&waiter);
        }
    }

    pub fn notify_ptracees(&mut self) {
        if let Some(ptrace) = &self.ptrace {
            ptrace.tracee_waiters.notify_all();
        }
    }

    pub fn take_captured_state(&mut self) -> Option<Box<CapturedThreadState>> {
        if self.captured_thread_state.is_some() {
            let mut state = None;
            std::mem::swap(&mut state, &mut self.captured_thread_state);
            return state;
        }
        None
    }

    pub fn copy_state_from(&mut self, current_task: &CurrentTask) {
        self.captured_thread_state = Some(Box::new(CapturedThreadState {
            thread_state: current_task.thread_state.extended_snapshot::<HeapRegs>(),
            dirty: false,
        }));
    }

    /// Returns the task's currently active signal mask.
    pub fn signal_mask(&self) -> SigSet {
        self.signals.mask()
    }

    /// Returns true if `signal` is currently blocked by this task's signal mask.
    pub fn is_signal_masked(&self, signal: Signal) -> bool {
        self.signals.mask().has_signal(signal)
    }

    /// Returns true if `signal` is blocked by the saved signal mask.
    ///
    /// Note that the current signal mask may still not be blocking the signal.
    pub fn is_signal_masked_by_saved_mask(&self, signal: Signal) -> bool {
        self.signals.saved_mask().is_some_and(|mask| mask.has_signal(signal))
    }

    /// Removes the currently active, temporary, signal mask and restores the
    /// previously active signal mask.
    pub fn restore_signal_mask(&mut self) {
        self.signals.restore_mask();
    }

    /// Returns true if the task's current `RunState` is blocked.
    pub fn is_blocked(&self) -> bool {
        self.run_state.is_blocked()
    }

    /// Sets the task's `RunState` to `run_state`.
    pub fn set_run_state(&mut self, run_state: RunState) {
        self.run_state = run_state;
    }

    pub fn run_state(&self) -> RunState {
        self.run_state.clone()
    }

    pub fn on_signal_stack(&self, stack_pointer_register: u64) -> bool {
        self.signals
            .alt_stack
            .map(|signal_stack| sigaltstack_contains_pointer(&signal_stack, stack_pointer_register))
            .unwrap_or(false)
    }

    pub fn set_sigaltstack(&mut self, stack: Option<sigaltstack>) {
        self.signals.alt_stack = stack;
    }

    pub fn sigaltstack(&self) -> Option<sigaltstack> {
        self.signals.alt_stack
    }

    pub fn wait_on_signal(&mut self, waiter: &Waiter) {
        self.signals.signal_wait.wait_async(waiter);
    }

    pub fn signals_mut(&mut self) -> &mut SignalState {
        &mut self.signals
    }

    pub fn wait_on_signal_fd_events(
        &self,
        waiter: &Waiter,
        mask: SigSet,
        handler: EventHandler,
    ) -> WaitCanceler {
        self.signals.signal_wait.wait_async_signal_mask(waiter, mask, handler)
    }

    pub fn notify_signal_waiters(&self, signal: &Signal) {
        self.signals.signal_wait.notify_signal(signal);
    }

    /// Thaw the task if has been frozen
    pub fn thaw(&mut self) {
        if let RunState::Frozen(waiter) = self.run_state() {
            waiter.notify();
        }
    }

    pub fn is_frozen(&self) -> bool {
        matches!(self.run_state(), RunState::Frozen(_))
    }

    #[cfg(test)]
    pub fn kernel_signals_for_test(&self) -> &VecDeque<KernelSignal> {
        &self.kernel_signals
    }
}

#[apply(state_implementation!)]
impl TaskMutableState<Base = Task> {
    pub fn set_stopped(
        &mut self,
        stopped: StopState,
        siginfo: Option<SignalInfo>,
        current_task: Option<&CurrentTask>,
        event: Option<PtraceEventData>,
    ) {
        if stopped.ptrace_only() && self.ptrace.is_none() {
            return;
        }

        if self.base.load_stopped().is_illegal_transition(stopped) {
            return;
        }

        // TODO(https://g-issues.fuchsia.dev/issues/306438676): When task can be
        // stopped inside user code, task will need to be either restarted or
        // stopped here.
        self.store_stopped(stopped);
        if stopped.is_stopped() {
            if let Some(ref current_task) = current_task {
                self.copy_state_from(current_task);
            }
        }
        if let Some(ptrace) = &mut self.ptrace {
            ptrace.set_last_signal(siginfo);
            ptrace.set_last_event(event);
        }
        if stopped == StopState::Waking || stopped == StopState::ForceWaking {
            self.notify_ptracees();
        }
        if !stopped.is_in_progress() {
            self.notify_ptracers();
        }
    }

    /// Enqueues a signal at the back of the task's signal queue.
    pub fn enqueue_signal(&mut self, signal: SignalInfo) {
        self.signals.enqueue(signal);
        self.set_flags(TaskFlags::SIGNALS_AVAILABLE, self.signals.is_any_pending());
    }

    /// Enqueues the signal, allowing the signal to skip straight to the front of the task's queue.
    ///
    /// `enqueue_signal` is the more common API to use.
    ///
    /// Note that this will not guarantee that the signal is dequeued before any process-directed
    /// signals.
    pub fn enqueue_signal_front(&mut self, signal: SignalInfo) {
        self.signals.enqueue(signal);
        self.set_flags(TaskFlags::SIGNALS_AVAILABLE, self.signals.is_any_pending());
    }

    /// Sets the current signal mask of the task.
    pub fn set_signal_mask(&mut self, mask: SigSet) {
        self.signals.set_mask(mask);
        self.set_flags(TaskFlags::SIGNALS_AVAILABLE, self.signals.is_any_pending());
    }

    /// Sets a temporary signal mask for the task.
    ///
    /// This mask should be removed by a matching call to `restore_signal_mask`.
    pub fn set_temporary_signal_mask(&mut self, mask: SigSet) {
        self.signals.set_temporary_mask(mask);
        self.set_flags(TaskFlags::SIGNALS_AVAILABLE, self.signals.is_any_pending());
    }

    /// Returns the number of pending signals for this task, without considering the signal mask.
    pub fn pending_signal_count(&self) -> usize {
        self.signals.num_queued() + self.base.thread_group().num_signals_queued()
    }

    /// Returns `true` if `signal` is pending for this task, without considering the signal mask.
    pub fn has_signal_pending(&self, signal: Signal) -> bool {
        self.signals.has_queued(signal) || self.base.thread_group().has_signal_queued(signal)
    }

    // Prepare a SignalInfo to be sent to the tracer, if any.
    pub fn prepare_signal_info(
        &mut self,
        stopped: StopState,
    ) -> Option<(Weak<ThreadGroup>, SignalInfo)> {
        if !stopped.is_stopped() {
            return None;
        }

        if let Some(ptrace) = &self.ptrace {
            if let Some(last_signal) = ptrace.get_last_signal_ref() {
                let signal_info = SignalInfo::with_detail(
                    SIGCHLD,
                    CLD_TRAPPED as i32,
                    SignalDetail::SIGCHLD {
                        pid: self.base.tid.clone(),
                        uid: self.base.real_creds().uid,
                        status: last_signal.signal.number() as i32,
                    },
                );

                return Some((ptrace.core_state.thread_group.clone(), signal_info));
            }
        }

        None
    }

    pub fn set_ptrace(&mut self, tracer: Option<Box<PtraceState>>) -> Result<(), Errno> {
        if tracer.is_some() && self.ptrace.is_some() {
            return error!(EPERM);
        }

        if tracer.is_none() {
            // Handle the case where this is called while the thread group is being released.
            if let Ok(tg_stop_state) = self.base.thread_group().load_stopped().as_in_progress() {
                self.set_stopped(tg_stop_state, None, None, None);
            }
        }
        self.ptrace = tracer;
        Ok(())
    }

    pub fn can_accept_ptrace_commands(&mut self) -> bool {
        !self.base.load_stopped().is_waking_or_awake()
            && self.is_ptraced()
            && !self.is_ptrace_listening()
    }

    fn store_stopped(&mut self, state: StopState) {
        // We don't actually use the guard but we require it to enforce that the
        // caller holds the thread group's mutable state lock (identified by
        // mutable access to the thread group's mutable state).

        self.base.stop_state.store(state, Ordering::Relaxed)
    }

    pub fn update_flags(&mut self, clear: TaskFlags, set: TaskFlags) {
        // We don't actually use the guard but we require it to enforce that the
        // caller holds the task's mutable state lock (identified by mutable
        // access to the task's mutable state).

        debug_assert_eq!(clear ^ set, clear | set);
        let observed = self.base.flags();
        let swapped = self.base.flags.swap((observed | set) & !clear, Ordering::Relaxed);
        debug_assert_eq!(swapped, observed);
    }

    pub fn set_flags(&mut self, flag: TaskFlags, v: bool) {
        let (clear, set) = if v { (TaskFlags::empty(), flag) } else { (flag, TaskFlags::empty()) };

        self.update_flags(clear, set);
    }

    pub fn set_spawned(&mut self) {
        self.set_flags(TaskFlags::SPAWNED, true);
    }

    pub fn set_exit_status(&mut self, status: ExitStatus) {
        self.set_flags(TaskFlags::EXITED, true);
        self.exit_status = Some(status);
    }

    pub fn set_exit_status_if_not_already(&mut self, status: ExitStatus) {
        self.set_flags(TaskFlags::EXITED, true);
        self.exit_status.get_or_insert(status);
    }

    /// The set of pending signals for the task, including the signals pending for the thread
    /// group.
    pub fn pending_signals(&self) -> SigSet {
        self.signals.pending() | self.base.thread_group().get_pending_signals()
    }

    /// The set of pending signals for the task specifically, not including the signals pending
    /// for the thread group.
    pub fn task_specific_pending_signals(&self) -> SigSet {
        self.signals.pending()
    }

    /// Returns true if any currently pending signal is allowed by `mask`.
    pub fn is_any_signal_allowed_by_mask(&self, mask: SigSet) -> bool {
        self.signals.is_any_allowed_by_mask(mask)
            || self.base.thread_group().is_any_signal_allowed_by_mask(mask)
    }

    /// Returns whether or not a signal is pending for this task, taking the current
    /// signal mask into account.
    pub fn is_any_signal_pending(&self) -> bool {
        let mask = self.signal_mask();
        self.signals.is_any_pending()
            || self.base.thread_group().is_any_signal_allowed_by_mask(mask)
    }

    /// Returns the next pending signal that passes `predicate`.
    fn take_next_signal_where<F>(&mut self, predicate: F) -> Option<SignalInfo>
    where
        F: Fn(&SignalInfo) -> bool,
    {
        if let Some(signal) = self.base.thread_group().take_next_signal_where(&predicate) {
            Some(signal)
        } else {
            let s = self.signals.take_next_where(&predicate);
            self.set_flags(TaskFlags::SIGNALS_AVAILABLE, self.signals.is_any_pending());
            s
        }
    }

    /// Removes and returns the next pending `signal` for this task.
    ///
    /// Returns `None` if `siginfo` is a blocked signal, or no such signal is pending.
    pub fn take_specific_signal(&mut self, siginfo: SignalInfo) -> Option<SignalInfo> {
        let signal_mask = self.signal_mask();
        if signal_mask.has_signal(siginfo.signal) {
            return None;
        }

        let predicate = |s: &SignalInfo| s.signal == siginfo.signal;
        self.take_next_signal_where(predicate)
    }

    /// Removes and returns a pending signal that is unblocked by the current signal mask.
    ///
    /// Returns `None` if there are no unblocked signals pending.
    pub fn take_any_signal(&mut self) -> Option<SignalInfo> {
        self.take_signal_with_mask(self.signal_mask())
    }

    /// Removes and returns a pending signal that is unblocked by `signal_mask`.
    ///
    /// Returns `None` if there are no signals pending that are unblocked by `signal_mask`.
    pub fn take_signal_with_mask(&mut self, signal_mask: SigSet) -> Option<SignalInfo> {
        let predicate = |s: &SignalInfo| !signal_mask.has_signal(s.signal) || s.force;
        self.take_next_signal_where(predicate)
    }

    /// Enqueues an internal signal at the back of the task's kernel signal queue.
    pub fn enqueue_kernel_signal(&mut self, signal: KernelSignal) {
        self.kernel_signals.push_back(signal);
        self.set_flags(TaskFlags::KERNEL_SIGNALS_AVAILABLE, true);
    }

    /// Removes and returns a pending internal signal.
    ///
    /// Returns `None` if there are no signals pending.
    pub fn take_kernel_signal(&mut self) -> Option<KernelSignal> {
        let signal = self.kernel_signals.pop_front();
        if self.kernel_signals.is_empty() {
            self.set_flags(TaskFlags::KERNEL_SIGNALS_AVAILABLE, false);
        }
        signal
    }

    #[cfg(test)]
    pub fn queued_signal_count(&self, signal: Signal) -> usize {
        self.signals.queued_count(signal)
            + self.base.thread_group().pending_signals.lock().queued_count(signal)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskStateCode {
    // Task is being executed.
    Running,

    // Task is waiting for an event.
    Sleeping,

    // Tracing stop
    TracingStop,

    // Task has exited.
    Zombie,
}

impl TaskStateCode {
    pub fn code_char(&self) -> char {
        match self {
            TaskStateCode::Running => 'R',
            TaskStateCode::Sleeping => 'S',
            TaskStateCode::TracingStop => 't',
            TaskStateCode::Zombie => 'Z',
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            TaskStateCode::Running => "running",
            TaskStateCode::Sleeping => "sleeping",
            TaskStateCode::TracingStop => "tracing stop",
            TaskStateCode::Zombie => "zombie",
        }
    }
}

/// The information of the task that needs to be available to the `ThreadGroup` while computing
/// which process a wait can target. It is necessary to shared this data with the `ThreadGroup` so
/// that it is available while the task is being dropped and so is not accessible from a weak
/// pointer.
#[derive(Debug, RcuDroppable)]
pub struct TaskPersistentInfoState {
    /// Immutable information about the task
    pub tid: Pid,
    pub pid: Pid,

    /// The command of this task.
    command: LockDepMutex<TaskCommand, TaskCommandLevel>,

    /// The security credentials for this task. These are only set when the task is the CurrentTask,
    /// or on task creation.
    creds: RcuDroppableArc<Credentials>,

    // A lock for the security credentials. Writers must take the lock, readers that need to ensure
    // that the task state does not change may take the lock.
    creds_lock: LockDepRwLock<(), TaskCredsLock>,
}

/// Guard for reading locked credentials.
pub struct CredentialsReadGuard<'a> {
    _lock: LockDepReadGuard<'a, ()>,
    creds: RcuReadGuard<Credentials>,
}

impl<'a> Deref for CredentialsReadGuard<'a> {
    type Target = Credentials;

    fn deref(&self) -> &Self::Target {
        self.creds.deref()
    }
}

/// Guard for writing credentials. No `CredentialsReadGuard` to the same task can concurrently
///  exist.
pub struct CredentialsWriteGuard<'a> {
    _lock: LockDepWriteGuard<'a, ()>,
    creds: &'a RcuDroppableArc<Credentials>,
}

impl<'a> CredentialsWriteGuard<'a> {
    pub fn update(&mut self, creds: Arc<Credentials>) {
        self.creds.update(creds);
    }
}

impl TaskPersistentInfoState {
    fn new(
        tid: Pid,
        pid: Pid,
        command: TaskCommand,
        creds: Arc<Credentials>,
    ) -> TaskPersistentInfo {
        Arc::new(Self {
            tid,
            pid,
            command: command.into(),
            creds: RcuDroppableArc::new(creds),
            creds_lock: Default::default(),
        })
    }

    pub fn command_guard(&self) -> LockDepGuard<'_, TaskCommand> {
        self.command.lock()
    }

    /// Snapshots the credentials, returning a short-lived RCU-guarded reference.
    pub fn real_creds(&self) -> RcuReadGuard<Credentials> {
        self.creds.read()
    }

    /// Snapshots the credentials, returning a new reference. Use this if you need to stash the
    /// credentials somewhere.
    pub fn clone_creds(&self) -> Arc<Credentials> {
        self.creds.to_arc()
    }

    /// Returns a read lock on the credentials. This is appropriate if you need to guarantee that
    ///  the Task's credentials will not change during a security-sensitive operation.
    pub fn lock_creds(&self) -> CredentialsReadGuard<'_> {
        let lock = self.creds_lock.read();
        CredentialsReadGuard { _lock: lock, creds: self.creds.read() }
    }

    /// Locks the credentials for writing, returning a guard that the `CurrentTask` can use to
    /// update both the objective `Task` credentials, and its own subjective cached copy.
    pub(in crate::task) fn write_current_task_creds(
        self: &Arc<Self>,
    ) -> CurrentTaskCredentialsWriteGuard {
        let persistent_info = self.clone();
        // SAFETY: `creds_lock` remains live via the `persistent_info` reference to `Self`.
        let lock = unsafe {
            let raw_lock = self.creds_lock.write();
            std::mem::transmute::<LockDepWriteGuard<'_, ()>, LockDepWriteGuard<'static, ()>>(
                raw_lock,
            )
        };
        CurrentTaskCredentialsWriteGuard { _lock: lock, persistent_info }
    }
}

impl std::borrow::Borrow<Pid> for TaskPersistentInfoState {
    fn borrow(&self) -> &Pid {
        &self.tid
    }
}

impl PartialEq for TaskPersistentInfoState {
    fn eq(&self, other: &Self) -> bool {
        self.tid == other.tid
    }
}

impl Eq for TaskPersistentInfoState {}

impl PartialOrd for TaskPersistentInfoState {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for TaskPersistentInfoState {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.tid.cmp(&other.tid)
    }
}

impl std::hash::Hash for TaskPersistentInfoState {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.tid.hash(state);
    }
}

pub type TaskPersistentInfo = Arc<TaskPersistentInfoState>;

pub struct CurrentTaskCredentialsWriteGuard {
    // Drop order is critical: the lock must be dropped BEFORE the persistent_info Arc.
    // Rust drops fields in declaration order (top-to-bottom).
    // So _lock is dropped first, then persistent_info.
    _lock: LockDepWriteGuard<'static, ()>,
    pub persistent_info: TaskPersistentInfo,
}

impl CurrentTaskCredentialsWriteGuard {
    pub fn update(self, current_task: &CurrentTask, creds: Arc<Credentials>) {
        self.persistent_info.creds.update(creds.clone());
        *current_task.current_creds.borrow_mut() = CurrentCreds::Cached(creds);

        // The /proc/pid directory's ownership is updated when the task's euid
        // or egid changes. See proc(5).
        let maybe_node = current_task.running_state().proc_pid_directory_cache.get();
        if let Some(node) = maybe_node {
            let creds = current_task.real_creds().euid_as_fscred();
            // SAFETY: The /proc/pid directory held by `proc_pid_directory_cache` represents the
            // current task. It's owner and group are supposed to track the current task's euid and
            // egid.
            unsafe {
                node.force_chown(creds);
            }
        }
    }
}

/// A unit of execution.
///
/// A task is the primary unit of execution in the Starnix kernel. Most tasks are *user* tasks,
/// which have an associated Zircon thread. The Zircon thread switches between restricted mode,
/// in which the thread runs userspace code, and normal mode, in which the thread runs Starnix
/// code.
///
/// Tasks track the resources used by userspace by referencing various objects, such as an
/// `FdTable`, a `MemoryManager`, and an `FsContext`. Many tasks can share references to these
/// objects. In principle, which objects are shared between which tasks can be largely arbitrary,
/// but there are common patterns of sharing. For example, tasks created with `pthread_create`
/// will share the `FdTable`, `MemoryManager`, and `FsContext` and are often called "threads" by
/// userspace programmers. Tasks created by `posix_spawn` do not share these objects and are often
/// called "processes" by userspace programmers. However, inside the kernel, there is no clear
/// definition of a "thread" or a "process".
///
/// During boot, the kernel creates the first task, often called `init`. The vast majority of other
/// tasks are created as transitive clones (e.g., using `clone(2)`) of that task. Sometimes, the
/// kernel will create new tasks from whole cloth, either with a corresponding userspace component
/// or to represent some background work inside the kernel.
///
/// See also `CurrentTask`, which represents the task corresponding to the thread that is currently
/// executing.
pub struct Task {
    /// Weak reference to this `Task`. This allows us to retrieve an `Arc` from a raw `Task`.
    pub weak_self: Weak<Self>,

    /// A unique identifier for this task.
    ///
    /// This value can be read in userspace using `gettid(2)`. In general, this value
    /// is different from the value return by `getpid(2)`, which returns the `id` of the leader
    /// of the `thread_group`.
    pub tid: Pid,

    /// The process key of this task.
    pub pid: Pid,

    /// The kernel to which this thread group belongs.
    pub kernel: Arc<Kernel>,

    /// The thread group to which this task belongs.
    ///
    /// The group of tasks in a thread group roughly corresponds to the userspace notion of a
    /// process.
    pub thread_group: Arc<ThreadGroup>,

    /// The running state of the task.
    ///
    /// This is `None` for exited tasks.
    pub running_state: RcuArc<TaskRunningState>,

    /// The stop state of the task, distinct from the stop state of the thread group.
    ///
    /// Must only be set when the `mutable_state` write lock is held.
    stop_state: AtomicStopState,

    /// The flags for the task.
    ///
    /// Must only be set the then `mutable_state` write lock is held.
    flags: AtomicTaskFlags,

    /// The mutable state of the Task.
    mutable_state:
        starnix_sync::LockDepRwLock<TaskMutableState, starnix_sync::TaskMutableStateLock>,

    /// The information of the task that needs to be available to the `ThreadGroup` while computing
    /// which process a wait can target.
    /// Contains the command line, the task credentials and the exit signal.
    /// See `TaskPersistentInfo` for more information.
    pub persistent_info: TaskPersistentInfo,

    /// For vfork and clone() with CLONE_VFORK, this is set when the task exits or calls execve().
    /// It allows the calling task to block until the fork has been completed. Only populated
    /// when created with the CLONE_VFORK flag.
    vfork_event: Option<Arc<zx::Event>>,

    /// Variable that can tell you whether there are currently seccomp
    /// filters without holding a lock
    pub seccomp_filter_state: SeccompState,

    /// Tell you whether you are tracing syscall entry / exit without a lock.
    pub trace_syscalls: AtomicBool,
}

/// The decoded cross-platform parts we care about for page fault exception reports.
#[derive(Debug)]
pub struct PageFaultExceptionReport {
    pub faulting_address: u64,
    pub not_present: bool, // Set when the page fault was due to a not-present page.
    pub is_write: bool,    // Set when the triggering memory operation was a write.
    pub is_execute: bool,  // Set when the triggering memory operation was an execute.
}

impl Task {
    pub fn kernel(&self) -> &Arc<Kernel> {
        &self.kernel
    }

    pub fn thread_group(&self) -> &Arc<ThreadGroup> {
        &self.thread_group
    }

    pub fn has_same_address_space(&self, other: Option<&Arc<MemoryManager>>) -> bool {
        match (self.mm(), other) {
            (Ok(this), Some(other)) => Arc::ptr_eq(&this, other),
            (Err(_), None) => true,
            _ => false,
        }
    }

    pub fn flags(&self) -> TaskFlags {
        self.flags.load(Ordering::Relaxed)
    }

    pub fn is_spawned(&self) -> bool {
        self.flags().contains(TaskFlags::SPAWNED)
    }

    /// When the task exits, if there is a notification that needs to propagate
    /// to a ptracer, make sure it will propagate.
    pub fn set_ptrace_zombie(&self, pids: &mut crate::task::PidTableGuard<'_>) {
        if !self.is_spawned() {
            // A task that has not fully spawned cannot become a zombie.
            return;
        }

        let pgid = self.thread_group().read().process_group.leader.clone();
        let exit_signal = self.thread_group().read().exit_signal.clone();
        let mut state = self.write();
        state.set_stopped(StopState::ForceAwake, None, None, None);
        if let Some(ptrace) = &mut state.ptrace {
            // Add a zombie that the ptracer will notice.
            ptrace.last_signal_waitable = true;
            let tracer_tg = ptrace.core_state.thread_group.upgrade();
            if let Some(tracer_tg) = tracer_tg {
                drop(state);
                let mut tracer_state = tracer_tg.write();
                if !tracer_state.is_running() {
                    // An exiting or exited tracer cannot accept new zombies.
                    return;
                }

                let exit_status = self.exit_status().unwrap_or_else(|| {
                    starnix_logging::log_error!("Exiting without an exit code.");
                    ExitStatus::Exit(u8::MAX)
                });
                let uid = self.real_creds().uid;
                let exit_info = ProcessExitInfo { status: exit_status, exit_signal };
                let zombie = ZombieProcess {
                    pid: self.pid.clone(),
                    pgid,
                    uid,
                    exit_info: exit_info,
                    // ptrace doesn't need this.
                    time_stats: TaskTimeStats::default(),
                    is_canonical: false,
                };

                tracer_state.zombie_ptracees.add(pids, self.tid.clone(), zombie);
            };
        }
    }

    /// Disconnects this task from the tracer.
    pub fn ptrace_disconnect(&self) {
        // Get a reference to the ptracer thread group through the weak reference in PtraceCoreState
        // to avoid acquiring a PidTable lock.
        let tracer_tg = self
            .read()
            .ptrace
            .as_ref()
            .map(|p| p.core_state.thread_group.clone())
            .and_then(|tg| tg.upgrade());
        if let Some(tg) = tracer_tg {
            tg.ptracees.lock().remove(&self.persistent_info);
        }
    }

    pub fn exit_status(&self) -> Option<ExitStatus> {
        self.is_exitted().then(|| self.read().exit_status.clone()).flatten()
    }

    pub fn is_exitted(&self) -> bool {
        self.flags().contains(TaskFlags::EXITED)
    }

    pub fn load_stopped(&self) -> StopState {
        self.stop_state.load(Ordering::Relaxed)
    }

    /// Upgrade a [`Weak<Task>`], returning [`Err(ESRCH)`] if the reference cannot be borrowed.
    pub fn from_weak(weak: &Weak<Task>) -> Result<Arc<Task>, Errno> {
        weak.upgrade().ok_or_else(|| errno!(ESRCH))
    }

    /// Internal function for creating a Task object. Useful when you need to specify the value of
    /// every field. create_process and create_thread are more likely to be what you want.
    ///
    /// Any fields that should be initialized fresh for every task, even if the task was created
    /// with fork, are initialized to their defaults inside this function. All other fields are
    /// passed as parameters.
    #[allow(clippy::let_and_return)]
    pub fn new(
        tid: Pid,
        command: TaskCommand,
        thread_group: Arc<ThreadGroup>,
        files: SharedFdTable,
        mm: Option<Arc<MemoryManager>>,
        // The only case where fs should be None if when building the initial task that is the
        // used to build the initial FsContext.
        fs: Arc<FsContext>,
        creds: Arc<Credentials>,
        abstract_socket_namespace: Arc<AbstractUnixSocketNamespace>,
        abstract_vsock_namespace: Arc<AbstractVsockSocketNamespace>,
        signal_mask: SigSet,
        kernel_signals: VecDeque<KernelSignal>,
        vfork_event: Option<Arc<zx::Event>>,
        scheduler_state: SchedulerState,
        uts_ns: UtsNamespaceHandle,
        no_new_privs: bool,
        seccomp_filter_state: SeccompState,
        seccomp_filters: SeccompFilterContainer,
        robust_list_head: RobustListHeadPtr,
        timerslack_ns: u64,
    ) -> Arc<Self> {
        let pid = thread_group.leader.clone();
        Arc::new_cyclic(|weak_self| {
            let task = Task {
                weak_self: weak_self.clone(),
                tid: tid.clone(),
                pid: pid.clone(),
                kernel: Arc::clone(&thread_group.kernel),
                thread_group,
                running_state: RcuArc::new(Some(Arc::new(TaskRunningState {
                    thread: Default::default(),
                    files: Some(files).into(),
                    mm: RcuArc::new(mm),
                    fs: RcuArc::new(Some(fs)),
                    abstract_socket_namespace,
                    abstract_vsock_namespace,
                    proc_pid_directory_cache: Default::default(),
                }))),
                vfork_event,
                stop_state: AtomicStopState::new(StopState::Awake),
                flags: AtomicTaskFlags::new(TaskFlags::empty()),
                mutable_state: TaskMutableState {
                    clear_child_tid: UserRef::default(),
                    signals: SignalState::with_mask(signal_mask),
                    run_state: RunState::default(),
                    kernel_signals,
                    exit_status: None,
                    scheduler_state,
                    uts_ns,
                    no_new_privs,
                    oom_score_adj: Default::default(),
                    seccomp_filters,
                    robust_list_head,
                    timerslack_ns,
                    // The default timerslack is set to the current timerslack of the creating thread.
                    default_timerslack_ns: timerslack_ns,
                    ptrace: None,
                    captured_thread_state: None,
                    last_applied_role: None,
                    cpuset_path: "/".to_string(),
                }
                .into(),
                persistent_info: TaskPersistentInfoState::new(tid, pid, command, creds),
                seccomp_filter_state,
                trace_syscalls: AtomicBool::new(false),
            };

            #[cfg(any(test, debug_assertions))]
            {
                // Note that `Kernel::pids` is already locked by the caller of `Task::new()`.
                let _l1 = task.persistent_info.lock_creds();
                let _l2 = task.read();
                let _l3 = task.persistent_info.command_guard();
            }
            task
        })
    }

    state_accessor!(Task, mutable_state);

    /// Returns the real credentials of the task as a short-lived RCU-guarded reference. These
    /// credentials are used to check permissions for actions performed on the task. If the task
    /// itself is performing an action, use `CurrentTask::current_creds` instead. This does not
    /// lock the credentials.
    pub fn real_creds(&self) -> RcuReadGuard<Credentials> {
        self.persistent_info.real_creds()
    }

    /// Returns a new long-lived reference to the real credentials of the task.  These credentials
    /// are used to check permissions for actions performed on the task. If the task itself is
    /// performing an action, use `CurrentTask::current_creds` instead. This does not lock the
    /// credentials.
    pub fn clone_creds(&self) -> Arc<Credentials> {
        self.persistent_info.clone_creds()
    }

    pub fn ptracer_task(&self) -> Option<Arc<Task>> {
        self.read().ptrace.as_ref().and_then(|p| p.core_state.task.upgrade())
    }

    /// Determine whether the task is running.
    ///
    /// # Thread Safety
    ///
    /// The task may exit immediately after `is_running()` returns `true`.
    pub fn is_running(&self) -> bool {
        self.running_state.is_some()
    }

    /// Returns the running state of the task, if it exists.
    ///
    /// # Errors
    ///
    /// Returns [`Err(ESRCH)`] if the task has already transitioned to a zombie state and its running
    /// resources have been dropped.
    #[track_caller]
    pub fn running_state(&self) -> Result<Arc<TaskRunningState>, Errno> {
        self.running_state.upgrade().ok_or_else(|| errno!(ESRCH))
    }

    /// Returns the file descriptor table of the task, if it exists.
    ///
    /// # Errors
    ///
    /// Returns [`Err(errno)`] where `errno` is:
    ///
    ///   - `ESRCH`: the task is dead and its live resources have been dropped.
    #[track_caller]
    pub fn files(&self) -> Result<Arc<FdTable>, Errno> {
        self.running_state()?.files()
    }

    /// Returns the file system context of the task, if it exists.
    ///
    /// # Errors
    ///
    /// Returns [`Err(errno)`] where `errno` is:
    ///
    ///   - `ESRCH`: the task is dead and its live resources have been dropped.
    #[track_caller]
    pub fn fs(&self) -> Result<Arc<FsContext>, Errno> {
        Ok(self.running_state()?.fs())
    }

    /// Returns the memory manager of the task, if it exists.
    ///
    /// # Errors
    ///
    /// Returns [`Err(errno)`] where `errno` is:
    ///
    ///   - `ESRCH`: the task is dead and its live resources have been dropped.
    ///   - `EINVAL`: the task does not have a memory manager.
    #[track_caller]
    pub fn mm(&self) -> Result<Arc<MemoryManager>, Errno> {
        // Retain an RCU read scope for the entire operation. This allows self.running_state() and
        // mm.upgrade() to use cheaper nested RCU read locks.
        let _scope = RcuReadScope::new();
        self.running_state()?.mm.upgrade().ok_or_else(|| errno!(EINVAL))
    }

    /// Modify the given elements of the scheduler state with new values and update the
    /// task's thread's role.
    pub(crate) fn set_scheduler_policy_priority_and_reset_on_fork(
        &self,
        policy: SchedulingPolicy,
        priority: RealtimePriority,
        reset_on_fork: bool,
    ) -> Result<(), Errno> {
        self.update_scheduler_state_then_role(|scheduler_state| {
            scheduler_state.policy = policy;
            scheduler_state.realtime_priority = priority;
            scheduler_state.reset_on_fork = reset_on_fork;
        })
    }

    /// Modify the scheduler state's priority and update the task's thread's role.
    pub(crate) fn set_scheduler_priority(&self, priority: RealtimePriority) -> Result<(), Errno> {
        self.update_scheduler_state_then_role(|scheduler_state| {
            scheduler_state.realtime_priority = priority
        })
    }

    /// Modify the scheduler state's nice and update the task's thread's role.
    pub(crate) fn set_scheduler_nice(&self, nice: NormalPriority) -> Result<(), Errno> {
        self.update_scheduler_state_then_role(|scheduler_state| {
            scheduler_state.normal_priority = nice
        })
    }

    /// Overwrite the existing scheduler state with a new one and update the task's thread's role.
    pub fn set_scheduler_state(&self, scheduler_state: SchedulerState) -> Result<(), Errno> {
        self.update_scheduler_state_then_role(|task_scheduler_state| {
            *task_scheduler_state = scheduler_state
        })
    }

    /// Update the task's thread's role based on its current scheduler state without making any
    /// changes to the state.
    ///
    /// This should be called on tasks that have newly created threads, e.g. after cloning.
    pub fn sync_scheduler_state_to_role(&self) -> Result<(), Errno> {
        self.update_scheduler_state_then_role(|_| {})
    }

    fn update_scheduler_state_then_role(
        &self,
        updater: impl FnOnce(&mut SchedulerState),
    ) -> Result<(), Errno> {
        let process_name = self.thread_group().read().leader_command();
        let thread_name = self.command();

        let (new_scheduler_state, cpuset_path, last_applied_role) = {
            let mut state = self.write();
            updater(&mut state.scheduler_state);
            (state.scheduler_state, state.cpuset_path.clone(), state.last_applied_role.clone())
        };

        let scheduler = &self.thread_group().kernel.scheduler;
        let role_name = scheduler.resolve_role_name(
            &process_name,
            &thread_name,
            &cpuset_path,
            new_scheduler_state,
        );
        if last_applied_role.as_deref() == Some(role_name) {
            return Ok(());
        }
        scheduler.set_thread_role(self, role_name)?;
        // Note: Re-acquiring the lock here to update `last_applied_role` has a minor race if
        // concurrent state updates execute `set_thread_role` out of order relative to cache writes.
        // Dropping the lock across `set_thread_role` avoids holding `Task` write lock over FIDL RPCs,
        // and any cache mismatch will self-correct on the next scheduler state or cgroup change.
        self.write().last_applied_role = Some(role_name.to_string());
        Ok(())
    }

    /// Signals the vfork event, if any, to unblock waiters.
    pub fn signal_vfork(&self) {
        if let Some(event) = &self.vfork_event {
            if let Err(status) = event.signal(Signals::NONE, Signals::USER_0) {
                log_warn!("Failed to set vfork signal {status}");
            }
        };
    }

    /// Blocks the caller until the task has exited or executed execve(). This is used to implement
    /// vfork() and clone(... CLONE_VFORK, ...). The task must have created with CLONE_EXECVE.
    pub fn wait_for_execve(&self, task_to_wait: Weak<Task>) -> Result<(), Errno> {
        let event = task_to_wait.upgrade().and_then(|t| t.vfork_event.clone());
        if let Some(event) = event {
            event
                .wait_one(zx::Signals::USER_0, zx::MonotonicInstant::INFINITE)
                .map_err(|status| from_status_like_fdio!(status))?;
        }
        Ok(())
    }

    /// If needed, clear the child tid for this task.
    ///
    /// Userspace can ask us to clear the child tid and issue a futex wake at
    /// the child tid address when we tear down a task. For example, bionic
    /// uses this mechanism to implement pthread_join. The thread that calls
    /// pthread_join sleeps using FUTEX_WAIT on the child tid address. We wake
    /// them up here to let them know the thread is done.
    pub fn clear_child_tid_if_needed(&self) -> Result<(), Errno> {
        let mut state = self.write();
        let user_tid = state.clear_child_tid;
        if !user_tid.is_null() {
            let zero: tid_t = 0;
            self.write_object(user_tid, &zero)?;
            self.kernel().shared_futexes.wake(
                self,
                user_tid.addr(),
                usize::MAX,
                FUTEX_BITSET_MATCH_ANY,
            )?;
            state.clear_child_tid = UserRef::default();
        }
        Ok(())
    }

    pub fn get_task(&self, tid: tid_t) -> Result<Arc<Task>, Errno> {
        self.kernel().pids.get(tid)?.get_task()
    }

    pub fn get_pid(&self) -> pid_t {
        self.pid.id
    }

    pub fn get_tid(&self) -> tid_t {
        self.tid.id
    }

    pub fn is_leader(&self) -> bool {
        self.get_pid() == self.get_tid()
    }

    pub fn read_argv(&self, max_len: usize) -> Result<Vec<FsString>, Errno> {
        // argv is empty for kthreads
        let Ok(mm) = self.mm() else {
            return Ok(vec![]);
        };
        let (argv_start, argv_end) = {
            let mm_state = mm.state.read();
            (mm_state.argv_start, mm_state.argv_end)
        };

        let len_to_read = std::cmp::min(argv_end - argv_start, max_len);
        self.read_nul_delimited_c_string_list(argv_start, len_to_read)
    }

    pub fn read_argv0(&self) -> Result<FsString, Errno> {
        // argv is empty for kthreads
        let Ok(mm) = self.mm() else {
            return Ok(FsString::default());
        };
        let argv_start = {
            let mm_state = mm.state.read();
            mm_state.argv_start
        };
        // Assuming a 64-bit arch width is fine for a type that's just u8's on all arches.
        let argv_start = UserCString::new(&ArchWidth::Arch64, argv_start);
        self.read_path(argv_start)
    }

    pub fn read_env(&self, max_len: usize) -> Result<Vec<FsString>, Errno> {
        // environment is empty for kthreads
        let Ok(mm) = self.mm() else { return Ok(vec![]) };
        let (env_start, env_end) = {
            let mm_state = mm.state.read();
            (mm_state.environ_start, mm_state.environ_end)
        };

        let len_to_read = std::cmp::min(env_end - env_start, max_len);
        self.read_nul_delimited_c_string_list(env_start, len_to_read)
    }

    pub fn thread_runtime_info(&self) -> Result<zx::TaskRuntimeInfo, Errno> {
        self.running_state()?
            .thread
            .get()
            .ok_or_else(|| errno!(EINVAL))?
            .get_runtime_info()
            .map_err(|status| from_status_like_fdio!(status))
    }

    pub fn real_fscred(&self) -> FsCred {
        self.real_creds().as_fscred()
    }

    /// Interrupts the current task.
    ///
    /// This will interrupt any blocking syscalls if the task is blocked on one.
    /// The signal_state of the task must not be locked.
    pub fn interrupt(&self) {
        let Ok(running_state) = self.running_state() else {
            log_warn!("Cannot interrupt dead task {}", self.get_tid());
            return;
        };

        self.read().run_state.wake();
        if let Some(thread) = running_state.thread.get() {
            #[allow(
                clippy::undocumented_unsafe_blocks,
                reason = "Force documented unsafe blocks in Starnix"
            )]
            let status = unsafe { zx::sys::zx_restricted_kick(thread.raw_handle(), 0) };
            if status != zx::sys::ZX_OK {
                // zx_restricted_kick() could return ZX_ERR_BAD_STATE if the target thread is already in the
                // DYING or DEAD states. That's fine since it means that the task is in the process of
                // tearing down, so allow it.
                assert_eq!(status, zx::sys::ZX_ERR_BAD_STATE);
            }
        }
    }

    pub fn command(&self) -> TaskCommand {
        self.persistent_info.command.lock().clone()
    }

    pub fn set_command_name(&self, mut new_name: TaskCommand) {
        let Ok(running_state) = self.running_state() else {
            log_warn!("Cannot set command name for dead task {}", self.get_tid());
            return;
        };

        // If we're going to update the process name, see if we can get a longer one than normally
        // provided in the Linux uapi. Only choose the argv0-based name if it's a superset of the
        // uapi-provided name to avoid clobbering the name provided by the user.
        if let Ok(argv0) = self.read_argv0() {
            let argv0 = TaskCommand::from_path_bytes(&argv0);
            if let Some(embedded_name) = argv0.try_embed(&new_name) {
                new_name = embedded_name;
            }
        }

        // Acquire this before modifying Zircon state to ensure consistency under concurrent access.
        // Ideally this would also guard the logic above to read argv[0] but we can't due to lock
        // cycles with SELinux checks.
        let mut command_guard = self.persistent_info.command_guard();

        // Set the name on the Linux thread.
        if let Some(thread) = running_state.thread.get() {
            set_zx_name(thread.thread.as_ref(), new_name.as_bytes());
        }

        // If this is the thread group leader, use this name for the process too.
        if self.is_leader() {
            set_zx_name(&*self.thread_group().process, new_name.as_bytes());
            let _ = zx::Thread::raise_user_exception(
                zx::RaiseExceptionOptions::TARGET_JOB_DEBUGGER,
                zx::sys::ZX_EXCP_USER_CODE_PROCESS_NAME_CHANGED,
                0,
            );
        }

        // Avoid a lock cycle by dropping the guard before notifying memory attribution of the
        // change.
        *command_guard = new_name;
        drop(command_guard);

        if self.is_leader() {
            if let Some(notifier) = &self.thread_group().read().notifier {
                let _ = notifier.send(MemoryAttributionLifecycleEvent::name_change(self.tid.id));
            }
        }

        if let Err(err) = self.sync_scheduler_state_to_role() {
            log_warn!(err:?; "Failed to update scheduler role after thread name change.");
        }
    }

    pub fn set_seccomp_state(&self, state: SeccompStateValue) -> Result<(), Errno> {
        self.seccomp_filter_state.set(&state)
    }

    pub fn state_code(&self) -> TaskStateCode {
        let status = self.read();
        if status.exit_status.is_some() {
            TaskStateCode::Zombie
        } else if status.run_state.is_blocked() {
            let stop_state = self.load_stopped();
            if stop_state.ptrace_only() && stop_state.is_stopped() {
                TaskStateCode::TracingStop
            } else {
                TaskStateCode::Sleeping
            }
        } else {
            TaskStateCode::Running
        }
    }

    pub fn time_stats(&self) -> TaskTimeStats {
        use zx::Task;
        // TODO(https://fxbug.dev/297440106): Return time stats for zombie tasks.
        let running_state = match self.running_state() {
            Ok(running_state) => running_state,
            Err(_) => return TaskTimeStats::default(),
        };
        let info = match running_state.thread.get() {
            Some(thread) => thread.get_runtime_info().expect("Failed to get thread stats"),
            None => return TaskTimeStats::default(),
        };

        TaskTimeStats {
            user_time: zx::MonotonicDuration::from_nanos(info.cpu_time),
            // TODO(https://fxbug.dev/42078242): How can we calculate system time?
            system_time: zx::MonotonicDuration::default(),
        }
    }

    pub fn get_signal_action(&self, signal: Signal) -> sigaction_t {
        self.thread_group().signal_actions.get(signal)
    }

    pub fn should_check_for_pending_signals(&self) -> bool {
        self.flags().intersects(
            TaskFlags::KERNEL_SIGNALS_AVAILABLE
                | TaskFlags::SIGNALS_AVAILABLE
                | TaskFlags::TEMPORARY_SIGNAL_MASK,
        ) || self.thread_group.has_pending_signals.load(Ordering::Relaxed)
    }

    /// Returns the process and thread KOIDs for this task if both are available.
    ///
    /// The thread handle is always attached before task startup; a task without a valid
    /// backing process (a Starnix kernel thread) is intentionally excluded.
    pub fn get_zircon_identity(&self) -> Option<ZirconIdentity> {
        let process = self.thread_group().get_process_koid().ok()?;
        let thread = self.running_state().ok()?.thread.get()?.koid;
        Some(ZirconIdentity { process, thread })
    }

    /// Record the pid - koid mapping for tracing and profiling tools.
    pub fn record_pid_koid_mapping(&self) {
        if !self.kernel().trace_event_manager.is_recording() {
            return;
        }
        if let Some(identity) = self.get_zircon_identity() {
            self.kernel().trace_event_manager.record(self.get_pid(), self.get_tid(), identity);
        }
    }
}

impl Drop for Task {
    fn drop(&mut self) {
        debug_assert!(self.running_state.is_none());
    }
}

impl MemoryAccessor for Task {
    fn read_memory<'a>(
        &self,
        addr: UserAddress,
        bytes: &'a mut [MaybeUninit<u8>],
    ) -> Result<&'a mut [u8], Errno> {
        // Using a `Task` to read memory generally indicates that the memory
        // is being read from a task different than the `CurrentTask`. When
        // this `Task` is not current, its address space is not mapped
        // so we need to go through the VMO.
        self.mm()?.syscall_read_memory(addr, bytes)
    }

    fn read_memory_partial_until_null_byte<'a>(
        &self,
        addr: UserAddress,
        bytes: &'a mut [MaybeUninit<u8>],
    ) -> Result<&'a mut [u8], Errno> {
        // Using a `Task` to read memory generally indicates that the memory
        // is being read from a task different than the `CurrentTask`. When
        // this `Task` is not current, its address space is not mapped
        // so we need to go through the VMO.
        self.mm()?.syscall_read_memory_partial_until_null_byte(addr, bytes)
    }

    fn read_memory_partial<'a>(
        &self,
        addr: UserAddress,
        bytes: &'a mut [MaybeUninit<u8>],
    ) -> Result<&'a mut [u8], Errno> {
        // Using a `Task` to read memory generally indicates that the memory
        // is being read from a task different than the `CurrentTask`. When
        // this `Task` is not current, its address space is not mapped
        // so we need to go through the VMO.
        self.mm()?.syscall_read_memory_partial(addr, bytes)
    }

    fn write_memory(&self, addr: UserAddress, bytes: &[u8]) -> Result<usize, Errno> {
        // Using a `Task` to write memory generally indicates that the memory
        // is being written to a task different than the `CurrentTask`. When
        // this `Task` is not current, its address space is not mapped
        // so we need to go through the VMO.
        self.mm()?.syscall_write_memory(addr, bytes)
    }

    fn write_memory_partial(&self, addr: UserAddress, bytes: &[u8]) -> Result<usize, Errno> {
        // Using a `Task` to write memory generally indicates that the memory
        // is being written to a task different than the `CurrentTask`. When
        // this `Task` is not current, its address space is not mapped
        // so we need to go through the VMO.
        self.mm()?.syscall_write_memory_partial(addr, bytes)
    }

    fn zero(&self, addr: UserAddress, length: usize) -> Result<usize, Errno> {
        // Using a `Task` to zero memory generally indicates that the memory
        // is being zeroed from a task different than the `CurrentTask`. When
        // this `Task` is not current, its address space is not mapped
        // so we need to go through the VMO.
        self.mm()?.syscall_zero(addr, length)
    }
}

impl TaskMemoryAccessor for Task {
    fn maximum_valid_address(&self) -> Option<UserAddress> {
        self.mm().map(|mm| mm.maximum_valid_user_address).ok()
    }
}

impl fmt::Debug for Task {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}[{}]", self.pid, self.tid, *self.persistent_info.command.lock())
    }
}

impl cmp::PartialEq for Task {
    fn eq(&self, other: &Self) -> bool {
        let ptr: *const Task = self;
        let other_ptr: *const Task = other;
        ptr == other_ptr
    }
}

impl cmp::Eq for Task {}

#[cfg(test)]
mod test {
    use super::*;
    use crate::security;
    use crate::testing::*;
    use starnix_uapi::auth::{CAP_SYS_ADMIN, Capabilities};
    use starnix_uapi::resource_limits::Resource;
    use starnix_uapi::signals::SIGCHLD;
    use starnix_uapi::{CLONE_SIGHAND, CLONE_THREAD, CLONE_VM, rlimit};

    #[::fuchsia::test]
    async fn test_tid_allocation() {
        spawn_kernel_and_run(async |current_task| {
            let kernel = current_task.kernel();
            assert_eq!(current_task.get_tid(), 1);
            let another_current = create_task(&kernel, "another-task");
            let another_tid = another_current.get_tid();
            assert!(another_tid >= 2);

            let pids = &kernel.pids;
            assert_eq!(pids.get(1).unwrap().get_task().unwrap().get_tid(), 1);
            assert_eq!(pids.get(another_tid).unwrap().get_task().unwrap().get_tid(), another_tid);
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_clone_pid_and_parent_pid() {
        spawn_kernel_and_run(async |current_task| {
            let thread = current_task.clone_task_for_test(
                (CLONE_THREAD | CLONE_VM | CLONE_SIGHAND) as u64,
                Some(SIGCHLD),
            );
            assert_eq!(current_task.get_pid(), thread.get_pid());
            assert_ne!(current_task.get_tid(), thread.get_tid());
            assert_eq!(current_task.pid, thread.thread_group().leader);

            let child_task = current_task.clone_task_for_test(0, Some(SIGCHLD));
            assert_ne!(current_task.get_pid(), child_task.get_pid());
            assert_ne!(current_task.get_tid(), child_task.get_tid());
            assert_eq!(current_task.get_pid(), child_task.thread_group().read().get_ppid());
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_root_capabilities() {
        spawn_kernel_and_run(async |current_task| {
            assert!(security::is_task_capable_noaudit(current_task, CAP_SYS_ADMIN));
            assert_eq!(current_task.real_creds().cap_inheritable, Capabilities::empty());

            current_task.set_creds(Credentials::with_ids(1, 1));
            assert!(!security::is_task_capable_noaudit(current_task, CAP_SYS_ADMIN));
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_is_spawned() {
        spawn_kernel_and_run(async |current_task| {
            // The init task should be marked as spawned, because it is executing.
            assert!(current_task.is_spawned());

            // A cloned task should not be marked as spawned, because it has not yet been executed.
            let child = current_task
                .clone_task(
                    0,
                    Some(SIGCHLD),
                    UserRef::default(),
                    UserRef::default(),
                    UserRef::default(),
                )
                .expect("failed to create task in test");
            assert!(!child.is_spawned());
            child.release(());

            // A cloned task for a test should be marked as spawned, because we intentionally avoid
            // spawning threads for test tasks but want them to behave as normal tasks.
            let test_child = current_task.clone_task_for_test(0, Some(SIGCHLD));
            assert!(test_child.is_spawned());
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_clone_rlimit() {
        spawn_kernel_and_run(async |current_task| {
            let prev_fsize = current_task.thread_group().get_rlimit(Resource::FSIZE);
            assert_ne!(prev_fsize, 10);
            current_task
                .thread_group()
                .limits
                .lock()
                .set(Resource::FSIZE, rlimit { rlim_cur: 10, rlim_max: 100 });
            let current_fsize = current_task.thread_group().get_rlimit(Resource::FSIZE);
            assert_eq!(current_fsize, 10);

            let child_task = current_task.clone_task_for_test(0, Some(SIGCHLD));
            let child_fsize = child_task.thread_group().get_rlimit(Resource::FSIZE);
            assert_eq!(child_fsize, 10)
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_set_command_name_syncs_scheduler_role() {
        use crate::task::{RoleOverrides, SchedulerManager};

        let mut builder = RoleOverrides::new();
        builder.add("renamed-thread", "renamed-thread", None, "test-role");
        let overrides = builder.build().unwrap();

        let scheduler_manager = SchedulerManager::new_for_tests(None, overrides);

        spawn_kernel_with_scheduler_and_run_sync(scheduler_manager, |current_task| {
            // Set did_exec = true so custom role overrides are applied.
            current_task.thread_group().write().did_exec = true;

            let scheduler = &current_task.thread_group().kernel.scheduler;

            // Before rename, check task's role name.
            let initial_role = scheduler.role_name(current_task).unwrap();
            assert_ne!(initial_role, "test-role");

            // Rename the task's thread to renamed-thread.
            current_task
                .set_command_name(starnix_task_command::TaskCommand::new(b"renamed-thread"));

            let renamed_role = scheduler.role_name(current_task).unwrap();
            assert_eq!(renamed_role, "test-role");
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_fork_does_not_inherit_custom_role() {
        use crate::task::{RoleOverrides, SchedulerManager};

        let mut builder = RoleOverrides::new();
        builder.add("renamed-thread", "renamed-thread", None, "test-role");
        let overrides = builder.build().unwrap();

        let scheduler_manager = SchedulerManager::new_for_tests(None, overrides);

        spawn_kernel_with_scheduler_and_run_sync(scheduler_manager, |current_task| {
            // Fork a child process (which sets did_exec = false on the child's thread group)
            let child = current_task.clone_task_for_test(0, None);

            let scheduler = &current_task.thread_group().kernel.scheduler;

            // Before rename, check child's role name.
            let initial_role = scheduler.role_name(&child).unwrap();
            assert_ne!(initial_role, "test-role");

            // Rename the child's thread to renamed-thread. Since did_exec is false on the child,
            // this should NOT map to "test-role" from the overrides.
            child.set_command_name(starnix_task_command::TaskCommand::new(b"renamed-thread"));

            let renamed_role = scheduler.role_name(&child).unwrap();
            assert_ne!(renamed_role, "test-role");
        })
        .await;
    }
}
