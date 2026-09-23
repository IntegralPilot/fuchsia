// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::arch::execution::new_syscall_from_state;
use crate::mm::{IOVecPtr, MemoryAccessor, MemoryAccessorExt};
use crate::ptrace::StopState;
use crate::security;
use crate::signals::syscalls::WaitingOptions;
use crate::signals::{
    SignalDetail, SignalInfo, UncheckedSignalInfo, send_signal_first, send_standard_signal,
};
use crate::task::{
    CurrentTask, Pid, PidTableGuard, ProcessSelector, Task, TaskMutableState, ThreadGroup,
    ThreadState, WaitQueue, ZombieNotification, ZombieProcess,
};
use bitflags::bitflags;
use starnix_logging::track_stub;
use starnix_registers::HeapRegs;
use starnix_syscalls::SyscallResult;
use starnix_syscalls::decls::SyscallDecl;
use starnix_types::ownership::{OwnedRef, Releasable};
use starnix_uapi::auth::PTRACE_MODE_ATTACH_REALCREDS;
use starnix_uapi::elf::ElfNoteType;
use starnix_uapi::errors::Errno;
use starnix_uapi::signals::{SIGKILL, SIGSTOP, SIGTRAP, SigSet, Signal, UncheckedSignal};
#[allow(unused_imports)]
use starnix_uapi::user_address::ArchSpecific;
use starnix_uapi::user_address::{LongPtr, MultiArchUserRef, UserAddress, UserRef};
use starnix_uapi::{
    PTRACE_CONT, PTRACE_DETACH, PTRACE_EVENT_CLONE, PTRACE_EVENT_EXEC, PTRACE_EVENT_EXIT,
    PTRACE_EVENT_FORK, PTRACE_EVENT_SECCOMP, PTRACE_EVENT_STOP, PTRACE_EVENT_VFORK,
    PTRACE_EVENT_VFORK_DONE, PTRACE_GET_SYSCALL_INFO, PTRACE_GETEVENTMSG, PTRACE_GETREGSET,
    PTRACE_GETSIGINFO, PTRACE_GETSIGMASK, PTRACE_INTERRUPT, PTRACE_KILL, PTRACE_LISTEN,
    PTRACE_O_EXITKILL, PTRACE_O_TRACECLONE, PTRACE_O_TRACEEXEC, PTRACE_O_TRACEEXIT,
    PTRACE_O_TRACEFORK, PTRACE_O_TRACESYSGOOD, PTRACE_O_TRACEVFORK, PTRACE_O_TRACEVFORKDONE,
    PTRACE_PEEKDATA, PTRACE_PEEKTEXT, PTRACE_PEEKUSR, PTRACE_POKEDATA, PTRACE_POKETEXT,
    PTRACE_POKEUSR, PTRACE_SETOPTIONS, PTRACE_SETREGSET, PTRACE_SETSIGINFO, PTRACE_SETSIGMASK,
    PTRACE_SYSCALL, PTRACE_SYSCALL_INFO_ENTRY, PTRACE_SYSCALL_INFO_EXIT, PTRACE_SYSCALL_INFO_NONE,
    clone_args, errno, error, ptrace_syscall_info, uapi,
};
use zerocopy::IntoBytes;

use std::collections::BTreeMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Weak};

#[cfg(target_arch = "x86_64")]
use starnix_uapi::{PTRACE_GETREGS, user};

#[cfg(all(target_arch = "aarch64"))]
use starnix_uapi::arch32::PTRACE_GETREGS;

type UserRegsStructPtr =
    MultiArchUserRef<starnix_uapi::user_regs_struct, starnix_uapi::arch32::user_regs_struct>;

uapi::check_arch_independent_layout! {
    ptrace_syscall_info {
        op,
        arch,
        instruction_pointer,
        stack_pointer,
        __bindgen_anon_1,
    }

    ptrace_syscall_info__bindgen_ty_1 {
        entry,
        exit,
        seccomp,
    }

    ptrace_syscall_info__bindgen_ty_1__bindgen_ty_1 {
        nr,
        args,
    }

    ptrace_syscall_info__bindgen_ty_1__bindgen_ty_2 {
        rval,
        is_error,
    }

    ptrace_syscall_info__bindgen_ty_1__bindgen_ty_3 {
        nr,
        args,
        ret_data,
    }
}

/// For most of the time, for the purposes of ptrace, a tracee is either "going"
/// or "stopped".  However, after certain ptrace calls, there are special rules
/// to be followed.
#[derive(Clone, Default, PartialEq)]
pub enum PtraceStatus {
    /// Proceed as otherwise indicated by the task's stop status.
    #[default]
    Default,
    /// Resuming after a ptrace_cont with a signal, so do not stop for signal-delivery-stop
    Continuing,
    /// "The state of the tracee after PTRACE_LISTEN is somewhat of a
    /// gray area: it is not in any ptrace-stop (ptrace commands won't work on it,
    /// and it will deliver waitpid(2) notifications), but it also may be considered
    /// "stopped" because it is not executing instructions (is not scheduled), and
    /// if it was in group-stop before PTRACE_LISTEN, it will not respond to signals
    /// until SIGCONT is received."
    Listening,
}

impl PtraceStatus {
    pub fn is_continuing(&self) -> bool {
        *self == PtraceStatus::Continuing
    }
}

/// Indicates the way that ptrace attached to the task.
#[derive(Copy, Clone, PartialEq)]
pub enum PtraceAttachType {
    /// Attached with PTRACE_ATTACH
    Attach,
    /// Attached with PTRACE_SEIZE
    Seize,
}

