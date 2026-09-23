// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::mm::MemoryAccessorExt;
use crate::signals::{SignalDetail, SignalInfo, send_standard_signal};
use crate::task::{
    CurrentTask, EventHandler, ExitStatus, Kernel, Task, TaskFlags, WaitCanceler, WaitQueue, Waiter,
};
use crate::vfs::buffers::{InputBuffer, OutputBuffer};
use crate::vfs::{
    Anon, FdFlags, FdNumber, FileObject, FileObjectState, FileOps, fileops_impl_nonseekable,
    fileops_impl_noop_sync,
};
use bstr::ByteSlice;
use ebpf::{
    BPF_ABS, BPF_IND, BPF_LD, BPF_ST, BPF_W, BpfProgramContext, CbpfConfig, EbpfProgram, MemoryId,
    NoMap, ProgramArgument, Type, bpf_addressing_mode, bpf_class, bpf_size, convert_and_link_cbpf,
};
use ebpf_api::SECCOMP_CBPF_CONFIG;
use linux_uapi::AUDIT_SECCOMP;
use starnix_logging::{log_warn, track_stub};
use starnix_sync::{LockDepMutex, SeccompNotifierLock};
use starnix_syscalls::decls::Syscall;
use starnix_syscalls::{SyscallArg, SyscallResult};
use starnix_uapi::errors::Errno;
use starnix_uapi::open_flags::OpenFlags;
use starnix_uapi::signals::{SIGKILL, SIGSYS};
#[cfg(target_arch = "aarch64")]
use starnix_uapi::user_address::ArchSpecific;
use starnix_uapi::user_address::{UserAddress, UserRef};
use starnix_uapi::vfs::FdEvents;
use starnix_uapi::{
    __NR_exit, __NR_read, __NR_write, SECCOMP_IOCTL_NOTIF_ADDFD, SECCOMP_IOCTL_NOTIF_ID_VALID,
    SECCOMP_IOCTL_NOTIF_RECV, SECCOMP_IOCTL_NOTIF_SEND, SECCOMP_MODE_DISABLED, SECCOMP_MODE_FILTER,
    SECCOMP_MODE_STRICT, SECCOMP_RET_ACTION_FULL, SECCOMP_RET_DATA,
    SECCOMP_USER_NOTIF_FLAG_CONTINUE, SYS_SECCOMP, errno, errno_from_code, error, seccomp_data,
    seccomp_notif, seccomp_notif_resp, sock_filter,
};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, LazyLock};
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

#[cfg(target_arch = "aarch64")]
use starnix_uapi::__NR_clock_getres;
#[cfg(target_arch = "aarch64")]
use starnix_uapi::__NR_clock_gettime;
#[cfg(target_arch = "aarch64")]
use starnix_uapi::__NR_gettimeofday;
#[cfg(target_arch = "aarch64")]
use starnix_uapi::{AUDIT_ARCH_AARCH64, AUDIT_ARCH_ARM};

#[cfg(target_arch = "x86_64")]
use starnix_uapi::__NR_clock_gettime;
#[cfg(target_arch = "x86_64")]
use starnix_uapi::__NR_getcpu;
#[cfg(target_arch = "x86_64")]
use starnix_uapi::__NR_gettimeofday;
#[cfg(target_arch = "x86_64")]
use starnix_uapi::__NR_time;
#[cfg(target_arch = "x86_64")]
use starnix_uapi::AUDIT_ARCH_X86_64;

#[cfg(target_arch = "riscv64")]
use starnix_uapi::AUDIT_ARCH_RISCV64;

pub struct SeccompFilter {
    /// The BPF program associated with this filter.
    program: EbpfProgram<SeccompFilter>,

    /// The unique-to-this-process id of this filter.  SECCOMP_FILTER_FLAG_TSYNC only works if all
    /// threads in this process have filters that are a prefix of the filters of the thread
    /// attempting to do the TSYNC. Identical filters attached in separate seccomp calls are treated
    /// as different from each other for this purpose, so we need a way of distinguishing them.
    unique_id: u64,

    // Whether to log the results of this filter
    log: bool,

    /// The notifier associated with this filter, if it was created with SECCOMP_FILTER_FLAG_NEW_LISTENER
    pub notifier: Option<SeccompNotifierHandle>,
}

/// The result of running a set of seccomp filters.
pub struct SeccompFilterResult {
    /// The action indicated by the seccomp filter with the highest priority result.
    action: SeccompAction,

    /// The filter that returned the highest priority result, as used by SECCOMP_RET_USER_NOTIF,
    /// which has to have access to its cookie value
    filter: Option<Arc<SeccompFilter>>,
}

