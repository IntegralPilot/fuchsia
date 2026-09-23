// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::mm::MemoryManager;
use crate::security;
use crate::signals::SignalActions;
use crate::task::{
    CurrentTask, Kernel, Pid, PidTableGuard, ProcessGroup, RobustListHeadPtr,
    SeccompFilterContainer, SeccompState, Task, TaskBuilder, ThreadGroup, ThreadGroupParent,
    ThreadGroupWriteGuard,
};
use crate::vfs::{FsContext, SharedFdTable};
use starnix_sync::allow_subclass;
use starnix_task_command::TaskCommand;
use starnix_types::arch::ArchWidth;
use starnix_types::release_on_error;
use starnix_uapi::auth::Credentials;
use starnix_uapi::errors::Errno;
use starnix_uapi::resource_limits::Resource;
use starnix_uapi::signals::{SIGCHLD, Signal};
use starnix_uapi::{errno, error, from_status_like_fdio, rlimit};
use std::ffi::CString;
use std::sync::Arc;

/// Result returned when creating new Zircon processes for tasks.
///
/// This does not include the task's Zircon thread. Backing threads are attached later in the task
/// lifecycle, when creating an execution context in [`execute_task()`].
pub struct TaskInfo {
    /// The thread group that the task should be added to.
    pub thread_group: Arc<ThreadGroup>,

    /// The memory manager to use for the task.
    pub memory_manager: Option<Arc<MemoryManager>>,
}

pub fn create_zircon_process(
    kernel: &Arc<Kernel>,
    parent: Option<ThreadGroupWriteGuard<'_>>,
    pid: Pid,
    exit_signal: Option<Signal>,
    process_group: Arc<ProcessGroup>,
    signal_actions: Arc<SignalActions>,
    name: TaskCommand,
) -> Result<TaskInfo, Errno> {
    // Don't allow new processes to be created once the kernel has started shutting down.
    if kernel.is_shutting_down() {
        return error!(EBUSY);
    }
    let (process, root_vmar) =
        create_shared(&kernel.kthreads.starnix_process, zx::ProcessOptions::empty(), name)
            .map_err(|status| from_status_like_fdio!(status))?;

    // Make sure that if this process panics in normal mode that the whole kernel's job is killed.
    fuchsia_runtime::job_default()
        .set_critical(zx::JobCriticalOptions::RETCODE_NONZERO, &process)
        .map_err(|status| from_status_like_fdio!(status))?;

    let thread_group = ThreadGroup::new(
        kernel.clone(),
        process,
        root_vmar,
        parent,
        pid,
        exit_signal,
        process_group,
        signal_actions,
    );

    Ok(TaskInfo { thread_group, memory_manager: None })
}

/// Creates a process that shares half its address space with this process.
///
/// The created process will also share its handle table and futex context with `self`.
///
/// Returns the created process and a handle to the created process' restricted address space.
///
/// Wraps the
/// [zx_process_create_shared](https://fuchsia.dev/fuchsia-src/reference/syscalls/process_create_shared.md)
/// syscall.
fn create_shared(
    process: &zx::Process,
    options: zx::ProcessOptions,
    name: TaskCommand,
) -> Result<(zx::Process, zx::Vmar), zx::Status> {
    let self_raw = process.raw_handle();
    let name_bytes = name.as_bytes();
    let mut process_out = 0;
    let mut restricted_vmar_out = 0;
    #[allow(
        clippy::undocumented_unsafe_blocks,
        reason = "Force documented unsafe blocks in Starnix"
    )]
    let status = unsafe {
        zx::sys::zx_process_create_shared(
            self_raw,
            options.bits(),
            name_bytes.as_ptr(),
            name_bytes.len(),
            &mut process_out,
            &mut restricted_vmar_out,
        )
    };
    zx::ok(status)?;
    #[allow(
        clippy::undocumented_unsafe_blocks,
        reason = "Force documented unsafe blocks in Starnix"
    )]
    unsafe {
        Ok((
            zx::Process::from(zx::NullableHandle::from_raw(process_out)),
            zx::Vmar::from(zx::NullableHandle::from_raw(restricted_vmar_out)),
        ))
    }
}