bitflags! {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    #[repr(transparent)]
    pub struct PtraceOptions: u32 {
        const EXITKILL = starnix_uapi::PTRACE_O_EXITKILL;
        const TRACECLONE = starnix_uapi::PTRACE_O_TRACECLONE;
        const TRACEEXEC = starnix_uapi::PTRACE_O_TRACEEXEC;
        const TRACEEXIT = starnix_uapi::PTRACE_O_TRACEEXIT;
        const TRACEFORK = starnix_uapi::PTRACE_O_TRACEFORK;
        const TRACESYSGOOD = starnix_uapi::PTRACE_O_TRACESYSGOOD;
        const TRACEVFORK = starnix_uapi::PTRACE_O_TRACEVFORK;
        const TRACEVFORKDONE = starnix_uapi::PTRACE_O_TRACEVFORKDONE;
        const TRACESECCOMP = starnix_uapi::PTRACE_O_TRACESECCOMP;
        const SUSPEND_SECCOMP = starnix_uapi::PTRACE_O_SUSPEND_SECCOMP;
    }
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PtraceEvent {
    #[default]
    None = 0,
    Stop = PTRACE_EVENT_STOP,
    Clone = PTRACE_EVENT_CLONE,
    Fork = PTRACE_EVENT_FORK,
    Vfork = PTRACE_EVENT_VFORK,
    VforkDone = PTRACE_EVENT_VFORK_DONE,
    Exec = PTRACE_EVENT_EXEC,
    Exit = PTRACE_EVENT_EXIT,
    Seccomp = PTRACE_EVENT_SECCOMP,
}

impl PtraceEvent {
    pub fn from_option(option: &PtraceOptions) -> Self {
        match *option {
            PtraceOptions::TRACECLONE => PtraceEvent::Clone,
            PtraceOptions::TRACEFORK => PtraceEvent::Fork,
            PtraceOptions::TRACEVFORK => PtraceEvent::Vfork,
            PtraceOptions::TRACEVFORKDONE => PtraceEvent::VforkDone,
            PtraceOptions::TRACEEXEC => PtraceEvent::Exec,
            PtraceOptions::TRACEEXIT => PtraceEvent::Exit,
            PtraceOptions::TRACESECCOMP => PtraceEvent::Seccomp,
            _ => unreachable!("Bad ptrace event specified"),
        }
    }
}

/// Information about what caused a ptrace-event-stop.
pub struct PtraceEventData {
    /// The event that caused the task to stop (e.g., PTRACE_EVENT_TRACEFORK or PTRACE_EVENT_EXIT).
    pub event: PtraceEvent,

    /// The message associated with the event (e.g., tid, exit status)..
    pub msg: u64,
}

impl PtraceEventData {
    pub fn new(option: PtraceOptions, msg: u64) -> Self {
        Self { event: PtraceEvent::from_option(&option), msg }
    }
    pub fn new_from_event(event: PtraceEvent, msg: u64) -> Self {
        Self { event, msg }
    }
}

/// The ptrace state that a new task needs to connect to the same tracer as the
/// task that clones it.
#[derive(Clone)]
pub struct PtraceCoreState {
    /// The task of the tracer
    pub task: Weak<Task>,

    /// The thread group of the tracer
    pub thread_group: Weak<ThreadGroup>,

    /// Whether the attach was a seize or an attach.  There are a few subtle
    /// differences in behavior of the different attach types - see ptrace(2).
    pub attach_type: PtraceAttachType,

    /// The options set by PTRACE_SETOPTIONS
    pub options: PtraceOptions,

    /// The tracer waits on this WaitQueue to find out if the tracee has done
    /// something worth being notified about.
    pub tracer_waiters: Arc<WaitQueue>,
}

impl PtraceCoreState {
    pub fn has_option(&self, option: PtraceOptions) -> bool {
        self.options.contains(option)
    }
}

/// Per-task ptrace-related state
pub struct PtraceState {
    /// The core state of the tracer, which can be shared between processes
    pub core_state: PtraceCoreState,

    /// The tracee waits on this WaitQueue to find out when it should stop or wake
    /// for ptrace-related shenanigans.
    pub tracee_waiters: WaitQueue,

    /// The signal that caused the task to enter the given state (for
    /// signal-delivery-stop)
    pub last_signal: Option<SignalInfo>,

    /// Whether waitpid() will return the last signal.  The presence of last_signal
    /// can't be used for that, because that needs to be saved for GETSIGINFO.
    pub last_signal_waitable: bool,

    /// Data about the PTRACE_EVENT that caused the most recent stop (if any).
    pub event_data: Option<PtraceEventData>,

    /// Indicates whether the last ptrace call put this thread into a state with
    /// special semantics for stopping behavior.
    pub stop_status: PtraceStatus,

    /// For SYSCALL_INFO_EXIT
    pub last_syscall_was_error: bool,
}

impl PtraceState {
    pub fn new(
        task: Weak<Task>,
        thread_group: Weak<ThreadGroup>,
        attach_type: PtraceAttachType,
        options: PtraceOptions,
    ) -> Box<Self> {
        Box::new(PtraceState {
            core_state: PtraceCoreState {
                task,
                thread_group,
                attach_type,
                options,
                tracer_waiters: Arc::new(WaitQueue::default()),
            },
            tracee_waiters: WaitQueue::default(),
            last_signal: None,
            last_signal_waitable: false,
            event_data: None,
            stop_status: PtraceStatus::default(),
            last_syscall_was_error: false,
        })
    }

    pub fn is_seized(&self) -> bool {
        self.core_state.attach_type == PtraceAttachType::Seize
    }

    pub fn get_attach_type(&self) -> PtraceAttachType {
        self.core_state.attach_type
    }

    pub fn is_waitable(&self, stop: StopState, options: &WaitingOptions) -> bool {
        if self.stop_status == PtraceStatus::Listening {
            // Waiting for any change of state
            return self.last_signal_waitable;
        }
        if !options.wait_for_continued && !stop.is_stopping_or_stopped() {
            // Only waiting for stops, but is not stopped.
            return false;
        }
        self.last_signal_waitable && !stop.is_in_progress()
    }

    pub fn set_last_signal(&mut self, mut signal: Option<SignalInfo>) {
        if let Some(ref mut siginfo) = signal {
            // We don't want waiters to think the process was unstopped because
            // of a sigkill. They will get woken when the process dies.
            if siginfo.signal == SIGKILL {
                return;
            }
            self.last_signal_waitable = true;
            self.last_signal = signal;
        }
    }

    pub fn set_last_event(&mut self, event: Option<PtraceEventData>) {
        if event.is_some() {
            self.event_data = event;
        }
    }

    pub fn get_last_signal_ref(&self) -> Option<&SignalInfo> {
        self.last_signal.as_ref()
    }

    // Gets the last signal, and optionally clears the wait state of the ptrace.
    pub fn get_last_signal(&mut self, keep_signal_waitable: bool) -> Option<SignalInfo> {
        self.last_signal_waitable = keep_signal_waitable;
        self.last_signal.clone()
    }

    pub fn has_option(&self, option: PtraceOptions) -> bool {
        self.core_state.has_option(option)
    }

    pub fn set_options_from_bits(&mut self, option: u32) -> Result<(), Errno> {
        if let Some(options) = PtraceOptions::from_bits(option) {
            self.core_state.options = options;
            Ok(())
        } else {
            error!(EINVAL)
        }
    }

    pub fn get_options(&self) -> PtraceOptions {
        self.core_state.options
    }

    /// Returns enough of the ptrace state to propagate it to a fork / clone / vforked task.
    pub fn get_core_state(&self) -> PtraceCoreState {
        self.core_state.clone()
    }

    pub fn tracer_waiters(&self) -> &Arc<WaitQueue> {
        &self.core_state.tracer_waiters
    }

    /// Returns an (i32, ptrace_syscall_info) pair.  The ptrace_syscall_info is
    /// the info associated with the syscall that the target task is currently
    /// blocked on, The i32 is (per ptrace(2)) "the number of bytes available to
    /// be written by the kernel.  If the size of the data to be written by the
    /// kernel exceeds the size specified by the addr argument, the output data
    /// is truncated."; ptrace(PTRACE_GET_SYSCALL_INFO) returns that value"
    pub fn get_target_syscall(
        &self,
        target: &Task,
        state: &TaskMutableState,
    ) -> Result<(i32, ptrace_syscall_info), Errno> {
        #[cfg(target_arch = "x86_64")]
        let arch = starnix_uapi::AUDIT_ARCH_X86_64;
        #[cfg(target_arch = "aarch64")]
        let arch = starnix_uapi::AUDIT_ARCH_AARCH64;
        #[cfg(target_arch = "riscv64")]
        let arch = starnix_uapi::AUDIT_ARCH_RISCV64;

        let mut info = ptrace_syscall_info { arch, ..Default::default() };
        let mut info_len = memoffset::offset_of!(ptrace_syscall_info, __bindgen_anon_1);

        match &state.captured_thread_state {
            Some(captured) => {
                let registers = captured.thread_state.registers.clone();
                info.instruction_pointer = registers.instruction_pointer_register();
                info.stack_pointer = registers.stack_pointer_register();
                #[cfg(target_arch = "aarch64")]
                if captured.thread_state.is_arch32() {
                    // If any additional arch32 archs are added, just use a cfg
                    // macro here.
                    info.arch = starnix_uapi::AUDIT_ARCH_ARM;
                }
                match target.load_stopped() {
                    StopState::SyscallEnterStopped => {
                        let syscall_decl = SyscallDecl::from_number(
                            registers.syscall_register(),
                            captured.thread_state.arch_width(),
                        );
                        let syscall = new_syscall_from_state(syscall_decl, &captured.thread_state);
                        info.op = PTRACE_SYSCALL_INFO_ENTRY as u8;
                        let entry = linux_uapi::ptrace_syscall_info__bindgen_ty_1__bindgen_ty_1 {
                            nr: syscall.decl.number,
                            args: [
                                syscall.arg0.raw(),
                                syscall.arg1.raw(),
                                syscall.arg2.raw(),
                                syscall.arg3.raw(),
                                syscall.arg4.raw(),
                                syscall.arg5.raw(),
                            ],
                        };
                        info_len += memoffset::offset_of!(
                            linux_uapi::ptrace_syscall_info__bindgen_ty_1__bindgen_ty_1,
                            args
                        ) + std::mem::size_of_val(&entry.args);
                        info.__bindgen_anon_1.entry = entry;
                    }
                    StopState::SyscallExitStopped => {
                        info.op = PTRACE_SYSCALL_INFO_EXIT as u8;
                        let exit = linux_uapi::ptrace_syscall_info__bindgen_ty_1__bindgen_ty_2 {
                            rval: registers.return_register() as i64,
                            is_error: state
                                .ptrace
                                .as_ref()
                                .map_or(0, |ptrace| ptrace.last_syscall_was_error as u8),
                            ..Default::default()
                        };
                        info_len += memoffset::offset_of!(
                            linux_uapi::ptrace_syscall_info__bindgen_ty_1__bindgen_ty_2,
                            is_error
                        ) + std::mem::size_of_val(&exit.is_error);
                        info.__bindgen_anon_1.exit = exit;
                    }
                    _ => {
                        info.op = PTRACE_SYSCALL_INFO_NONE as u8;
                    }
                };
            }
            _ => (),
        }
        Ok((info_len as i32, info))
    }

    /// Gets the core state for this ptrace if the options set on this ptrace
    /// match |trace_kind|.  Returns a pair: the trace option you *should* use
    /// (sometimes this is different from the one that the caller thinks it
    /// should use), and the core state.
    pub fn get_core_state_for_clone(
        &self,
        clone_args: &clone_args,
    ) -> (PtraceOptions, Option<PtraceCoreState>) {
        // ptrace(2): If the tracee calls clone(2) with the CLONE_VFORK flag,
        // PTRACE_EVENT_VFORK will be delivered instead if PTRACE_O_TRACEVFORK
        // is set, otherwise if the tracee calls clone(2) with the exit signal
        // set to SIGCHLD, PTRACE_EVENT_FORK will be delivered if
        // PTRACE_O_TRACEFORK is set.
        let trace_type = if clone_args.flags & (starnix_uapi::CLONE_UNTRACED as u64) != 0 {
            PtraceOptions::empty()
        } else {
            if clone_args.flags & (starnix_uapi::CLONE_VFORK as u64) != 0 {
                PtraceOptions::TRACEVFORK
            } else if clone_args.exit_signal != (starnix_uapi::SIGCHLD as u64) {
                PtraceOptions::TRACECLONE
            } else {
                PtraceOptions::TRACEFORK
            }
        };

        if !self.has_option(trace_type)
            && (clone_args.flags & (starnix_uapi::CLONE_PTRACE as u64) == 0)
        {
            return (PtraceOptions::empty(), None);
        }

        (trace_type, Some(self.get_core_state()))
    }
}

/// A zombie that must delivered to a tracer process for a traced process.
struct TracedZombie {
    /// An artificial zombie that must be delivered to the tracer program.
    artificial_zombie: ZombieProcess,

    /// The real zombie notification to be sent after the artificial zombie has been delivered to
    /// the tracer.
    notification: Option<ZombieNotification>,
}

impl TracedZombie {
    fn new(artificial_zombie: ZombieProcess) -> Self {
        Self { artificial_zombie, notification: None }
    }

    fn new_with_notification(
        artificial_zombie: ZombieProcess,
        notification: ZombieNotification,
    ) -> Self {
        Self { artificial_zombie, notification: Some(notification) }
    }

    fn set_parent(
        &mut self,
        new_zombie: Option<OwnedRef<ZombieProcess>>,
        new_parent: &ThreadGroup,
    ) {
        if let Some(new_zombie) = new_zombie {
            self.notification = Some(ZombieNotification {
                recipient: new_parent.weak_self.clone(),
                zombie: new_zombie,
            });
        } else if let Some(ref mut notification) = self.notification {
            notification.recipient = new_parent.weak_self.clone();
        }
    }

    fn detach(self, pids: &mut PidTableGuard<'_>) -> Option<ZombieNotification> {
        self.artificial_zombie.release(pids);
        self.notification
    }
}

/// A list of zombie processes that were traced by a given tracer, but which
/// have not yet notified that tracer of their exit.  Once the tracer is
/// notified, the original parent will be notified.
#[derive(Default)]
pub struct ZombiePtracees {
    /// A list of zombies that have to be delivered to the ptracer.  The key is
    /// the tid of the traced process.
    zombies: BTreeMap<Pid, TracedZombie>,
}

impl Drop for ZombiePtracees {
    fn drop(&mut self) {
        assert_eq!(self.zombies.len(), 0);
    }
}

impl ZombiePtracees {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a zombie tracee to the list, but does not provide a parent task to
    /// notify when the tracer is done.
    pub fn add(&mut self, pids: &mut PidTableGuard<'_>, tid: Pid, zombie: ZombieProcess) {
        if let std::collections::btree_map::Entry::Vacant(entry) = self.zombies.entry(tid) {
            entry.insert(TracedZombie::new(zombie));
        } else {
            zombie.release(pids);
        }
    }

    /// Detaches from the zombie tracee with the given TID.
    ///
    /// Returns the notification to deliver to the tracee's real parent.
    pub fn detach(
        &mut self,
        pids: &mut PidTableGuard<'_>,
        tid: &Pid,
    ) -> Option<ZombieNotification> {
        self.zombies.remove(tid).and_then(|traced_zombie| traced_zombie.detach(pids))
    }

    /// Detaches from every zombie tracee.
    ///
    /// Returns the notifications to deliver to the tracees' real parents.
    pub fn detach_all(&mut self, pids: &mut PidTableGuard<'_>) -> Vec<ZombieNotification> {
        let traced_zombies = std::mem::replace(&mut self.zombies, Default::default());
        traced_zombies
            .into_iter()
            .filter_map(|(_, traced_zombie)| traced_zombie.detach(pids))
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.zombies.is_empty()
    }

    /// Provide a parent task and a zombie to notify when the tracer has been
    /// notified.
    pub fn set_parent_of(
        &mut self,
        tracee: &Pid,
        new_zombie: Option<OwnedRef<ZombieProcess>>,
        new_parent: &ThreadGroup,
    ) {
        match self.zombies.entry(tracee.clone()) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                if let Some(new_zombie) = new_zombie {
                    entry.insert(TracedZombie::new_with_notification(
                        new_zombie.as_artificial(),
                        ZombieNotification::new(new_parent.weak_self.clone(), new_zombie),
                    ));
                }
            }
            std::collections::btree_map::Entry::Occupied(mut entry) => {
                entry.get_mut().set_parent(new_zombie, new_parent);
            }
        }
    }

    /// When a parent dies without having been notified, replace it with a given
    /// new parent.
    pub fn reparent(old_parent: &ThreadGroup, new_parent: &ThreadGroup) {
        let mut lockless_list = old_parent.read().deferred_zombie_ptracers.clone();

        for deferred_zombie_ptracer in &lockless_list {
            if let Ok(tg) = deferred_zombie_ptracer.tracer_pid.get_thread_group() {
                tg.write().zombie_ptracees.set_parent_of(
                    &deferred_zombie_ptracer.tracee_tid,
                    None,
                    new_parent,
                );
            }
        }
        let mut new_state = new_parent.write();
        new_state.deferred_zombie_ptracers.append(&mut lockless_list);
    }

    /// Returns true iff there is a zombie waiting to be delivered to the tracers matching the
    /// given selector.
    pub fn has_zombie_matching(&self, selector: &ProcessSelector) -> bool {
        self.zombies.values().any(|z| z.artificial_zombie.matches_selector(selector))
    }

    /// Returns true iff the given `tid` is a traced thread that needs to deliver a zombie to the
    /// tracer.
    pub fn has_tracee(&self, tid: &Pid) -> bool {
        self.zombies.contains_key(tid)
    }

    /// Returns a zombie matching the given selector and options, and
    /// (optionally) a thread group to notify after the caller has consumed that
    /// zombie.
    pub fn get_waitable_entry(
        &mut self,
        selector: &ProcessSelector,
        options: &WaitingOptions,
    ) -> Option<(ZombieProcess, Option<(Weak<ThreadGroup>, OwnedRef<ZombieProcess>)>)> {
        // Look for the last zombie in the map that matches process
        // selector and waiting options.
        let Some((t, found_zombie)) = self
            .zombies
            .iter()
            .map(|(t, z)| (t.clone(), &z.artificial_zombie))
            .rfind(|(_, zombie)| zombie.matches_selector_and_waiting_option(selector, options))
        else {
            return None;
        };

        let result;
        if !options.keep_waitable_state {
            // Maybe notify child waiters.
            result = self.zombies.remove(&t).map(|traced_zombie| {
                (
                    traced_zombie.artificial_zombie,
                    traced_zombie.notification.map(|n| (n.recipient, n.zombie)),
                )
            });
        } else {
            result = Some((found_zombie.as_artificial(), None));
        }

        result
    }
}