impl SeccompFilter {
    /// Creates a SeccompFilter object from the given sock_filter.  Associates the user-provided
    /// id with it, which is intended to be unique to this process.
    pub fn from_cbpf(
        code: &Vec<sock_filter>,
        maybe_unique_id: u64,
        should_log: bool,
        notifier: Option<SeccompNotifierHandle>,
    ) -> Result<Self, Errno> {
        for insn in code {
            // If an instruction loads from / stores to an absolute address, that address has to be
            // 32-bit aligned and inside the struct seccomp_data passed in.
            if (bpf_class(insn) == BPF_LD || bpf_class(insn) == BPF_ST)
                && (bpf_addressing_mode(insn) == BPF_ABS)
                && (insn.k & 0x3 != 0 || insn.k as usize >= std::mem::size_of::<seccomp_data>())
            {
                return error!(EINVAL);
            }
            // Indirect loads (BPF_IND) are strictly forbidden.
            if (bpf_class(insn) == BPF_LD || bpf_class(insn) == BPF_ST)
                && bpf_addressing_mode(insn) == BPF_IND
            {
                return error!(EINVAL);
            }
            // 8 and 16 bits read and write are strictly forbidden.
            if (bpf_class(insn) == BPF_LD || bpf_class(insn) == BPF_ST) && bpf_size(insn) != BPF_W {
                return error!(EINVAL);
            }
        }

        let program = convert_and_link_cbpf::<SeccompFilter>(code).map_err(|errmsg| {
            log_warn!("{}", errmsg);
            errno!(EINVAL)
        })?;

        Ok(SeccompFilter { program, unique_id: maybe_unique_id, log: should_log, notifier })
    }

    pub fn run(&self, data: &seccomp_data) -> u32 {
        self.program.run(&mut (), &SeccompData(*data)) as u32
    }
}

// Wrapper for `seccomp_data`. Required in order to implement the `ProgramArgument` trait below.
#[repr(C)]
#[derive(Debug, Default, Clone, IntoBytes, FromBytes, KnownLayout, Immutable)]
pub struct SeccompData(seccomp_data);

impl BpfProgramContext for SeccompFilter {
    type RunContext<'a> = ();
    type Packet<'a> = &'a SeccompData;
    type Map = NoMap;
    const CBPF_CONFIG: &'static CbpfConfig = &SECCOMP_CBPF_CONFIG;
}

ebpf::empty_static_helper_set!(SeccompFilter);

static SECCOMP_DATA_TYPE: LazyLock<Type> =
    LazyLock::new(|| Type::PtrToMemory { id: MemoryId::new(), offset: 0.into(), buffer_size: 0 });

impl ProgramArgument for &'_ SeccompData {
    fn get_type() -> &'static Type {
        &*SECCOMP_DATA_TYPE
    }
}

const SECCOMP_MAX_INSNS_PER_PATH: u16 = 32768;

/// A list of seccomp filters, intended to be associated with a specific process.
#[derive(Default)]
pub struct SeccompFilterContainer {
    /// List of currently installed seccomp_filters; most recently added is last.
    pub filters: Vec<Arc<SeccompFilter>>,

    // The total length of the provided seccomp filters, which cannot
    // exceed SECCOMP_MAX_INSNS_PER_PATH - 4 * the number of filters.  This is stored
    // instead of computed because we store seccomp filters in an
    // expanded form, and it is impossible to get the original length.
    pub provided_instructions: u16,
}

impl Clone for SeccompFilterContainer {
    fn clone(&self) -> Self {
        for filter in &self.filters {
            if let Some(n) = &filter.notifier {
                n.lock().add_thread();
            }
        }
        SeccompFilterContainer {
            filters: self.filters.clone(),
            provided_instructions: self.provided_instructions,
        }
    }
}

impl Drop for SeccompFilterContainer {
    fn drop(&mut self) {
        for filter in &self.filters {
            if let Some(n) = &filter.notifier {
                // Notifier needs to send threads a HUP when there is no one left
                // referencing it.
                n.lock().remove_thread();
            }
        }
    }
}

fn make_seccomp_data(
    #[allow(unused_variables)] current_task: &CurrentTask,
    syscall: &Syscall,
    ip: u64,
) -> seccomp_data {
    #[cfg(target_arch = "x86_64")]
    let arch_val = AUDIT_ARCH_X86_64;
    #[cfg(target_arch = "aarch64")]
    let arch_val = if current_task.is_arch32() { AUDIT_ARCH_ARM } else { AUDIT_ARCH_AARCH64 };
    #[cfg(target_arch = "riscv64")]
    let arch_val = AUDIT_ARCH_RISCV64;
    seccomp_data {
        nr: syscall.decl.number as i32,
        arch: arch_val,
        instruction_pointer: ip,
        args: [
            syscall.arg0.raw(),
            syscall.arg1.raw(),
            syscall.arg2.raw(),
            syscall.arg3.raw(),
            syscall.arg4.raw(),
            syscall.arg5.raw(),
        ],
    }
}

impl SeccompFilterContainer {
    /// Ensures that this set of seccomp filters can be "synced to" the given set.
    /// This means that our filters are a prefix of the given set of filters.
    pub fn can_sync_to(&self, source: &SeccompFilterContainer) -> bool {
        if source.filters.len() < self.filters.len() {
            return false;
        }
        for (filter, other_filter) in self.filters.iter().zip(source.filters.iter()) {
            if other_filter.unique_id != filter.unique_id {
                return false;
            }
        }
        true
    }

