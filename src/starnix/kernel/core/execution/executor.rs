// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::execution::loop_entry::enter_syscall_loop;
use crate::ptrace::{PtraceCoreState, ptrace_attach_from_state};
use crate::task::{CurrentTask, DelayedReleaser, ExitStatus, TaskBuilder, ZirconThread};
use anyhow::Error;
use starnix_logging::{log_error, log_warn};
use starnix_sync::{ExecutorVmarManagerLock, LockDepMutex};
use starnix_uapi::errors::Errno;
use starnix_uapi::{errno, error};
use std::os::unix::thread::JoinHandleExt;
use std::sync::Arc;
use std::sync::mpsc::sync_channel;
use thread_create_vmars::ThreadCreateVmars;

/// Wrapper for `ThreadCreateVmars` to be stored in the kernel expando.
///
/// This is a module-private singleton used to manage VMARs for thread creation.
struct ExecutorVmarManager(LockDepMutex<ThreadCreateVmars, ExecutorVmarManagerLock>);

pub fn execute_task_with_prerun_result<F, R, G>(
    task_builder: TaskBuilder,
    pre_run: F,
    task_complete: G,
    ptrace_state: Option<PtraceCoreState>,
) -> Result<R, Errno>
where
    F: FnOnce(&mut CurrentTask) -> Result<R, Errno> + Send + Sync + 'static,
    R: Send + Sync + 'static,
    G: FnOnce(Result<ExitStatus, Error>) + Send + Sync + 'static,
{
    let (sender, receiver) = sync_channel::<Result<R, Errno>>(1);
    execute_task(
        task_builder,
        move |current_task| match pre_run(current_task) {
            Err(errno) => {
                let _ = sender.send(Err(errno.clone()));
                Err(errno)
            }
            Ok(value) => sender.send(Ok(value)).map_err(|error| {
                log_error!("Unable to send `pre_run` result: {error:?}");
                errno!(EINVAL)
            }),
        },
        task_complete,
        ptrace_state,
    )?;
    receiver.recv().map_err(|e| {
        log_error!("Unable to retrieve result from `pre_run`: {e:?}");
        errno!(EINVAL)
    })?
}

pub fn execute_task<F, G>(
    task_builder: TaskBuilder,
    pre_run: F,
    task_complete: G,
    ptrace_state: Option<PtraceCoreState>,
) -> Result<(), Errno>
where
    F: FnOnce(&mut CurrentTask) -> Result<(), Errno> + Send + Sync + 'static,
    G: FnOnce(Result<ExitStatus, Error>) + Send + Sync + 'static,
{
    // Set the process handle to the new task's process, so the new thread is spawned in that
    // process.
    let process_handle = task_builder.task.thread_group().process.raw_handle();

    let kernel = task_builder.task.kernel();
    let create_vmars =
        kernel.expando.get_or_init(|| ExecutorVmarManager(ThreadCreateVmars::new().into()));
    let mut create_vmars = create_vmars.0.lock();

    // SAFETY: thread_set_zx_create_handles only manipulates the handles for the current thread and
    // so there is no possibility of races. The process_handle is only used for diagnostic
    // purposes. The remaining handles are defined by ThreadCreateVmars to not be valid and not
    // destroyed until ThreadCreateVmars is destroyed. As ThreadCreateVmars has a lifetime of the
    // kernel, it will not be destroyed until all threads are terminated, at which point all usages
    // and references to these handles will have ended.
    let old_handles = unsafe {
        thrd_set_zx_create_handles(thrd_zx_create_handles {
            process: process_handle,
            machine_stack_vmar: create_vmars.machine_stack.probe()?.raw_handle(),
            security_stack_vmar: create_vmars.security_stack.probe()?.raw_handle(),
            thread_block_vmar: create_vmars.thread_block.probe()?.raw_handle(),
        })
    };
    scopeguard::defer! {
        // SAFETY: thrd_set_zx_create_handles only manipulates handles for the current thread and
        // so there is no possibility of races. This is resetting to the old values that were
        // present before our previous call to thrd_set_zx_create_handles, which must have been safe
        // to have been set.
        unsafe {
            thrd_set_zx_create_handles(old_handles);
        };
    };

    if let Some(ptrace_state) = ptrace_state {
        let _ = ptrace_attach_from_state(&task_builder.task, ptrace_state);
    }

    let ref_task = Arc::clone(&task_builder.task);
    let running_state = ref_task.running_state().unwrap();

    // Spawn the process' thread. Note, this closure ends up executing in the process referred to by
    // `process_handle`.
    let (sender, receiver) = sync_channel::<TaskBuilder>(1);
    let result = std::thread::Builder::new().name("user-thread".to_string()).spawn(move || {
        // Note, cross-process shared resources allocated in this function that aren't freed by the
        // Zircon kernel upon thread and/or process termination (like mappings in the shared region)
        // should be freed using the delayed finalizer mechanism and Task drop.
        let mut current_task: CurrentTask = receiver
            .recv()
            .expect("caller should always send task builder before disconnecting")
            .into();

        // We don't need the receiver anymore. If we don't drop the receiver now, we'll keep it
        // allocated for the lifetime of the thread.
        std::mem::drop(receiver);

        let pre_run_result = { pre_run(&mut current_task) };
        if pre_run_result.is_err() {
            // Only log if the pre run didn't exit the task. Otherwise, consider this is expected
            // by the caller.
            if current_task.exit_status().is_none() {
                log_error!("Pre run failed from {pre_run_result:?}. The task will not be run.");
            }

            // Drop the task_complete callback to ensure that the closure isn't holding any
            // releasables.
            std::mem::drop(task_complete);
        } else {
            let exit_status = enter_syscall_loop(&mut current_task);
            current_task.write().set_exit_status(exit_status.clone());
            task_complete(Ok(exit_status));
        }

        // `release` must be called as the absolute last action on this thread to ensure that
        // any deferred release are done before it.
        current_task.release(());

        // Ensure that no releasables are registered after this point as we unwind the stack.
        DelayedReleaser::finalize();
    });
    let join_handle = match result {
        Ok(handle) => handle,
        Err(e) => {
            task_builder.release(());
            match e.kind() {
                std::io::ErrorKind::WouldBlock => return error!(EAGAIN),
                other => panic!("unexpected error on thread spawn: {other}"),
            }
        }
    };
    // Drop the VMAR manager lock immediately after thread creation completes to avoid holding
    // the mutex across post-spawn operations (like setting thread roles and handle duplication).
    // Local ownership of `task_builder` guarantees that the task and its VMAR remain valid
    // and safe from destruction or concurrent mutation throughout post-spawn setup.
    std::mem::drop(create_vmars);

    // The process thread spawned successfully. Mark the task as having spawned.
    task_builder.task.write().set_spawned();

    // Update the thread and task information before sending the task_builder to the spawned thread.
    // This will make sure the mapping between linux tid and fuchsia koid is set before trace events
    // are emitted from the linux code.

    // Set the task's thread handle
    let pthread = join_handle.as_pthread_t();
    #[allow(
        clippy::undocumented_unsafe_blocks,
        reason = "Force documented unsafe blocks in Starnix"
    )]
    let raw_thread_handle =
        unsafe { zx::Unowned::<'_, zx::Thread>::from_raw_handle(thrd_get_zx_handle(pthread)) };
    let thread = Arc::new(
        raw_thread_handle
            .duplicate_handle(zx::Rights::SAME_RIGHTS)
            .expect("must have RIGHT_DUPLICATE on handle we created"),
    );
    running_state.thread.set(ZirconThread::new(thread)).expect("thread should only be set once");
    // Now that the task has a thread handle, update the thread's role using the policy configured.
    if let Err(err) = ref_task.sync_scheduler_state_to_role() {
        log_warn!(err:?; "Couldn't update freshly spawned thread's profile.");
    }

    // Record the thread and process ids for tracing after the task_thread is unlocked.
    ref_task.record_pid_koid_mapping();

    // Wait to send the `TaskBuilder` to the spawned thread until we know that it
    // spawned successfully, as we need to ensure the builder is always explicitly
    // released.
    sender
        .send(task_builder)
        .expect("receiver should not be disconnected because thread spawned successfully");

    Ok(())
}