// PR_SET_PTRACER_ANY is defined as ((unsigned long) -1) in Linux UAPI.
pub const PR_SET_PTRACER_ANY: u64 = u64::MAX;
pub const PR_SET_PTRACER_ANY_ARCH32: u64 = u32::MAX as u64;

/// Indicates processes specifically allowed to trace a given process if using
/// SCOPE_RESTRICTED.  Used by prctl(PR_SET_PTRACER).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum PtraceAllowedPtracers {
    #[default]
    None,
    Some(Pid),
    Any,
}

#[derive(Copy, Clone)]
pub enum PtraceTracer<'a> {
    /// Tracer thread group is exiting, which detaches all its tracees.
    Exiting(&'a ThreadGroup),
    /// Tracer invoked a ptrace syscall.
    Syscall(&'a Arc<Task>),
}

impl<'a> PtraceTracer<'a> {
    fn thread_group(self) -> &'a ThreadGroup {
        match self {
            PtraceTracer::Exiting(tg) => tg,
            PtraceTracer::Syscall(task) => &task.thread_group,
        }
    }

    fn matches_task(self, task: &Arc<Task>) -> bool {
        match self {
            PtraceTracer::Syscall(expected_task) => task == expected_task,
            PtraceTracer::Exiting(expected_tg) => &*task.thread_group == expected_tg,
        }
    }
}

