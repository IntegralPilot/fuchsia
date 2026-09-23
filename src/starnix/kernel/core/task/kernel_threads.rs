// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::execution::create_kernel_thread;
use crate::task::dynamic_thread_spawner::DynamicThreadSpawner;
use crate::task::{CurrentTask, DelayedReleaser, Kernel, Task, ThreadGroup};
use fragile::Fragile;
use fuchsia_async as fasync;
use pin_project::pin_project;
use scopeguard::ScopeGuard;

use starnix_task_command::TaskCommand;

use starnix_uapi::errors::Errno;
use starnix_uapi::{errno, error};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, OnceLock, Weak};
use std::task::{Context, Poll};

/// The threads that the kernel runs internally.
///
/// These threads run in the main starnix process and outlive any specific userspace process.
pub struct KernelThreads {
    /// The main starnix process. This process is used to create new processes when using the
    /// restricted executor.
    pub starnix_process: zx::Process,

    /// A handle to the async executor running in `starnix_process`.
    ///
    /// You can spawn tasks on this executor using `spawn_future`. However, those task must not
    /// block. If you need to block, you can spawn a worker thread using `spawner`.
    ehandle: fasync::EHandle,

    /// The thread pool to spawn blocking calls to.
    spawner: OnceLock<DynamicThreadSpawner>,

    /// Information about the main system task that is bound to the kernel main thread.
    system_task: OnceLock<SystemTask>,

    /// A weak reference to the kernel owning this struct.
    kernel: Weak<Kernel>,
}

impl KernelThreads {
    /// Create a KernelThreads object for the given Kernel.
    ///
    /// Must be called in the initial Starnix process on a thread with an async executor. This
    /// function captures the async executor for this thread for use with spawned futures.
    ///
    /// Used during kernel boot.
    pub fn new(kernel: Weak<Kernel>) -> Self {
        KernelThreads {
            starnix_process: fuchsia_runtime::process_self()
                .duplicate_handle(zx::Rights::SAME_RIGHTS)
                .expect("Failed to duplicate process self"),
            ehandle: fasync::EHandle::local(),
            spawner: Default::default(),
            system_task: Default::default(),
            kernel,
        }
    }

    /// Initialize this object with the system task that will be used for spawned threads.
    ///
    /// This function must be called before this object is used to spawn threads.
    pub fn init(&self, system_task: CurrentTask) -> Result<(), Errno> {
        self.system_task.set(SystemTask::new(system_task)).map_err(|_| errno!(EEXIST))?;
        self.spawner
            .set(DynamicThreadSpawner::new(2, self.system_task().weak_task(), "kthreadd/init"))
            .map_err(|_| errno!(EEXIST))?;
        Ok(())
    }

    /// Spawn an async task in the main async executor to await the given future.
    ///
    /// Use this function to run async tasks in the background. These tasks cannot block or else
    /// they will starve the main async executor.
    ///
    /// Prefer this function to `spawn` for non-blocking work.
    pub fn spawn_future(
        &self,
        future: impl AsyncFnOnce() -> () + Send + 'static,
        name: &'static str,
    ) {
        self.ehandle.spawn_detached(WrappedMainFuture::new(
            self.kernel.clone(),
            async move { fasync::Task::local(future()).await },
            name,
        ));
    }

    /// The dynamic thread spawner used to spawn threads.
    ///
    /// To spawn a thread in this thread pool, use `spawn()`.
    pub fn spawner(&self) -> &DynamicThreadSpawner {
        self.spawner.get().as_ref().unwrap()
    }

    /// Access the `CurrentTask` for the kernel main thread.
    ///
    /// This function can only be called from the kernel main thread itself.
    pub fn system_task(&self) -> &CurrentTask {
        self.system_task.get().expect("KernelThreads::init must be called").system_task.get()
    }

    /// Access the `ThreadGroup` for the system tasks.
    ///
    /// This function can be safely called from anywhere as soon as `KernelThreads::init` has been
    /// called.
    pub fn system_thread_group(&self) -> Arc<ThreadGroup> {
        self.system_task
            .get()
            .expect("KernelThreads::init must be called")
            .system_thread_group
            .upgrade()
            .expect("System task must be still alive")
    }
}

impl Drop for KernelThreads {
    // TODO: Replace with .release. This is not actually safe, since locks
    // may be held elsewhere on this thread.
    fn drop(&mut self) {
        if let Some(system_task) = self.system_task.take() {
            system_task.system_task.into_inner().release(());
        }
    }
}

/// Create a new system task, register it on the thread and run the given closure with it.

pub fn with_new_current_task<F, R>(
    system_task: &Weak<Task>,
    task_name: String,
    f: F,
) -> Result<R, Errno>
where
    F: FnOnce(&CurrentTask) -> R,
{
    let current_task = {
        let Some(system_task) = system_task.upgrade() else {
            return error!(ESRCH);
        };
        create_kernel_thread(&system_task, TaskCommand::new(task_name.as_bytes())).unwrap()
    };
    let result = f(&current_task);
    current_task.release(());

    // Ensure that no releasables are registered after this point as we unwind the stack.
    DelayedReleaser::finalize();

    Ok(result)
}

struct SystemTask {
    /// The system task is bound to the kernel main thread. `Fragile` ensures a runtime crash if it
    /// is accessed from any other thread.
    system_task: Fragile<CurrentTask>,

    /// The system `ThreadGroup` is accessible from everywhere.
    system_thread_group: Weak<ThreadGroup>,
}

impl SystemTask {
    fn new(system_task: CurrentTask) -> Self {
        let system_thread_group = Arc::downgrade(&system_task.thread_group());
        Self { system_task: system_task.into(), system_thread_group }
    }
}

// The order is important here. Rust will drop fields in declaration order and we want
// the future to be dropped before the ScopeGuard runs.
#[pin_project]
pub(crate) struct WrappedFuture<F, C: Clone> {
    #[pin]
    fut: F,
    cleaner: fn(C),
    context: ScopeGuard<C, fn(C)>,
    name: &'static str,
}

impl<F, C: Clone> WrappedFuture<F, C> {
    pub(crate) fn new_with_cleaner(context: C, cleaner: fn(C), fut: F, name: &'static str) -> Self {
        // We need the ScopeGuard in case the future queues releasers when dropped.
        Self { fut, cleaner, context: ScopeGuard::with_strategy(context, cleaner), name }
    }
}

impl<F: Future, C: Clone> Future for WrappedFuture<F, C> {
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        fuchsia_trace::duration!(starnix_logging::CATEGORY_STARNIX, &*this.name);
        let result = this.fut.poll(cx);

        (this.cleaner)(this.context.clone());
        result
    }
}

type WrappedMainFuture<F> = WrappedFuture<F, Weak<Kernel>>;

impl<F> WrappedMainFuture<F> {
    fn new(kernel: Weak<Kernel>, fut: F, name: &'static str) -> Self {
        Self::new_with_cleaner(kernel, trigger_delayed_releaser, fut, name)
    }
}

fn trigger_delayed_releaser(kernel: Weak<Kernel>) {
    if let Some(kernel) = kernel.upgrade() {
        if let Some(system_task) = kernel.kthreads.system_task.get() {
            system_task.system_task.get().trigger_delayed_releaser();
        }
    }
}