#[repr(C)]
#[derive(Debug)]
pub struct thrd_zx_create_handles {
    pub process: zx::sys::zx_handle_t,
    pub machine_stack_vmar: zx::sys::zx_handle_t,
    pub security_stack_vmar: zx::sys::zx_handle_t,
    pub thread_block_vmar: zx::sys::zx_handle_t,
}
unsafe extern "C" {
    fn thrd_set_zx_create_handles(handles: thrd_zx_create_handles) -> thrd_zx_create_handles;

    // Gets the thread handle underlying a specific thread.
    // In C the 'thread' parameter is thrd_t which on Fuchsia is the same as pthread_t.
    fn thrd_get_zx_handle(thread: u64) -> zx::sys::zx_handle_t;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ptrace::StopState;
    use crate::signals::SignalInfo;
    use crate::testing::*;
    use starnix_uapi::signals::{SIGCONT, SIGSTOP};

    #[::fuchsia::test]
    async fn test_block_if_stopped_stop_and_continue() {
        spawn_kernel_and_run(async |task| {
            // The task is not stopped.
            assert!(!task.block_if_stopped());

            // Stop the task.
            task.thread_group().set_stopped(
                StopState::GroupStopping,
                Some(SignalInfo::kernel(SIGSTOP)),
                false,
            );

            let thread = std::thread::spawn({
                let task = task.weak_task();
                move || {
                    let task = task.upgrade().expect("task must be alive");
                    // Wait for the task to have a waiter.
                    while !task.read().is_blocked() {
                        std::thread::sleep(std::time::Duration::from_millis(10));
                    }

                    // Continue the task.
                    task.thread_group().set_stopped(
                        StopState::Waking,
                        Some(SignalInfo::kernel(SIGCONT)),
                        false,
                    );
                }
            });

            // Block until continued.
            assert!(task.block_if_stopped());

            // Join the thread, which will ensure set_stopped terminated.
            thread.join().expect("joined");

            // The task should not be blocked anymore.
            assert!(!task.block_if_stopped());
        })
        .await;
    }

    #[::fuchsia::test]
    async fn test_block_if_stopped_stop_and_exit() {
        spawn_kernel_and_run(async |task| {
            // The task is neither stopped nor exited.
            assert!(!task.block_if_stopped());

            // Stop the task.
            task.thread_group().set_stopped(
                StopState::GroupStopping,
                Some(SignalInfo::kernel(SIGSTOP)),
                false,
            );

            let thread = std::thread::spawn({
                let task = task.weak_task();
                move || {
                    let task = task.upgrade().expect("task must be alive");
                    // Wait for the task to have a waiter.
                    while !task.read().is_blocked() {
                        std::thread::sleep(std::time::Duration::from_millis(10));
                    }

                    // exit the task.
                    task.thread_group().kill(ExitStatus::Exit(1), None);
                }
            });

            // Block until continued.
            assert!(task.block_if_stopped());

            // Join the task, which will ensure thread_group.exit terminated.
            thread.join().expect("joined");

            // The task should not be blocked because it is stopped.
            assert!(!task.block_if_stopped());
        })
        .await;
    }
}
