// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::arch::task::handle_hardware_exception;
use crate::execution::{TaskInfo, create_zircon_process};
use crate::mm::{DumpPolicy, MemoryAccessor, MemoryAccessorExt, MemoryManager, TaskMemoryAccessor};
use crate::ptrace::{PtraceCoreState, PtraceEvent, PtraceEventData, PtraceOptions, StopState};
use crate::security;
use crate::signals::{SignalDetail, SignalInfo, send_signal_first, send_standard_signal};
use crate::task::loader::{
    ResolvedProgram, load_executable, resolve_elf_interpreter, resolve_executable,
};
use crate::task::waiter::WaiterOptions;
use crate::task::{
    CurrentTaskCredentialsWriteGuard, ExitStatus, PageFaultExceptionReport, RobustListHeadPtr,
    RunState, SeccompFilter, SeccompFilterContainer, SeccompState, SeccompStateValue, Task,
    TaskFlags, TaskRunningState, ThreadState, Waiter,
};
use crate::vfs::{
    AccessCheck, DirectoryMode, FdFlags, FdNumber, FdTable, FileHandle, FileMapping,
    FileWriteGuardMode, FsContext, FsStr, LookupContext, LookupVec, MAX_SYMLINK_FOLLOWS,
    NamespaceNode, OpenAccessCheck, ResolveBase, SymlinkMode, SymlinkTarget, new_pidfd,
};
use futures::FutureExt;
use linux_uapi::CLONE_PIDFD;
use starnix_logging::{CATEGORY_STARNIX, log_error, log_warn, track_file_not_found, track_stub};
use starnix_registers::{HeapRegs, RegisterStorageEnum};
use starnix_stack::clean_stack;
use starnix_sync::{EventWaitGuard, UninterruptibleLock, WakeReason, assert_lock_level};
use starnix_syscalls::SyscallResult;
use starnix_syscalls::decls::Syscall;
use starnix_task_command::TaskCommand;
use starnix_types::futex_address::FutexAddress;
use starnix_types::ownership::{Releasable, release_on_error};
use starnix_uapi::auth::{
    CAP_KILL, CAP_SYS_ADMIN, CAP_SYS_PTRACE, Credentials, FsCred, PTRACE_MODE_FSCREDS,
    PTRACE_MODE_REALCREDS, PtraceAccessMode,
};
use starnix_uapi::device_id::DeviceId;
use starnix_uapi::errors::Errno;
use starnix_uapi::file_mode::{Access, FileMode};
use starnix_uapi::open_flags::OpenFlags;
use starnix_uapi::signals::{
    SIGCHLD, SIGCONT, SIGILL, SIGKILL, SIGSEGV, SIGSYS, SIGTRAP, SigSet, Signal, UncheckedSignal,
};
use starnix_uapi::user_address::{ArchSpecific, UserAddress, UserRef};
use starnix_uapi::vfs::ResolveFlags;
use starnix_uapi::{
    CLONE_CHILD_CLEARTID, CLONE_CHILD_SETTID, CLONE_CLEAR_SIGHAND, CLONE_FILES, CLONE_FS,
    CLONE_INTO_CGROUP, CLONE_NEWUTS, CLONE_PARENT, CLONE_PARENT_SETTID, CLONE_PTRACE, CLONE_SETTLS,
    CLONE_SIGHAND, CLONE_SYSVSEM, CLONE_THREAD, CLONE_VFORK, CLONE_VM, FUTEX_OWNER_DIED,
    FUTEX_TID_MASK, ROBUST_LIST_LIMIT, SECCOMP_FILTER_FLAG_LOG, SECCOMP_FILTER_FLAG_NEW_LISTENER,
    SECCOMP_FILTER_FLAG_TSYNC, SECCOMP_FILTER_FLAG_TSYNC_ESRCH, clone_args, errno, error, pid_t,
    sock_filter, ucred,
};
use std::cell::{Ref, RefCell};
use std::collections::VecDeque;
use std::ffi::CString;
use std::fmt;
use std::marker::PhantomData;
use std::mem::MaybeUninit;
use std::sync::{Arc, Weak};
use zx::sys::zx_restricted_state_t;

use super::ThreadGroupLifecycleWaitValue;

pub struct TaskBuilder {
    /// The underlying task object.
    pub task: Arc<Task>,

    pub thread_state: ThreadState<HeapRegs>,
}

impl TaskBuilder {
    pub fn new(task: Arc<Task>) -> Self {
        Self { task, thread_state: Default::default() }
    }

    #[inline(always)]
    pub fn release(self, _context: ()) {
        Releasable::release(self, ());
    }
}

impl From<TaskBuilder> for CurrentTask {
    fn from(builder: TaskBuilder) -> Self {
        Self::new(builder.task, builder.thread_state.into())
    }
}

impl Releasable for TaskBuilder {
    type Context<'a> = ();

    fn release<'a>(self, _context: Self::Context<'a>) {
        // Build a temporary CurrentTask to run release actions that require ThreadState.
        let current_task = CurrentTask::new(self.task, self.thread_state.into());
        current_task.exit();
    }
}

impl std::ops::Deref for TaskBuilder {
    type Target = Task;
    fn deref(&self) -> &Self::Target {
        &self.task
    }
}

/// The task object associated with the currently executing thread.
///
/// We often pass the `CurrentTask` as the first argument to functions if those functions need to
/// know contextual information about the thread on which they are running. For example, we often
/// use the `CurrentTask` to perform access checks, which ensures that the caller is authorized to
/// perform the requested operation.
///
/// The `CurrentTask` also has state that can be referenced only on the currently executing thread,
/// such as the register state for that thread. Syscalls are given a mutable references to the
/// `CurrentTask`, which lets them manipulate this state.
///
/// See also `Task` for more information about tasks.
pub struct CurrentTask {
    /// The underlying task object.
    pub task: Arc<Task>,

    pub thread_state: ThreadState<RegisterStorageEnum>,

    /// The cached running state of the task.
    ///
    /// Extracting `TaskRunningState` from a generic `Task` requires acquiring an RCU read lock.
    /// While this is a relatively inexpensive operation, it is unnecessary in most cases because
    /// `CurrentTask` always corresponds to a running `Task`, and every running `Task` has a
    /// `TaskRunningState`. As such, the `CurrentTask` can safely cache a reference to its
    /// `TaskRunningState`. This reference will only be invalidated during task exit.
    pub running_state: Option<Arc<TaskRunningState>>,

    /// The cached file descriptor table of the task.
    ///
    /// Extracting `FdTable` from a generic `Task` is a heavy operation with multiple levels of
    /// synchronization and reference counting. However, because `CurrentTask` always corresponds to
    /// a running `Task`, and every running `Task` has an `FdTable`, the `CurrentTask` can safely
    /// cache a reference to its `FdTable`.
    pub files: RefCell<Option<Arc<FdTable>>>,

    /// The current subjective credentials of the task.
    // TODO(https://fxbug.dev/433548348): Avoid interior mutability here by passing a
    // &mut CurrentTask around instead of &CurrentTask.
    pub current_creds: RefCell<CurrentCreds>,

    pub security_state: security::CurrentTaskState,

    /// Makes CurrentTask neither Sync not Send.
    _local_marker: PhantomData<*mut u8>,
}

/// Represents the current state of the task's subjective credentials.
pub enum CurrentCreds {
    /// The task does not have overridden credentials, the subjective creds are identical to the
    /// objective creds stored in the Task. Since credentials are often accessed from the current
    /// task, we hold a reference here that does not necessitate going through the RCU machinery to
    /// read.
    Cached(Arc<Credentials>),
    /// The task has overridden subjective credentials.
    Overridden(Arc<Credentials>),
}

impl CurrentCreds {
    fn creds(&self) -> &Arc<Credentials> {
        match self {
            CurrentCreds::Cached(creds) => creds,
            CurrentCreds::Overridden(creds) => creds,
        }
    }
}

impl Releasable for CurrentTask {
    type Context<'a> = ();

    fn release<'a>(self, _context: Self::Context<'a>) {
        self.exit();
    }
}

impl std::ops::Deref for CurrentTask {
    type Target = Task;
    fn deref(&self) -> &Self::Target {
        &self.task
    }
}

impl fmt::Debug for CurrentTask {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.task.fmt(f)
    }
}

impl CurrentTask {
    pub fn new(task: Arc<Task>, thread_state: ThreadState<RegisterStorageEnum>) -> Self {
        let current_creds = RefCell::new(CurrentCreds::Cached(task.clone_creds()));
        let running_state = task.running_state().expect("CurrentTask must have TaskRunningState");
        let files = running_state.files().expect("CurrentTask must have FdTable");
        Self {
            task,
            thread_state,
            running_state: Some(running_state),
            files: RefCell::new(Some(files)),
            current_creds,
            security_state: Default::default(),
            _local_marker: Default::default(),
        }
    }

    /// Exit the task by dropping its running state.
    pub fn exit(mut self) {
        // When this method returns, the following invariants must be met:
        // 1. No new references to running `Task` state must be obtainable.
        // 2. All externally-visible `Task` state must reflect that the `Task` has exited.
        // 3. All observers of `Task` exit events must be notified.

        self.notify_robust_list();
        let _ignored = self.clear_child_tid_if_needed();

        self.signal_vfork();

        // Release references to resources specific to the running task before triggering its
        // delayed releaser for the last time. This schedules any RCU-guarded references retained
        // solely by this task for RCU reclamation. Triggering the delayed releaser runs RCU
        // callbacks, ensuring that:
        //
        // 1. Any delayed release actions registered by the resource being dropped during
        //    reclamation are applied during the final delayed releaser trigger.
        // 2. Any other drop side-effects happen before the thread group sends zombie notifications.
        //
        // Specifically, the following resources require explicit release:
        //
        // 1. `running_state`: Transitively releases `fs` and `proc_pid_directory_cache`
        // 2. `files`: Drops `FileHandle` to close open file descriptors
        // 3. `mm`: Drops `FsNodeHandle` to remove memory-mapped filesystem nodes and drops
        //    `FileWriteGuard` for executable mappings
        // 4. `fs`: Drops `MountClientMarker` to allow unmounting and drops `FsNodeHandle` to remove
        //    namespace filesystem nodes
        // 5. `proc_pid_directory_cache`: Drops `FsNodeHandle` to remove /proc/<pid> nodes

        if let Some(running_state) = self.running_state.take() {
            *running_state.files.lock() = None;
            running_state.mm.update(None);
        }

        *self.files.borrow_mut() = None;
        self.task.running_state.update(None);

        self.trigger_delayed_releaser();

        // We remove from the thread group here because the Weak in the pid
        // table to this task must be valid until this task is removed from the
        // thread group, and the code below will invalidate it.
        // Moreover, this requires an Arc of the task to ensure the tasks of
        // the thread group are always valid.
        self.task.thread_group().remove(self.kernel().pids.lock(), &self.task);

        self.ptrace_disconnect();
    }