    /// Adds the given filter to this list.  The original_length parameter is the length of
    /// the originally provided BPF (i.e., the number of sock_filter instructions), used
    /// to ensure the total length does not exceed SECCOMP_MAX_INSNS_PER_PATH
    pub fn add_filter(
        &mut self,
        filter: Arc<SeccompFilter>,
        original_length: u16,
    ) -> Result<(), Errno> {
        let maybe_new_length = self.provided_instructions + original_length + 4;
        if maybe_new_length > SECCOMP_MAX_INSNS_PER_PATH {
            return error!(ENOMEM);
        }

        self.provided_instructions = maybe_new_length;
        self.filters.push(filter);
        Ok(())
    }

    /// Runs all of the seccomp filters in this container, most-to-least recent.  Returns the
    /// highest priority result (which contains a reference to the filter that generated it)
    pub fn run_all(&self, current_task: &CurrentTask, syscall: &Syscall) -> SeccompFilterResult {
        let mut r = SeccompFilterResult { action: SeccompAction::Allow, filter: None };

        // VDSO calls can't be caught by seccomp, so most seccomp filters forget to declare them.
        // But our VDSO implementation is incomplete, and most of the calls forward to the actual
        // syscalls. So seccomp should ignore them until they're implemented correctly in the VDSO.
        #[cfg(target_arch = "x86_64")] // The set of VDSO calls is arch dependent.
        #[allow(non_upper_case_globals)]
        if let __NR_clock_gettime | __NR_getcpu | __NR_gettimeofday | __NR_time =
            syscall.decl.number as u32
        {
            return r;
        }
        #[cfg(target_arch = "aarch64")]
        #[allow(non_upper_case_globals)]
        if let __NR_clock_gettime | __NR_clock_getres | __NR_gettimeofday =
            syscall.decl.number as u32
        {
            return r;
        }

        let data = make_seccomp_data(
            current_task,
            syscall,
            current_task.thread_state.registers.instruction_pointer_register(),
        );

        // Filters are executed in reverse order of addition
        for filter in self.filters.iter().rev() {
            let new_result = filter.run(&data);

            let action = SeccompAction::from_u32(new_result).unwrap_or(SeccompAction::KillProcess);

            if SeccompAction::has_prio(&action, &r.action) == std::cmp::Ordering::Less {
                r = SeccompFilterResult { action, filter: Some(filter.clone()) };
            }
        }
        r
    }

    pub fn create_notifier() -> SeccompNotifierHandle {
        SeccompNotifier::new()
    }

    pub fn register_listener(
        current_task: &CurrentTask,
        notifier: SeccompNotifierHandle,
    ) -> Result<FdNumber, Errno> {
        // Create the `Anon` handle file before taking the write lock on the task, because
        // `Anon::new_file()` needs to read the `current_task` SID to label the file object.
        let handle = Anon::new_file(
            current_task,
            Box::new(SeccompNotifierFileObject { notifier: notifier.clone() }),
            OpenFlags::RDWR,
            "seccomp notify",
        )?;

        // Take the write lock to check for an existing notifier.
        let filters = &mut current_task.write().seccomp_filters;
        if filters.filters.iter().any(|f| f.notifier.is_some()) {
            return error!(EBUSY);
        }
        let fd = current_task.add_file(handle, FdFlags::CLOEXEC)?;
        {
            let mut state = notifier.lock();
            state.add_thread();
        }
        Ok(fd)
    }
}

/// Possible values for the current status of the seccomp filters for
/// this process.
#[repr(u8)]
#[derive(Clone, Copy, PartialEq)]
pub enum SeccompStateValue {
    None = SECCOMP_MODE_DISABLED as u8,
    Strict = SECCOMP_MODE_STRICT as u8,
    UserDefined = SECCOMP_MODE_FILTER as u8,
}

/// Per-process state that cannot be stored in the container (e.g., whether there is a container).
#[derive(Default)]
pub struct SeccompState {
    // This AtomicU8 corresponds to a SeccompStateValue.
    filter_state: AtomicU8,
}

impl SeccompState {
    pub fn from(state: &SeccompState) -> SeccompState {
        SeccompState { filter_state: AtomicU8::new(state.filter_state.load(Ordering::Acquire)) }
    }

    fn from_u8(value: u8) -> SeccompStateValue {
        match value {
            v if v == SECCOMP_MODE_DISABLED as u8 => SeccompStateValue::None,
            v if v == SECCOMP_MODE_STRICT as u8 => SeccompStateValue::Strict,
            v if v == SECCOMP_MODE_FILTER as u8 => SeccompStateValue::UserDefined,
            _ => unreachable!(),
        }
    }

    pub fn get(&self) -> SeccompStateValue {
        Self::from_u8(self.filter_state.load(Ordering::Acquire))
    }

