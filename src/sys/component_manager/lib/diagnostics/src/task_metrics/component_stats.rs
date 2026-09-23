// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::task_metrics::measurement::Measurement;
use crate::task_metrics::runtime_stats_source::RuntimeStatsSource;
use crate::task_metrics::task_info::TaskInfo;
use fuchsia_inspect as inspect;
use std::fmt::Debug;
use std::sync::Arc;

/// Tracks the tasks associated to some component and provides utilities for measuring them.
pub struct ComponentStats<T: RuntimeStatsSource + Debug> {
    tasks: Vec<Arc<TaskInfo<T>>>,
    histogram: Arc<inspect::UintLinearHistogramProperty>,
}

impl<T: 'static + RuntimeStatsSource + Debug + Send + Sync> ComponentStats<T> {
    /// Creates a new `ComponentStats` and starts taking measurements.
    pub fn new(histogram: inspect::UintLinearHistogramProperty) -> Self {
        Self { tasks: vec![], histogram: Arc::new(histogram) }
    }

    pub fn histogram(&self) -> Arc<inspect::UintLinearHistogramProperty> {
        self.histogram.clone()
    }

    /// Associate a task with this component.
    pub fn add_task(&mut self, task: Arc<TaskInfo<T>>) {
        self.tasks.push(task);
    }

    /// A `ComponentStats` is alive when:
    /// - It has not started measuring yet: this means we are still waiting for the diagnostics
    ///   data to arrive from the runner, or
    /// - Any of its tasks are alive.
    pub fn is_alive(&self) -> bool {
        self.tasks.iter().any(|task| task.is_alive())
    }

    /// Takes a runtime info measurement and records it. Drops old ones if the maximum amount is
    /// exceeded.
    ///
    /// Applies to tasks which have TaskState::Alive or TaskState::Terminated.
    pub fn measure(&self) -> Measurement {
        let mut result = Measurement::default();
        for task in self.tasks.iter() {
            if let Some(measurement) = task.measure_if_no_parent() {
                result += &measurement;
            }
        }

        result
    }

    /// This produces measurements for tasks which have TaskState::TerminatedAndMeasured
    /// but also have measurement data for the past hour.
    pub fn measure_tracked_dead_tasks(&self) -> Measurement {
        let mut result = Measurement::default();

        for task in self.tasks.iter() {
            let locked_task = task.stats.lock();

            // this implies that `clean_stale()` will take the measurement
            if locked_task.measurements.no_true_measurements() {
                continue;
            }

            if let Some(m) = locked_task.exited_cpu.clone() {
                result += &m;
            }
        }

        result
    }

    /// Removes all tasks that are not alive.
    ///
    /// Returns the koids of the ones that were deleted and the sum of the final measurements
    /// of the dead tasks. The measurement produced is of Tasks with
    /// TaskState::TerminatedAndMeasured.
    pub fn clean_stale(&mut self) -> (Vec<zx::sys::zx_koid_t>, Measurement) {
        let mut deleted_koids = vec![];
        let mut exited_cpu_time = Measurement::default();

        // Grab the old list, leaving self.tasks empty
        let old_tasks = std::mem::take(&mut self.tasks);

        for task in old_tasks {
            if task.is_alive() {
                self.tasks.push(task);
            } else {
                deleted_koids.push(task.koid());
                if let Some(m) = task.exited_cpu() {
                    exited_cpu_time += &m;
                }
            }
        }

        (deleted_koids, exited_cpu_time)
    }

    pub fn remove_by_koids(&mut self, remove: &[zx::sys::zx_koid_t]) {
        // Keeps the task only if its koid is NOT in the `remove` list
        self.tasks.retain(|task| !remove.contains(&task.koid()));
    }

    pub fn gather_dead_tasks(&self) -> Vec<(zx::BootInstant, Arc<TaskInfo<T>>)> {
        let mut dead_tasks = Vec::with_capacity(self.tasks.len());
        for task in &self.tasks {
            if let Some(t) = task.most_recent_measurement() {
                dead_tasks.push((t, task.clone()));
            }
        }
        dead_tasks.shrink_to_fit();

        dead_tasks
    }

    /// Writes the stats to inspect under the given node. Returns the number of tasks that were
    /// written.
    pub fn record_to_node(&self, node: &inspect::Node) -> u64 {
        for task in &self.tasks {
            task.record_to_node(&node);
        }
        self.tasks.len() as u64
    }

    #[cfg(test)]
    pub fn total_measurements(&self) -> usize {
        let mut sum = 0;
        for task in &self.tasks {
            sum += task.total_measurements();
        }
        sum
    }

    #[cfg(test)]
    pub fn tasks(&self) -> &[Arc<TaskInfo<T>>] {
        &self.tasks
    }

    #[cfg(test)]
    pub fn tasks_mut(&mut self) -> &mut [Arc<TaskInfo<T>>] {
        &mut self.tasks
    }
}