/// Continues the target thread, optionally detaching from it.
///
/// # Arguments
/// * `tracer` - The context of the tracer performing the operation.
/// * `tracee` - The target thread to continue.
/// * `data` - Is treated as it is in PTRACE_CONT.
/// * `detach` - If true, the tracer will detach from the tracee.
fn ptrace_cont(
    tracer: PtraceTracer<'_>,
    tracee: &Task,
    data: &UserAddress,
    detach: bool,
) -> Result<(), Errno> {
    let data = data.ptr() as u64;
    let new_state;
    let mut siginfo = if data != 0 {
        let signal = Signal::try_from(UncheckedSignal::new(data))?;
        Some(SignalInfo::kernel(signal))
    } else {
        None
    };

    let mut state = tracee.write();

    // Verify under lock that we are still the tracer.
    // This check is performed under the tracee task lock to prevent races where the tracee
    // is concurrently detached or re-attached to another tracer in the window between our
    // initial check (e.g. in ptrace_dispatch or wait_on_pid) and when we actually execute
    // the continuation. This ensures we don't accidentally control a task we no longer trace,
    // or interfere with another tracer's control.
    {
        let ptrace = state.ptrace.as_ref().ok_or_else(|| errno!(ESRCH))?;
        let tracer_tg = ptrace.core_state.thread_group.upgrade().ok_or_else(|| errno!(ESRCH))?;
        let tracer_task = ptrace.core_state.task.upgrade();
        let is_match = match tracer_task {
            Some(tracer_task) if !tracer_task.is_exitted() => tracer.matches_task(&tracer_task),
            _ => tracer.thread_group() == &*tracer_tg,
        };
        if !is_match {
            return error!(ESRCH);
        }
    }
    let is_listen = state.is_ptrace_listening();

    if tracee.load_stopped().is_waking_or_awake() && !is_listen {
        if detach && matches!(tracer, PtraceTracer::Exiting(_)) {
            // If the tracer thread group is exiting, we must force-detach even if the
            // tracee is currently running (waking/awake), otherwise the tracee would
            // be left permanently attached to a dead tracer.
            state.set_ptrace(None)?;
            return Ok(());
        }
        // Manual PTRACE_DETACH via syscall (tracer is CurrentTask) is not allowed
        // on running tracees and returns EIO.
        return error!(EIO);
    }

    if !state.can_accept_ptrace_commands() && !detach {
        return error!(ESRCH);
    }

    if let Some(ptrace) = &mut state.ptrace {
        if data != 0 {
            new_state = PtraceStatus::Continuing;
            if let Some(last_signal) = &mut ptrace.last_signal {
                if let Some(si) = siginfo {
                    let new_signal = si.signal;
                    last_signal.signal = new_signal;
                }
                siginfo = Some(last_signal.clone());
            }
        } else {
            new_state = PtraceStatus::Default;
            ptrace.last_signal = None;
            ptrace.event_data = None;
        }
        ptrace.stop_status = new_state;

        if is_listen {
            state.notify_ptracees();
        }
    }

    if let Some(siginfo) = siginfo {
        // This will wake up the task for us, and also release state
        send_signal_first(&tracee, state, siginfo);
    } else {
        state.set_stopped(StopState::Waking, None, None, None);
        drop(state);
        tracee.thread_group().set_stopped(StopState::Waking, None, false);
    }
    if detach {
        tracee.write().set_ptrace(None)?;
    }
    Ok(())
}