    pub fn set(&self, state: &SeccompStateValue) -> Result<(), Errno> {
        loop {
            let seccomp_filter_status = self.get();
            if seccomp_filter_status == *state {
                return Ok(());
            }
            if seccomp_filter_status != SeccompStateValue::None {
                return error!(EINVAL);
            }

            if self
                .filter_state
                .compare_exchange(
                    seccomp_filter_status as u8,
                    *state as u8,
                    Ordering::Release,
                    Ordering::Acquire,
                )
                .is_ok()
            {
                return Ok(());
            }
        }
    }

    /// Check to see if this syscall is allowed in STRICT mode, and, if not,
    /// send the current task a SIGKILL.
    pub fn do_strict(task: &Task, syscall: &Syscall) -> Option<Result<SyscallResult, Errno>> {
        if syscall.decl.number as u32 != __NR_exit
            && syscall.decl.number as u32 != __NR_read
            && syscall.decl.number as u32 != __NR_write
        {
            send_standard_signal(task, SignalInfo::kernel(SIGKILL));
            return Some(Err(errno_from_code!(0)));
        }
        None
    }

    // This is supposed to be put in the audit log, but starnix does not yet have an
    // audit log.  Also, it does not match the Linux format.  Still, the machinery
    // is in place for when we have to support it for real.
    fn log_action(task: &CurrentTask, syscall: &Syscall) {
        let creds = task.current_creds();
        let (uid, gid) = (creds.uid, creds.gid);
        let arch = if cfg!(target_arch = "x86_64") {
            "x86_64"
        } else if cfg!(target_arch = "aarch64") {
            "aarch64"
        } else {
            "unknown"
        };
        task.kernel().audit_logger().audit_log(AUDIT_SECCOMP as u16, || {
            format!(
                "uid={} gid={} pid={} comm={} syscall={} ip={} ARCH={} SYSCALL={}",
                uid,
                gid,
                task.pid,
                task.command(),
                syscall.decl.number,
                task.thread_state.registers.instruction_pointer_register(),
                arch,
                syscall.decl.name(),
            )
        });
    }