/// Create a process that is a child of the `init` process.
///
/// The created process will be a task that is the leader of a new thread group.
///
/// Most processes are created by userspace and are descendants of the `init` process. In
/// some situations, the kernel needs to create a process itself. This function is the
/// preferred way of creating an actual userspace process because making the process a child of
/// `init` means that `init` is responsible for waiting on the process when it dies and thereby
/// cleaning up its zombie.
///
/// If you just need a kernel task, and not an entire userspace process, consider using
/// `create_system_task` instead. Even better, consider using the `kthreads` threadpool.
///
/// If `seclabel` is set, or the container specified a `default_seclabel`, then it will be
/// resolved against the `kernel`'s active security policy, and applied to the new task.
/// Otherwise the task will inherit its LSM state from the "init" task.
///
/// This function creates an underlying Zircon process to host the new task.
pub fn create_init_child_process(
    kernel: &Arc<Kernel>,
    initial_name: TaskCommand,
    mut creds: Credentials,
    seclabel: Option<&CString>,
) -> Result<TaskBuilder, Errno> {
    let init_task = kernel.get_init_task()?;

    let fs = init_task.running_state()?.fs().fork();

    let security_state = if let Some(seclabel) = seclabel {
        security::task_for_context(&init_task, seclabel.as_bytes().into())?
    } else if let Some(default_seclabel) = kernel.features.default_seclabel.as_ref() {
        security::task_for_context(&init_task, default_seclabel.as_bytes().into())?
    } else {
        // If SELinux is enabled then this call will fail with `EINVAL`.
        security::task_for_context(&init_task, b"".into()).map_err(|_| {
            errno!(EINVAL, "Container has SELinux enabled but no Security Context specified")
        })?
    };
    creds.security_state = security_state;

    let task = create_task(
        kernel,
        initial_name.clone(),
        fs,
        |pid, process_group| {
            create_zircon_process(
                kernel,
                None,
                pid,
                Some(SIGCHLD),
                process_group,
                SignalActions::default(),
                initial_name.clone(),
            )
        },
        creds.into(),
    )?;
    {
        let mut init_writer = init_task.thread_group().write();
        // Init is the parent of every other process, so this matches the lock
        // ordering from parent to child.
        let _token = allow_subclass();
        let mut new_process_writer = task.thread_group().write();
        new_process_writer.parent =
            Some(ThreadGroupParent::new(Arc::downgrade(&init_task.thread_group())));
        init_writer.children.insert(task.tid.id, Arc::downgrade(task.thread_group()));
    }
    // A child process created via fork(2) inherits its parent's
    // resource limits.  Resource limits are preserved across execve(2).
    let limits = init_task.thread_group().limits.lock().clone();
    *task.thread_group().limits.lock() = limits;
    Ok(task)
}

/// Creates the initial process for a kernel.
///
/// The created process will be a task that is the leader of a new thread group.
///
/// The init process is special because it's the root of the parent/child relationship between
/// tasks. If a task dies, the init process is ultimately responsible for waiting on that task
/// and removing it from the zombie list.
///
/// It's possible for the kernel to create tasks whose ultimate parent isn't init, but such
/// tasks cannot be created by userspace directly.
///
/// This function should only be called as part of booting a kernel instance. To create a
/// process after the kernel has already booted, consider `create_init_child_process`
/// or `create_system_task`.
///
/// The process created by this function should always have pid 1. We require the caller to
/// pass the `pid` as an argument to clarify that it's the callers responsibility to determine
/// the pid for the process.
pub fn create_init_process(
    kernel: &Arc<Kernel>,
    pid: Pid,
    initial_name: TaskCommand,
    fs: Arc<FsContext>,
    rlimits: &[(Resource, u64)],
) -> Result<TaskBuilder, Errno> {
    assert_eq!(pid.id, 1);
    let builder = create_task_with_pid(
        kernel,
        kernel.pids.lock(),
        pid,
        initial_name.clone(),
        fs,
        |pid, process_group| {
            create_zircon_process(
                kernel,
                None,
                pid,
                Some(SIGCHLD),
                process_group,
                SignalActions::default(),
                initial_name.clone(),
            )
        },
        Credentials::root(),
        rlimits,
    )?;
    let _ = kernel.init_task.set(Arc::downgrade(&builder.task));
    Ok(builder)
}

/// Create a task that runs inside the kernel.
///
/// There is no underlying Zircon process to host the task. Instead, the work done by this task
/// is performed by a thread in the original Starnix process, possible as part of a thread
/// pool.
///
/// This function is the preferred way to create a context for doing background work inside the
/// kernel.
///
/// Rather than calling this function directly, consider using `kthreads`, which provides both
/// a system task and a threadpool on which the task can do work.
pub fn create_system_task(kernel: &Arc<Kernel>, fs: Arc<FsContext>) -> Result<CurrentTask, Errno> {
    let builder = create_task(
        kernel,
        TaskCommand::new(b"kthreadd"),
        fs,
        |pid, process_group| {
            let thread_group = ThreadGroup::for_system(kernel.clone(), pid, process_group);
            Ok(TaskInfo { thread_group, memory_manager: None }.into())
        },
        Credentials::root(),
    )?;
    Ok(builder.into())
}

pub fn create_task<F>(
    kernel: &Kernel,
    initial_name: TaskCommand,
    root_fs: Arc<FsContext>,
    task_info_factory: F,
    creds: Arc<Credentials>,
) -> Result<TaskBuilder, Errno>
where
    F: FnOnce(Pid, Arc<ProcessGroup>) -> Result<TaskInfo, Errno>,
{
    let mut guard = kernel.pids.lock();
    let pid = guard.allocate_pid()?;
    create_task_with_pid(kernel, guard, pid, initial_name, root_fs, task_info_factory, creds, &[])
}