    /// Returns the [`TaskRunningState`] for the [`Task`].
    ///
    /// # Panics
    ///
    /// Calling `running_state()` on a [`CurrentTask`] for which the [`Task`] has no running state
    /// (i.e. exited tasks) panics. However, such tasks should not have a [`CurrentTask`].
    ///
    /// This is primarily a risk in delayed release actions, which receive `&CurrentTask` when its
    /// delayed releaser is triggered for the final time. At that point the task is mid-exit and its
    /// [`TaskRunningState`] has been dropped. As such, delayed releases must not use the following
    /// accessors:
    ///
    /// - [`Self::running_state()`]
    /// - [`Self::files()`]
    /// - [`Self::fs()`]
    ///
    /// If access to [`TaskRunningState`] is required in a delayed release action, use
    /// [`Task::running_state()`] or an equivalent fallible accessor.
    #[track_caller]
    pub fn running_state(&self) -> &Arc<TaskRunningState> {
        self.running_state.as_ref().expect("CurrentTask must have TaskRunningState")
    }

    /// Returns the [`FdTable`] for the [`Task`].
    ///
    /// # Panics
    ///
    /// Calling `files()` on a [`CurrentTask`] for which the [`Task`] has no file descriptor table
    /// (i.e. exited tasks) panics. However, such tasks should not have a `CurrentTask`.
    #[track_caller]
    pub fn files(&self) -> Arc<FdTable> {
        self.files.borrow().as_ref().expect("CurrentTask must have FdTable").clone()
    }

    pub fn fs(&self) -> Arc<FsContext> {
        self.running_state().fs()
    }

    pub fn has_shared_fs(&self) -> bool {
        let fs = self.fs();
        // This check is incorrect because someone else could be holding a temporary Arc to the
        // FsContext and therefore increasing the strong count.
        Arc::strong_count(&fs) > 2usize
    }

    pub fn unshare_fs(&self) {
        let new_fs = self.fs().fork();
        self.running_state().fs.update(new_fs);
    }