    /// Take the given |action| on the given |task|.  The action is one of the SECCOMP_RET values
    /// (ALLOW, LOG, KILL, KILL_PROCESS, TRAP, ERRNO, USER_NOTIF, TRACE).  |task| is the thread that
    /// invoked the syscall, and |syscall| is the syscall that was invoked.
    /// Returns the result that the syscall will be forced to return by this
    /// filter, or None, if the syscall should return its actual return value.
    // NB: Allow warning below so that it is clear what we are doing on KILL_PROCESS
    #[allow(clippy::wildcard_in_or_patterns)]
    pub fn do_user_defined(
        result: SeccompFilterResult,
        current_task: &mut CurrentTask,
        syscall: &Syscall,
    ) -> Option<Result<SyscallResult, Errno>> {
        let action = result.action;
        if let Some(filter) = result.filter.as_ref() {
            if action.is_logged(current_task.kernel(), filter.log) {
                Self::log_action(current_task, syscall);
            }
        }
        match action {
            SeccompAction::Allow => None,
            SeccompAction::Errno(code) => Some(Err(errno_from_code!(code as i16))),
            SeccompAction::KillThread => {
                let siginfo = SignalInfo::kernel(SIGSYS);

                let is_last_thread = current_task.thread_group().read().tasks_count() == 1;
                let mut task_state = current_task.write();

                if is_last_thread {
                    task_state.set_flags(TaskFlags::DUMP_ON_EXIT, true);
                    task_state.set_exit_status_if_not_already(ExitStatus::CoreDump(siginfo));
                } else {
                    task_state.set_exit_status_if_not_already(ExitStatus::Kill(siginfo));
                }
                Some(Err(errno_from_code!(0)))
            }
            SeccompAction::KillProcess => {
                current_task.kill_thread_group(ExitStatus::CoreDump(SignalInfo::kernel(SIGSYS)));
                Some(Err(errno_from_code!(0)))
            }
            SeccompAction::Log => {
                Self::log_action(current_task, syscall);
                None
            }
            SeccompAction::Trace => {
                track_stub!(TODO("https://fxbug.dev/297311898"), "ptrace seccomp support");
                Some(error!(ENOSYS))
            }
            SeccompAction::Trap(errno) => {
                #[cfg(target_arch = "x86_64")]
                let arch_val = AUDIT_ARCH_X86_64;
                #[cfg(target_arch = "aarch64")]
                let arch_val =
                    if current_task.is_arch32() { AUDIT_ARCH_ARM } else { AUDIT_ARCH_AARCH64 };
                #[cfg(target_arch = "riscv64")]
                let arch_val = AUDIT_ARCH_RISCV64;

                let siginfo = SignalInfo::new(
                    SIGSYS,
                    errno as i32,
                    SYS_SECCOMP as i32,
                    SignalDetail::SIGSYS {
                        call_addr: current_task
                            .thread_state
                            .registers
                            .instruction_pointer_register()
                            .into(),
                        syscall: syscall.decl.number as i32,
                        arch: arch_val,
                    },
                    true,
                    None,
                );

                send_standard_signal(current_task, siginfo);
                Some(Err(errno_from_code!(-(syscall.decl.number as i16))))
            }
            SeccompAction::UserNotif => {
                if let Some(notifier) = result.filter.as_ref().and_then(|f| f.notifier.clone()) {
                    let waiter = Waiter::new();
                    let cookie;
                    {
                        let mut notifier = notifier.lock();
                        if notifier.is_closed {
                            return Some(error!(ENOSYS));
                        }
                        cookie = notifier.next_cookie();
                        let msg = seccomp_notif {
                            id: cookie,
                            pid: current_task.tid.id as u32,
                            flags: 0,
                            data: make_seccomp_data(
                                current_task,
                                syscall,
                                current_task.thread_state.registers.instruction_pointer_register(),
                            ),
                        };
                        notifier.create_notification(cookie, msg);
                        notifier.waiters.wait_async_value(&waiter, cookie);
                    }

                    // Next, wait for a response from the supervisor
                    if let Err(e) = waiter.wait(current_task) {
                        return Some(Err(e));
                    }

                    // Fetch the response.
                    let resp: Option<seccomp_notif_resp>;
                    {
                        let mut notifier = notifier.lock();
                        resp = notifier.get_response(cookie);
                        notifier.delete_notification(cookie);
                    }

                    // The response indicates what you are supposed to do with this syscall.
                    if let Some(response) = resp {
                        if response.val != 0 {
                            return Some(Ok(response.val.into()));
                        }
                        if response.error != 0 {
                            if response.error > 0 {
                                return Some(Ok(response.error.into()));
                            } else {
                                return Some(Err(errno_from_code!(-response.error as i16)));
                            }
                        }
                        if response.flags & SECCOMP_USER_NOTIF_FLAG_CONTINUE != 0 {
                            return None;
                        }
                    }
                    Some(Ok(0.into()))
                } else {
                    Some(error!(ENOSYS))
                }
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum SeccompAction {
    Allow,
    Errno(u32),
    KillProcess,
    KillThread,
    Log,
    Trap(u32),
    Trace,
    UserNotif,
}

impl SeccompAction {
    pub fn is_action_available(action: u32) -> Result<SyscallResult, Errno> {
        if SeccompAction::from_u32(action).is_none() {
            return error!(EOPNOTSUPP);
        }
        Ok(0.into())
    }

    pub fn from_u32(action: u32) -> Option<SeccompAction> {
        match action & !SECCOMP_RET_DATA {
            linux_uapi::SECCOMP_RET_ALLOW => Some(Self::Allow),
            linux_uapi::SECCOMP_RET_ERRNO => {
                let mut action = action & SECCOMP_RET_DATA;
                // Linux kernel compatibility: if errno exceeds 0xfff, it is capped at 0xfff.
                action = std::cmp::min(action & 0xffff, 0xfff);
                Some(Self::Errno(action))
            }
            linux_uapi::SECCOMP_RET_KILL_PROCESS => Some(Self::KillProcess),
            linux_uapi::SECCOMP_RET_KILL_THREAD => Some(Self::KillThread),
            linux_uapi::SECCOMP_RET_LOG => Some(Self::Log),
            linux_uapi::SECCOMP_RET_TRACE => Some(Self::Trace),
            linux_uapi::SECCOMP_RET_TRAP => Some(Self::Trap(action & SECCOMP_RET_DATA)),

            linux_uapi::SECCOMP_RET_USER_NOTIF => Some(Self::UserNotif),
            _ => None,
        }
    }

    pub fn to_isize(self) -> isize {
        match self {
            Self::Allow => linux_uapi::SECCOMP_RET_ALLOW as isize,
            Self::Errno(x) => (linux_uapi::SECCOMP_RET_ERRNO | x) as isize,
            Self::KillProcess => linux_uapi::SECCOMP_RET_KILL_PROCESS as isize,
            Self::KillThread => linux_uapi::SECCOMP_RET_KILL_THREAD as isize,
            Self::Log => linux_uapi::SECCOMP_RET_LOG as isize,
            Self::Trace => linux_uapi::SECCOMP_RET_TRACE as isize,
            Self::Trap(x) => (linux_uapi::SECCOMP_RET_TRAP | x) as isize,
            Self::UserNotif => linux_uapi::SECCOMP_RET_USER_NOTIF as isize,
        }
    }

    pub fn canonical_name(self) -> &'static str {
        match self {
            Self::Allow => &"allow",
            Self::Errno(_) => &"errno",
            Self::KillProcess => &"kill_process",
            Self::KillThread => &"kill_thread",
            Self::Log => &"log",
            Self::Trace => &"trace",
            Self::Trap(_) => &"trap",
            Self::UserNotif => &"user_notif",
        }
    }

    pub fn has_prio(a: &SeccompAction, b: &SeccompAction) -> std::cmp::Ordering {
        let anum = a.to_isize() as i32;
        let bnum = b.to_isize() as i32;
        let fullnum = SECCOMP_RET_ACTION_FULL as i32;
        let aval = anum & fullnum;
        let bval = bnum & fullnum;
        aval.cmp(&bval)
    }

    /// Returns a vector of all available actions, sorted by priority.
    pub fn all_actions() -> Vec<SeccompAction> {
        let mut result = vec![
            Self::Allow,
            Self::Errno(0),
            Self::KillProcess,
            Self::KillThread,
            Self::Log,
            Self::Trace,
            Self::Trap(0),
            Self::UserNotif,
        ];

        result.sort_by(Self::has_prio);
        result
    }

    /// Gets the contents of /proc/sys/kernel/seccomp/actions_avail
    pub fn get_actions_avail_file() -> Vec<u8> {
        let all_actions = Self::all_actions();
        if all_actions.len() == 0 {
            return vec![];
        }
        let mut result = String::from(all_actions[0].canonical_name());
        for i in 1..all_actions.len() {
            result.push_str(" ");
            result.push_str(all_actions[i].canonical_name());
        }
        result.push('\n');
        result.into_bytes()
    }

    fn logged_bit_offset(&self) -> u32 {
        match self {
            Self::Allow => 1,
            Self::Errno(_) => 2,
            Self::KillProcess => 3,
            Self::KillThread => 4,
            Self::Log => 5,
            Self::Trace => 6,
            Self::Trap(_) => 7,
            Self::UserNotif => 8,
        }
    }

    fn set_logged_bit(&self, dst: &mut u16) {
        *dst |= 1 << self.logged_bit_offset();
    }

    pub fn is_logged(&self, kernel: &Kernel, filter_flag: bool) -> bool {
        if kernel.actions_logged.load(Ordering::Relaxed) & (1 << self.logged_bit_offset()) != 0 {
            match self {
                // Per the documentation on audit logging of seccomp actions in
                // seccomp(2), just because it is listed as logged, that doesn't
                // mean we actually log it.

                // If it is KILL_PROCESS or KILL_THREAD, return true
                Self::KillProcess | Self::KillThread => true,
                // If it is one of these and the filter flag was set, return true.
                Self::Errno(_) | Self::Log | Self::Trap(_) | Self::UserNotif => filter_flag,
                // Never log ALLOW
                _ => false,
            }
        } else {
            false
        }
    }

    pub fn set_actions_logged(kernel: &Kernel, data: &[u8]) -> Result<(), Errno> {
        let mut new_actions_logged: u16 = 0;
        for action_res in data.fields_with(|c| c.is_ascii_whitespace()) {
            if let Ok(action) = action_res.to_str() {
                match action {
                    "errno" => Self::Errno(0).set_logged_bit(&mut new_actions_logged),
                    "kill_process" => Self::KillProcess.set_logged_bit(&mut new_actions_logged),
                    "kill_thread" => Self::KillThread.set_logged_bit(&mut new_actions_logged),
                    "log" => Self::Log.set_logged_bit(&mut new_actions_logged),
                    "trace" => Self::Trace.set_logged_bit(&mut new_actions_logged),
                    "trap" => Self::Trap(0).set_logged_bit(&mut new_actions_logged),
                    "user_notif" => Self::UserNotif.set_logged_bit(&mut new_actions_logged),
                    // Not allowed to write anything other than the approved actions to that list.
                    _ => return error!(EINVAL),
                }
            } else {
                return error!(EINVAL);
            }
        }
        kernel.actions_logged.store(new_actions_logged, Ordering::Relaxed);
        Ok(())
    }

    pub fn get_actions_logged(kernel: &Kernel) -> Vec<u8> {
        let al = kernel.actions_logged.load(Ordering::Relaxed);
        let mut result: String = "".to_string();
        for action in Self::all_actions() {
            if (al & (1 << action.logged_bit_offset())) != 0 {
                result.push_str(action.canonical_name());
                result.push(' ');
            }
        }
        if !result.is_empty() {
            // remove trailing whitespace.
            result.pop();
        }

        result.into_bytes()
    }
}

/// This struct contains data that needs to be shuttled back and forth between the thread doing
/// a USER_NOTIF and the supervisor thread responding to it.
#[derive(Default)]
struct SeccompNotification {
    /// notif is the notification set by the filter.  When this is set, the associated fd will
    /// be set to POLLIN.
    notif: seccomp_notif,

    /// Consumed indicates whether a supervisor process has read this notification (and so it
    /// can no longer be consumed by any other SECCOMP_IOCTL_NOTIF_RECV ioctl).  When the notif
    /// is consumed, the associated fd will be set to POLLOUT, indicating that it is ready to
    /// receive a response.
    consumed: bool,

    /// resp is the response that the supervisor sends.  When this is set, an event will be sent
    /// to SeccompNotifiers::waiters corresponding to the unique id of the notification.  This
    /// will wake up the filter that is waiting for this particular response.
    resp: Option<seccomp_notif_resp>,
}

impl SeccompNotification {
    fn new(data: seccomp_notif) -> SeccompNotification {
        SeccompNotification { notif: data, resp: None, consumed: false }
    }
}

/// The underlying implementation of the file descriptor that connects a process that triggers a
/// SECCOMP_RET_USER_NOTIF with the monitoring process. This support seccomp's ability to notify a
/// user-space process on specific syscall triggers. See seccomp_unotify(2) for the semantics.
pub struct SeccompNotifier {
    waiters: WaitQueue,

    pending_notifications: HashMap<u64, SeccompNotification>,

    // This keeps track of the number of threads using this notifier as a filter.  If that hits
    // zero, the listeners need to receive a HUP.
    num_active_threads: u64,

    // notifiers are referenced both by fds and in SeccompFilterContainer. If the file no longer
    // has fds referring to it, it will be closed, and the SeccompFilterContainers should stop
    // using it.
    pub is_closed: bool,

    next_cookie: u64,
}

pub type SeccompNotifierHandle = Arc<LockDepMutex<SeccompNotifier, SeccompNotifierLock>>;

impl SeccompNotifier {
    pub fn new() -> SeccompNotifierHandle {
        Arc::new(
            SeccompNotifier {
                waiters: WaitQueue::default(),
                pending_notifications: HashMap::default(),
                num_active_threads: 0,
                is_closed: false,
                next_cookie: 0,
            }
            .into(),
        )
    }

    fn next_cookie(&mut self) -> u64 {
        let cookie = self.next_cookie;
        self.next_cookie += 1;
        cookie
    }

    fn add_thread(&mut self) {
        self.num_active_threads += 1;
    }

    fn remove_thread(&mut self) {
        self.num_active_threads -= 1;
        if self.num_active_threads == 0 {
            self.waiters.notify_fd_events(FdEvents::POLLHUP);
        }
    }

    // Creates a pending notification for communication between the
    // target thread and a supervisor, and notifies readers there is
    // an opportunity to read.
    fn create_notification(&mut self, cookie: u64, notif: seccomp_notif) {
        self.pending_notifications.insert(cookie, SeccompNotification::new(notif));
        self.waiters.notify_fd_events(FdEvents::POLLIN | FdEvents::POLLRDNORM);
    }

    // Gets a notification that needs to be handled by a supervisor,
    // and notifies waiters that there is an opportunity to write.
    fn consume_some_notification(&mut self) -> Option<seccomp_notif> {
        for (_, notif) in self.pending_notifications.iter_mut() {
            if !notif.consumed {
                notif.consumed = true;
                self.waiters.notify_fd_events(FdEvents::POLLOUT | FdEvents::POLLWRNORM);
                return Some(notif.notif);
            }
        }
        None
    }

    // In case something goes wrong after we consume the notification.
    fn unconsume(&mut self, cookie: u64) {
        if let Some(n) = self.pending_notifications.get_mut(&cookie).as_mut() {
            n.consumed = false;
        }
    }

    // Returns the appropriate notifications if someone is waiting with poll/epoll/select.
    fn get_fd_notifications(&self) -> FdEvents {
        let mut events = FdEvents::empty();

        for (_, notification) in self.pending_notifications.iter() {
            if !notification.consumed {
                events |= FdEvents::POLLIN | FdEvents::POLLRDNORM;
            } else if notification.resp.is_none() {
                events |= FdEvents::POLLOUT | FdEvents::POLLWRNORM;
            }
        }

        if self.num_active_threads == 0 {
            events |= FdEvents::POLLHUP;
        }
        events
    }

    // Sets the value read by the target in response to this notification.  Intended for use by the
    // supervisor.  Notifies the filter there is a response to this request.
    fn set_response(&mut self, cookie: u64, resp: seccomp_notif_resp) -> Option<Errno> {
        if let Some(entry) = self.pending_notifications.get_mut(&cookie) {
            if entry.resp.is_some() {
                return Some(errno!(EINPROGRESS));
            }
            entry.resp = Some(resp);
            self.waiters.notify_value(resp.id);
            None
        } else {
            Some(errno!(EINVAL))
        }
    }

    // Gets the value set by the supervisor for the target to read.
    fn get_response(&self, cookie: u64) -> Option<seccomp_notif_resp> {
        if let Some(value) = self.pending_notifications.get(&cookie) {
            return value.resp;
        }
        None
    }

    // Returns whether the cookie represents an active notification.
    fn notification_pending(&self, cookie: u64) -> bool {
        self.pending_notifications.contains_key(&cookie)
    }

    // Deletes the notification, when the target is done processing it.
    fn delete_notification(&mut self, cookie: u64) {
        let _ = self.pending_notifications.remove(&cookie);
    }
}

struct SeccompNotifierFileObject {
    notifier: SeccompNotifierHandle,
}

impl FileOps for SeccompNotifierFileObject {
    fileops_impl_nonseekable!();
    fileops_impl_noop_sync!();

    fn close(self: Box<Self>, _file: &FileObjectState, _current_task: &CurrentTask) {
        let mut state = self.notifier.lock();

        for (cookie, notification) in state.pending_notifications.iter() {
            if !notification.consumed {
                state.waiters.notify_value(*cookie);
                state.waiters.notify_fd_events(FdEvents::POLLIN | FdEvents::POLLRDNORM);
            } else if notification.resp.is_none() {
                state.waiters.notify_fd_events(FdEvents::POLLOUT | FdEvents::POLLWRNORM);
            }
        }
        state.waiters.notify_fd_events(FdEvents::POLLHUP);

        state.pending_notifications.clear();

        state.is_closed = true;
    }

    fn read(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        _offset: usize,
        _usize: &mut dyn OutputBuffer,
    ) -> Result<usize, Errno> {
        error!(EINVAL)
    }

    fn write(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        _offset: usize,
        _buffer: &mut dyn InputBuffer,
    ) -> Result<usize, Errno> {
        error!(EINVAL)
    }

    fn ioctl(
        &self,
        _file: &FileObject,
        current_task: &CurrentTask,
        request: u32,
        arg: SyscallArg,
    ) -> Result<SyscallResult, Errno> {
        let user_addr = UserAddress::from(arg);
        match request {
            SECCOMP_IOCTL_NOTIF_RECV => {
                if let Ok(notif) =
                    current_task.read_memory_to_vec(user_addr, std::mem::size_of::<seccomp_notif>())
                {
                    for value in notif.iter() {
                        if *value != 0 {
                            return error!(EINVAL);
                        }
                    }
                }
                // A RECV reads a notification, optionally waiting for one to become available.
                let mut notif: Option<seccomp_notif>;
                loop {
                    // Grab a notification or wait for one to become readable.
                    let waiter = Waiter::new();
                    {
                        let mut notifier = self.notifier.lock();
                        notif = notifier.consume_some_notification();
                        if notif.is_some() {
                            break;
                        }
                        notifier.waiters.wait_async_fd_events(
                            &waiter,
                            FdEvents::POLLIN | FdEvents::POLLHUP,
                            EventHandler::None,
                        );
                    }
                    waiter.wait(current_task)?;
                }
                if let Some(notif) = notif {
                    if let Err(e) =
                        current_task.write_object(UserRef::<seccomp_notif>::new(user_addr), &notif)
                    {
                        self.notifier.lock().unconsume(notif.id);
                        return Err(e);
                    }
                }

                Ok(0.into())
            }
            SECCOMP_IOCTL_NOTIF_SEND => {
                // A SEND sends a response to a previously received notification.
                let resp: seccomp_notif_resp = current_task.read_object(UserRef::new(user_addr))?;
                if resp.flags & !SECCOMP_USER_NOTIF_FLAG_CONTINUE != 0 {
                    return error!(EINVAL);
                }
                if resp.flags & SECCOMP_USER_NOTIF_FLAG_CONTINUE != 0
                    && (resp.error != 0 || resp.val != 0)
                {
                    return error!(EINVAL);
                }
                {
                    let mut notifier = self.notifier.lock();
                    if let Some(err) = notifier.set_response(resp.id, resp) {
                        return Err(err);
                    }
                }
                Ok(0.into())
            }
            SECCOMP_IOCTL_NOTIF_ID_VALID => {
                // An ID_VALID indicates that the notification is still in progress.
                let cookie: u64 = current_task.read_object(UserRef::new(user_addr))?;
                {
                    let notifier = self.notifier.lock();
                    if notifier.notification_pending(cookie) {
                        Ok(0.into())
                    } else {
                        error!(ENOENT)
                    }
                }
            }
            SECCOMP_IOCTL_NOTIF_ADDFD => error!(EINVAL),
            _ => error!(EINVAL),
        }
    }

    fn wait_async(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        waiter: &Waiter,
        events: FdEvents,
        handler: EventHandler,
    ) -> Option<WaitCanceler> {
        let notifier = self.notifier.lock();
        Some(notifier.waiters.wait_async_fd_events(waiter, events, handler))
    }

    fn query_events(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
    ) -> Result<FdEvents, Errno> {
        Ok(self.notifier.lock().get_fd_notifications())
    }
}

#[cfg(test)]
mod test {
    use crate::task::SeccompAction;
    use crate::testing::spawn_kernel_and_run;

    #[::fuchsia::test]
    async fn test_actions_logged_accepts_legal_string() {
        spawn_kernel_and_run(async |current_task| {
            let kernel = current_task.kernel();
            let mut actions = SeccompAction::get_actions_avail_file();
            // This is a test in Rust instead of a syscall test because we don't want to change the
            // global config in a test.
            assert!(
                SeccompAction::set_actions_logged(&kernel, &actions[..]).is_err(),
                "Should not be able to write allow to actions_logged file"
            );
            let action_string = std::string::String::from_utf8(actions.clone()).unwrap();
            if let Some(action_index) = action_string.find("allow") {
                actions.drain(action_index..action_index + "allow".len());
            }
            let write_result = SeccompAction::set_actions_logged(&kernel, &actions[..]);
            assert!(
                write_result.is_ok(),
                "Could not write legal string \"{}\" to actions_logged file: error {}",
                std::string::String::from_utf8(actions.clone()).unwrap(),
                write_result.unwrap_err()
            );
        })
        .await;
    }
}