fn create_task_with_pid<F>(
    kernel: &Kernel,
    mut pids: PidTableGuard<'_>,
    pid: Pid,
    initial_name: TaskCommand,
    root_fs: Arc<FsContext>,
    task_info_factory: F,
    creds: Arc<Credentials>,
    rlimits: &[(Resource, u64)],
) -> Result<TaskBuilder, Errno>
where
    F: FnOnce(Pid, Arc<ProcessGroup>) -> Result<TaskInfo, Errno>,
{
    debug_assert!(pid.get_task().is_err());

    let process_group = ProcessGroup::new(pid.clone(), None);

    let TaskInfo { thread_group, memory_manager } =
        task_info_factory(pid.clone(), process_group.clone())?;

    pids.add_process_group(&process_group);
    process_group.insert(&thread_group);

    // > The timer slack values of init (PID 1), the ancestor of all processes, are 50,000
    // > nanoseconds (50 microseconds).  The timer slack value is inherited by a child created
    // > via fork(2), and is preserved across execve(2).
    // https://man7.org/linux/man-pages/man2/prctl.2.html
    let default_timerslack = 50_000;
    let builder = TaskBuilder {
        task: Task::new(
            pid,
            initial_name,
            thread_group,
            SharedFdTable::default(),
            memory_manager,
            root_fs,
            creds,
            Arc::clone(&kernel.default_abstract_socket_namespace),
            Arc::clone(&kernel.default_abstract_vsock_namespace),
            Default::default(),
            Default::default(),
            None,
            Default::default(),
            kernel.root_uts_ns.clone(),
            false,
            SeccompState::default(),
            SeccompFilterContainer::default(),
            RobustListHeadPtr::null(&ArchWidth::Arch64),
            default_timerslack,
        ),
        thread_state: Default::default(),
    };
    release_on_error!(builder, {
        builder.thread_group().add(Arc::clone(&builder.task))?;
        for (resource, limit) in rlimits {
            builder
                .thread_group()
                .limits
                .lock()
                .set(*resource, rlimit { rlim_cur: *limit, rlim_max: *limit });
        }

        pids.add_task(Arc::clone(&builder.task));
        Ok(())
    });
    Ok(builder)
}

/// Create a kernel task in the same ThreadGroup as the given `system_task`.
///
/// There is no underlying Zircon thread to host the task.
pub fn create_kernel_thread(
    system_task: &Task,
    initial_name: TaskCommand,
) -> Result<CurrentTask, Errno> {
    let mut pids = system_task.kernel().pids.lock();
    let pid = pids.allocate_pid()?;

    let scheduler_state;
    let uts_ns;
    let default_timerslack_ns;
    {
        let state = system_task.read();
        scheduler_state = state.scheduler_state;
        uts_ns = state.uts_ns.clone();
        default_timerslack_ns = state.default_timerslack_ns;
    }

    let mm;
    let fs;
    let abstract_socket_namespace;
    let abstract_vsock_namespace;
    {
        let running_state = system_task.running_state()?;
        mm = running_state.mm.upgrade();
        fs = running_state.fs();
        abstract_socket_namespace = running_state.abstract_socket_namespace.clone();
        abstract_vsock_namespace = running_state.abstract_vsock_namespace.clone();
    }

    let current_task: CurrentTask = TaskBuilder::new(Task::new(
        pid,
        initial_name,
        system_task.thread_group().clone(),
        SharedFdTable::default(),
        mm,
        fs,
        system_task.clone_creds(),
        abstract_socket_namespace,
        abstract_vsock_namespace,
        Default::default(),
        Default::default(),
        None,
        scheduler_state,
        uts_ns,
        false,
        SeccompState::default(),
        SeccompFilterContainer::default(),
        RobustListHeadPtr::null(&ArchWidth::Arch64),
        default_timerslack_ns,
    ))
    .into();
    release_on_error!(current_task, {
        current_task.thread_group().add(Arc::clone(&current_task.task))?;
        pids.add_task(Arc::clone(&current_task.task));
        Ok(())
    });
    Ok(current_task)
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::testing::*;

    #[::fuchsia::test]
    async fn test_create_task_failure_does_not_add_process_group() {
        spawn_kernel_and_run(async |current_task| {
            let kernel = current_task.kernel();
            let (kept_pg_sender, kept_pg_receiver) = std::sync::mpsc::channel();
            let result = super::create_task(
                kernel,
                TaskCommand::new(b"failed_task"),
                current_task.fs(),
                |_pid, process_group| {
                    let _ = kept_pg_sender.send(process_group);
                    error!(EINVAL)
                },
                Credentials::root(),
            );
            assert!(result.is_err());
            let kept_pg = kept_pg_receiver.recv().unwrap();
            let pid_entry = kernel.pids.get(kept_pg.leader.id).unwrap().clone();
            assert_eq!(pid_entry.get_process_group(), error!(ESRCH));
        })
        .await;
    }
}