    /// Returns the current subjective credentials of the task.
    ///
    /// The subjective credentials are the credentials that are used to check permissions for
    /// actions performed by the task.
    pub fn current_creds(&self) -> Ref<'_, Arc<Credentials>> {
        Ref::map(self.current_creds.borrow(), CurrentCreds::creds)
    }

    pub fn current_fscred(&self) -> FsCred {
        self.current_creds().as_fscred()
    }

    pub fn current_ucred(&self) -> ucred {
        let creds = self.current_creds();
        ucred { pid: self.get_pid(), uid: creds.uid, gid: creds.gid }
    }

    /// Save the current creds and security state, alter them by calling `alter_creds`, then call
    /// `callback`.
    /// The creds and security state will be restored to their original values at the end of the
    /// call. Only the "subjective" state of the CurrentTask, accessed with `current_creds()` and
    ///  used to check permissions for actions performed by the task, is altered. The "objective"
    ///  state, accessed through `Task::real_creds()` by other tasks and used to check permissions
    /// for actions performed on the task, is not altered, and changes to the credentials are not
    /// externally visible.
    pub async fn override_creds_async<R>(
        &self,
        new_creds: Arc<Credentials>,
        callback: impl AsyncFnOnce() -> R,
    ) -> R {
        let saved = self.current_creds.replace(CurrentCreds::Overridden(new_creds));
        let result = callback().await;
        self.current_creds.replace(saved);
        result
    }

    /// Save the current creds and security state, alter them by calling `alter_creds`, then call
    /// `callback`.
    /// The creds and security state will be restored to their original values at the end of the
    /// call. Only the "subjective" state of the CurrentTask, accessed with `current_creds()` and
    ///  used to check permissions for actions performed by the task, is altered. The "objective"
    ///  state, accessed through `Task::real_creds()` by other tasks and used to check permissions
    /// for actions performed on the task, is not altered, and changes to the credentials are not
    /// externally visible.
    pub fn override_creds<R>(
        &self,
        new_creds: Arc<Credentials>,
        callback: impl FnOnce() -> R,
    ) -> R {
        self.override_creds_async(new_creds, async move || callback())
            .now_or_never()
            .expect("Future should be ready")
    }

    pub fn has_overridden_creds(&self) -> bool {
        matches!(*self.current_creds.borrow(), CurrentCreds::Overridden(_))
    }

    pub fn trigger_delayed_releaser(&self) {
        self.kernel().delayed_releaser.apply(self);
    }

    pub fn weak_task(&self) -> Weak<Task> {
        Arc::downgrade(&self.task)
    }

    /// Locks the `CurrentTask`'s credentials for writing, allowing readers to coordinate by using
    /// `Task::lock_creds()` where necessary.  e.g. This is used to avoid ptrace attachment racing
    /// with critical security checks affecting the task's `Credentials` during `exec()`.
    pub fn write_creds(&self) -> CurrentTaskCredentialsWriteGuard {
        assert!(!self.has_overridden_creds());
        self.persistent_info.write_current_task_creds()
    }

    /// Change the current and real creds of the task. This is invalid to call while temporary
    /// credentials are present.
    pub fn set_creds(&self, creds: Credentials) {
        let creds = Arc::new(creds);
        self.write_creds().update(self, creds);
    }

    #[inline(always)]
    pub fn release(self, _context: ()) {
        Releasable::release(self, ());
    }

    pub fn set_syscall_restart_func<R: Into<SyscallResult>>(
        &mut self,
        f: impl FnOnce(&mut CurrentTask) -> Result<R, Errno> + Send + Sync + 'static,
    ) {
        self.thread_state.syscall_restart_func =
            Some(Box::new(|current_task| Ok(f(current_task)?.into())));
    }

    pub fn add_file(&self, file: FileHandle, flags: FdFlags) -> Result<FdNumber, Errno> {
        self.files().add(self, file, flags)
    }

    /// Sets the task's signal mask to `signal_mask` and runs `wait_function`.
    ///
    /// Signals are dequeued prior to the original signal mask being restored. This is done by the
    /// signal machinery in the syscall dispatch loop.
    ///
    /// The returned result is the result returned from the wait function.
    pub fn wait_with_temporary_mask<F, T>(
        &mut self,
        signal_mask: SigSet,
        wait_function: F,
    ) -> Result<T, Errno>
    where
        F: FnOnce(&CurrentTask) -> Result<T, Errno>,
    {
        {
            let mut state = self.write();
            state.set_flags(TaskFlags::TEMPORARY_SIGNAL_MASK, true);
            state.set_temporary_signal_mask(signal_mask);
        }
        wait_function(self)
    }

    /// If waking, promotes from waking to awake.  If not waking, make waiter async
    /// wait until woken.  Returns true if woken.
    pub fn wake_or_wait_until_unstopped_async(&self, waiter: &Waiter) -> bool {
        let group_state = self.thread_group().read();
        let mut task_state = self.write();

        // Wake up if
        //   a) we should wake up, meaning:
        //      i) we're in group stop, and the thread group has exited group stop, or
        //      ii) we're waking up,
        //   b) and ptrace isn't stopping us from waking up, but
        //   c) always wake up if we got a SIGKILL.
        let task_stop_state = self.load_stopped();
        let group_stop_state = self.thread_group().load_stopped();
        if ((task_stop_state == StopState::GroupStopped && group_stop_state.is_waking_or_awake())
            || task_stop_state.is_waking_or_awake())
            && (!task_state.is_ptrace_listening() || task_stop_state.is_force())
        {
            let new_state = if task_stop_state.is_waking_or_awake() {
                task_stop_state.finalize()
            } else {
                group_stop_state.finalize()
            };
            if let Ok(new_state) = new_state {
                task_state.set_stopped(new_state, None, Some(self), None);
                drop(group_state);
                drop(task_state);
                // It is possible for the stop state to be changed by another
                // thread between when it is checked above and the following
                // invocation, but set_stopped does sufficient checking while
                // holding the lock to make sure that such a change won't result
                // in corrupted state.
                self.thread_group().set_stopped(new_state, None, false);
                return true;
            }
        }

        // We will wait.
        if self.thread_group().load_stopped().is_stopped() || task_stop_state.is_stopped() {
            // If we've stopped or PTRACE_LISTEN has been sent, wait for a
            // signal or instructions from the tracer.
            group_state
                .lifecycle_waiters
                .wait_async_value(&waiter, ThreadGroupLifecycleWaitValue::Stopped);
            task_state.wait_on_ptracer(&waiter);
        } else if task_state.can_accept_ptrace_commands() {
            // If we're stopped because a tracer has seen the stop and not taken
            // further action, wait for further instructions from the tracer.
            task_state.wait_on_ptracer(&waiter);
        } else if task_state.is_ptrace_listening() {
            // A PTRACE_LISTEN is a state where we can get signals and notify a
            // ptracer, but otherwise remain blocked.
            if let Some(ptrace) = &mut task_state.ptrace {
                ptrace.set_last_signal(Some(SignalInfo::kernel(SIGTRAP)));
                ptrace.set_last_event(Some(PtraceEventData::new_from_event(PtraceEvent::Stop, 0)));
            }
            task_state.wait_on_ptracer(&waiter);
            task_state.notify_ptracers();
        }
        false
    }

    /// Set the RunState for the current task to the given value and then call the given callback.
    ///
    /// When the callback is done, the run_state is restored to `RunState::Running`.
    ///
    /// This function is typically used just before blocking the current task on some operation.
    /// The given `run_state` registers the mechanism for interrupting the blocking operation with
    /// the task and the given `callback` actually blocks the task.
    ///
    /// This function can only be called in the `RunState::Running` state and cannot set the
    /// run state to `RunState::Running`. For this reason, this function cannot be reentered.
    pub fn run_in_state<F, T>(&self, run_state: RunState, callback: F) -> Result<T, Errno>
    where
        F: FnOnce() -> Result<T, Errno>,
    {
        assert_ne!(run_state, RunState::Running);

        // Check we do not hold any uninterruptible lock
        assert_lock_level::<UninterruptibleLock>();
        // As an optimization, decommit unused pages of the stack to reduce memory pressure while
        // the thread is blocked.
        clean_stack();

        {
            let mut state = self.write();
            assert!(!state.is_blocked());

            if matches!(run_state, RunState::Frozen(_)) {
                // Freeze is a kernel signal and is handled before other user signals. A frozen task
                // ignores all other signals except SIGKILL until it is thawed.
                if state.has_signal_pending(SIGKILL) {
                    return error!(EINTR);
                }
            } else if state.is_any_signal_pending() && !state.is_ptrace_listening() {
                // A note on PTRACE_LISTEN - the thread cannot be scheduled
                // regardless of pending signals.
                return error!(EINTR);
            }
            state.set_run_state(run_state.clone());
        }

        let _waiting_guard = crate::task::ThreadLockupDetector::pause_tracking();
        let result = callback();

        {
            let mut state = self.write();
            assert_eq!(
                state.run_state(),
                run_state,
                "SignalState run state changed while waiting!"
            );
            state.set_run_state(RunState::Running);
        };

        result
    }

    pub fn block_until(
        &self,
        guard: EventWaitGuard<'_>,
        deadline: zx::MonotonicInstant,
    ) -> Result<(), Errno> {
        self.block_with_optional_owner_until(guard, None, deadline)
    }

    pub fn block_with_owner_until(
        &self,
        guard: EventWaitGuard<'_>,
        new_owner: &zx::Thread,
        deadline: zx::MonotonicInstant,
    ) -> Result<(), Errno> {
        self.block_with_optional_owner_until(guard, Some(new_owner), deadline)
    }

    pub fn block_with_optional_owner_until(
        &self,
        guard: EventWaitGuard<'_>,
        new_owner: Option<&zx::Thread>,
        deadline: zx::MonotonicInstant,
    ) -> Result<(), Errno> {
        self.run_in_state(RunState::Event(guard.event().clone()), move || {
            guard.block_until(new_owner, deadline).map_err(|e| match e {
                WakeReason::Interrupted => errno!(EINTR),
                WakeReason::DeadlineExpired => errno!(ETIMEDOUT),
            })
        })
    }

    /// Determine namespace node indicated by the dir_fd.
    ///
    /// Returns the namespace node and the path to use relative to that node.
    pub fn resolve_dir_fd<'a>(
        &self,
        dir_fd: FdNumber,
        mut path: &'a FsStr,
        flags: ResolveFlags,
    ) -> Result<(NamespaceNode, &'a FsStr), Errno> {
        let path_is_absolute = path.starts_with(b"/");
        if path_is_absolute {
            if flags.contains(ResolveFlags::BENEATH) {
                return error!(EXDEV);
            }
            path = &path[1..];
        }

        let dir = if path_is_absolute && !flags.contains(ResolveFlags::IN_ROOT) {
            self.fs().root()
        } else if dir_fd == FdNumber::AT_FDCWD {
            self.fs().cwd()
        } else {
            // O_PATH allowed for:
            //
            //   Passing the file descriptor as the dirfd argument of
            //   openat() and the other "*at()" system calls.  This
            //   includes linkat(2) with AT_EMPTY_PATH (or via procfs
            //   using AT_SYMLINK_FOLLOW) even if the file is not a
            //   directory.
            //
            // See https://man7.org/linux/man-pages/man2/open.2.html
            let file = self.files().get_allowing_opath(dir_fd)?;
            file.name.to_passive()
        };

        if !path.is_empty() {
            if !dir.entry.node.is_dir() {
                return error!(ENOTDIR);
            }
            dir.check_access(self, AccessCheck::for_internal(Access::EXEC))?;
        }
        Ok((dir, path.into()))
    }

    /// A convenient wrapper for opening files relative to FdNumber::AT_FDCWD.
    ///
    /// Returns a FileHandle but does not install the FileHandle in the FdTable
    /// for this task.
    pub fn open_file(&self, path: &FsStr, flags: OpenFlags) -> Result<FileHandle, Errno> {
        if flags.contains(OpenFlags::CREAT) {
            // In order to support OpenFlags::CREAT we would need to take a
            // FileMode argument.
            return error!(EINVAL);
        }
        self.open_file_at(
            FdNumber::AT_FDCWD,
            path,
            flags,
            FileMode::default(),
            ResolveFlags::empty(),
        )
    }

    /// Opens an executable or interpreter file for binary execution.
    ///
    /// This method is intended for opening initial executables, script interpreters (`#!`),
    /// and ELF dynamic linkers (`PT_INTERP`).
    ///
    /// Resolves the target [`NamespaceNode`] and verifies that it is a regular file before opening
    /// it, avoiding unintended driver initialization on device nodes or blocking on FIFOs. Returns
    /// [`EACCES`] if the target is not a regular file, or [`ELOOP`] if [`OpenFlags::NOFOLLOW`] was
    /// specified and the target is a symbolic link.
    ///
    /// Opens the file for execution, verifying DAC execute permissions and filesystem mount
    /// `MS_NOEXEC`.
    ///
    /// Returns an [`Arc<FileMapping>`] with an execution write guard.
    pub fn open_file_for_exec(
        &self,
        dir_fd: FdNumber,
        path: &FsStr,
        flags: OpenFlags,
    ) -> Result<Arc<FileMapping>, Errno> {
        debug_assert!(
            (flags & !(OpenFlags::RDONLY | OpenFlags::NOFOLLOW)).is_empty(),
            "unexpected flags passed to open_file_for_exec: {flags:?}"
        );
        if !(flags & !(OpenFlags::RDONLY | OpenFlags::NOFOLLOW)).is_empty() {
            return error!(EINVAL);
        }

        let (dir, path) = self.resolve_dir_fd(dir_fd, path, ResolveFlags::empty())?;
        let nofollow = flags.contains(OpenFlags::NOFOLLOW);
        let mut context =
            LookupContext::new(if nofollow { SymlinkMode::NoFollow } else { SymlinkMode::Follow });
        context.update_for_path(path);
        let name = self.lookup_path(&mut context, dir, path)?;

        // From <https://man7.org/linux/man-pages/man2/execveat.2.html>:
        //
        //   ELOOP  flags includes AT_SYMLINK_NOFOLLOW and the file identified by
        //          dirfd and pathname is a symbolic link.
        if nofollow && name.entry.node.info().mode.is_lnk() {
            return error!(ELOOP);
        }

        // From <https://man7.org/linux/man-pages/man2/execve.2.html>:
        //
        //   EACCES The file or a script interpreter is not a regular file.
        if !name.entry.node.is_reg() {
            return error!(EACCES);
        }

        // From <https://man7.org/linux/man-pages/man2/execve.2.html>:
        //
        //   EACCES Execute permission is denied for the file or a script or ELF
        //          interpreter.
        //
        //   EACCES The filesystem is mounted noexec.
        let file = name.open(self, OpenAccessCheck::for_exec())?;
        FileMapping::new(file, Some(FileWriteGuardMode::ExecMapping))
    }

    /// Resolves a path for open.
    ///
    /// If the final path component points to a symlink, the symlink is followed (as long as
    /// the symlink traversal limit has not been reached).
    ///
    /// If the final path component (after following any symlinks, if enabled) does not exist,
    /// and `flags` contains `OpenFlags::CREAT`, a new node is created at the location of the
    /// final path component.
    ///
    /// This returns the resolved node, and a boolean indicating whether the node has been created.
    fn resolve_open_path(
        &self,
        context: &mut LookupContext,
        dir: &NamespaceNode,
        path: &FsStr,
        mode: FileMode,
        flags: OpenFlags,
    ) -> Result<(NamespaceNode, bool), Errno> {
        context.update_for_path(path);
        let mut parent_content = context.with(SymlinkMode::Follow, context.directory_mode);
        let (parent, basename) = self.lookup_parent(&mut parent_content, dir, path)?;
        context.remaining_follows = parent_content.remaining_follows;

        let must_create = flags.contains(OpenFlags::CREAT) && flags.contains(OpenFlags::EXCL);

        // Lookup the child, without following a symlink or expecting it to be a directory.
        let mut child_context = context.with(SymlinkMode::NoFollow, DirectoryMode::AllowAny);

        match parent.lookup_child(self, &mut child_context, basename) {
            Ok(name) => {
                if name.entry.node.is_lnk() {
                    if flags.contains(OpenFlags::PATH)
                        && context.symlink_mode == SymlinkMode::NoFollow
                    {
                        // When O_PATH is specified in flags, if pathname is a symbolic link
                        // and the O_NOFOLLOW flag is also specified, then the call returns
                        // a file descriptor referring to the symbolic link.
                        // See https://man7.org/linux/man-pages/man2/openat.2.html
                        //
                        // If the trailing component (i.e., basename) of
                        // pathname is a symbolic link, how.resolve contains
                        // RESOLVE_NO_SYMLINKS, and how.flags contains both
                        // O_PATH and O_NOFOLLOW, then an O_PATH file
                        // descriptor referencing the symbolic link will be
                        // returned.
                        // See https://man7.org/linux/man-pages/man2/openat2.2.html
                        return Ok((name, false));
                    }

                    if (!flags.contains(OpenFlags::PATH)
                        && context.symlink_mode == SymlinkMode::NoFollow)
                        || context.resolve_flags.contains(ResolveFlags::NO_SYMLINKS)
                        || context.remaining_follows == 0
                    {
                        if must_create {
                            // Since `must_create` is set, and a node was found, this returns EEXIST
                            // instead of ELOOP.
                            return error!(EEXIST);
                        }
                        // A symlink was found, but one of the following is true:
                        // * flags specified O_NOFOLLOW but not O_PATH.
                        // * how.resolve contains RESOLVE_NO_SYMLINKS
                        // * too many symlink traversals have been attempted
                        return error!(ELOOP);
                    }

                    context.remaining_follows -= 1;
                    match name.readlink(self)? {
                        SymlinkTarget::Path(path) => {
                            let dir = if path[0] == b'/' { self.fs().root() } else { parent };
                            self.resolve_open_path(context, &dir, path.as_ref(), mode, flags)
                        }
                        SymlinkTarget::Node(name) => {
                            if context.resolve_flags.contains(ResolveFlags::NO_MAGICLINKS)
                                || name.entry.node.is_lnk()
                            {
                                error!(ELOOP)
                            } else {
                                Ok((name, false))
                            }
                        }
                    }
                } else {
                    if must_create {
                        return error!(EEXIST);
                    }
                    Ok((name, false))
                }
            }
            Err(e) if e == errno!(ENOENT) && flags.contains(OpenFlags::CREAT) => {
                if context.directory_mode == DirectoryMode::MustBeDirectory {
                    return error!(EISDIR);
                }
                Ok((
                    parent.open_create_node(
                        self,
                        basename,
                        mode.with_type(FileMode::IFREG),
                        DeviceId::NONE,
                        flags,
                    )?,
                    true,
                ))
            }
            Err(e) => Err(e),
        }
    }

    /// The primary entry point for opening files relative to a task.
    ///
    /// Absolute paths are resolve relative to the root of the FsContext for
    /// this task. Relative paths are resolve relative to dir_fd. To resolve
    /// relative to the current working directory, pass FdNumber::AT_FDCWD for
    /// dir_fd.
    ///
    /// Returns a FileHandle but does not install the FileHandle in the FdTable
    /// for this task.
    pub fn open_file_at(
        &self,
        dir_fd: FdNumber,
        path: &FsStr,
        flags: impl Into<OpenAccessCheck>,
        mode: FileMode,
        resolve_flags: ResolveFlags,
    ) -> Result<FileHandle, Errno> {
        if path.is_empty() {
            return error!(ENOENT);
        }

        let (dir, path) = self.resolve_dir_fd(dir_fd, path, resolve_flags)?;
        self.open_namespace_node_at(dir, path, flags, mode, resolve_flags)
    }

    pub fn open_namespace_node_at(
        &self,
        dir: NamespaceNode,
        path: &FsStr,
        open_check: impl Into<OpenAccessCheck>,
        mode: FileMode,
        mut resolve_flags: ResolveFlags,
    ) -> Result<FileHandle, Errno> {
        let open_check = open_check.into();
        // 64-bit kernels force the O_LARGEFILE flag to be on.
        let mut flags = open_check.open_flags() | OpenFlags::LARGEFILE;
        let opath = flags.contains(OpenFlags::PATH);
        if opath {
            // When O_PATH is specified in flags, flag bits other than O_CLOEXEC,
            // O_DIRECTORY, and O_NOFOLLOW are ignored.
            const ALLOWED_FLAGS: OpenFlags = OpenFlags::from_bits_truncate(
                OpenFlags::PATH.bits()
                    | OpenFlags::CLOEXEC.bits()
                    | OpenFlags::DIRECTORY.bits()
                    | OpenFlags::NOFOLLOW.bits(),
            );
            flags &= ALLOWED_FLAGS;
        }

        if flags.contains(OpenFlags::TMPFILE) && !flags.can_write() {
            return error!(EINVAL);
        }

        let nofollow = flags.contains(OpenFlags::NOFOLLOW);
        let must_create = flags.contains(OpenFlags::CREAT) && flags.contains(OpenFlags::EXCL);

        let symlink_mode =
            if nofollow || must_create { SymlinkMode::NoFollow } else { SymlinkMode::Follow };

        let resolve_base = match (
            resolve_flags.contains(ResolveFlags::BENEATH),
            resolve_flags.contains(ResolveFlags::IN_ROOT),
        ) {
            (false, false) => ResolveBase::None,
            (true, false) => ResolveBase::Beneath(dir.clone()),
            (false, true) => ResolveBase::InRoot(dir.clone()),
            (true, true) => return error!(EINVAL),
        };

        // `RESOLVE_BENEATH` and `RESOLVE_IN_ROOT` imply `RESOLVE_NO_MAGICLINKS`. This matches
        // Linux behavior. Strictly speaking it's is not really required, but it's hard to
        // implement `BENEATH` and `IN_ROOT` flags correctly otherwise.
        if resolve_base != ResolveBase::None {
            resolve_flags.insert(ResolveFlags::NO_MAGICLINKS);
        }

        let mut context = LookupContext {
            symlink_mode,
            remaining_follows: MAX_SYMLINK_FOLLOWS,
            directory_mode: if flags.contains(OpenFlags::DIRECTORY) {
                DirectoryMode::MustBeDirectory
            } else {
                DirectoryMode::AllowAny
            },
            resolve_flags,
            resolve_base,
        };
        let (name, created) = match self.resolve_open_path(&mut context, &dir, path, mode, flags) {
            Ok((n, c)) => (n, c),
            Err(e) => {
                let mut abs_path = dir.path(&self.fs());
                abs_path.extend(&**path);
                track_file_not_found(abs_path);
                return Err(e);
            }
        };

        let name = if flags.contains(OpenFlags::TMPFILE) {
            // `O_TMPFILE` is incompatible with `O_CREAT`
            if flags.contains(OpenFlags::CREAT) {
                return error!(EINVAL);
            }
            name.create_tmpfile(self, mode.with_type(FileMode::IFREG), flags)?
        } else {
            let mode = name.entry.node.info().mode;

            // These checks are not needed in the `O_TMPFILE` case because `mode` refers to the
            // file we are opening. With `O_TMPFILE`, that file is the regular file we just
            // created rather than the node we found by resolving the path.
            //
            // For example, we do not need to produce `ENOTDIR` when `must_be_directory` is set
            // because `must_be_directory` refers to the node we found by resolving the path.
            // If that node was not a directory, then `create_tmpfile` will produce an error.
            //
            // Similarly, we never need to call `truncate` because `O_TMPFILE` is newly created
            // and therefor already an empty file.

            if !opath && nofollow && mode.is_lnk() {
                return error!(ELOOP);
            }

            if mode.is_dir() {
                if flags.can_write()
                    || flags.contains(OpenFlags::CREAT)
                    || flags.contains(OpenFlags::TRUNC)
                {
                    return error!(EISDIR);
                }
                if flags.contains(OpenFlags::DIRECT) {
                    return error!(EINVAL);
                }
            } else if context.directory_mode == DirectoryMode::MustBeDirectory {
                return error!(ENOTDIR);
            }

            if flags.contains(OpenFlags::TRUNC) && mode.is_reg() && !created {
                // You might think we should check file.can_write() at this
                // point, which is what the docs suggest, but apparently we
                // are supposed to truncate the file if this task can write
                // to the underlying node, even if we are opening the file
                // as read-only. See OpenTest.CanTruncateReadOnly.
                name.truncate(self, 0)?;
            }

            name
        };

        // If the node has been created, the open operation should not verify access right:
        // From <https://man7.org/linux/man-pages/man2/open.2.html>
        //
        // > Note that mode applies only to future accesses of the newly created file; the
        // > open() call that creates a read-only file may well return a  read/write  file
        // > descriptor.
        let file = if created {
            name.open(self, OpenAccessCheck::skip(flags))?
        } else {
            name.open(self, OpenAccessCheck::new(flags, open_check.access_check()))?
        };

        // If the new `FileHandle` represents an open file (rather than a handle to a location in
        // the virtual file system, as created with `O_PATH`), then LSM permission checks may be
        // required.
        if !opath {
            security::file_open(self, &file)?;
        }

        Ok(file)
    }

    /// A wrapper for FsContext::lookup_parent_at that resolves the given
    /// dir_fd to a NamespaceNode.
    ///
    /// Absolute paths are resolve relative to the root of the FsContext for
    /// this task. Relative paths are resolve relative to dir_fd. To resolve
    /// relative to the current working directory, pass FdNumber::AT_FDCWD for
    /// dir_fd.
    pub fn lookup_parent_at<'a>(
        &self,
        context: &mut LookupContext,
        dir_fd: FdNumber,
        path: &'a FsStr,
    ) -> Result<(NamespaceNode, &'a FsStr), Errno> {
        let (dir, path) = self.resolve_dir_fd(dir_fd, path, ResolveFlags::empty())?;
        self.lookup_parent(context, &dir, path)
    }

    /// Lookup the parent of a namespace node.
    ///
    /// Consider using Task::open_file_at or Task::lookup_parent_at rather than
    /// calling this function directly.
    ///
    /// This function resolves all but the last component of the given path.
    /// The function returns the parent directory of the last component as well
    /// as the last component.
    ///
    /// If path is empty, this function returns dir and an empty path.
    /// Similarly, if path ends with "." or "..", these components will be
    /// returned along with the parent.
    ///
    /// The returned parent might not be a directory.
    pub fn lookup_parent<'a>(
        &self,
        context: &mut LookupContext,
        dir: &NamespaceNode,
        path: &'a FsStr,
    ) -> Result<(NamespaceNode, &'a FsStr), Errno> {
        context.update_for_path(path);

        let components = split_path(path);
        if components.is_empty() {
            return Ok((dir.clone(), Default::default()));
        }
        let result = dir.lookup_children(self, context, &components[0..components.len() - 1])?;
        Ok((result, components.last().unwrap()))
    }

    /// Lookup a namespace node.
    ///
    /// Consider using Task::open_file_at or Task::lookup_parent_at rather than
    /// calling this function directly.
    ///
    /// This function resolves the component of the given path.
    pub fn lookup_path(
        &self,
        context: &mut LookupContext,
        dir: NamespaceNode,
        path: &FsStr,
    ) -> Result<NamespaceNode, Errno> {
        let components = split_path(path);
        dir.lookup_children(self, context, &components)
    }

    /// Lookup a namespace node starting at the root directory.
    ///
    /// Resolves symlinks.
    pub fn lookup_path_from_root(&self, path: &FsStr) -> Result<NamespaceNode, Errno> {
        let mut context = LookupContext::default();
        self.lookup_path(&mut context, self.fs().root(), path)
    }

    pub fn exec(
        &mut self,
        executable: Arc<FileMapping>,
        path: CString,
        argv: Vec<CString>,
        environ: Vec<CString>,
    ) -> Result<(), Errno> {
        // Serialize against ptrace_attach by holding the credentials write lock.
        let writable_creds = self.write_creds();

        // Evaluate LSM access checks and domain transitions (`bprm_creds_for_exec`) against the
        // initial executable before resolving `#!` scripts, so that script execution transitions
        // to the domain determined by the script file rather than its interpreter. Once `#!`
        // scripts are resolved to the target ELF binary, apply SUID/SGID and file capabilities
        // (`bprm_creds_from_file`) and validate `PT_INTERP` before committing the exec.
        let mut resolved_program = ResolvedProgram::new(self, executable, path, argv, environ);
        security::bprm_creds_for_exec(self, &mut resolved_program)?;
        resolve_executable(self, &mut resolved_program)?;
        security::bprm_creds_from_file(self, &mut resolved_program)?;
        resolve_elf_interpreter(self, &mut resolved_program)?;

        if self.thread_group().read().tasks_count() > 1 {
            track_stub!(TODO("https://fxbug.dev/297434895"), "exec on multithread process");
            return error!(EINVAL);
        }

        // Commit the exec. Failures after this point are unrecoverable.
        if let Err(err) = self.finish_exec(resolved_program, writable_creds) {
            log_warn!("unrecoverable error in exec: {err:?}");

            send_standard_signal(self, SignalInfo::forced(SIGSEGV));
            return Err(err);
        }

        self.ptrace_event(PtraceOptions::TRACEEXEC, self.task.tid.id as u64);
        self.signal_vfork();
        self.task.thread_group.sync_syscall_log_level();

        Ok(())
    }

    /// After the memory is unmapped, any failure in exec is unrecoverable and results in the
    /// process crashing. This function is for that second half; any error returned from this
    /// function will be considered unrecoverable.
    fn finish_exec(
        &mut self,
        resolved_program: ResolvedProgram,
        writable_creds: CurrentTaskCredentialsWriteGuard,
    ) -> Result<(), Errno> {
        let elf_arch_width = resolved_program.arch_width().ok_or_else(|| errno!(EINVAL))?;

        // Now that the exec will definitely finish (or crash), notify owners of
        // locked futexes for the current process, which will be impossible to
        // update after process image is replaced.  See get_robust_list(2).
        self.notify_robust_list();

        // Tear down the old address space and create a new one for the resolved ELF.
        let mm = {
            let new_mm = MemoryManager::exec(
                self.thread_group().root_vmar.unowned(),
                self.mm().ok(),
                resolved_program.file.name().to_passive(),
                elf_arch_width,
            )?;
            self.running_state().mm.update(Some(new_mm.clone()));
            new_mm
        };
        // From <https://man7.org/linux/man-pages/man2/execve.2.html>:
        //
        //   All threads other than the calling thread are destroyed during an
        //   execve(). Mutual exclusion locks, condition variables, and other
        //   pthreads objects are not preserved.
        //
        // TODO(https://fxbug.dev/42082680): Implement thread destruction.

        // From <https://man7.org/linux/man-pages/man2/execve.2.html>:
        //
        //   POSIX timers (timer_create(2)) are not preserved.
        //
        // TODO: Implement this.

        // TODO: Ensure that the filesystem context is un-shared, undoing the effect of CLONE_FS.

        // From <https://man7.org/linux/man-pages/man2/execve.2.html>:
        //
        //   If the calling process was sharing its file descriptor table (via
        //   the use of CLONE_FILES with clone(2)), then this sharing is undone.
        self.running_state().unshare_files(self);
        self.files().exec();

        {
            let mut state = self.write();

            // From <https://man7.org/linux/man-pages/man2/execve.2.html>:
            //
            //   The process's "dumpable" attribute is set to the value 1,
            //   unless a set-user-ID program, a set-group-ID program, or a
            //   program with capabilities is being executed, in which case the
            //   dumpable flag may instead be reset to the value in
            //   /proc/sys/fs/suid_dumpable, in the circumstances described
            //   under PR_SET_DUMPABLE in prctl(2).
            let dumpable =
                if resolved_program.secure_exec { DumpPolicy::Disable } else { DumpPolicy::User };
            *mm.dumpable.lock() = dumpable;

            state.set_sigaltstack(None);
            state.robust_list_head = RobustListHeadPtr::null(self);
            // From <https://man7.org/linux/man-pages/man2/execve.2.html>:
            //
            //   If a set-user-ID or set-group-ID
            //   program is being executed, then the parent death signal set by
            //   prctl(2) PR_SET_PDEATHSIG flag is cleared.
            //
            // TODO(https://fxbug.dev/356684424): Implement the behavior above once we support
            // the PR_SET_PDEATHSIG flag.
        }

        security::bprm_committing_creds(self, &resolved_program)?;

        let new_creds = Arc::new(resolved_program.creds.clone());
        writable_creds.update(self, new_creds);

        self.thread_group().signal_actions.reset_for_exec();
        security::bprm_committed_creds(self)?;

        let command_name = TaskCommand::from_path_bytes(resolved_program.path().to_bytes());
        let start_info = load_executable(self, resolved_program)?;

        let regs: zx_restricted_state_t = start_info.into();
        self.thread_state.registers.load(regs);
        self.thread_state.extended_pstate.reset();

        // The exit signal (and that of the children) is reset to SIGCHLD.
        {
            let mut thread_group_state = self.thread_group().write();
            thread_group_state.exit_signal = Some(SIGCHLD);
            for (_, weak_child) in &mut thread_group_state.children {
                if let Some(child) = weak_child.upgrade() {
                    // This allow_subclass is safe because locking parent then child strictly
                    // follows the top-down traversal of the thread group tree, which cannot form
                    // cycles.
                    let _token = starnix_sync::allow_subclass();
                    let mut child_state = child.write();
                    child_state.exit_signal = Some(SIGCHLD);
                }
            }
        }

        self.thread_group().write().did_exec = true;

        self.set_command_name(command_name);

        Ok(())
    }

    pub fn set_command_name(&self, new_name: TaskCommand) {
        // set_command_name needs to run before leader_command() in cases where self is the leader.
        self.task.set_command_name(new_name.clone());
        let leader_command = self.thread_group().read().leader_command();
        starnix_logging::set_current_task_info(new_name, leader_command, self.pid.id, self.tid.id);
    }

    pub fn add_seccomp_filter(
        &mut self,
        code: Vec<sock_filter>,
        flags: u32,
    ) -> Result<SyscallResult, Errno> {
        let mut notifier = None;
        if flags & SECCOMP_FILTER_FLAG_NEW_LISTENER != 0 {
            notifier = Some(SeccompFilterContainer::create_notifier());
        }

        let new_filter = Arc::new(SeccompFilter::from_cbpf(
            &code,
            self.thread_group().next_seccomp_filter_id.add(1),
            flags & SECCOMP_FILTER_FLAG_LOG != 0,
            notifier.clone(),
        )?);

        let mut maybe_fd: Option<FdNumber> = None;
        if let Some(notifier) = notifier {
            maybe_fd = Some(SeccompFilterContainer::register_listener(self, notifier)?);
        }

        // We take the process lock here because we can't change any of the threads
        // while doing a tsync.  So, you hold the process lock while making any changes.
        let state = self.thread_group().write();

        if flags & SECCOMP_FILTER_FLAG_TSYNC != 0 {
            // TSYNC synchronizes all filters for all threads in the current process to
            // the current thread's

            // We collect the filters for the current task upfront to save us acquiring
            // the task's lock a lot of times below.
            let mut filters: SeccompFilterContainer = self.read().seccomp_filters.clone();

            // For TSYNC to work, all of the other thread filters in this process have to
            // be a prefix of this thread's filters, and none of them can be in
            // strict mode.
            let tasks = state.tasks();
            for task in &tasks {
                if task.tid.id == self.tid.id {
                    continue;
                }
                let other_task_state = task.read();

                // Target threads cannot be in SECCOMP_MODE_STRICT
                if task.seccomp_filter_state.get() == SeccompStateValue::Strict {
                    return Self::seccomp_tsync_error(task.tid.id, flags);
                }

                // Target threads' filters must be a subsequence of this thread's
                if !other_task_state.seccomp_filters.can_sync_to(&filters) {
                    return Self::seccomp_tsync_error(task.tid.id, flags);
                }
            }

            // Now that we're sure we're allowed to do so, add the filter to all threads.
            filters.add_filter(new_filter, code.len() as u16)?;

            for task in &tasks {
                let mut other_task_state = task.write();

                other_task_state.enable_no_new_privs();
                other_task_state.seccomp_filters = filters.clone();
                task.set_seccomp_state(SeccompStateValue::UserDefined)?;
            }
        } else {
            let mut task_state = self.task.write();

            task_state.seccomp_filters.add_filter(new_filter, code.len() as u16)?;
            self.set_seccomp_state(SeccompStateValue::UserDefined)?;
        }

        if let Some(fd) = maybe_fd { Ok(fd.into()) } else { Ok(().into()) }
    }

    pub fn run_seccomp_filters(
        &mut self,
        syscall: &Syscall,
    ) -> Option<Result<SyscallResult, Errno>> {
        // Implementation of SECCOMP_FILTER_STRICT, which has slightly different semantics
        // from user-defined seccomp filters.
        if self.seccomp_filter_state.get() == SeccompStateValue::Strict {
            return SeccompState::do_strict(self, syscall);
        }

        // Run user-defined seccomp filters
        let result = self.task.read().seccomp_filters.run_all(self, syscall);

        SeccompState::do_user_defined(result, self, syscall)
    }

    fn seccomp_tsync_error(id: i32, flags: u32) -> Result<SyscallResult, Errno> {
        // By default, TSYNC indicates failure state by returning the first thread
        // id not to be able to sync, rather than by returning -1 and setting
        // errno.  However, if TSYNC_ESRCH is set, it returns ESRCH.  This
        // prevents conflicts with fact that SECCOMP_FILTER_FLAG_NEW_LISTENER
        // makes seccomp return an fd.
        if flags & SECCOMP_FILTER_FLAG_TSYNC_ESRCH != 0 { error!(ESRCH) } else { Ok(id.into()) }
    }

    // Notify all futexes in robust list.  The robust list is in user space, so we
    // are very careful about walking it, and there are a lot of quiet returns if
    // we fail to walk it.
    // TODO(https://fxbug.dev/42079081): This only sets the FUTEX_OWNER_DIED bit; it does
    // not wake up a waiter.
    pub fn notify_robust_list(&self) {
        let task_state = self.write();
        let robust_list_addr = task_state.robust_list_head.addr();
        if robust_list_addr == UserAddress::NULL {
            // No one has called set_robust_list.
            return;
        }
        let robust_list_res = self.read_multi_arch_object(task_state.robust_list_head);

        let head = if let Ok(head) = robust_list_res {
            head
        } else {
            return;
        };

        let offset = head.futex_offset;

        let mut entries_count = 0;
        let mut curr_ptr = head.list.next;
        while curr_ptr.addr() != robust_list_addr.into() && entries_count < ROBUST_LIST_LIMIT {
            let curr_ref = self.read_multi_arch_object(curr_ptr);

            let curr = if let Ok(curr) = curr_ref {
                curr
            } else {
                return;
            };

            let Some(futex_base) = curr_ptr.addr().checked_add_signed(offset) else {
                return;
            };

            let futex_addr = match FutexAddress::try_from(futex_base) {
                Ok(addr) => addr,
                Err(_) => {
                    return;
                }
            };

            let Ok(mm) = self.mm() else {
                log_error!("Asked to notify robust list futexes in system task.");
                return;
            };
            let futex = if let Ok(futex) = mm.atomic_load_u32_relaxed(futex_addr) {
                futex
            } else {
                return;
            };

            if (futex & FUTEX_TID_MASK) as i32 == self.tid.id {
                let owner_died = FUTEX_OWNER_DIED | futex;
                if mm.atomic_store_u32_relaxed(futex_addr, owner_died).is_err() {
                    return;
                }
            }
            curr_ptr = curr.next;
            entries_count += 1;
        }
    }

    pub(crate) fn handle_page_fault(
        &self,
        decoded: PageFaultExceptionReport,
        status: zx::Status,
    ) -> ExceptionResult {
        if let Ok(mm) = self.mm() {
            mm.handle_page_fault(decoded, status)
        } else {
            panic!(
                "system task is handling a major page fault status={:?}, report={:?}",
                status, decoded
            );
        }
    }

    /// Processes a Zircon exception associated with this task.
    pub fn process_exception(&self, report: &zx::ExceptionReport) -> ExceptionResult {
        if let Some(result) = handle_hardware_exception(self, report) {
            return result;
        }

        match report.ty {
            zx::ExceptionType::General => {
                log_error!("Unrecognized general exception: {:?}", report);
                ExceptionResult::Signal(SignalInfo::kernel(SIGILL))
            }
            zx::ExceptionType::ProcessNameChanged => {
                log_error!("Received unexpected process name changed exception");
                ExceptionResult::Handled
            }
            zx::ExceptionType::ProcessStarting
            | zx::ExceptionType::ThreadStarting
            | zx::ExceptionType::ThreadExiting => {
                log_error!("Received unexpected task lifecycle exception");
                ExceptionResult::Signal(SignalInfo::kernel(SIGSYS))
            }
            zx::ExceptionType::PolicyError(policy_code) => {
                log_error!(policy_code:?; "Received Zircon policy error exception");
                ExceptionResult::Signal(SignalInfo::kernel(SIGSYS))
            }
            zx::ExceptionType::UnknownUserGenerated { code, data } => {
                log_error!(code:?, data:?; "Received unexpected unknown user generated exception");
                ExceptionResult::Signal(SignalInfo::kernel(SIGSYS))
            }
            zx::ExceptionType::Unknown { ty, code, data } => {
                log_error!(ty:?, code:?, data:?; "Received unexpected exception");
                ExceptionResult::Signal(SignalInfo::kernel(SIGSYS))
            }
            _ => {
                log_error!("Received unknown zircon exception: {:?}", report.ty);
                ExceptionResult::Signal(SignalInfo::kernel(SIGSYS))
            }
        }
    }

    /// Clone this task.
    ///
    /// Creates a new task object that shares some state with this task
    /// according to the given flags.
    ///
    /// Used by the clone() syscall to create both processes and threads.
    ///
    /// The exit signal is broken out from the flags parameter like clone3() rather than being
    /// bitwise-ORed like clone().
    pub fn clone_task(
        &self,
        flags: u64,
        child_exit_signal: Option<Signal>,
        user_parent_tid: UserRef<pid_t>,
        user_child_tid: UserRef<pid_t>,
        user_pidfd: UserRef<FdNumber>,
    ) -> Result<TaskBuilder, Errno> {
        const IMPLEMENTED_FLAGS: u64 = ((CLONE_VM
            | CLONE_FS
            | CLONE_FILES
            | CLONE_SIGHAND
            | CLONE_THREAD
            | CLONE_SYSVSEM
            | CLONE_SETTLS
            | CLONE_PARENT
            | CLONE_PARENT_SETTID
            | CLONE_PIDFD
            | CLONE_CHILD_CLEARTID
            | CLONE_CHILD_SETTID
            | CLONE_VFORK
            | CLONE_NEWUTS
            | CLONE_PTRACE) as u64)
            | CLONE_CLEAR_SIGHAND;

        // A mask with all valid flags set, because we want to return a different error code for an
        // invalid flag vs an unimplemented flag. Subtracting 1 from the largest valid flag gives a
        // mask with all flags below it set. Shift up by one to make sure the largest flag is also
        // set.
        const VALID_FLAGS: u64 = (CLONE_INTO_CGROUP << 1) - 1;

        // CLONE_SETTLS is implemented by sys_clone.

        let clone_files = flags & (CLONE_FILES as u64) != 0;
        let clone_fs = flags & (CLONE_FS as u64) != 0;
        let clone_parent = flags & (CLONE_PARENT as u64) != 0;
        let clone_parent_settid = flags & (CLONE_PARENT_SETTID as u64) != 0;
        let clone_pidfd = flags & (CLONE_PIDFD as u64) != 0;
        let clone_child_cleartid = flags & (CLONE_CHILD_CLEARTID as u64) != 0;
        let clone_child_settid = flags & (CLONE_CHILD_SETTID as u64) != 0;
        let clone_sysvsem = flags & (CLONE_SYSVSEM as u64) != 0;
        let clone_ptrace = flags & (CLONE_PTRACE as u64) != 0;
        let clone_thread = flags & (CLONE_THREAD as u64) != 0;
        let clone_vm = flags & (CLONE_VM as u64) != 0;
        let clone_sighand = flags & (CLONE_SIGHAND as u64) != 0;
        let clone_vfork = flags & (CLONE_VFORK as u64) != 0;
        let clone_newuts = flags & (CLONE_NEWUTS as u64) != 0;
        let clone_into_cgroup = flags & CLONE_INTO_CGROUP != 0;
        let clone_clear_sighand = flags & (CLONE_CLEAR_SIGHAND as u64) != 0;

        if clone_ptrace {
            track_stub!(TODO("https://fxbug.dev/322874630"), "CLONE_PTRACE");
        }

        if clone_sysvsem {
            track_stub!(TODO("https://fxbug.dev/322875185"), "CLONE_SYSVSEM");
        }

        if clone_into_cgroup {
            track_stub!(TODO("https://fxbug.dev/403612570"), "CLONE_INTO_CGROUP");
        }

        if clone_sighand && !clone_vm {
            return error!(EINVAL);
        }
        if clone_clear_sighand && clone_sighand {
            return error!(EINVAL);
        }
        if clone_thread && !clone_sighand {
            return error!(EINVAL);
        }

        if clone_pidfd && clone_thread {
            return error!(EINVAL);
        }
        if clone_pidfd && clone_parent_settid && user_parent_tid.addr() == user_pidfd.addr() {
            // `clone()` uses the same out-argument for these, so error out if they have the same
            // user address.
            return error!(EINVAL);
        }

        if flags & !VALID_FLAGS != 0 {
            return error!(EINVAL);
        }

        if clone_vm && !clone_thread {
            // TODO(https://fxbug.dev/42066087) Implement CLONE_VM for child processes (not just child
            // threads). Currently this executes CLONE_VM (explicitly passed to clone() or as
            // used by vfork()) as a fork (the VM in the child is copy-on-write) which is almost
            // always OK.
            //
            // CLONE_VM is primarily as an optimization to avoid making a copy-on-write version of a
            // process' VM that will be immediately replaced with a call to exec(). The main users
            // (libc and language runtimes) don't actually rely on the memory being shared between
            // the two processes. And the vfork() man page explicitly allows vfork() to be
            // implemented as fork() which is what we do here.
            if !clone_vfork {
                track_stub!(
                    TODO("https://fxbug.dev/322875227"),
                    "CLONE_VM without CLONE_THREAD or CLONE_VFORK"
                );
            }
        } else if clone_thread && !clone_vm {
            track_stub!(TODO("https://fxbug.dev/322875167"), "CLONE_THREAD without CLONE_VM");
            return error!(ENOSYS);
        }

        if flags & !IMPLEMENTED_FLAGS != 0 {
            track_stub!(
                TODO("https://fxbug.dev/322875130"),
                "clone unknown flags",
                flags & !IMPLEMENTED_FLAGS
            );
            return error!(ENOSYS);
        }

        let fs = if clone_fs { self.fs() } else { self.fs().fork() };
        let files = if clone_files {
            self.running_state().share_files()
        } else {
            self.running_state().fork_files()
        }
        .expect("Task must have FdTable");

        let kernel = self.kernel();

        let mut pids = kernel.pids.lock();

        // Lock the cgroup process hierarchy so that the parent process cannot move to a different
        // cgroup while a new task or thread_group is created. This may be unnecessary if
        // CLONE_INTO_CGROUP is implemented and passed in.
        let mut cgroup2_pid_table = kernel.cgroups.lock_cgroup2_pid_table();
        // Create a `KernelSignal::Freeze` to put onto the new task, if the cgroup is frozen.
        let child_kernel_signals = cgroup2_pid_table
            .maybe_create_freeze_signal(&self.pid)
            .into_iter()
            .collect::<VecDeque<_>>();

        let pid;
        let command;
        let creds;
        let scheduler_state;
        let no_new_privs;
        let seccomp_filters;
        let robust_list_head = RobustListHeadPtr::null(self);
        let child_signal_mask;
        let timerslack_ns;
        let uts_ns;

        let TaskInfo { thread_group, memory_manager } = {
            // These variables hold the original parent in case we need to switch the parent of the
            // new task because of CLONE_PARENT.
            let weak_original_parent;
            let original_parent;

            // Make sure to drop these locks ASAP to avoid inversion
            let thread_group_state = {
                let thread_group_state = self.thread_group().write();
                if clone_parent {
                    // With the CLONE_PARENT flag, the parent of the new task is our parent
                    // instead of ourselves.
                    weak_original_parent =
                        thread_group_state.parent.clone().ok_or_else(|| errno!(EINVAL))?;
                    std::mem::drop(thread_group_state);
                    original_parent = weak_original_parent.upgrade();
                    original_parent.write()
                } else {
                    thread_group_state
                }
            };

            let state = self.read();

            no_new_privs = state.no_new_privs();
            seccomp_filters = state.seccomp_filters.clone();
            child_signal_mask = state.signal_mask();

            pid = pids.allocate_pid()?;
            command = self.command();
            creds = self.current_creds().clone();
            scheduler_state = state.scheduler_state.fork();
            timerslack_ns = state.timerslack_ns;

            uts_ns = if clone_newuts {
                security::check_task_capable(self, CAP_SYS_ADMIN)?;
                state.uts_ns.read().fork()
            } else {
                state.uts_ns.clone()
            };

            if clone_thread {
                TaskInfo {
                    thread_group: self.thread_group().clone(),
                    memory_manager: self.mm().ok(),
                }
            } else {
                // Drop the lock on this task before entering `create_zircon_process`, because it will
                // take a lock on the new thread group, and locks on thread groups have a higher
                // priority than locks on the task in the thread group.
                std::mem::drop(state);
                let signal_actions = if clone_sighand {
                    self.thread_group().signal_actions.clone()
                } else if clone_clear_sighand {
                    let actions = self.thread_group().signal_actions.fork();
                    actions.reset_for_exec();
                    actions
                } else {
                    self.thread_group().signal_actions.fork()
                };
                let process_group = thread_group_state.process_group.clone();

                let task_info = {
                    fuchsia_trace::duration!(CATEGORY_STARNIX, "create_zircon_process");
                    create_zircon_process(
                        kernel,
                        Some(thread_group_state),
                        pid.clone(),
                        child_exit_signal,
                        process_group,
                        signal_actions,
                        command.clone(),
                    )?
                };

                cgroup2_pid_table.inherit_cgroup(self.thread_group(), &task_info.thread_group);

                task_info
            }
        };

        // Drop the lock on the cgroup pid_table before creating the TaskBuilder.
        // If the TaskBuilder creation fails, the TaskBuilder is dropped, which calls
        // ThreadGroup::remove. ThreadGroup::remove takes the cgroup pid_table lock, causing
        // a cyclic lock dependency.
        std::mem::drop(cgroup2_pid_table);

        // Only create the vfork event when the caller requested CLONE_VFORK.
        let vfork_event = if clone_vfork { Some(Arc::new(zx::Event::create())) } else { None };

        // Clone running state in a nested scope to ensure that the RCU read scope is not held
        // across the release_on_error block.
        let abstract_socket_namespace;
        let abstract_vsock_namespace;
        {
            let running_state = self.running_state();
            abstract_socket_namespace = running_state.abstract_socket_namespace.clone();
            abstract_vsock_namespace = running_state.abstract_vsock_namespace.clone();
        }

        let mut child = TaskBuilder::new(Task::new(
            pid,
            command,
            thread_group,
            files,
            memory_manager,
            fs,
            creds,
            abstract_socket_namespace,
            abstract_vsock_namespace,
            child_signal_mask,
            child_kernel_signals,
            vfork_event,
            scheduler_state,
            uts_ns,
            no_new_privs,
            SeccompState::from(&self.seccomp_filter_state),
            seccomp_filters,
            robust_list_head,
            timerslack_ns,
        ));
        let parent_cpuset_path = self.read().cpuset_path.clone();
        child.task.write().cpuset_path = parent_cpuset_path;

        release_on_error!(child, {
            // Drop the pids lock as soon as possible after creating the child. Destroying the child
            // and removing it from the pids table itself requires the pids lock, so an early exit
            // would cause a self deadlock.
            pids.add_task(Arc::clone(&child.task));
            std::mem::drop(pids);

            // Child lock must be taken before this lock. Drop the lock on the task, take a writable
            // lock on the child and take the current state back.

            #[cfg(any(test, debug_assertions))]
            {
                // Take the lock on the thread group and its child in the correct order to ensure
                // any wrong ordering will trigger the tracing-mutex at the right call site.
                if !clone_thread {
                    let _l1 = self.thread_group().read();
                    // This allow_subclass is safe because locking parent then child strictly
                    // follows the top-down traversal of the thread group tree, which cannot form
                    // cycles.
                    let _token = starnix_sync::allow_subclass();
                    let _l2 = child.thread_group().read();
                }
            }

            if clone_thread {
                self.thread_group().add(Arc::clone(&child.task))?;
            } else {
                child.thread_group().add(Arc::clone(&child.task))?;

                // These manipulations of the signal handling state appear to be related to
                // CLONE_SIGHAND and CLONE_VM rather than CLONE_THREAD. However, we do not support
                // all the combinations of these flags, which means doing these operations here
                // might actually be correct. However, if you find a test that fails because of the
                // placement of this logic here, we might need to move it.
                let (sigaltstack, signal_mask) = {
                    let state = self.read();
                    (state.sigaltstack(), state.signal_mask())
                };
                let mut child_state = child.write();
                child_state.set_sigaltstack(sigaltstack);
                child_state.set_signal_mask(signal_mask);
            }

            if !clone_vm {
                // We do not support running threads in the same process with different
                // MemoryManagers.
                assert!(!clone_thread);
                let child_mm = MemoryManager::snapshot_of(
                    &self.mm()?,
                    child.thread_group.root_vmar.unowned(),
                    self.thread_state.arch_width(),
                )?;
                child.running_state()?.mm.update(Some(child_mm));
            }

            if clone_parent_settid {
                self.write_object(user_parent_tid, &child.tid.id)?;
            }

            if clone_child_cleartid {
                child.write().clear_child_tid = user_child_tid;
            }

            if clone_child_settid {
                child.write_object(user_child_tid, &child.tid.id)?;
            }

            if clone_pidfd {
                let file = new_pidfd(self, child.pid.clone(), OpenFlags::empty())?;
                let pidfd = self.add_file(file, FdFlags::CLOEXEC)?;
                self.write_object(user_pidfd, &pidfd)?;
            }

            // TODO(https://fxbug.dev/42066087): We do not support running different processes with
            // the same MemoryManager. Instead, we implement a rough approximation of that behavior
            // by making a copy-on-write clone of the memory from the original process.
            if clone_vm && !clone_thread {
                let child_mm = MemoryManager::snapshot_of(
                    &self.mm()?,
                    child.thread_group.root_vmar.unowned(),
                    self.thread_state.arch_width(),
                )?;
                child.running_state()?.mm.update(Some(child_mm));
            }

            child.thread_state = self.thread_state.snapshot::<HeapRegs>();
            Ok(())
        });

        // Take the lock on thread group and task in the correct order to ensure any wrong ordering
        // will trigger the tracing-mutex at the right call site.
        #[cfg(any(test, debug_assertions))]
        {
            let _l1 = child.thread_group().read();
            let _l2 = child.read();
        }

        Ok(child)
    }

    /// Sets the stop state (per set_stopped), and also notifies all listeners,
    /// including the parent process and the tracer if appropriate.
    pub fn set_stopped_and_notify(&self, stopped: StopState, siginfo: Option<SignalInfo>) {
        let maybe_signal_info = {
            let mut state = self.write();
            state.copy_state_from(self);
            state.set_stopped(stopped, siginfo, Some(self), None);
            state.prepare_signal_info(stopped)
        };

        if let Some((tracer, signal_info)) = maybe_signal_info {
            if let Some(tracer) = tracer.upgrade() {
                tracer.write().send_signal(signal_info);
            }
        }

        if !stopped.is_in_progress() {
            let parent = self.thread_group().read().parent.clone();
            if let Some(parent) = parent {
                parent
                    .upgrade()
                    .write()
                    .lifecycle_waiters
                    .notify_value(ThreadGroupLifecycleWaitValue::ChildStatus);
            }
        }
    }

    /// Finalizes the stop state of the task, and if the task should be stopped,
    /// blocks the execution of `current_task` as long as the task is stopped and
    /// not terminated.
    ///
    /// Returns true if the task was stopped and blocked (and has now woken up),
    /// or false if it was not stopped and returned immediately.
    pub fn block_if_stopped(&mut self) -> bool {
        if self.finalize_stop_state() {
            self.block_while_stopped();
            true
        } else {
            false
        }
    }

    /// If the task is stopping, set it as stopped. return whether the caller
    /// should stop.  The task might also be waking up.
    fn finalize_stop_state(&mut self) -> bool {
        let stopped = self.load_stopped();

        if !stopped.is_stopping_or_stopped() {
            // If we are waking up, potentially write back state a tracer may have modified.
            let captured_state = self.write().take_captured_state();
            if let Some(captured) = captured_state {
                if captured.dirty {
                    self.thread_state.replace_registers(&captured.thread_state);
                }
            }
        }

        // Stopping because the thread group is stopping.
        // Try to flip to GroupStopped - will fail if we shouldn't.
        if self.thread_group().set_stopped(StopState::GroupStopped, None, true)
            == StopState::GroupStopped
        {
            let signal = self.thread_group().read().last_signal.clone();
            // stopping because the thread group has stopped
            let event = Some(PtraceEventData::new_from_event(PtraceEvent::Stop, 0));
            self.write().set_stopped(StopState::GroupStopped, signal, Some(self), event);
            return true;
        }

        // Stopping because the task is stopping
        if stopped.is_stopping_or_stopped() {
            if let Ok(stopped) = stopped.finalize() {
                self.set_stopped_and_notify(stopped, None);
            }
            return true;
        }

        false
    }

    /// Block the execution of `current_task` as long as the task is stopped and
    /// not terminated.
    fn block_while_stopped(&mut self) {
        let waiter = Waiter::with_options(WaiterOptions::IGNORE_SIGNALS);
        loop {
            // If we've exited, unstop the threads and return without notifying
            // waiters.
            if self.is_exitted() {
                self.thread_group().set_stopped(StopState::ForceAwake, None, false);
                self.write().set_stopped(StopState::ForceAwake, None, Some(self), None);
                return;
            }

            if self.wake_or_wait_until_unstopped_async(&waiter) {
                return;
            }

            // Do the wait. Result is not needed, as this is not in a syscall.
            let _: Result<(), Errno> = waiter.wait(self);

            // Maybe go from stopping to stopped, if we are currently stopping
            // again.
            self.finalize_stop_state();
        }
    }

    /// For traced tasks, this will return the data neceessary for a cloned task
    /// to attach to the same tracer.
    pub fn get_ptrace_core_state_for_clone(
        &mut self,
        clone_args: &clone_args,
    ) -> (PtraceOptions, Option<PtraceCoreState>) {
        let state = self.write();
        if let Some(ptrace) = &state.ptrace {
            ptrace.get_core_state_for_clone(clone_args)
        } else {
            (PtraceOptions::empty(), None)
        }
    }

    /// If currently being ptraced with the given option, emit the appropriate
    /// event.  PTRACE_EVENTMSG will return the given message.  Also emits the
    /// appropriate event for execve in the absence of TRACEEXEC.
    ///
    /// Note that the Linux kernel has a documented bug where, if TRACEEXIT is
    /// enabled, SIGKILL will trigger an event.  We do not exhibit this
    /// behavior.
    pub fn ptrace_event(&mut self, trace_kind: PtraceOptions, msg: u64) {
        if !trace_kind.is_empty() {
            {
                let mut state = self.write();
                if let Some(ptrace) = &mut state.ptrace {
                    if !ptrace.has_option(trace_kind) {
                        // If this would be a TRACEEXEC, but TRACEEXEC is not
                        // turned on, then send a SIGTRAP.
                        if trace_kind == PtraceOptions::TRACEEXEC && !ptrace.is_seized() {
                            // Send a SIGTRAP so that the parent can gain control.
                            send_signal_first(self, state, SignalInfo::kernel(SIGTRAP));
                        }

                        return;
                    }
                    let ptrace_event = PtraceEvent::from_option(&trace_kind) as u32;
                    let siginfo = SignalInfo::with_detail(
                        SIGTRAP,
                        ((ptrace_event << 8) | SIGTRAP.number()) as i32,
                        SignalDetail::None,
                    );
                    state.set_stopped(
                        StopState::PtraceEventStopping,
                        Some(siginfo),
                        None,
                        Some(PtraceEventData::new(trace_kind, msg)),
                    );
                } else {
                    return;
                }
            }
            self.block_if_stopped();
        }
    }

    /// Causes the current thread's thread group to exit, notifying any ptracer
    /// of this task first.
    pub fn kill_thread_group(&mut self, exit_status: ExitStatus) {
        self.ptrace_event(PtraceOptions::TRACEEXIT, exit_status.signal_info_status() as u64);
        self.thread_group().kill(exit_status, None);
    }

    /// The flags indicates only the flags as in clone3(), and does not use the low 8 bits for the
    /// exit signal as in clone().
    pub fn clone_task_builder_for_test(
        &self,
        flags: u64,
        exit_signal: Option<Signal>,
    ) -> TaskBuilder {
        let result = self
            .clone_task(
                flags,
                exit_signal,
                UserRef::default(),
                UserRef::default(),
                UserRef::default(),
            )
            .expect("failed to create task in test");
        result.task.write().set_spawned();
        result
    }

    /// The flags indicates only the flags as in clone3(), and does not use the low 8 bits for the
    /// exit signal as in clone().
    pub fn clone_task_for_test(
        &self,
        flags: u64,
        exit_signal: Option<Signal>,
    ) -> crate::testing::AutoReleasableTask {
        self.clone_task_builder_for_test(flags, exit_signal).into()
    }

    // See "Ptrace access mode checking" in https://man7.org/linux/man-pages/man2/ptrace.2.html
    pub fn check_ptrace_access_mode(
        &self,
        mode: PtraceAccessMode,
        target: &Task,
    ) -> Result<(), Errno> {
        // (1)  If the calling thread and the target thread are in the same
        //      thread group, access is always allowed.
        if self.pid == target.pid {
            return Ok(());
        }

        // (2)  If the access mode specifies PTRACE_MODE_FSCREDS, then, for
        //      the check in the next step, employ the caller's filesystem
        //      UID and GID.  (As noted in credentials(7), the filesystem
        //      UID and GID almost always have the same values as the
        //      corresponding effective IDs.)
        //
        //      Otherwise, the access mode specifies PTRACE_MODE_REALCREDS,
        //      so use the caller's real UID and GID for the checks in the
        //      next step.  (Most APIs that check the caller's UID and GID
        //      use the effective IDs.  For historical reasons, the
        //      PTRACE_MODE_REALCREDS check uses the real IDs instead.)
        let (uid, gid) = if mode.contains(PTRACE_MODE_FSCREDS) {
            let fscred = self.current_creds().as_fscred();
            (fscred.uid, fscred.gid)
        } else if mode.contains(PTRACE_MODE_REALCREDS) {
            let creds = self.current_creds();
            (creds.uid, creds.gid)
        } else {
            unreachable!();
        };

        // (3)  Deny access if neither of the following is true:
        //
        //      -  The real, effective, and saved-set user IDs of the target
        //         match the caller's user ID, and the real, effective, and
        //         saved-set group IDs of the target match the caller's
        //         group ID.
        //
        //      -  The caller has the CAP_SYS_PTRACE capability in the user
        //         namespace of the target.
        let target_creds = target.persistent_info.lock_creds();
        if !(target_creds.uid == uid
            && target_creds.euid == uid
            && target_creds.saved_uid == uid
            && target_creds.gid == gid
            && target_creds.egid == gid
            && target_creds.saved_gid == gid)
        {
            security::check_task_capable(self, CAP_SYS_PTRACE)?;
        }

        // (4)  Deny access if the target process "dumpable" attribute has a
        //      value other than 1 (SUID_DUMP_USER; see the discussion of
        //      PR_SET_DUMPABLE in prctl(2)), and the caller does not have
        //      the CAP_SYS_PTRACE capability in the user namespace of the
        //      target process.
        let dumpable = *target.mm()?.dumpable.lock();
        match dumpable {
            DumpPolicy::User => (),
            DumpPolicy::Disable => security::check_task_capable(self, CAP_SYS_PTRACE)?,
        }

        // (5)  The kernel LSM security_ptrace_access_check() interface is
        //      invoked to see if ptrace access is permitted.
        security::ptrace_access_check(self, target, mode)?;

        // (6)  If access has not been denied by any of the preceding steps,
        //      then access is allowed.
        Ok(())
    }

    pub fn can_signal(
        &self,
        target: &Task,
        unchecked_signal: UncheckedSignal,
    ) -> Result<(), Errno> {
        // If both the tasks share a thread group the signal can be sent. This is not documented
        // in kill(2) because kill does not support task-level granularity in signal sending.
        if self.thread_group == target.thread_group {
            return Ok(());
        }

        let self_creds = self.current_creds();
        let target_creds = target.real_creds();
        // From https://man7.org/linux/man-pages/man2/kill.2.html:
        //
        // > For a process to have permission to send a signal, it must either be
        // > privileged (under Linux: have the CAP_KILL capability in the user
        // > namespace of the target process), or the real or effective user ID of
        // > the sending process must equal the real or saved set- user-ID of the
        // > target process.
        //
        // Returns true if the credentials are considered to have the same user ID.
        if self_creds.euid == target_creds.saved_uid
            || self_creds.euid == target_creds.uid
            || self_creds.uid == target_creds.uid
            || self_creds.uid == target_creds.saved_uid
        {
            return Ok(());
        }

        if Signal::try_from(unchecked_signal) == Ok(SIGCONT) {
            let target_session = target.thread_group().read().process_group.session.leader.clone();
            let self_session = self.thread_group().read().process_group.session.leader.clone();
            if target_session == self_session {
                return Ok(());
            }
        }

        security::check_task_capable(self, CAP_KILL)
    }
}

