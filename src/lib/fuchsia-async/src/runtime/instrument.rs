// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Pluggable instrumentation for the async executor.

use crate::ScopeHandle;
pub use crate::runtime::fuchsia::executor::atomic_future::AtomicFutureHandle;
use std::any::Any;

/// A trait for instrumenting futures.
///
/// This trait provides a way to receive callbacks for various events that occur
/// for a future, such as completion, and polling.
pub trait Hooks {
    /// Called when the task has completed.
    fn task_completed(&mut self);

    /// Called when the task is about to be polled.
    fn task_poll_start(&mut self);

    /// Called when the task has finished being polled.
    fn task_poll_end(&mut self);
}

/// A trait for instrumenting the async executor.
///
/// This trait provides a way to receive callbacks for various events that occur
/// within the executor, such as task creation, completion, and polling.
pub trait TaskInstrument: Send + Sync + 'static {
    /// Called when a new task is created.
    /// Typically, implementers will want to call `task.add_hooks()` here
    /// to add hooks to the task.
    fn task_created<'a>(&self, parent_scope: &ScopeHandle, task: &mut AtomicFutureHandle<'a>);

    /// Called when scope is created
    ///
    /// # Arguments
    ///
    /// * `scope_name`: An optional name for the scope.
    /// * `parent_scope`: A reference to the parent scope, or None for the root.
    ///
    /// # Returns
    ///
    /// A boxed `Any` trait object representing the created scope
    /// which contains data that can later be retrieved from the
    /// scope using instrument_data() on the scope.
    fn scope_created(
        &self,
        scope_name: &str,
        parent_scope: Option<&ScopeHandle>,
    ) -> Box<dyn Any + Send + Sync>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Scope, ScopeHandle, SendExecutorBuilder, yield_now};
    use fuchsia_sync::Mutex;
    use std::any::Any;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // Instrumentation to track scope associations
    struct TrackedTask {
        poll_count: AtomicUsize,
        poll_end_count: AtomicUsize,
        completed: AtomicUsize,
    }

    struct TrackedScope {
        name: String,
        tasks: Mutex<Vec<Arc<TrackedTask>>>,
        scopes: Mutex<Vec<Arc<TrackedScope>>>,
    }

    struct ScopeTrackingInstrument {
        scopes: Mutex<Vec<Arc<TrackedScope>>>,
    }

    impl ScopeTrackingInstrument {
        fn new() -> Self {
            Self { scopes: Mutex::new(Vec::new()) }
        }

        fn get_scopes(&self) -> Vec<Arc<TrackedScope>> {
            self.scopes.lock().clone()
        }
    }

    struct TrackedHooks {
        task: Arc<TrackedTask>,
    }

    impl Hooks for TrackedHooks {
        fn task_completed(&mut self) {
            // Relaxed ordering is fine because we hold a mut reference,
            // so nothing else can mutate this (though because we're
            // technically sharing this state with the test main,
            // the borrow checker can't reason about this,
            // making either unsafe or atomics necessary.)
            self.task.completed.fetch_add(1, Ordering::Relaxed);
        }

        fn task_poll_end(&mut self) {
            assert_eq!(
                self.task.poll_count.load(Ordering::Relaxed) - 1,
                self.task.poll_end_count.load(Ordering::Relaxed)
            );
            self.task.poll_end_count.fetch_add(1, Ordering::Relaxed);
        }

        fn task_poll_start(&mut self) {
            self.task.poll_count.fetch_add(1, Ordering::Relaxed);
        }
    }

    impl TaskInstrument for ScopeTrackingInstrument {
        fn task_created<'a>(
            &self,
            parent_scope: &ScopeHandle,
            handle: &mut AtomicFutureHandle<'a>,
        ) {
            // Extract scope name from the parent scope
            let parent = parent_scope
                .instrument_data()
                .unwrap()
                .downcast_ref::<Arc<TrackedScope>>()
                .unwrap()
                .clone();
            let task = Arc::new(TrackedTask {
                poll_count: AtomicUsize::new(0),
                completed: AtomicUsize::new(0),
                poll_end_count: AtomicUsize::new(0),
            });

            // Add task to scope
            let mut tasks = parent.tasks.lock();
            tasks.push(task.clone());

            handle.add_hooks(TrackedHooks { task });
        }

        fn scope_created(
            &self,
            scope_name: &str,
            parent_scope: Option<&ScopeHandle>,
        ) -> Box<dyn Any + Send + Sync> {
            let tracked_scope = Arc::new(TrackedScope {
                name: scope_name.to_string(),
                tasks: Default::default(),
                scopes: Default::default(),
            });
            // Extract parent scope
            if let Some(parent_handle) = parent_scope
                && let Some(parent_scope) = parent_handle
                    .instrument_data()
                    .and_then(|data| data.downcast_ref::<Arc<TrackedScope>>())
            {
                parent_scope.scopes.lock().push(tracked_scope.clone());
            }

            self.scopes.lock().push(tracked_scope.clone());

            Box::new(tracked_scope)
        }
    }

    #[test]
    fn test_global_spawn_with_scope() {
        let instrumentation = Arc::new(ScopeTrackingInstrument::new());

        let mut executor = SendExecutorBuilder::new()
            .num_threads(4)
            .instrument(Some(instrumentation.clone()))
            .build();
        executor.run(async move {
            let root_scope = Scope::new_with_name("test_root");

            // Create a hierarchy of scopes
            let level2_scope = root_scope.new_child_with_name("level2");
            let level3_scope = level2_scope.new_child_with_name("level3".to_string());

            // Spawn tasks in different scopes
            root_scope.spawn(async {});

            level2_scope.spawn(async {
                yield_now().await; // Multiple polls
            });

            level3_scope.spawn(async {});

            level2_scope.spawn(async {});

            level3_scope.spawn(async {});

            // Wait for all tasks to complete
            root_scope.await;
        });

        // Verify the hierarchy
        let scopes = instrumentation.get_scopes();
        assert_eq!(scopes.len(), 4);

        // The Fuchsia executor creates its own scope called "root",
        // which is the true root scope here. All other scopes are
        // children under that one.
        let root_scope = &scopes[0];
        assert_eq!(root_scope.name, "root".to_string());
        assert_eq!(root_scope.tasks.lock().len(), 1);
        assert_eq!(root_scope.scopes.lock().len(), 1);

        let test_root_scope = &root_scope.scopes.lock()[0];
        assert_eq!(test_root_scope.name, "test_root".to_string());
        assert_eq!(test_root_scope.tasks.lock().len(), 1);
        assert_eq!(test_root_scope.scopes.lock().len(), 1);

        let level2_scope = &test_root_scope.scopes.lock()[0];
        assert_eq!(level2_scope.name, "level2".to_string());
        assert_eq!(level2_scope.tasks.lock().len(), 2);
        assert_eq!(level2_scope.scopes.lock().len(), 1);

        let level3_scope = &level2_scope.scopes.lock()[0];
        assert_eq!(level3_scope.name, "level3".to_string());
        assert_eq!(level3_scope.tasks.lock().len(), 2);
        assert_eq!(level3_scope.scopes.lock().len(), 0);

        // Assert poll counts
        let root_tasks = root_scope.tasks.lock();
        // We can't assert the number of polls for the root task,
        // as that is nondeterministic on a multithreaded executor.
        assert_eq!(root_tasks[0].completed.load(Ordering::Relaxed), 1);

        let test_root_tasks = test_root_scope.tasks.lock();
        assert_eq!(test_root_tasks[0].poll_count.load(Ordering::Relaxed), 1);
        assert_eq!(test_root_tasks[0].completed.load(Ordering::Relaxed), 1);

        let level2_tasks = level2_scope.tasks.lock();
        assert_eq!(level2_tasks[0].poll_count.load(Ordering::Relaxed), 2);
        assert_eq!(level2_tasks[0].completed.load(Ordering::Relaxed), 1);
        assert_eq!(level2_tasks[1].poll_count.load(Ordering::Relaxed), 1);
        assert_eq!(level2_tasks[1].completed.load(Ordering::Relaxed), 1);

        let level3_tasks = level3_scope.tasks.lock();
        assert_eq!(level3_tasks[0].poll_count.load(Ordering::Relaxed), 1);
        assert_eq!(level3_tasks[0].completed.load(Ordering::Relaxed), 1);
        assert_eq!(level3_tasks[1].poll_count.load(Ordering::Relaxed), 1);
        assert_eq!(level3_tasks[1].completed.load(Ordering::Relaxed), 1);
    }
}