fn ptrace_interrupt(tracee: &Task) -> Result<(), Errno> {
    let mut state = tracee.write();
    if let Some(ptrace) = &mut state.ptrace {
        if !ptrace.is_seized() {
            return error!(EIO);
        }
        let status = ptrace.stop_status.clone();
        ptrace.stop_status = PtraceStatus::Default;
        let event_data = Some(PtraceEventData::new_from_event(PtraceEvent::Stop, 0));
        if status == PtraceStatus::Listening {
            let signal = ptrace.last_signal.clone();
            // "If the tracee was already stopped by a signal and PTRACE_LISTEN
            // was sent to it, the tracee stops with PTRACE_EVENT_STOP and
            // WSTOPSIG(status) returns the stop signal"
            state.set_stopped(StopState::PtraceEventStopped, signal, None, event_data);
        } else {
            state.set_stopped(
                StopState::PtraceEventStopping,
                Some(SignalInfo::kernel(SIGTRAP)),
                None,
                event_data,
            );
            drop(state);
            tracee.interrupt();
        }
    }
    Ok(())
}

fn ptrace_listen(tracee: &Task) -> Result<(), Errno> {
    let mut state = tracee.write();
    if let Some(ptrace) = &mut state.ptrace {
        if !ptrace.is_seized()
            || (ptrace.last_signal_waitable
                && ptrace
                    .event_data
                    .as_ref()
                    .is_some_and(|event_data| event_data.event != PtraceEvent::Stop))
        {
            return error!(EIO);
        }
        ptrace.stop_status = PtraceStatus::Listening;
    }
    Ok(())
}

pub fn ptrace_detach(
    pids: &mut PidTableGuard<'_>,
    tracer: PtraceTracer<'_>,
    tracee: &Task,
    data: &UserAddress,
) -> Result<(), Errno> {
    let thread_group = tracer.thread_group();
    {
        let mut ptracees = thread_group.ptracees.lock();
        ptrace_cont(tracer, tracee, &data, true)?;
        ptracees.remove(&tracee.persistent_info);
    }
    let zombie_notification = thread_group.write().zombie_ptracees.detach(pids, &tracee.tid);
    if let Some(zombie_notification) = zombie_notification {
        zombie_notification.deliver(pids);
    }
    Ok(())
}

