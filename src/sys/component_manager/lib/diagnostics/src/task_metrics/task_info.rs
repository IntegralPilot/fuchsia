// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::task_metrics::constants::{COMPONENT_CPU_MAX_SAMPLES, CPU_SAMPLE_PERIOD};
use crate::task_metrics::measurement::{Measurement, MeasurementsQueue};
use crate::task_metrics::runtime_stats_source::RuntimeStatsSource;
use fuchsia_async as fasync;
use fuchsia_inspect::{self as inspect, HistogramProperty, UintLinearHistogramProperty};
use fuchsia_sync::Mutex;
use injectable_time::TimeSource;
use log::debug;
use moniker::ExtendedMoniker;
use std::fmt::Debug;
use std::sync::{Arc, Weak};
use zx::sys::{self as zx_sys, zx_system_get_num_cpus};

pub(crate) fn create_cpu_histogram(
    node: &inspect::Node,
    moniker: &ExtendedMoniker,
) -> inspect::UintLinearHistogramProperty {
    node.create_uint_linear_histogram(
        moniker.to_string(),
        inspect::LinearHistogramParams { floor: 1, step_size: 1, buckets: 99 },
    )
}

fn num_cpus() -> i64 {
    // zx_system_get_num_cpus() is FFI to C++. It simply returns a value from a static struct
    // so it should always be safe to call.
    (unsafe { zx_system_get_num_cpus() }) as i64
}

#[derive(Debug)]
pub(crate) enum TaskState<T: RuntimeStatsSource + Debug> {
    TerminatedAndMeasured,
    Terminated(T),
    Alive(T),
}

impl<T> From<T> for TaskState<T>
where
    T: RuntimeStatsSource + Debug,
{
    fn from(task: T) -> TaskState<T> {
        TaskState::Alive(task)
    }
}

#[derive(Debug)]
pub(crate) struct TaskStats {
    pub(crate) has_parent_task: bool,
    pub(crate) measurements: MeasurementsQueue,
    pub(crate) exited_cpu: Option<Measurement>,
    previous_cpu: zx::MonotonicDuration,
    previous_histogram_timestamp: i64,
}

#[derive(Debug)]
pub struct TaskInfo<T: RuntimeStatsSource + Debug> {
    koid: zx_sys::zx_koid_t,
    pub(crate) task: Arc<Mutex<TaskState<T>>>,
    pub(crate) time_source: Arc<dyn TimeSource + Sync + Send>,
    histogram: Option<Arc<UintLinearHistogramProperty>>,
    cpu_cores: i64,
    sample_period: std::time::Duration,
    _terminated_task: fasync::Task<()>,
    pub(crate) most_recent_measurement_nanos: Arc<Mutex<Option<i64>>>,
    children: Mutex<Vec<Weak<TaskInfo<T>>>>,
    pub(crate) stats: Mutex<TaskStats>,
}

impl<T: 'static + RuntimeStatsSource + Debug + Send + Sync> TaskInfo<T> {
    /// Creates a new `TaskInfo` from the given cpu stats provider.
    // Due to https://github.com/rust-lang/rust/issues/50133 we cannot just derive TryFrom on a
    // generic type given a collision with the blanket implementation.
    pub fn try_from(
        task: T,
        histogram: Option<Arc<UintLinearHistogramProperty>>,
        time_source: Arc<dyn TimeSource + Sync + Send>,
    ) -> Result<Self, zx::Status> {
        Self::try_from_internal(task, histogram, time_source, CPU_SAMPLE_PERIOD, num_cpus())
    }
}

