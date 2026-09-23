// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fuchsia_async::ScopeHandle;
use fuchsia_async::instrument::{AtomicFutureHandle, Hooks, TaskInstrument};
use fuchsia_inspect::{self as inspect, Node, NumericProperty, Property};
use fuchsia_sync::Mutex;
use std::any::Any;
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

fn get_unique_name(names: &mut HashSet<String>, base_name: String) -> String {
    if names.insert(base_name.clone()) {
        return base_name;
    }
    let mut i = 1;
    loop {
        let new_name = format!("{}_{}", base_name, i);
        if names.insert(new_name.clone()) {
            return new_name;
        }
        i += 1;
    }
}

/// An implementation of `TaskInstrument` that uses Inspect.
pub struct InspectTaskInstrument {
    root: Node,
    next_task_id: AtomicUsize,
    child_names: Arc<Mutex<HashSet<String>>>,
}

struct TaskNode {
    _node: Node,
    polls: inspect::UintProperty,
    completed: inspect::BoolProperty,
    max_poll_duration_micros: inspect::UintProperty,
}

struct InspectHooks {
    task_node: TaskNode,
    poll_start_time: zx::BootInstant,
    max_poll_duration_micros: u64,
    name: String,
    parent_names: Arc<Mutex<HashSet<String>>>,
}

impl Drop for InspectHooks {
    fn drop(&mut self) {
        self.parent_names.lock().remove(&self.name);
    }
}

impl Hooks for InspectHooks {
    fn task_completed(&mut self) {
        self.task_node.completed.set(true);
    }

    fn task_poll_end(&mut self) {
        let duration = fuchsia_async::BootInstant::now().into_zx() - self.poll_start_time;
        let duration_micros = duration.into_micros().max(0) as u64;
        if duration_micros > self.max_poll_duration_micros {
            self.max_poll_duration_micros = duration_micros;
            self.task_node.max_poll_duration_micros.set(duration_micros);
        }
    }

    fn task_poll_start(&mut self) {
        self.poll_start_time = fuchsia_async::BootInstant::now().into_zx();
        self.task_node.polls.add(1);
    }
}

struct ScopeInspect {
    node: Node,
    child_names: Arc<Mutex<HashSet<String>>>,
    name: String,
    parent_names: Arc<Mutex<HashSet<String>>>,
}

impl Drop for ScopeInspect {
    fn drop(&mut self) {
        self.parent_names.lock().remove(&self.name);
    }
}

/// A configuration object for `InspectTaskInstrument`.
///
/// This struct allows for future expansion of configuration options without breaking
/// the existing API.
pub struct InspectTaskConfiguration {
    /// The root inspect node under which task and scope nodes will be created.
    pub inspect_root: Node,
}

impl InspectTaskConfiguration {
    pub fn new(inspect_root: Node) -> Self {
        Self { inspect_root }
    }
}

impl InspectTaskInstrument {
    /// Create a new `InspectTaskInstrument`.
    pub fn new(config: InspectTaskConfiguration) -> Arc<Self> {
        Arc::new(Self {
            root: config.inspect_root,
            next_task_id: AtomicUsize::new(0),
            child_names: Arc::new(Mutex::new(HashSet::new())),
        })
    }
}

impl TaskInstrument for InspectTaskInstrument {
    fn task_created<'a>(&self, parent_scope: &ScopeHandle, task: &mut AtomicFutureHandle<'a>) {
        let id = self.next_task_id.fetch_add(1, Ordering::Relaxed);
        let base_name = format!("task_{}", id);
        let (parent_node, parent_names) = parent_scope
            .instrument_data()
            .and_then(|scope| scope.downcast_ref::<ScopeInspect>())
            .map(|scope| (&scope.node, Arc::clone(&scope.child_names)))
            .unwrap_or_else(|| (&self.root, Arc::clone(&self.child_names)));
        let name = {
            let mut names = parent_names.lock();
            get_unique_name(&mut names, base_name)
        };
        let node = parent_node.create_child(&name);
        let polls = node.create_uint("polls", 0);
        let completed = node.create_bool("completed", false);
        let max_poll_duration_micros = node.create_uint("max_poll_duration_micros", 0);
        let task_node = TaskNode { _node: node, polls, completed, max_poll_duration_micros };
        task.add_hooks(InspectHooks {
            task_node,
            poll_start_time: zx::BootInstant::get(),
            max_poll_duration_micros: 0,
            name,
            parent_names,
        })
    }

    fn scope_created(
        &self,
        scope_name: &str,
        parent_scope: Option<&ScopeHandle>,
    ) -> Box<dyn Any + Send + Sync> {
        let (parent_node, parent_names) = parent_scope
            .map(|scope| scope.instrument_data())
            .flatten()
            .and_then(|scope| scope.downcast_ref::<ScopeInspect>())
            .map(|scope| (&scope.node, Arc::clone(&scope.child_names)))
            .unwrap_or_else(|| (&self.root, Arc::clone(&self.child_names)));
        let name = {
            let mut names = parent_names.lock();
            get_unique_name(&mut names, scope_name.to_string())
        };
        let node = parent_node.create_child(&name);
        Box::new(ScopeInspect {
            node,
            child_names: Arc::new(Mutex::new(HashSet::new())),
            name,
            parent_names,
        })
    }
}