impl ArchSpecific for CurrentTask {
    fn is_arch32(&self) -> bool {
        self.thread_state.is_arch32()
    }
}

impl MemoryAccessor for CurrentTask {
    fn read_memory<'a>(
        &self,
        addr: UserAddress,
        bytes: &'a mut [MaybeUninit<u8>],
    ) -> Result<&'a mut [u8], Errno> {
        self.mm()?.unified_read_memory(self, addr, bytes)
    }

    fn read_memory_partial_until_null_byte<'a>(
        &self,
        addr: UserAddress,
        bytes: &'a mut [MaybeUninit<u8>],
    ) -> Result<&'a mut [u8], Errno> {
        self.mm()?.unified_read_memory_partial_until_null_byte(self, addr, bytes)
    }

    fn read_memory_partial<'a>(
        &self,
        addr: UserAddress,
        bytes: &'a mut [MaybeUninit<u8>],
    ) -> Result<&'a mut [u8], Errno> {
        self.mm()?.unified_read_memory_partial(self, addr, bytes)
    }

    fn write_memory(&self, addr: UserAddress, bytes: &[u8]) -> Result<usize, Errno> {
        self.mm()?.unified_write_memory(self, addr, bytes)
    }

    fn write_memory_partial(&self, addr: UserAddress, bytes: &[u8]) -> Result<usize, Errno> {
        self.mm()?.unified_write_memory_partial(self, addr, bytes)
    }

    fn zero(&self, addr: UserAddress, length: usize) -> Result<usize, Errno> {
        self.mm()?.unified_zero(self, addr, length)
    }
}

impl TaskMemoryAccessor for CurrentTask {
    fn maximum_valid_address(&self) -> Option<UserAddress> {
        self.mm().ok().map(|mm| mm.maximum_valid_user_address)
    }
}

pub enum ExceptionResult {
    /// The exception was handled and no further action is required.
    Handled,

    // The exception generated a signal that should be delivered.
    Signal(SignalInfo),
}

fn split_path(path: &FsStr) -> LookupVec<&FsStr> {
    path.split(|c| *c == b'/').filter(|p| !p.is_empty()).map(<&FsStr>::from).collect()
}

#[cfg(test)]
mod tests {
    use crate::testing::spawn_kernel_and_run;
    use starnix_uapi::auth::Credentials;

    // This test will run `override_creds` and check it doesn't crash. This ensures that the
    // delegation to `override_creds_async` is correct.
    #[::fuchsia::test]
    async fn test_override_creds_can_delegate_to_async_version() {
        spawn_kernel_and_run(async move |current_task| {
            assert_eq!(current_task.override_creds(Credentials::root(), || 0), 0);
        })
        .await;
    }
}