impl<T: 'static + RuntimeStatsSource + Debug + Send + Sync> TaskInfo<T> {
    // Injects a couple of test dependencies
    fn try_from_internal(
        task: T,
        histogram: Option<Arc<UintLinearHistogramProperty>>,
        time_source: Arc<dyn TimeSource + Sync + Send>,
        sample_period: std::time::Duration,
        cpu_cores: i64,
    ) -> Result<Self, zx::Status> {
        let koid = task.koid()?;
        let maybe_handle = task.handle_ref().duplicate_handle(zx::Rights::SAME_RIGHTS).ok();
        let task_state = Arc::new(Mutex::new(TaskState::from(task)));
        let weak_task_state = Arc::downgrade(&task_state);
        let most_recent_measurement_nanos = Arc::new(Mutex::new(None));
        let movable_most_recent_measurement_nanos = most_recent_measurement_nanos.clone();
        let movable_time_source = time_source.clone();
        let _terminated_task = fasync::Task::spawn(async move {
            if let Some(handle) = maybe_handle {
                fasync::OnSignals::new(&handle, zx::Signals::TASK_TERMINATED)
                    .await
                    .map(|_: fidl::Signals| ()) // Discard.
                    .unwrap_or_else(|error| debug!(error:%; "error creating signal handler"));
            }

            // If we failed to duplicate the handle then still mark this task as terminated to
            // ensure it's cleaned up.
            if let Some(task_state) = weak_task_state.upgrade() {
                {
                    let mut terminated_at_nanos_guard =
                        movable_most_recent_measurement_nanos.lock();
                    *terminated_at_nanos_guard = Some(movable_time_source.now());
                }
                let mut state = task_state.lock();
                *state = match std::mem::replace(&mut *state, TaskState::TerminatedAndMeasured) {
                    s @ TaskState::TerminatedAndMeasured => s,
                    TaskState::Alive(t) => TaskState::Terminated(t),
                    s @ TaskState::Terminated(_) => s,
                };
            }
        });

        let task_stats = TaskStats {
            has_parent_task: false,
            measurements: MeasurementsQueue::new(COMPONENT_CPU_MAX_SAMPLES, time_source.clone()),
            exited_cpu: None,
            previous_cpu: zx::MonotonicDuration::from_nanos(0),
            previous_histogram_timestamp: time_source.now(),
        };

        Ok(Self {
            koid,
            task: task_state,
            cpu_cores,
            sample_period,
            histogram,
            time_source,
            _terminated_task,
            most_recent_measurement_nanos,
            children: Mutex::new(vec![]),
            stats: Mutex::new(task_stats),
        })
    }

    /// Take a new measurement. If the handle of this task is invalid, then it keeps track of how
    /// many measurements would have been done. When the maximum amount allowed is hit, then it
    /// drops the oldest measurement.
    pub fn measure_if_no_parent(&self) -> Option<Measurement> {
        // Tasks with a parent are measured by the parent as done right below in the internal
        // `measure_subtree`.
        if self.stats.lock().has_parent_task {
            return None;
        }

        self.measure_subtree()
    }

    /// Adds a weak pointer to a task for which this task is the parent.
    /// Removes pointers if a child task has finished and died to avoid leaks.
    pub fn add_child(&self, task: Weak<TaskInfo<T>>) {
        let mut children = self.children.lock();
        children.retain(|child| child.strong_count() > 0);
        children.push(task);
    }

    pub fn most_recent_measurement(&self) -> Option<zx::BootInstant> {
        self.most_recent_measurement_nanos.lock().map(|t| zx::BootInstant::from_nanos(t))
    }

    /// Takes the MeasurementsQueue from this task, replacing it with an empty one.
    /// This function is only valid when `self.task == TaskState::Terminated*`.
    /// The task will be considered stale after this function runs.
    pub fn take_measurements_queue(&self) -> Result<MeasurementsQueue, ()> {
        let is_terminated = match &*self.task.lock() {
            TaskState::TerminatedAndMeasured | TaskState::Terminated(_) => true,
            _ => false,
        };

        if is_terminated {
            Ok(std::mem::replace(
                &mut self.stats.lock().measurements,
                MeasurementsQueue::new(COMPONENT_CPU_MAX_SAMPLES, self.time_source.clone()),
            ))
        } else {
            Err(())
        }
    }

    /// Take a zero-valued measurement at timestamp `t`.
    ///
    /// Specifically meant for the very first measurement taken.
    pub fn record_measurement_with_start_time(&self, t: zx::BootInstant) {
        let mut stats = self.stats.lock();
        self.record_measurement_internal(&mut stats, Measurement::empty(t));
    }

    fn record_measurement_internal(&self, stats: &mut TaskStats, m: Measurement) {
        let current_cpu = *m.cpu_time();

        if let Some(histogram) = &self.histogram {
            let time_value: i64 = m.timestamp().into_nanos();
            let elapsed_time = time_value - stats.previous_histogram_timestamp;
            stats.previous_histogram_timestamp = time_value;

            if elapsed_time >= ((self.sample_period.as_nanos() as i64) * 9 / 10) {
                let available_core_time = elapsed_time * self.cpu_cores;
                if available_core_time != 0 {
                    let cpu_time_delta = current_cpu - stats.previous_cpu;
                    let cpu_numerator =
                        (cpu_time_delta.into_nanos() as i64) * 100 + available_core_time - 1;
                    histogram.insert((cpu_numerator / available_core_time) as u64);
                }
            }
        }

        stats.previous_cpu = current_cpu;
        stats.measurements.insert(m);
    }

    fn measure_subtree<'a>(&'a self) -> Option<Measurement> {
        let (task_terminated_can_measure, runtime_info_res) = {
            let mut guard = self.task.lock();
            match &*guard {
                TaskState::TerminatedAndMeasured => {
                    // Use try_lock for stats to prevent blocking during measurement.
                    if let Some(mut stats_guard) = self.stats.try_lock() {
                        stats_guard.measurements.insert_post_invalidation();
                    }
                    return None;
                }
                TaskState::Terminated(task) => {
                    let result = task.get_runtime_info();
                    *guard = TaskState::TerminatedAndMeasured;

                    if let Some(mut term_guard) = self.most_recent_measurement_nanos.try_lock() {
                        *term_guard = Some(self.time_source.now());
                    }
                    (true, result)
                }
                TaskState::Alive(task) => (false, task.get_runtime_info()),
            }
        };

        if let Ok(runtime_info) = runtime_info_res {
            let mut measurement = Measurement::from_runtime_info(
                runtime_info,
                zx::BootInstant::from_nanos(self.time_source.now()),
            );

            let children_to_measure = {
                let mut children_guard = self.children.lock();
                std::mem::take(&mut *children_guard)
            };

            let mut alive_children = vec![];
            for weak_child in children_to_measure {
                if let Some(child) = weak_child.upgrade() {
                    if let Some(child_measurement) = child.measure_subtree() {
                        measurement -= &child_measurement;
                    }
                    if child.is_alive() {
                        alive_children.push(weak_child);
                    }
                }
            }

            self.children.lock().extend(alive_children);

            if let Some(mut stats_guard) = self.stats.try_lock() {
                self.record_measurement_internal(&mut stats_guard, measurement);

                if task_terminated_can_measure {
                    stats_guard.exited_cpu =
                        stats_guard.measurements.most_recent_measurement().cloned();
                    return None;
                }
                return stats_guard.measurements.most_recent_measurement().cloned();
            }
        }
        None
    }

    /// A task is alive when:
    /// - Its handle is valid, or
    /// - There's at least one measurement saved.
    pub fn is_alive(&self) -> bool {
        let task_state_terminated_and_measured =
            matches!(*self.task.lock(), TaskState::TerminatedAndMeasured);
        let task_has_real_measurements = !self.stats.lock().measurements.no_true_measurements();

        !task_state_terminated_and_measured || task_has_real_measurements
    }

    pub fn exited_cpu(&self) -> Option<Measurement> {
        self.stats.lock().exited_cpu.clone()
    }

    /// Writes the task measurements under the given inspect node `parent`.
    pub fn record_to_node(&self, parent: &inspect::Node) {
        let node = parent.create_child(self.koid.to_string());
        self.stats.lock().measurements.record_to_node(&node);
        parent.record(node);
    }

    pub fn koid(&self) -> zx_sys::zx_koid_t {
        self.koid
    }

    #[cfg(test)]
    pub fn total_measurements(&self) -> usize {
        self.stats.lock().measurements.true_measurement_count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task_metrics::testing::FakeTask;
    use assert_matches::assert_matches;
    use diagnostics_assertions::assert_data_tree;
    use diagnostics_hierarchy::{ArrayContent, DiagnosticsHierarchyGetter, LinearHistogram};
    use injectable_time::FakeTime;

    fn take_measurement_then_tick_clock<
        'a,
        T: 'static + RuntimeStatsSource + Debug + Send + Sync,
    >(
        ti: &'a mut TaskInfo<T>,
        clock: &Arc<FakeTime>,
    ) -> Option<Measurement> {
        let m = ti.measure_if_no_parent();
        clock.add_ticks(CPU_SAMPLE_PERIOD.as_nanos() as i64);
        m
    }

    #[fuchsia::test]
    async fn rotates_measurements_per_task() {
        // Set up test
        let clock = Arc::new(FakeTime::new());
        let mut task: TaskInfo<FakeTask> =
            TaskInfo::try_from(FakeTask::default(), None /* histogram */, clock.clone()).unwrap();
        assert!(task.is_alive());

        // Take three measurements.
        take_measurement_then_tick_clock(&mut task, &clock);
        assert_eq!(task.stats.lock().measurements.true_measurement_count(), 1);
        take_measurement_then_tick_clock(&mut task, &clock);
        assert_eq!(task.stats.lock().measurements.true_measurement_count(), 2);
        take_measurement_then_tick_clock(&mut task, &clock);
        assert!(task.is_alive());
        assert_eq!(task.stats.lock().measurements.true_measurement_count(), 3);

        // Terminate the task
        task.force_terminate().await;

        // This will perform the post-termination measurement and bring the state to terminated and
        // measured.
        take_measurement_then_tick_clock(&mut task, &clock);
        assert_eq!(task.stats.lock().measurements.true_measurement_count(), 4);
        assert_matches!(*task.task.lock(), TaskState::TerminatedAndMeasured);

        for _ in 4..COMPONENT_CPU_MAX_SAMPLES {
            take_measurement_then_tick_clock(&mut task, &clock);
            assert_eq!(task.stats.lock().measurements.true_measurement_count(), 4);
        }

        take_measurement_then_tick_clock(&mut task, &clock); // 1 dropped, 3 left
        assert!(task.is_alive());
        assert_eq!(task.stats.lock().measurements.true_measurement_count(), 3);
        take_measurement_then_tick_clock(&mut task, &clock); // 2 dropped, 2 left
        assert!(task.is_alive());
        assert_eq!(task.stats.lock().measurements.true_measurement_count(), 2);
        take_measurement_then_tick_clock(&mut task, &clock); // 3 dropped, 1 left
        assert!(task.is_alive());
        assert_eq!(task.stats.lock().measurements.true_measurement_count(), 1);

        // Take one last measure.
        take_measurement_then_tick_clock(&mut task, &clock); // 4 dropped, 0 left
        assert!(!task.is_alive());
        assert_eq!(task.stats.lock().measurements.true_measurement_count(), 0);
    }

    #[fuchsia::test]
    async fn write_inspect() {
        let time = Arc::new(FakeTime::new());
        let task = TaskInfo::try_from(
            FakeTask::new(
                1,
                vec![
                    zx::TaskRuntimeInfo {
                        cpu_time: 2,
                        queue_time: 4,
                        ..zx::TaskRuntimeInfo::default()
                    },
                    zx::TaskRuntimeInfo {
                        cpu_time: 6,
                        queue_time: 8,
                        ..zx::TaskRuntimeInfo::default()
                    },
                ],
            ),
            None, /* histogram */
            time.clone(),
        )
        .unwrap();

        time.set_ticks(1);
        task.measure_if_no_parent();
        time.set_ticks(2);
        task.measure_if_no_parent();

        let inspector = inspect::Inspector::default();
        task.record_to_node(inspector.root());
        assert_data_tree!(inspector, root: {
            "1": {
                timestamps: vec![1i64, 2],
                cpu_times: vec![2i64, 6],
                queue_times: vec![4i64, 8],
            }
        });
    }

    #[fuchsia::test]
    async fn write_more_than_max_samples() {
        let inspector = inspect::Inspector::default();
        let clock = Arc::new(FakeTime::new());
        let mut task = TaskInfo::try_from(
            FakeTask::new(
                1,
                vec![
                    zx::TaskRuntimeInfo {
                        cpu_time: 2,
                        queue_time: 4,
                        ..zx::TaskRuntimeInfo::default()
                    },
                    zx::TaskRuntimeInfo {
                        cpu_time: 6,
                        queue_time: 8,
                        ..zx::TaskRuntimeInfo::default()
                    },
                ],
            ),
            None, /* histogram */
            clock.clone(),
        )
        .unwrap();

        for _ in 0..(COMPONENT_CPU_MAX_SAMPLES + 10) {
            assert!(take_measurement_then_tick_clock(&mut task, &clock).is_some());
        }

        assert_eq!(
            task.stats.lock().measurements.true_measurement_count(),
            COMPONENT_CPU_MAX_SAMPLES
        );
        task.record_to_node(inspector.root());
        assert_eq!(60, COMPONENT_CPU_MAX_SAMPLES);
        assert_eq!(task.stats.lock().measurements.true_measurement_count(), 60);

        let hierarchy = inspector.get_diagnostics_hierarchy().await;
        for top_level in &hierarchy.children {
            let child = hierarchy.get_child(&top_level.name).unwrap();
            let timestamps = child.get_property("timestamps").unwrap().int_array().unwrap();
            assert_eq!(timestamps.len(), COMPONENT_CPU_MAX_SAMPLES);
            let cpu_times = child.get_property("cpu_times").unwrap().int_array().unwrap();
            assert_eq!(cpu_times.len(), COMPONENT_CPU_MAX_SAMPLES);
            let queue_times = child.get_property("queue_times").unwrap().int_array().unwrap();
            assert_eq!(queue_times.len(), COMPONENT_CPU_MAX_SAMPLES);
        }
    }

    #[fuchsia::test]
    async fn more_than_max_samples_offset_time() {
        let inspector = inspect::Inspector::default();
        let clock = Arc::new(FakeTime::new());
        let mut task = TaskInfo::try_from(
            FakeTask::new(
                1,
                vec![
                    zx::TaskRuntimeInfo {
                        cpu_time: 2,
                        queue_time: 4,
                        ..zx::TaskRuntimeInfo::default()
                    },
                    zx::TaskRuntimeInfo {
                        cpu_time: 6,
                        queue_time: 8,
                        ..zx::TaskRuntimeInfo::default()
                    },
                ],
            ),
            None, /* histogram */
            clock.clone(),
        )
        .unwrap();

        for _ in 0..COMPONENT_CPU_MAX_SAMPLES {
            assert!(take_measurement_then_tick_clock(&mut task, &clock).is_some());
        }

        task.measure_if_no_parent();

        // this will create >COMPONENT_CPU_MAX_SAMPLES within the max duration of one hour,
        // but should still cause eviction
        clock.add_ticks((CPU_SAMPLE_PERIOD - std::time::Duration::from_secs(1)).as_nanos() as i64);
        task.measure_if_no_parent();

        assert_eq!(
            task.stats.lock().measurements.true_measurement_count(),
            COMPONENT_CPU_MAX_SAMPLES
        );
        task.record_to_node(inspector.root());
    }

    #[fuchsia::test]
    async fn measure_with_children() {
        let clock = Arc::new(FakeTime::new());
        let mut task = TaskInfo::try_from(
            FakeTask::new(
                1,
                vec![
                    zx::TaskRuntimeInfo {
                        cpu_time: 100,
                        queue_time: 200,
                        ..zx::TaskRuntimeInfo::default()
                    },
                    zx::TaskRuntimeInfo {
                        cpu_time: 300,
                        queue_time: 400,
                        ..zx::TaskRuntimeInfo::default()
                    },
                ],
            ),
            None, /* histogram */
            clock.clone(),
        )
        .unwrap();

        let child_1 = Arc::new(
            TaskInfo::try_from(
                FakeTask::new(
                    2,
                    vec![
                        zx::TaskRuntimeInfo {
                            cpu_time: 10,
                            queue_time: 20,
                            ..zx::TaskRuntimeInfo::default()
                        },
                        zx::TaskRuntimeInfo {
                            cpu_time: 30,
                            queue_time: 40,
                            ..zx::TaskRuntimeInfo::default()
                        },
                    ],
                ),
                None, /* histogram */
                clock.clone(),
            )
            .unwrap(),
        );

        let child_2 = Arc::new(
            TaskInfo::try_from(
                FakeTask::new(
                    3,
                    vec![
                        zx::TaskRuntimeInfo {
                            cpu_time: 5,
                            queue_time: 2,
                            ..zx::TaskRuntimeInfo::default()
                        },
                        zx::TaskRuntimeInfo {
                            cpu_time: 15,
                            queue_time: 4,
                            ..zx::TaskRuntimeInfo::default()
                        },
                    ],
                ),
                None, /* histogram */
                clock.clone(),
            )
            .unwrap(),
        );

        task.add_child(Arc::downgrade(&child_1));
        task.add_child(Arc::downgrade(&child_2));

        {
            let measurement = take_measurement_then_tick_clock(&mut task, &clock).unwrap();
            assert_eq!(measurement.cpu_time().into_nanos(), 100 - 10 - 5);
            assert_eq!(measurement.queue_time().into_nanos(), 200 - 20 - 2);
        }
        assert_eq!(child_1.total_measurements(), 1);
        assert_eq!(child_2.total_measurements(), 1);

        // Fake child 2 not being alive anymore. It should be removed.
        {
            *child_2.task.lock() = TaskState::TerminatedAndMeasured;
            child_2.stats.lock().measurements =
                MeasurementsQueue::new(COMPONENT_CPU_MAX_SAMPLES, clock.clone());
        }

        assert_eq!(task.children.lock().len(), 2);
        {
            let measurement = take_measurement_then_tick_clock(&mut task, &clock).unwrap();
            assert_eq!(measurement.cpu_time().into_nanos(), 300 - 30);
            assert_eq!(measurement.queue_time().into_nanos(), 400 - 40);
        }

        assert_eq!(task.children.lock().len(), 1); // after measuring dead children are cleaned.
        assert_eq!(child_1.total_measurements(), 2);
    }

    type BucketPairs = Vec<(i64, i64)>;

    use diagnostics_hierarchy::Property;

    // Returns a list of <bucket index, count> for linear histogram buckets where count > 0.
    async fn linear_histogram_non_zero_values(inspector: &inspect::Inspector) -> BucketPairs {
        let mut output = vec![];
        let hierarchy = inspector.get_diagnostics_hierarchy().await;
        let histogram = hierarchy.get_property_by_path(&["foo"]).unwrap();
        if let Property::UintArray(_, data) = histogram {
            if let ArrayContent::LinearHistogram(LinearHistogram { counts, indexes, .. }) = data {
                match indexes {
                    None => {
                        for (index, count) in counts.iter().enumerate() {
                            if *count > 0 && *count <= i64::MAX as u64 {
                                output.push((index as i64, *count as i64));
                            }
                        }
                    }
                    Some(indexes) => {
                        for (index, count) in indexes.iter().zip(counts.iter()) {
                            if *count > 0
                                && *count <= i64::MAX as u64
                                && *index <= i64::MAX as usize
                            {
                                output.push((*index as i64, *count as i64));
                            }
                        }
                    }
                }
            }
        }
        output
    }

    fn fake_readings(id: u64, cpu_deltas: Vec<u64>) -> FakeTask {
        let mut cpu_time = 0i64;
        let mut readings = vec![];
        for delta in cpu_deltas.iter() {
            cpu_time += *delta as i64;
            readings.push(zx::TaskRuntimeInfo { cpu_time, ..zx::TaskRuntimeInfo::default() })
        }
        FakeTask::new(id, readings)
    }

    // Test that the ceil function works: 0 cpu goes in bucket 0, 0.1..1 in bucket 1, etc.
    #[fuchsia::test]
    async fn bucket_cutoffs() {
        let readings = fake_readings(1, vec![1, 0, 500, 989, 990, 991, 999, 0]);
        let inspector = inspect::Inspector::default();
        let clock = FakeTime::new();
        let histogram =
            create_cpu_histogram(&inspector.root(), &ExtendedMoniker::parse_str("foo").unwrap());
        //assert_data_tree!(            inspector,            root: {});
        let task = TaskInfo::try_from_internal(
            readings,
            Some(Arc::new(histogram)),
            Arc::new(clock.clone()),
            std::time::Duration::from_nanos(1000),
            1, /* cores */
        )
        .unwrap();

        clock.add_ticks(1000);
        task.measure_if_no_parent(); // 1
        let answer = vec![(1, 1)];
        assert_eq!(linear_histogram_non_zero_values(&inspector).await, answer);

        clock.add_ticks(1000);
        task.measure_if_no_parent(); // 0
        let answer = vec![(0, 1), (1, 1)];
        assert_eq!(linear_histogram_non_zero_values(&inspector).await, answer);

        clock.add_ticks(1000);
        task.measure_if_no_parent(); // 500
        let answer = vec![(0, 1), (1, 1), (50, 1)];
        assert_eq!(linear_histogram_non_zero_values(&inspector).await, answer);

        clock.add_ticks(1000);
        task.measure_if_no_parent(); // 989
        let answer = vec![(0, 1), (1, 1), (50, 1), (99, 1)];
        assert_eq!(linear_histogram_non_zero_values(&inspector).await, answer);

        clock.add_ticks(1000);
        task.measure_if_no_parent(); // 990
        let answer = vec![(0, 1), (1, 1), (50, 1), (99, 2)];
        assert_eq!(linear_histogram_non_zero_values(&inspector).await, answer);

        clock.add_ticks(1000);
        task.measure_if_no_parent(); // 991
        let answer = vec![(0, 1), (1, 1), (50, 1), (99, 2), (100, 1)];
        assert_eq!(linear_histogram_non_zero_values(&inspector).await, answer);

        clock.add_ticks(1000);
        task.measure_if_no_parent(); // 999
        let answer = vec![(0, 1), (1, 1), (50, 1), (99, 2), (100, 2)];
        assert_eq!(linear_histogram_non_zero_values(&inspector).await, answer);

        clock.add_ticks(1000);
        task.measure_if_no_parent(); // 0...
        let answer = vec![(0, 2), (1, 1), (50, 1), (99, 2), (100, 2)];
        assert_eq!(linear_histogram_non_zero_values(&inspector).await, answer);
    }

    // Test that short time intervals (less than 90% of sample_period) are discarded.
    // Extra-long intervals should be recorded. In all cases, CPU % should be calculated over the
    // actual interval, not the sample_period.
    #[fuchsia::test]
    async fn discard_short_intervals() {
        let readings = fake_readings(1, vec![100, 100, 100, 100]);
        let inspector = inspect::Inspector::default();
        let clock = FakeTime::new();
        let histogram =
            create_cpu_histogram(&inspector.root(), &ExtendedMoniker::parse_str("foo").unwrap());
        let task = TaskInfo::try_from_internal(
            readings,
            Some(Arc::new(histogram)),
            Arc::new(clock.clone()),
            std::time::Duration::from_nanos(1000),
            1, /* cores */
        )
        .unwrap();

        assert_eq!(linear_histogram_non_zero_values(&inspector).await, vec![]);

        clock.add_ticks(900);
        task.measure_if_no_parent();
        assert_eq!(linear_histogram_non_zero_values(&inspector).await, vec![(12, 1)]);

        clock.add_ticks(899);
        task.measure_if_no_parent();
        assert_eq!(linear_histogram_non_zero_values(&inspector).await, vec![(12, 1)]); // No change

        clock.add_ticks(2000);
        task.measure_if_no_parent();
        assert_eq!(linear_histogram_non_zero_values(&inspector).await, (vec![(5, 1), (12, 1)]));

        clock.add_ticks(1000);
        task.measure_if_no_parent();
        assert_eq!(
            linear_histogram_non_zero_values(&inspector).await,
            (vec![(5, 1), (10, 1), (12, 1)])
        );
    }

    // Test that the CPU% takes the number of cores into account - that is, with N cores
    // the CPU% should be 1/N the amount it would be for 1 core.
    #[fuchsia::test]
    async fn divide_by_cores() {
        let readings = fake_readings(1, vec![400]);
        let inspector = inspect::Inspector::default();
        let clock = FakeTime::new();
        let histogram =
            create_cpu_histogram(&inspector.root(), &ExtendedMoniker::parse_str("foo").unwrap());
        let task = TaskInfo::try_from_internal(
            readings,
            Some(Arc::new(histogram)),
            Arc::new(clock.clone()),
            std::time::Duration::from_nanos(1000),
            4, /* cores */
        )
        .unwrap();

        assert_eq!(linear_histogram_non_zero_values(&inspector).await, vec![]);

        clock.add_ticks(1000);
        task.measure_if_no_parent();
        assert_eq!(linear_histogram_non_zero_values(&inspector).await, vec![(10, 1)]);
    }
}