pub fn default_root() -> Node {
    fuchsia_inspect::component::inspector()
        .root()
        .create_child("fuchsia.inspect.AsyncInstrumentation")
}

#[cfg(test)]
mod tests {
    use super::*;
    use diagnostics_assertions::assert_data_tree;
    use fuchsia_async::{self as fasync, EHandle, MonotonicInstant};
    use std::pin::pin;

    #[fuchsia::test]
    fn test_max_poll_duration() {
        let inspector = inspect::Inspector::default();
        let instrument = InspectTaskInstrument::new(InspectTaskConfiguration::new(
            inspector.root().clone_weak(),
        ));
        let mut exec =
            fasync::TestExecutorBuilder::new().fake_time(true).instrument(instrument).build();

        exec.set_fake_time(MonotonicInstant::from_nanos(0));
        let mut fut = pin!(async {
            let child_scope = fuchsia_async::Scope::new_with_name("task_1");
            let child_task = fuchsia_async::Task::local(async move {
                futures::future::pending::<()>().await;
            });
            assert_data_tree!(inspector, root: {
                root: {
                    // Our task
                    task_0:{
                        polls: 1u64,
                        completed: false,
                        max_poll_duration_micros: 0u64,
                    },
                    // Task that was spawned under us
                    task_1:{},
                    task_1_1:{
                        polls: 0u64,
                        completed: false,
                        max_poll_duration_micros: 0u64,
                    },
                }
            });
            // Drop the task, which should remove it from Inspect
            drop(child_task);
            drop(child_scope);
            // Dropping a task doesn't immediately drop it,
            // but in a TestExecutor it should be dropped
            // after polling again.
            fuchsia_async::yield_now().await;
            assert_data_tree!(inspector, root: {
                root: {
                    // Our task
                    task_0:{
                        polls: 2u64,
                        completed: false,
                        max_poll_duration_micros: 0u64,
                    },
                }
            });
            // Wait 200 microseconds
            EHandle::local().set_fake_time(MonotonicInstant::from_nanos(1000 * 200));
            fuchsia_async::yield_now().await;
            assert_data_tree!(inspector, root: {
                root: {
                    // Our task
                    task_0:{
                        polls: 3u64,
                        completed: false,
                        max_poll_duration_micros: 200u64,
                    },
                }
            });
            let scope = fuchsia_async::Scope::new_with_name("test scope");
            let _scope_2 = fuchsia_async::Scope::new_with_name("test scope");
            let _scope_3 = fuchsia_async::Scope::new_with_name("test scope");

            let child_scope = scope.new_child_with_name("test child scope");
            let _child_task = child_scope.spawn(async move {});
            // Assert that we have a completed future with the correct hierarchy
            assert_data_tree!(inspector, root: {
                root: {
                    // Our task
                    task_0:{
                        polls: 3u64,
                        completed: false,
                        max_poll_duration_micros: 200u64,
                    },
                    "test scope":{
                        "test child scope":{
                            task_2:{
                                polls: 0u64,
                                completed: false,
                                max_poll_duration_micros: 0u64,
                            }
                        }
                    },
                    "test scope_1":{},
                    "test scope_2":{},
                }
            });
        });
        let _ = exec.run_until_stalled(&mut fut);
    }
}