/// For all ptrace requests that require an attached tracee
pub fn ptrace_dispatch(
    current_task: &mut CurrentTask,
    request: u32,
    pid: &Pid,
    addr: UserAddress,
    data: UserAddress,
) -> Result<SyscallResult, Errno> {
    let tracee = pid.get_task()?;

    if let Some(ptrace) = &tracee.read().ptrace {
        let is_tracer = ptrace
            .core_state
            .thread_group
            .upgrade()
            .is_some_and(|tg| Arc::ptr_eq(&tg, current_task.thread_group()));
        if !is_tracer {
            return error!(ESRCH);
        }
    }

    // These requests may be run without the thread in a stop state, or
    // check the stop state themselves.
    match request {
        PTRACE_KILL => {
            let siginfo = SignalInfo::with_detail(
                SIGKILL,
                (SIGTRAP.number() | PTRACE_KILL << 8) as i32,
                SignalDetail::None,
            );
            send_standard_signal(&tracee, siginfo);
            return Ok(starnix_syscalls::SUCCESS);
        }
        PTRACE_INTERRUPT => {
            ptrace_interrupt(tracee.as_ref())?;
            return Ok(starnix_syscalls::SUCCESS);
        }
        PTRACE_LISTEN => {
            ptrace_listen(&tracee)?;
            return Ok(starnix_syscalls::SUCCESS);
        }
        PTRACE_CONT => {
            ptrace_cont(PtraceTracer::Syscall(&current_task.task), tracee.as_ref(), &data, false)?;
            return Ok(starnix_syscalls::SUCCESS);
        }
        PTRACE_SYSCALL => {
            tracee.trace_syscalls.store(true, std::sync::atomic::Ordering::Relaxed);
            ptrace_cont(PtraceTracer::Syscall(&current_task.task), tracee.as_ref(), &data, false)?;
            return Ok(starnix_syscalls::SUCCESS);
        }
        PTRACE_DETACH => {
            let mut pids = current_task.kernel().pids.lock();
            ptrace_detach(
                &mut pids,
                PtraceTracer::Syscall(&current_task.task),
                tracee.as_ref(),
                &data,
            )?;
            return Ok(starnix_syscalls::SUCCESS);
        }
        _ => {}
    }

    // The remaining requests (to be added) require the thread to be stopped.
    let mut state = tracee.write();
    if !state.can_accept_ptrace_commands() {
        return error!(ESRCH);
    }

    match request {
        PTRACE_PEEKDATA | PTRACE_PEEKTEXT => {
            let Some(captured) = &mut state.captured_thread_state else {
                return error!(ESRCH);
            };

            // NB: The behavior of the syscall is different from the behavior in ptrace(2),
            // which is provided by libc.
            let src = LongPtr::new(captured.as_ref(), addr);
            let val = tracee.read_multi_arch_object(src)?;

            let dst = LongPtr::new(&src, data);
            current_task.write_multi_arch_object(dst, val)?;
            Ok(starnix_syscalls::SUCCESS)
        }
        PTRACE_POKEDATA | PTRACE_POKETEXT => {
            let Some(captured) = &mut state.captured_thread_state else {
                return error!(ESRCH);
            };

            let bytes = if captured.is_arch32() {
                u32::try_from(data.ptr()).map_err(|_| errno!(EINVAL))?.to_ne_bytes().to_vec()
            } else {
                data.ptr().to_ne_bytes().to_vec()
            };

            tracee.mm()?.force_write_memory(addr, &bytes)?;

            Ok(starnix_syscalls::SUCCESS)
        }
        PTRACE_PEEKUSR => {
            let Some(captured) = &mut state.captured_thread_state else {
                return error!(ESRCH);
            };

            let dst = LongPtr::new(captured.as_ref(), data);
            let val = ptrace_peekuser(&mut captured.thread_state, addr.ptr() as usize)?;
            current_task.write_multi_arch_object(dst, val as u64)?;
            return Ok(starnix_syscalls::SUCCESS);
        }
        PTRACE_POKEUSR => {
            ptrace_pokeuser(&mut *state, data.ptr() as usize, addr.ptr() as usize)?;
            return Ok(starnix_syscalls::SUCCESS);
        }
        PTRACE_GETREGSET => {
            if let Some(ref mut captured) = state.captured_thread_state {
                let uiv = IOVecPtr::new(current_task, data);
                let mut iv = current_task.read_multi_arch_object(uiv)?;
                let base = iv.iov_base.addr;
                let mut len = iv.iov_len as usize;
                ptrace_getregset(
                    current_task,
                    &captured.thread_state,
                    ElfNoteType::try_from(addr.ptr() as usize)?,
                    base,
                    &mut len,
                )?;
                iv.iov_len = len as u64;
                current_task.write_multi_arch_object(uiv, iv)?;
                return Ok(starnix_syscalls::SUCCESS);
            }
            error!(ESRCH)
        }
        PTRACE_SETREGSET => {
            if let Some(ref mut captured) = state.captured_thread_state {
                captured.dirty = true;
                let uiv = IOVecPtr::new(current_task, data);
                let iv = current_task.read_multi_arch_object(uiv)?;
                let base = iv.iov_base.addr;
                let len = iv.iov_len as usize;
                ptrace_setregset(
                    current_task,
                    &mut captured.thread_state,
                    ElfNoteType::try_from(addr.ptr() as usize)?,
                    base,
                    len,
                )?;
                return Ok(starnix_syscalls::SUCCESS);
            }
            error!(ESRCH)
        }
        #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
        PTRACE_GETREGS => {
            if let Some(captured) = &mut state.captured_thread_state {
                let mut len = usize::MAX;
                ptrace_getregset(
                    current_task,
                    &captured.thread_state,
                    ElfNoteType::PrStatus,
                    data.ptr() as u64,
                    &mut len,
                )?;
                return Ok(starnix_syscalls::SUCCESS);
            }
            error!(ESRCH)
        }
        PTRACE_SETSIGMASK => {
            // addr is the size of the buffer pointed to
            // by data, but has to be sizeof(sigset_t).
            if addr.ptr() != std::mem::size_of::<SigSet>() {
                return error!(EINVAL);
            }
            // sigset comes from *data.
            let src: UserRef<SigSet> = UserRef::from(data);
            let val = current_task.read_object(src)?;
            state.set_signal_mask(val);

            Ok(starnix_syscalls::SUCCESS)
        }
        PTRACE_GETSIGMASK => {
            // addr is the size of the buffer pointed to
            // by data, but has to be sizeof(sigset_t).
            if addr.ptr() != std::mem::size_of::<SigSet>() {
                return error!(EINVAL);
            }
            // sigset goes in *data.
            let dst: UserRef<SigSet> = UserRef::from(data);
            let val = state.signal_mask();
            current_task.write_object(dst, &val)?;
            Ok(starnix_syscalls::SUCCESS)
        }
        PTRACE_GETSIGINFO => {
            if let Some(ptrace) = &state.ptrace {
                if let Some(signal) = ptrace.last_signal.as_ref() {
                    let dst = MultiArchUserRef::<uapi::siginfo_t, uapi::arch32::siginfo_t>::new(
                        current_task,
                        data,
                    );
                    signal.write(current_task, dst)?;
                } else {
                    return error!(EINVAL);
                }
            }
            Ok(starnix_syscalls::SUCCESS)
        }
        PTRACE_SETSIGINFO => {
            let siginfo = UncheckedSignalInfo::read_from_siginfo(current_task, data)?.try_into()?;
            if let Some(ptrace) = &mut state.ptrace {
                ptrace.last_signal = Some(siginfo);
            }
            Ok(starnix_syscalls::SUCCESS)
        }
        PTRACE_GET_SYSCALL_INFO => {
            if let Some(ptrace) = &state.ptrace {
                let (size, info) = ptrace.get_target_syscall(&tracee, &state)?;
                let dst: UserRef<ptrace_syscall_info> = UserRef::from(data);
                let len = std::cmp::min(std::mem::size_of::<ptrace_syscall_info>(), addr.ptr());
                // SAFETY: ptrace_syscall_info does not implement FromBytes/IntoBytes,
                // so this has to happen manually.
                let src = unsafe {
                    std::slice::from_raw_parts(
                        &info as *const ptrace_syscall_info as *const u8,
                        len as usize,
                    )
                };
                current_task.write_memory(dst.addr(), src)?;
                Ok(size.into())
            } else {
                error!(ESRCH)
            }
        }
        PTRACE_SETOPTIONS => {
            let mask = data.ptr() as u32;
            // This is what we currently support.
            if mask != 0
                && (mask
                    & !(PTRACE_O_TRACESYSGOOD
                        | PTRACE_O_TRACECLONE
                        | PTRACE_O_TRACEFORK
                        | PTRACE_O_TRACEVFORK
                        | PTRACE_O_TRACEVFORKDONE
                        | PTRACE_O_TRACEEXEC
                        | PTRACE_O_TRACEEXIT
                        | PTRACE_O_EXITKILL)
                    != 0)
            {
                track_stub!(TODO("https://fxbug.dev/322874463"), "ptrace(PTRACE_SETOPTIONS)", mask);
                return error!(ENOSYS);
            }
            if let Some(ptrace) = &mut state.ptrace {
                ptrace.set_options_from_bits(mask)?;
            }
            Ok(starnix_syscalls::SUCCESS)
        }
        PTRACE_GETEVENTMSG => {
            if let Some(ptrace) = &state.ptrace {
                if let Some(event_data) = &ptrace.event_data {
                    let dst = LongPtr::new(current_task, data);
                    current_task.write_multi_arch_object(dst, event_data.msg)?;
                    return Ok(starnix_syscalls::SUCCESS);
                }
            }
            error!(EIO)
        }
        _ => {
            track_stub!(TODO("https://fxbug.dev/322874463"), "ptrace", request);
            error!(ENOSYS)
        }
    }
}

/// Makes the given thread group trace the given task.
fn do_attach(
    thread_group: &ThreadGroup,
    tracer_task: Weak<Task>,
    task: &Arc<Task>,
    attach_type: PtraceAttachType,
    options: PtraceOptions,
) -> Result<(), Errno> {
    let mut ptracees = thread_group.ptracees.lock();

    if !thread_group.read().is_running() {
        return error!(ESRCH);
    }

    let process_state = &mut task.thread_group().write();
    let mut state = task.write();
    state.set_ptrace(Some(PtraceState::new(
        tracer_task,
        thread_group.weak_self.clone(),
        attach_type,
        options,
    )))?;

    ptracees.insert(task.persistent_info.clone());

    // If the tracee is already stopped, make sure that the tracer can
    // identify that right away.
    if process_state.is_waitable()
        && process_state.base.load_stopped() == StopState::GroupStopped
        && task.load_stopped() == StopState::GroupStopped
    {
        if let Some(ptrace) = &mut state.ptrace {
            ptrace.last_signal_waitable = true;
        }
    }

    Ok(())
}

/// Uses the given core ptrace state (including tracer, attach type, etc) to
/// attach to another task, given by `tracee_task`.  Also sends a signal to stop
/// tracee_task.  Typical for when inheriting ptrace state from another task.
pub fn ptrace_attach_from_state(
    tracee_task: &Arc<Task>,
    ptrace_state: PtraceCoreState,
) -> Result<(), Errno> {
    {
        let tracer_tg = ptrace_state.thread_group.upgrade().ok_or_else(|| errno!(ESRCH))?;
        do_attach(
            &tracer_tg,
            ptrace_state.task.clone(),
            tracee_task,
            ptrace_state.attach_type,
            ptrace_state.options,
        )?;
    }
    let mut state = tracee_task.write();
    if let Some(ptrace) = &mut state.ptrace {
        ptrace.core_state.tracer_waiters = Arc::clone(&ptrace_state.tracer_waiters);
    }

    // The newly started tracee starts with a signal that depends on the attach type.
    let signal = if ptrace_state.attach_type == PtraceAttachType::Seize {
        if let Some(ptrace) = &mut state.ptrace {
            ptrace.set_last_event(Some(PtraceEventData::new_from_event(PtraceEvent::Stop, 0)));
        }
        // Ptrace-emitted SIGTRAP signal cannot be blocked.
        SignalInfo::forced(SIGTRAP)
    } else {
        // Note, SIGSTOP can never be blocked, but we use `forced` anyway to be consistent.
        SignalInfo::forced(SIGSTOP)
    };
    send_signal_first(tracee_task, state, signal);

    // If the tracer is already sleeping in waitpid, it is waiting on the shared `tracer_waiters`
    // queue. We must wake it up here so it can register on the new tracee's queue (and update its
    // wait registration loop) rather than missing the initial stopped status notification.
    ptrace_state.tracer_waiters.notify_all();

    Ok(())
}

pub fn ptrace_traceme(current_task: &mut CurrentTask) -> Result<SyscallResult, Errno> {
    let parent = current_task.thread_group().read().parent.clone();
    if let Some(parent) = parent {
        let parent = parent.upgrade();
        // TODO: Move this check into `do_attach()` so that there is a single `ptrace_access_check(tracer, tracee)`?
        let parent_task = parent.leader.get_task().map_err(|_| errno!(EINVAL))?;
        security::ptrace_traceme(current_task, &parent_task)?;
        let parent_task = Arc::downgrade(&parent_task);

        do_attach(
            &parent,
            parent_task,
            &current_task.task,
            PtraceAttachType::Attach,
            PtraceOptions::empty(),
        )?;
        Ok(starnix_syscalls::SUCCESS)
    } else {
        error!(EPERM)
    }
}

pub fn ptrace_attach(
    current_task: &mut CurrentTask,
    pid: &Pid,
    attach_type: PtraceAttachType,
    data: UserAddress,
) -> Result<SyscallResult, Errno> {
    let tracee = pid.get_task()?;

    if tracee.thread_group == current_task.thread_group {
        return error!(EPERM);
    }

    current_task.check_ptrace_access_mode(PTRACE_MODE_ATTACH_REALCREDS, &tracee)?;
    let tracer_task = Arc::downgrade(&current_task.task);
    do_attach(
        current_task.thread_group(),
        tracer_task,
        &tracee,
        attach_type,
        PtraceOptions::empty(),
    )?;
    if attach_type == PtraceAttachType::Attach {
        send_standard_signal(&tracee, SignalInfo::kernel(SIGSTOP));
    } else if attach_type == PtraceAttachType::Seize {
        // When seizing, |data| should be used as the options bitmask.
        let mut state = tracee.write();
        if let Some(ptrace) = &mut state.ptrace {
            ptrace.set_options_from_bits(data.ptr() as u32)?;
        }
    }
    Ok(starnix_syscalls::SUCCESS)
}

/// Implementation of ptrace(PTRACE_PEEKUSER).  The user struct holds the
/// registers and other information about the process.  See ptrace(2) and
/// sys/user.h for full details.
pub fn ptrace_peekuser(
    thread_state: &mut ThreadState<HeapRegs>,
    offset: usize,
) -> Result<usize, Errno> {
    #[cfg(any(target_arch = "x86_64"))]
    if offset >= std::mem::size_of::<user>() {
        return error!(EIO);
    }
    if offset < UserRegsStructPtr::size_of_object_for(thread_state) {
        let result = thread_state.get_user_register(offset)?;
        return Ok(result);
    }
    error!(EIO)
}

pub fn ptrace_pokeuser(
    state: &mut TaskMutableState,
    value: usize,
    offset: usize,
) -> Result<(), Errno> {
    if let Some(ref mut thread_state) = state.captured_thread_state {
        thread_state.dirty = true;

        #[cfg(any(target_arch = "x86_64"))]
        if offset >= std::mem::size_of::<user>() {
            return error!(EIO);
        }
        if offset < UserRegsStructPtr::size_of_object_for(thread_state.as_ref()) {
            return thread_state.thread_state.set_user_register(offset, value);
        }
    }
    error!(EIO)
}

pub fn ptrace_getregset(
    current_task: &CurrentTask,
    thread_state: &ThreadState<HeapRegs>,
    regset_type: ElfNoteType,
    base: u64,
    len: &mut usize,
) -> Result<(), Errno> {
    match regset_type {
        ElfNoteType::PrStatus => {
            let user_regs_struct_len = UserRegsStructPtr::size_of_object_for(thread_state);
            *len = std::cmp::min(*len, user_regs_struct_len);

            if thread_state.is_arch32() {
                let regs = thread_state.registers.to_user_regs_struct_arch32();
                current_task.write_memory(UserAddress::from(base), &regs.as_bytes()[..*len])?;
            } else {
                let regs = thread_state.registers.to_user_regs_struct();
                current_task.write_memory(UserAddress::from(base), &regs.as_bytes()[..*len])?;
            }
            Ok(())
        }
        _ => {
            error!(EINVAL)
        }
    }
}

pub fn ptrace_setregset(
    current_task: &CurrentTask,
    thread_state: &mut ThreadState<HeapRegs>,
    regset_type: ElfNoteType,
    base: u64,
    len: usize,
) -> Result<(), Errno> {
    match regset_type {
        ElfNoteType::PrStatus => {
            let user_regs_struct_len = UserRegsStructPtr::size_of_object_for(thread_state);
            if len < user_regs_struct_len {
                return error!(EINVAL);
            }

            if thread_state.is_arch32() {
                let mut regs = starnix_uapi::arch32::user_regs_struct::default();
                current_task.read_memory_to_slice(UserAddress::from(base), regs.as_mut_bytes())?;
                thread_state.registers.from_user_regs_struct_arch32(&regs);
            } else {
                let mut regs = starnix_uapi::user_regs_struct::default();
                current_task.read_memory_to_slice(UserAddress::from(base), regs.as_mut_bytes())?;
                thread_state.registers.from_user_regs_struct(&regs);
            }
            Ok(())
        }
        _ => error!(EINVAL),
    }
}

#[inline(never)]
pub fn ptrace_syscall_enter(current_task: &mut CurrentTask) {
    let block = {
        let mut state = current_task.write();
        if state.ptrace.is_some() {
            current_task.trace_syscalls.store(false, Ordering::Relaxed);
            let mut sig = SignalInfo::with_detail(
                SIGTRAP,
                (linux_uapi::SIGTRAP | 0x80) as i32,
                SignalDetail::None,
            );
            if state
                .ptrace
                .as_ref()
                .is_some_and(|ptrace| ptrace.has_option(PtraceOptions::TRACESYSGOOD))
            {
                sig.signal.set_ptrace_syscall_bit();
            }
            state.set_stopped(StopState::SyscallEnterStopping, Some(sig), None, None);
            true
        } else {
            false
        }
    };
    if block {
        current_task.block_if_stopped();
    }
}

#[inline(never)]
pub fn ptrace_syscall_exit(current_task: &mut CurrentTask, is_error: bool) {
    let block = {
        let mut state = current_task.write();
        current_task.trace_syscalls.store(false, Ordering::Relaxed);
        if state.ptrace.is_some() {
            let mut sig = SignalInfo::with_detail(
                SIGTRAP,
                (linux_uapi::SIGTRAP | 0x80) as i32,
                SignalDetail::None,
            );
            if state
                .ptrace
                .as_ref()
                .is_some_and(|ptrace| ptrace.has_option(PtraceOptions::TRACESYSGOOD))
            {
                sig.signal.set_ptrace_syscall_bit();
            }

            state.set_stopped(StopState::SyscallExitStopping, Some(sig), None, None);
            if let Some(ptrace) = &mut state.ptrace {
                ptrace.last_syscall_was_error = is_error;
            }
            true
        } else {
            false
        }
    };
    if block {
        current_task.block_if_stopped();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::syscalls::sys_prctl;
    use crate::testing::{create_task, spawn_kernel_and_run};
    use starnix_uapi::PR_SET_PTRACER;
    use starnix_uapi::auth::CAP_SYS_PTRACE;

    #[::fuchsia::test]
    async fn test_set_ptracer() {
        spawn_kernel_and_run(async |current_task| {
            let kernel = current_task.kernel().clone();
            let mut tracee = create_task(&kernel, "tracee");
            let mut tracer = create_task(&kernel, "tracer");

            let mut creds = tracer.real_creds().clone();
            creds.cap_effective &= !CAP_SYS_PTRACE;
            tracer.set_creds(creds);

            kernel.ptrace_scope.store(security::yama::SCOPE_RESTRICTED, Ordering::Relaxed);
            assert_eq!(sys_prctl(&mut tracee, PR_SET_PTRACER, 0xFFF, 0, 0, 0), error!(EINVAL));

            assert_eq!(
                ptrace_attach(
                    &mut tracer,
                    &tracee.as_ref().task.tid,
                    PtraceAttachType::Attach,
                    UserAddress::NULL,
                ),
                error!(EPERM)
            );

            assert!(
                sys_prctl(
                    &mut tracee,
                    PR_SET_PTRACER,
                    tracer.thread_group().leader.id as u64,
                    0,
                    0,
                    0
                )
                .is_ok()
            );

            let mut not_tracer = create_task(&kernel, "not-tracer");
            not_tracer.set_creds(tracer.real_creds().clone());
            assert_eq!(
                ptrace_attach(
                    &mut not_tracer,
                    &tracee.as_ref().task.tid,
                    PtraceAttachType::Attach,
                    UserAddress::NULL,
                ),
                error!(EPERM)
            );

            assert!(
                ptrace_attach(
                    &mut tracer,
                    &tracee.as_ref().task.tid,
                    PtraceAttachType::Attach,
                    UserAddress::NULL,
                )
                .is_ok()
            );
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_set_ptracer_any() {
        spawn_kernel_and_run(async |current_task| {
            let kernel = current_task.kernel().clone();
            let mut tracee = create_task(&kernel, "tracee");
            let mut tracer = create_task(&kernel, "tracer");

            let mut creds = tracer.real_creds().clone();
            creds.cap_effective &= !CAP_SYS_PTRACE;
            tracer.set_creds(creds);

            kernel.ptrace_scope.store(security::yama::SCOPE_RESTRICTED, Ordering::Relaxed);
            assert_eq!(sys_prctl(&mut tracee, PR_SET_PTRACER, 0xFFF, 0, 0, 0), error!(EINVAL));

            assert_eq!(
                ptrace_attach(
                    &mut tracer,
                    &tracee.as_ref().task.tid,
                    PtraceAttachType::Attach,
                    UserAddress::NULL,
                ),
                error!(EPERM)
            );

            assert!(sys_prctl(&mut tracee, PR_SET_PTRACER, PR_SET_PTRACER_ANY, 0, 0, 0).is_ok());

            assert!(
                ptrace_attach(
                    &mut tracer,
                    &tracee.as_ref().task.tid,
                    PtraceAttachType::Attach,
                    UserAddress::NULL,
                )
                .is_ok()
            );
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_unspawned_task_remove_does_not_register_zombie() {
        spawn_kernel_and_run(async |current_task| {
            let kernel = current_task.kernel().clone();
            let tracee = create_task(&kernel, "tracee");
            let mut tracer = create_task(&kernel, "tracer");

            assert!(
                ptrace_attach(
                    &mut tracer,
                    &tracee.as_ref().task.tid,
                    PtraceAttachType::Attach,
                    UserAddress::NULL,
                )
                .is_ok()
            );

            // create_task() returns an unspawned task. Dropping the tracee causes it to exit, which
            // triggers zombie tracee registration. The tracee must not register with the tracer
            // because it never spawned.
            assert!(!tracee.is_spawned());
            drop(tracee);
            assert!(tracer.thread_group().write().zombie_ptracees.is_empty());
        })
        .await;
    }
}
