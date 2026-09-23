// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::task_metrics::component_stats::ComponentStats;
use crate::task_metrics::constants::*;
use crate::task_metrics::measurement::{Measurement, MeasurementsQueue};
use crate::task_metrics::runtime_stats_source::{
    ComponentStartedInfo, RuntimeStatsContainer, RuntimeStatsSource,
};
use crate::task_metrics::task_info::{TaskInfo, create_cpu_histogram};
use async_trait::async_trait;
use errors::ModelError;
use fidl_fuchsia_component_runner::Task as DiagnosticsTask;
use fuchsia_async as fasync;
use fuchsia_inspect::{self as inspect, ArrayProperty, HistogramProperty};
use fuchsia_sync::Mutex;
use futures::channel::{mpsc, oneshot};
use futures::{FutureExt, StreamExt};
use hooks::{Event, EventPayload, EventType, HasEventType, Hook, HooksRegistration};
use injectable_time::{BootInstant, TimeSource};
use log::warn;
use moniker::{ExtendedMoniker, Moniker};
use std::collections::{BTreeMap, VecDeque};
use std::fmt::Debug;
use std::sync::{Arc, Weak};
use zx::sys as zx_sys;

macro_rules! maybe_return {
    ($e:expr) => {
        match $e {
            None => return,
            Some(v) => v,
        }
    };
}

const MAX_INSPECT_SIZE : usize = 2 * 1024 * 1024 /* 2MB */;

const AGGREGATE_SAMPLES: &'static str = "@aggregated";

/// Provides stats for all components running in the system.
pub struct ComponentTreeStats<T: RuntimeStatsSource + Debug> {
    /// Map from a moniker of a component running in the system to its stats.
    tree: Mutex<BTreeMap<ExtendedMoniker, Arc<Mutex<ComponentStats<T>>>>>,

    /// Stores all the tasks we know about. This provides direct access for updating a task's
    /// children.
    tasks: Mutex<BTreeMap<zx_sys::zx_koid_t, Weak<TaskInfo<T>>>>,

    /// The root of the tree stats.
    node: inspect::Node,

    /// The node under which CPU usage histograms will be stored.
    histograms_node: inspect::Node,

    /// A histogram storing stats about the time it took to process the CPU stats measurements.
    processing_times: inspect::IntExponentialHistogramProperty,

    /// The task that takes CPU samples every minute.
    sampler_task: Mutex<Option<fasync::Task<()>>>,

    /// Aggregated CPU stats.
    totals: Mutex<AggregatedStats>,

    _wait_diagnostics_drain: fasync::Task<()>,

    diagnostics_waiter_task_sender: mpsc::UnboundedSender<fasync::Task<()>>,

    time_source: Arc<dyn TimeSource + Send + Sync>,

    /// A queue of data taken from tasks which have been terminated.
    /// If the ComponentTreeStats object has too many dead tasks, it will begin to drop
    /// the individual `TaskInfo<T>` objects and aggregate their data into this queue.
    aggregated_dead_task_data: Mutex<MeasurementsQueue>,

    /// Cumulative CPU time of tasks that are terminated.
    exited_measurements: Mutex<Measurement>,
}

impl<T: 'static + RuntimeStatsSource + Debug + Send + Sync> ComponentTreeStats<T> {
    pub fn new(node: inspect::Node) -> Arc<Self> {
        Self::new_with_timesource(node, Arc::new(BootInstant::new()))
    }

    fn new_with_timesource(
        node: inspect::Node,
        time_source: Arc<dyn TimeSource + Send + Sync>,
    ) -> Arc<Self> {
        let processing_times = node.create_int_exponential_histogram(
            "processing_times_ns",
            inspect::ExponentialHistogramParams {
                floor: 1000,
                initial_step: 1000,
                step_multiplier: 2,
                buckets: 16,
            },
        );

        let histograms_node = node.create_child("histograms");
        let totals = Mutex::new(AggregatedStats::new());
        let (snd, rcv) = mpsc::unbounded();
        let this = Arc::new(Self {
            tree: Mutex::new(BTreeMap::new()),
            tasks: Mutex::new(BTreeMap::new()),
            node,
            histograms_node,
            processing_times,
            sampler_task: Mutex::new(None),
            totals,
            diagnostics_waiter_task_sender: snd,
            _wait_diagnostics_drain: fasync::Task::spawn(async move {
                rcv.for_each_concurrent(None, |rx| async move { rx.await }).await;
            }),
            time_source: time_source.clone(),
            aggregated_dead_task_data: Mutex::new(MeasurementsQueue::new(
                COMPONENT_CPU_MAX_SAMPLES,
                time_source,
            )),
            exited_measurements: Mutex::new(Measurement::default()),
        });

        let weak_self = Arc::downgrade(&this);

        let weak_self_for_fut = weak_self.clone();
        this.node.record_lazy_child("measurements", move || {
            let weak_self_clone = weak_self_for_fut.clone();
            async move {
                if let Some(this) = weak_self_clone.upgrade() {
                    Ok(this.write_measurements_to_inspect())
                } else {
                    Ok(inspect::Inspector::default())
                }
            }
            .boxed()
        });

        let weak_self_clone_for_fut = weak_self.clone();
        this.node.record_lazy_child("recent_usage", move || {
            let weak_self_clone = weak_self_clone_for_fut.clone();
            async move {
                if let Some(this) = weak_self_clone.upgrade() {
                    Ok(this.write_recent_usage_to_inspect())
                } else {
                    Ok(inspect::Inspector::default())
                }
            }
            .boxed()
        });
        let weak_self_for_fut = weak_self;
        this.node.record_lazy_child("@total", move || {
            let weak_self_clone = weak_self_for_fut.clone();
            async move {
                if let Some(this) = weak_self_clone.upgrade() {
                    Ok(this.write_totals_to_inspect())
                } else {
                    Ok(inspect::Inspector::default())
                }
            }
            .boxed()
        });

        this
    }

    /// Perform an initial measurement followed by spawning a task that will perform a measurement
    /// every `CPU_SAMPLE_PERIOD` seconds.
    pub fn start_measuring(self: &Arc<Self>) {
        let weak_self = Arc::downgrade(self);
        self.measure();
        *(self.sampler_task.lock()) = Some(fasync::Task::spawn(async move {
            loop {
                fasync::Timer::new(CPU_SAMPLE_PERIOD).await;
                match weak_self.upgrade() {
                    None => break,
                    Some(this) => {
                        this.measure();
                    }
                }
            }
        }));
    }

    /// Initializes a new component stats with the given task.
    fn track_ready(&self, moniker: ExtendedMoniker, task: T) {
        let stats = {
            let mut tree_guard = self.tree.lock();
            tree_guard
                .entry(moniker.clone())
                .or_insert_with(|| {
                    let histogram = create_cpu_histogram(&self.histograms_node, &moniker);
                    Arc::new(Mutex::new(ComponentStats::new(histogram)))
                })
                .clone()
        };

        let histogram = stats.lock().histogram();

        if let Ok(task_info) = TaskInfo::try_from(task, Some(histogram), self.time_source.clone()) {
            let koid = task_info.koid();
            let arc_task_info = Arc::new(task_info);

            let mut tasks_guard = self.tasks.lock();

            stats.lock().add_task(arc_task_info.clone());

            tasks_guard.insert(koid, Arc::downgrade(&arc_task_info));
        }
    }

    fn write_measurements_to_inspect(self: &Arc<Self>) -> inspect::Inspector {
        let inspector =
            inspect::Inspector::new(inspect::InspectorConfig::default().size(MAX_INSPECT_SIZE));
        let components = inspector.root().create_child("components");
        let (component_count, task_count) = self.write_measurements(&components);
        self.write_aggregate_measurements(&components);
        inspector.root().record_uint("component_count", component_count);
        inspector.root().record_uint("task_count", task_count);
        inspector.root().record(components);

        let stats_node = inspect::stats::StatsNode::new(&inspector);
        stats_node.record_data_to(inspector.root());

        inspector
    }

    fn write_recent_usage_to_inspect(self: &Arc<Self>) -> inspect::Inspector {
        let inspector = inspect::Inspector::default();
        self.totals.lock().write_recents_to(inspector.root());
        inspector
    }

    fn write_totals_to_inspect(self: &Arc<Self>) -> inspect::Inspector {
        let inspector = inspect::Inspector::default();
        self.totals.lock().write_totals_to(inspector.root());
        inspector
    }

    fn write_aggregate_measurements(&self, components_node: &inspect::Node) {
        let locked_aggregate = self.aggregated_dead_task_data.lock();
        if locked_aggregate.no_true_measurements() {
            return;
        }

        let aggregate = components_node.create_child(&*AGGREGATE_SAMPLES);
        locked_aggregate.record_to_node(&aggregate);
        components_node.record(aggregate);
    }

    fn write_measurements(&self, node: &inspect::Node) -> (u64, u64) {
        let mut task_count = 0;
        let tree = self.tree.lock();
        for (moniker, stats) in tree.iter() {
            let stats_guard = stats.lock();
            let key = match moniker {
                ExtendedMoniker::ComponentManager => moniker.to_string(),
                ExtendedMoniker::ComponentInstance(m) => {
                    if *m == Moniker::root() {
                        "<root>".to_string()
                    } else {
                        m.to_string()
                    }
                }
            };
            let child = node.create_child(key);
            task_count += stats_guard.record_to_node(&child);
            node.record(child);
        }
        (tree.len() as u64, task_count)
    }

    /// Takes a measurement of all tracked tasks and updated the totals. If any task is not alive
    /// anymore it deletes it. If any component is not alive any more and no more historical
    /// measurements are available for it, deletes it too.
    pub fn measure(self: &Arc<Self>) {
        let start = zx::BootInstant::get();

        // Copy the stats and release the lock.
        let stats = self
            .tree
            .lock()
            .iter()
            .map(|(k, v)| (k.clone(), Arc::downgrade(&v)))
            .collect::<Vec<_>>();

        let mut aggregated = Measurement::clone_with_time(&*self.exited_measurements.lock(), start);
        let mut stats_to_remove = vec![];
        let mut koids_to_remove = vec![];

        for (moniker, weak_stats) in stats.into_iter() {
            if let Some(stats) = weak_stats.upgrade() {
                let mut stat_guard = stats.lock();
                // Order is important: measure, then measure_tracked_dead_tasks, then clean_stale
                aggregated += &stat_guard.measure();
                aggregated += &stat_guard.measure_tracked_dead_tasks();
                let (mut stale_koids, exited_cpu_of_deleted) = stat_guard.clean_stale();
                aggregated += &exited_cpu_of_deleted;

                *self.exited_measurements.lock() += &exited_cpu_of_deleted;

                koids_to_remove.append(&mut stale_koids);
                if !stat_guard.is_alive() {
                    stats_to_remove.push(moniker);
                }
            }
        }

        // Lock the tree so that we ensure no modifications are made while we are deleting
        let mut stats = self.tree.lock();
        for moniker in stats_to_remove {
            // Ensure that they are still not alive (if a component restarted it might be alive
            // again).
            if let Some(stat) = stats.get(&moniker) {
                if !stat.lock().is_alive() {
                    stats.remove(&moniker);
                }
            }
        }

        let mut tasks = self.tasks.lock();
        for koid in koids_to_remove {
            tasks.remove(&koid);
        }

        self.totals.lock().insert(aggregated);
        self.processing_times.insert((zx::BootInstant::get() - start).into_nanos());
    }

    fn prune_dead_tasks(self: &Arc<Self>, max_dead_tasks: usize) {
        let mut all_dead_tasks = BTreeMap::new();
        for (_moniker, component) in self.tree.lock().iter() {
            let dead_tasks = component.lock().gather_dead_tasks();
            for (timestamp, task) in dead_tasks {
                all_dead_tasks.insert(timestamp, task);
            }
        }

        if all_dead_tasks.len() <= max_dead_tasks {
            return;
        }

        let remove_count = all_dead_tasks.len() - (max_dead_tasks / 2);
        let to_remove = all_dead_tasks.iter().take(remove_count);

        let mut koids_to_remove = Vec::with_capacity(remove_count);

        for (_, task) in to_remove {
            if let Ok(measurements) = task.take_measurements_queue() {
                koids_to_remove.push(task.koid());
                *self.aggregated_dead_task_data.lock() += measurements;
            }
        }

        self.tree.lock().retain(|_, stats| {
            let mut stat_guard = stats.lock();
            stat_guard.remove_by_koids(&koids_to_remove);
            stat_guard.is_alive()
        });

        let mut tasks = self.tasks.lock();
        for koid in &koids_to_remove {
            tasks.remove(koid);
        }
    }

    fn on_component_started<P, C>(self: &Arc<Self>, moniker: &Moniker, runtime: &P)
    where
        P: ComponentStartedInfo<C, T>,
        C: RuntimeStatsContainer<T> + Send + Sync + 'static,
    {
        if let Some(receiver) = runtime.get_receiver() {
            let task = fasync::Task::spawn(Self::diagnostics_waiter_task(
                Arc::downgrade(&self),
                moniker.clone().into(),
                receiver,
                runtime.start_time(),
            ));
            let _ = self.diagnostics_waiter_task_sender.unbounded_send(task);
        }
    }

    async fn diagnostics_waiter_task<C>(
        weak_self: Weak<Self>,
        moniker: ExtendedMoniker,
        receiver: oneshot::Receiver<C>,
        start_time: zx::BootInstant,
    ) where
        C: RuntimeStatsContainer<T> + Send + Sync + 'static,
    {
        let mut source = maybe_return!(receiver.await.ok());
        let this = maybe_return!(weak_self.upgrade());

        let stats = {
            let mut tree_lock = this.tree.lock();
            if let Some(stats) = tree_lock.get(&moniker) {
                stats.clone()
            } else {
                let histogram = create_cpu_histogram(&this.histograms_node, &moniker);
                let stats = Arc::new(Mutex::new(ComponentStats::new(histogram)));
                tree_lock.insert(moniker.clone(), stats.clone());
                stats
            }
        };

        let task = maybe_return!(source.take_component_task());

        let histogram = stats.lock().histogram();
        let task_info =
            maybe_return!(TaskInfo::try_from(task, Some(histogram), this.time_source.clone()).ok());

        let parent_koid = source
            .take_parent_task()
            .and_then(|task| TaskInfo::try_from(task, None, this.time_source.clone()).ok())
            .map(|task| task.koid());

        let koid = task_info.koid();

        // At this point we haven't set the parent yet.
        // We take two types of initial measurement for the task:
        //  1) a zero-valued measurement that anchors the data at the provided start_time
        //     with a cpu_time and queue_time of 0.
        //  2) a "real" measurement that captures the first changed CPU data
        task_info.record_measurement_with_start_time(start_time);
        task_info.measure_if_no_parent();

        let task_info = {
            let mut task_guard = this.tasks.lock();
            let task_info = match parent_koid {
                None => {
                    // If there's no parent task measure this task directly, otherwise
                    // we'll measure on the parent.
                    Arc::new(task_info)
                }
                Some(parent_koid) => {
                    task_info.stats.lock().has_parent_task = true;
                    let task_info = Arc::new(task_info);
                    if let Some(parent) = task_guard.get(&parent_koid).and_then(|p| p.upgrade()) {
                        parent.add_child(Arc::downgrade(&task_info));
                    }
                    task_info
                }
            };
            task_guard.insert(koid, Arc::downgrade(&task_info));
            task_info
        };

        stats.lock().add_task(task_info);

        this.prune_dead_tasks(MAX_DEAD_TASKS);
    }
}

impl ComponentTreeStats<DiagnosticsTask> {
    pub fn hooks(self: &Arc<Self>) -> Vec<HooksRegistration> {
        vec![HooksRegistration::new(
            "ComponentTreeStats",
            vec![EventType::Started],
            Arc::downgrade(self) as Weak<dyn Hook>,
        )]
    }

    /// Starts tracking component manager own stats.
    pub fn track_component_manager_stats(&self) {
        match fuchsia_runtime::job_default().duplicate_handle(zx::Rights::SAME_RIGHTS) {
            Ok(job) => {
                self.track_ready(ExtendedMoniker::ComponentManager, DiagnosticsTask::Job(job));
            }
            Err(err) => warn!(
                "Failed to duplicate component manager job. Not tracking its own stats: {:?}",
                err
            ),
        }
    }
}

#[async_trait]
impl Hook for ComponentTreeStats<DiagnosticsTask> {
    async fn on(self: Arc<Self>, event: &Event) -> Result<(), ModelError> {
        let target_moniker = event
            .target_moniker
            .unwrap_instance_moniker_or(ModelError::UnexpectedComponentManagerMoniker)?;
        match event.event_type() {
            EventType::Started => {
                if let EventPayload::Started { runtime, .. } = &event.payload {
                    self.on_component_started(target_moniker, &**runtime);
                }
            }
            _ => {}
        }
        Ok(())
    }
}

struct AggregatedStats {
    /// A queue storing all total measurements. The last one is the most recent.
    measurements: VecDeque<Measurement>,
}

impl AggregatedStats {
    fn new() -> Self {
        Self { measurements: VecDeque::with_capacity(COMPONENT_CPU_MAX_SAMPLES) }
    }

    fn insert(&mut self, measurement: Measurement) {
        while self.measurements.len() >= COMPONENT_CPU_MAX_SAMPLES {
            self.measurements.pop_front();
        }
        self.measurements.push_back(measurement);
    }

    fn write_totals_to(&self, node: &inspect::Node) {
        let count = self.measurements.len();
        let timestamps = node.create_int_array(TIMESTAMPS, count);
        let cpu_times = node.create_int_array(CPU_TIMES, count);
        let queue_times = node.create_int_array(QUEUE_TIMES, count);
        for (i, measurement) in self.measurements.iter().enumerate() {
            timestamps.set(i, measurement.timestamp().into_nanos());
            cpu_times.set(i, measurement.cpu_time().into_nanos());
            queue_times.set(i, measurement.queue_time().into_nanos());
        }
        node.record(timestamps);
        node.record(cpu_times);
        node.record(queue_times);
    }

    fn write_recents_to(&self, node: &inspect::Node) {
        if self.measurements.is_empty() {
            return;
        }
        if self.measurements.len() >= 2 {
            let measurement = self.measurements.get(self.measurements.len() - 2).unwrap();
            node.record_int("previous_cpu_time", measurement.cpu_time().into_nanos());
            node.record_int("previous_queue_time", measurement.queue_time().into_nanos());
            node.record_int("previous_timestamp", measurement.timestamp().into_nanos());
        }
        let measurement = self.measurements.get(self.measurements.len() - 1).unwrap();
        node.record_int("recent_cpu_time", measurement.cpu_time().into_nanos());
        node.record_int("recent_queue_time", measurement.queue_time().into_nanos());
        node.record_int("recent_timestamp", measurement.timestamp().into_nanos());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task_metrics::testing::{FakeDiagnosticsContainer, FakeRuntime, FakeTask};
    use diagnostics_assertions::{AnyProperty, assert_data_tree};
    use diagnostics_hierarchy::DiagnosticsHierarchy;
    use fuchsia_inspect::DiagnosticsHierarchyGetter;

    use injectable_time::{FakeTime, IncrementingFakeTime};

    #[fuchsia::test]
    async fn total_tracks_cpu_after_termination() {
        let inspector = inspect::Inspector::default();
        let clock = Arc::new(FakeTime::new());
        let stats = ComponentTreeStats::new_with_timesource(
            inspector.root().create_child("stats"),
            clock.clone(),
        );

        let mut previous_task_count = 0;
        for i in 0..10 {
            clock.add_ticks(1);
            let component_task = FakeTask::new(
                i as u64,
                create_measurements_vec_for_fake_task(COMPONENT_CPU_MAX_SAMPLES as i64 * 3, 2, 4),
            );

            let moniker = Moniker::try_from([format!("moniker-{}", i).as_ref()]).unwrap();
            let fake_runtime =
                Box::new(FakeRuntime::new(FakeDiagnosticsContainer::new(component_task, None)));
            stats.on_component_started(&moniker, &*fake_runtime);

            loop {
                let current = stats.tree.lock().len();
                if current != previous_task_count {
                    previous_task_count = current;
                    break;
                }
                fasync::Timer::new(fasync::MonotonicInstant::after(
                    zx::MonotonicDuration::from_millis(100i64),
                ))
                .await;
            }
        }

        assert_eq!(stats.tasks.lock().len(), 10);
        assert_eq!(stats.tree.lock().len(), 10);

        for _ in 0..=COMPONENT_CPU_MAX_SAMPLES - 2 {
            stats.measure();
            clock.add_ticks(CPU_SAMPLE_PERIOD.as_nanos() as i64);
        }

        // Data is produced by `measure`
        // Both recent and previous exist
        {
            let totals = stats.totals.lock();
            let recent_measurement = totals
                .measurements
                .get(totals.measurements.len() - 1)
                .expect("there's at least one measurement");
            assert_eq!(recent_measurement.cpu_time().into_nanos(), 1180);
            assert_eq!(recent_measurement.queue_time().into_nanos(), 2360);

            let previous_measurement = totals
                .measurements
                .get(totals.measurements.len() - 2)
                .expect("there's a previous measurement");
            assert_eq!(previous_measurement.cpu_time().into_nanos(), 1160);
            assert_eq!(previous_measurement.queue_time().into_nanos(), 2320,);
        }

        // Terminate all tasks
        for i in 0..10 {
            let moniker = Moniker::try_from([format!("moniker-{}", i).as_ref()]).unwrap();

            let tasks_to_terminate: Vec<_> = {
                let tree_guard = stats.tree.lock();
                let mut node_guard = tree_guard.get(&moniker.into()).unwrap().lock();

                node_guard.tasks_mut().iter().map(|t| t.clone()).collect()
            };

            for task in tasks_to_terminate {
                task.force_terminate().await;
                // the timestamp for termination is used as a key when pruning,
                // so all of the tasks cannot be removed at exactly the same time
                clock.add_ticks(1);
            }
        }

        // Data is produced by measure_dead_tasks
        {
            let totals = stats.totals.lock();
            let recent_measurement = totals
                .measurements
                .get(totals.measurements.len() - 1)
                .expect("there's at least one measurement");
            assert_eq!(recent_measurement.cpu_time().into_nanos(), 1180);
            assert_eq!(recent_measurement.queue_time().into_nanos(), 2360);

            let previous_measurement = totals
                .measurements
                .get(totals.measurements.len() - 2)
                .expect("there's a previous measurement");
            assert_eq!(previous_measurement.cpu_time().into_nanos(), 1160);
            assert_eq!(previous_measurement.queue_time().into_nanos(), 2320);
        }

        // Data is produced by measure_dead_tasks
        stats.measure();
        clock.add_ticks(CPU_SAMPLE_PERIOD.as_nanos() as i64);

        {
            let totals = stats.totals.lock();
            let recent_measurement = totals
                .measurements
                .get(totals.measurements.len() - 1)
                .expect("there's at least one measurement");
            assert_eq!(recent_measurement.cpu_time().into_nanos(), 1200);
            assert_eq!(recent_measurement.queue_time().into_nanos(), 2400);

            let previous_measurement = totals
                .measurements
                .get(totals.measurements.len() - 2)
                .expect("there's a previous measurement");
            assert_eq!(previous_measurement.cpu_time().into_nanos(), 1180);
            assert_eq!(previous_measurement.queue_time().into_nanos(), 2360);
        }

        stats.measure();
        clock.add_ticks(CPU_SAMPLE_PERIOD.as_nanos() as i64);

        {
            let totals = stats.totals.lock();
            let recent_measurement = totals
                .measurements
                .get(totals.measurements.len() - 1)
                .expect("there's at least one measurement");
            assert_eq!(recent_measurement.cpu_time().into_nanos(), 1200);
            assert_eq!(recent_measurement.queue_time().into_nanos(), 2400);

            let previous_measurement = totals
                .measurements
                .get(totals.measurements.len() - 2)
                .expect("there's a previous measurement");
            assert_eq!(previous_measurement.cpu_time().into_nanos(), 1200);
            assert_eq!(previous_measurement.queue_time().into_nanos(), 2400);
        }

        // Push all the measurements in the queues out. @totals should still be accurate
        for _ in 0..COMPONENT_CPU_MAX_SAMPLES {
            stats.measure();
            clock.add_ticks(CPU_SAMPLE_PERIOD.as_nanos() as i64);
        }

        // Data is produced by clean_stale
        assert_eq!(stats.tasks.lock().len(), 0);
        assert_eq!(stats.tree.lock().len(), 0);

        // Expect that cumulative totals are still around, plus a post-termination measurement
        {
            let totals = stats.totals.lock();
            let recent_measurement = totals
                .measurements
                .get(totals.measurements.len() - 1)
                .expect("there's at least one measurement");
            assert_eq!(recent_measurement.cpu_time().into_nanos(), 1200);
            assert_eq!(recent_measurement.queue_time().into_nanos(), 2400);

            let previous_measurement = totals
                .measurements
                .get(totals.measurements.len() - 2)
                .expect("there's a previous measurement");
            assert_eq!(previous_measurement.cpu_time().into_nanos(), 1200);
            assert_eq!(previous_measurement.queue_time().into_nanos(), 2400);
        }
    }

    #[fuchsia::test]
    async fn components_are_deleted_when_all_tasks_are_gone() {
        let inspector = inspect::Inspector::default();
        let clock = Arc::new(FakeTime::new());
        let stats = ComponentTreeStats::new_with_timesource(
            inspector.root().create_child("stats"),
            clock.clone(),
        );
        let moniker: Moniker = ["a"].try_into().unwrap();
        let moniker: ExtendedMoniker = moniker.into();
        stats.track_ready(moniker.clone(), FakeTask::default());
        for _ in 0..=COMPONENT_CPU_MAX_SAMPLES {
            stats.measure();
            clock.add_ticks(CPU_SAMPLE_PERIOD.as_nanos() as i64);
        }
        assert_eq!(stats.tree.lock().len(), 1);
        assert_eq!(stats.tasks.lock().len(), 1);
        assert_eq!(
            stats.tree.lock().get(&moniker).unwrap().lock().total_measurements(),
            COMPONENT_CPU_MAX_SAMPLES
        );

        // Invalidate the handle, to simulate that the component stopped.
        let tasks_to_terminate: Vec<_> = {
            let tree_guard = stats.tree.lock();
            let mut node_guard = tree_guard.get(&moniker).unwrap().lock();
            node_guard.tasks_mut().iter().cloned().collect()
        };

        for task in tasks_to_terminate {
            task.force_terminate().await;
            clock.add_ticks(1);
        }

        // All post-invalidation measurements; this will push out true measurements
        for i in 0..COMPONENT_CPU_MAX_SAMPLES {
            stats.measure();
            clock.add_ticks(CPU_SAMPLE_PERIOD.as_nanos() as i64);
            assert_eq!(
                stats.tree.lock().get(&moniker).unwrap().lock().total_measurements(),
                COMPONENT_CPU_MAX_SAMPLES - i,
            );
        }
        stats.measure();
        clock.add_ticks(CPU_SAMPLE_PERIOD.as_nanos() as i64);
        assert!(stats.tree.lock().get(&moniker).is_none());
        assert_eq!(stats.tree.lock().len(), 0);
        assert_eq!(stats.tasks.lock().len(), 0);
    }

    fn create_measurements_vec_for_fake_task(
        num_measurements: i64,
        init_cpu: i64,
        init_queue: i64,
    ) -> Vec<zx::TaskRuntimeInfo> {
        let mut v = vec![];
        for i in 0..num_measurements {
            v.push(zx::TaskRuntimeInfo {
                cpu_time: i * init_cpu,
                queue_time: i * init_queue,
                ..zx::TaskRuntimeInfo::default()
            });
        }

        v
    }

    #[fuchsia::test]
    async fn dead_tasks_are_pruned() {
        let clock = Arc::new(FakeTime::new());
        let inspector = inspect::Inspector::default();
        let stats = Arc::new(ComponentTreeStats::new_with_timesource(
            inspector.root().create_child("stats"),
            clock.clone(),
        ));

        let mut previous_task_count = 0;
        for i in 0..(MAX_DEAD_TASKS * 2) {
            clock.add_ticks(1);
            let component_task =
                FakeTask::new(i as u64, create_measurements_vec_for_fake_task(300, 2, 4));

            let moniker = Moniker::try_from([format!("moniker-{}", i).as_ref()]).unwrap();
            let fake_runtime =
                Box::new(FakeRuntime::new(FakeDiagnosticsContainer::new(component_task, None)));
            stats.on_component_started(&moniker, &*fake_runtime);

            loop {
                let current = stats.tree.lock().len();
                if current != previous_task_count {
                    previous_task_count = current;
                    break;
                }
                fasync::Timer::new(fasync::MonotonicInstant::after(
                    zx::MonotonicDuration::from_millis(100i64),
                ))
                .await;
            }

            let extended_moniker: ExtendedMoniker = moniker.clone().into();

            let tasks_to_terminate: Vec<_> = {
                let tree_guard = stats.tree.lock();
                let mut node_guard = tree_guard.get(&extended_moniker).unwrap().lock();

                node_guard.tasks_mut().iter().cloned().collect()
            };

            for task in tasks_to_terminate {
                task.force_terminate().await;
                clock.add_ticks(1);
            }
        }

        let task_count = stats.tasks.lock().len();
        let moniker_count = stats.tree.lock().len();
        assert_eq!(task_count, 88);
        assert_eq!(moniker_count, 88);
    }

    #[fuchsia::test]
    async fn aggregated_data_available_inspect() {
        let max_dead_tasks = 4;
        let clock = Arc::new(FakeTime::new());
        let inspector = inspect::Inspector::default();
        let stats = Arc::new(ComponentTreeStats::new_with_timesource(
            inspector.root().create_child("stats"),
            clock.clone(),
        ));

        let mut moniker_list = vec![];
        for i in 0..(max_dead_tasks * 2) {
            clock.add_ticks(1);
            let moniker = Moniker::try_from([format!("moniker-{}", i).as_ref()]).unwrap();
            moniker_list.push(moniker.clone());
            let component_task =
                FakeTask::new(i as u64, create_measurements_vec_for_fake_task(5, 1, 1));
            stats.track_ready(moniker.into(), component_task);
        }

        clock.add_ticks(CPU_SAMPLE_PERIOD.as_nanos() as i64);
        stats.measure();
        clock.add_ticks(CPU_SAMPLE_PERIOD.as_nanos() as i64);
        stats.measure();
        clock.add_ticks(CPU_SAMPLE_PERIOD.as_nanos() as i64);
        stats.measure();

        assert_data_tree!(inspector, root: {
            stats: contains {
                measurements: contains {
                    components: {
                        "moniker-0": contains {},
                        "moniker-1": contains {},
                        "moniker-2": contains {},
                        "moniker-3": contains {},
                        "moniker-4": contains {},
                        "moniker-5": contains {},
                        "moniker-6": contains {},
                        "moniker-7": contains {},
                    }
                }
            }
        });

        for moniker in moniker_list {
            let extended_moniker = moniker.clone().into();

            let tasks_to_terminate = {
                let tree_guard = stats.tree.lock();
                let mut node_guard = tree_guard.get(&extended_moniker).unwrap().lock();

                node_guard.tasks_mut().to_vec()
            };

            for task in tasks_to_terminate {
                task.force_terminate().await;
                clock.add_ticks(1);
            }
        }

        stats.prune_dead_tasks(max_dead_tasks);

        let hierarchy = inspector.get_diagnostics_hierarchy().await;
        assert_data_tree!(inspector, root: {
            stats: contains {
                measurements: contains {
                    components: {
                        "@aggregated": {
                            "timestamps": AnyProperty,
                            "cpu_times": vec![0i64, 6i64, 12i64],
                            "queue_times": vec![0i64, 6i64, 12i64],
                        },
                        "moniker-6": contains {},
                        "moniker-7": contains {},
                    }
                }
            }
        });
        let (timestamps, _, _) = get_data(&hierarchy, "@aggregated", None);
        assert_eq!(timestamps.len(), 3);
        assert!(timestamps[1] > timestamps[0]);
        assert!(timestamps[2] > timestamps[1]);
    }

    #[fuchsia::test]
    async fn total_holds_sum_of_stats() {
        let inspector = inspect::Inspector::default();
        let stats = ComponentTreeStats::new(inspector.root().create_child("stats"));
        stats.measure();
        stats.track_ready(
            ExtendedMoniker::ComponentInstance(["a"].try_into().unwrap()),
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
        );
        stats.track_ready(
            ExtendedMoniker::ComponentInstance(["b"].try_into().unwrap()),
            FakeTask::new(
                2,
                vec![
                    zx::TaskRuntimeInfo {
                        cpu_time: 1,
                        queue_time: 3,
                        ..zx::TaskRuntimeInfo::default()
                    },
                    zx::TaskRuntimeInfo {
                        cpu_time: 5,
                        queue_time: 7,
                        ..zx::TaskRuntimeInfo::default()
                    },
                ],
            ),
        );

        stats.measure();
        let hierarchy = inspect::reader::read(&inspector).await.expect("read inspect hierarchy");
        let (timestamps, cpu_times, queue_times) = get_data_at(&hierarchy, &["stats", "@total"]);
        assert_eq!(timestamps.len(), 2);
        assert_eq!(cpu_times, vec![0, 2 + 1]);
        assert_eq!(queue_times, vec![0, 4 + 3]);

        stats.measure();
        let hierarchy = inspect::reader::read(&inspector).await.expect("read inspect hierarchy");
        let (timestamps, cpu_times, queue_times) = get_data_at(&hierarchy, &["stats", "@total"]);
        assert_eq!(timestamps.len(), 3);
        assert_eq!(cpu_times, vec![0, 2 + 1, 6 + 5]);
        assert_eq!(queue_times, vec![0, 4 + 3, 8 + 7]);
    }

    #[fuchsia::test]
    async fn recent_usage() {
        // Set up the test
        let inspector = inspect::Inspector::default();
        let stats = ComponentTreeStats::new(inspector.root().create_child("stats"));
        stats.measure();

        stats.track_ready(
            ExtendedMoniker::ComponentInstance(["a"].try_into().unwrap()),
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
        );
        stats.track_ready(
            ExtendedMoniker::ComponentInstance(["b"].try_into().unwrap()),
            FakeTask::new(
                2,
                vec![
                    zx::TaskRuntimeInfo {
                        cpu_time: 1,
                        queue_time: 3,
                        ..zx::TaskRuntimeInfo::default()
                    },
                    zx::TaskRuntimeInfo {
                        cpu_time: 5,
                        queue_time: 7,
                        ..zx::TaskRuntimeInfo::default()
                    },
                ],
            ),
        );

        stats.measure();
        let hierarchy = inspect::reader::read(&inspector).await.expect("read inspect hierarchy");

        // Verify initially there's no second most recent measurement since we only
        // have the initial measurement written.
        assert_data_tree!(&hierarchy, root: contains {
            stats: contains {
                recent_usage: {
                    previous_cpu_time: 0i64,
                    previous_queue_time: 0i64,
                    previous_timestamp: AnyProperty,
                    recent_cpu_time: 2 + 1i64,
                    recent_queue_time: 4 + 3i64,
                    recent_timestamp: AnyProperty,
                }
            }
        });

        // Verify that the recent values are equal to the total values.
        let initial_timestamp = get_recent_property(&hierarchy, "recent_timestamp");
        let (timestamps, cpu_times, queue_times) = get_data_at(&hierarchy, &["stats", "@total"]);
        assert_eq!(timestamps.len(), 2);
        assert_eq!(timestamps[1], initial_timestamp);
        assert_eq!(cpu_times, vec![0, 2 + 1]);
        assert_eq!(queue_times, vec![0, 4 + 3]);

        // Add one measurement
        stats.measure();
        let hierarchy = inspect::reader::read(&inspector).await.expect("read inspect hierarchy");

        // Verify that previous is now there and holds the previously recent values.
        assert_data_tree!(&hierarchy, root: contains {
            stats: contains {
                recent_usage: {
                    previous_cpu_time: 2 + 1i64,
                    previous_queue_time: 4 + 3i64,
                    previous_timestamp: initial_timestamp,
                    recent_cpu_time: 6 + 5i64,
                    recent_queue_time: 8 + 7i64,
                    recent_timestamp: AnyProperty,
                }
            }
        });

        // Verify that the recent timestamp is higher than the previous timestamp.
        let recent_timestamp = get_recent_property(&hierarchy, "recent_timestamp");
        assert!(recent_timestamp > initial_timestamp);
    }

    #[fuchsia::test]
    async fn component_stats_are_available_in_inspect() {
        let inspector = inspect::Inspector::default();
        let stats = ComponentTreeStats::new(inspector.root().create_child("stats"));
        stats.track_ready(
            ExtendedMoniker::ComponentInstance(["a"].try_into().unwrap()),
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
        );

        stats.measure();

        let hierarchy = inspector.get_diagnostics_hierarchy().await;
        assert_data_tree!(hierarchy, root: {
            stats: contains {
                measurements: contains {
                    components: {
                        "a": {
                            "1": {
                                timestamps: AnyProperty,
                                cpu_times: vec![2i64],
                                queue_times: vec![4i64],
                            }
                        }
                    }
                }
            }
        });
        let (timestamps, _, _) = get_data(&hierarchy, "a", Some("1"));
        assert_eq!(timestamps.len(), 1);

        // Add another measurement
        stats.measure();

        let hierarchy = inspector.get_diagnostics_hierarchy().await;
        assert_data_tree!(hierarchy, root: {
            stats: contains {
                measurements: contains {
                    components: {
                        "a": {
                            "1": {
                                timestamps: AnyProperty,
                                cpu_times: vec![2i64, 6],
                                queue_times: vec![4i64, 8],
                            }
                        }
                    }
                }
            }
        });
        let (timestamps, _, _) = get_data(&hierarchy, "a", Some("1"));
        assert_eq!(timestamps.len(), 2);
        assert!(timestamps[1] > timestamps[0]);
    }

    #[fuchsia::test]
    async fn on_started_handles_parent_task() {
        let inspector = inspect::Inspector::default();
        let clock = Arc::new(FakeTime::new());
        // set ticks to 20 to avoid interfering with the start times reported
        // by FakeRuntime
        clock.add_ticks(20);
        let stats = Arc::new(ComponentTreeStats::new_with_timesource(
            inspector.root().create_child("stats"),
            clock.clone(),
        ));
        let parent_task = FakeTask::new(
            1,
            vec![
                zx::TaskRuntimeInfo {
                    cpu_time: 20,
                    queue_time: 40,
                    ..zx::TaskRuntimeInfo::default()
                },
                zx::TaskRuntimeInfo {
                    cpu_time: 60,
                    queue_time: 80,
                    ..zx::TaskRuntimeInfo::default()
                },
            ],
        );
        let component_task = FakeTask::new(
            2,
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
        );

        let fake_runtime = Box::new(FakeRuntime::new_with_start_times(
            FakeDiagnosticsContainer::new(parent_task.clone(), None),
            IncrementingFakeTime::new(3, std::time::Duration::from_nanos(5)),
        ));
        stats.on_component_started(&Moniker::try_from(["parent"]).unwrap(), &*fake_runtime);

        let fake_runtime = Box::new(FakeRuntime::new_with_start_times(
            FakeDiagnosticsContainer::new(component_task, Some(parent_task)),
            IncrementingFakeTime::new(8, std::time::Duration::from_nanos(5)),
        ));
        stats.on_component_started(&Moniker::try_from(["child"]).unwrap(), &*fake_runtime);

        // Wait for diagnostics data to be received since it's done in a non-blocking way on
        // started.
        loop {
            if stats.tree.lock().len() == 2 {
                break;
            }
            fasync::Timer::new(fasync::MonotonicInstant::after(
                zx::MonotonicDuration::from_millis(100i64),
            ))
            .await;
        }

        assert_data_tree!(inspector, root: {
            stats: contains {
                measurements: contains {
                    components: {
                        "parent": {
                            "1": {
                                "timestamps": AnyProperty,
                                "cpu_times": vec![0i64, 20],
                                "queue_times": vec![0i64, 40],
                            },
                        },
                        "child": {
                            "2": {
                                "timestamps": AnyProperty,
                                "cpu_times": vec![0i64, 2],
                                "queue_times": vec![0i64, 4],
                            }
                        }
                    }
                }
            }
        });
    }

    #[fuchsia::test]
    async fn child_tasks_garbage_collection() {
        let inspector = inspect::Inspector::default();
        let clock = Arc::new(FakeTime::new());
        let stats = Arc::new(ComponentTreeStats::new_with_timesource(
            inspector.root().create_child("stats"),
            clock.clone(),
        ));
        let parent_task = FakeTask::new(
            1,
            vec![
                zx::TaskRuntimeInfo {
                    cpu_time: 20,
                    queue_time: 40,
                    ..zx::TaskRuntimeInfo::default()
                },
                zx::TaskRuntimeInfo {
                    cpu_time: 60,
                    queue_time: 80,
                    ..zx::TaskRuntimeInfo::default()
                },
            ],
        );
        let component_task = FakeTask::new(
            2,
            vec![zx::TaskRuntimeInfo {
                cpu_time: 2,
                queue_time: 4,
                ..zx::TaskRuntimeInfo::default()
            }],
        );
        let fake_parent_runtime =
            Box::new(FakeRuntime::new(FakeDiagnosticsContainer::new(parent_task.clone(), None)));
        stats.on_component_started(&Moniker::try_from(["parent"]).unwrap(), &*fake_parent_runtime);

        let child_moniker = Moniker::try_from(["child"]).unwrap();
        let fake_runtime = Box::new(FakeRuntime::new(FakeDiagnosticsContainer::new(
            component_task,
            Some(parent_task),
        )));
        stats.on_component_started(&child_moniker, &*fake_runtime);

        // Wait for diagnostics data to be received since it's done in a non-blocking way on
        // started.
        loop {
            if stats.tree.lock().len() == 2 {
                break;
            }
            fasync::Timer::new(fasync::MonotonicInstant::after(
                zx::MonotonicDuration::from_millis(100i64),
            ))
            .await;
        }

        assert_eq!(stats.tree.lock().len(), 2);
        assert_eq!(stats.tasks.lock().len(), 2);

        let extended_moniker = child_moniker.into();
        // Mark as terminated, to simulate that the component completely stopped.
        let tasks_to_terminate: Vec<_> = {
            let tree_guard = stats.tree.lock();
            let mut node_guard = tree_guard.get(&extended_moniker).unwrap().lock();
            node_guard.tasks_mut().iter().cloned().collect()
        };

        for task in tasks_to_terminate {
            task.force_terminate().await;
            clock.add_ticks(1);
        }

        // This will perform the (last) post-termination sample.
        stats.measure();
        clock.add_ticks(CPU_SAMPLE_PERIOD.as_nanos() as i64);

        // These will start incrementing the counter of post-termination samples, but won't sample.
        for _ in 0..COMPONENT_CPU_MAX_SAMPLES {
            stats.measure();
            clock.add_ticks(CPU_SAMPLE_PERIOD.as_nanos() as i64);
        }

        // Causes the task to be gone since it has been terminated for long enough.
        stats.measure();

        // Child is gone and only the parent exists now.
        assert!(stats.tree.lock().get(&extended_moniker).is_none());
        assert_eq!(stats.tree.lock().len(), 1);
        assert_eq!(stats.tasks.lock().len(), 1);
    }

    fn get_recent_property(hierarchy: &DiagnosticsHierarchy, name: &str) -> i64 {
        hierarchy.get_property_by_path(&vec!["stats", "recent_usage", name]).unwrap().int().unwrap()
    }

    fn get_data(
        hierarchy: &DiagnosticsHierarchy,
        moniker: &str,
        task: Option<&str>,
    ) -> (Vec<i64>, Vec<i64>, Vec<i64>) {
        let mut path = vec!["stats", "measurements", "components", moniker];
        if let Some(task) = task {
            path.push(task);
        }
        get_data_at(&hierarchy, &path)
    }

    fn get_data_at(
        hierarchy: &DiagnosticsHierarchy,
        path: &[&str],
    ) -> (Vec<i64>, Vec<i64>, Vec<i64>) {
        let node = hierarchy.get_child_by_path(&path).expect("found stats node");
        let cpu_times = node
            .get_property("cpu_times")
            .expect("found cpu")
            .int_array()
            .expect("cpu are ints")
            .raw_values();
        let queue_times = node
            .get_property("queue_times")
            .expect("found queue")
            .int_array()
            .expect("queue are ints")
            .raw_values();
        let timestamps = node
            .get_property("timestamps")
            .expect("found timestamps")
            .int_array()
            .expect("timestamps are ints")
            .raw_values();
        (timestamps.into_owned(), cpu_times.into_owned(), queue_times.into_owned())
    }

    #[fuchsia::test]
    async fn component_restart_shares_cpu_histogram() {
        let inspector = inspect::Inspector::default();
        let clock = Arc::new(FakeTime::new());
        let stats = Arc::new(ComponentTreeStats::new_with_timesource(
            inspector.root().create_child("stats"),
            clock.clone(),
        ));

        let moniker = Moniker::try_from(["restarting-component"]).unwrap();
        let ext_moniker: ExtendedMoniker = moniker.clone().into();

        // 1. Start first instance of the component.
        let task1 = FakeTask::new(1, create_measurements_vec_for_fake_task(10, 1, 1));
        let fake_runtime1 =
            Box::new(FakeRuntime::new(FakeDiagnosticsContainer::new(task1.clone(), None)));
        stats.on_component_started(&moniker, &*fake_runtime1);

        loop {
            if stats.tree.lock().len() == 1 {
                break;
            }
            fasync::Timer::new(fasync::MonotonicInstant::after(
                zx::MonotonicDuration::from_millis(10),
            ))
            .await;
        }

        clock.add_ticks(CPU_SAMPLE_PERIOD.as_nanos() as i64);
        stats.measure();

        // 2. Terminate first instance.
        task1.terminate();
        clock.add_ticks(1);

        // 3. Start second instance (restart) before task 1 is expired/pruned.
        let task2 = FakeTask::new(2, create_measurements_vec_for_fake_task(10, 1, 1));
        let fake_runtime2 =
            Box::new(FakeRuntime::new(FakeDiagnosticsContainer::new(task2.clone(), None)));
        stats.on_component_started(&moniker, &*fake_runtime2);

        loop {
            if stats
                .tree
                .lock()
                .get(&ext_moniker)
                .is_some_and(|comp_stats| comp_stats.lock().tasks().len() == 2)
            {
                break;
            }
            fasync::Timer::new(fasync::MonotonicInstant::after(
                zx::MonotonicDuration::from_millis(10),
            ))
            .await;
        }

        clock.add_ticks(CPU_SAMPLE_PERIOD.as_nanos() as i64);
        stats.measure();

        // 4. Verify Inspect hierarchy contains exactly ONE histogram property for this moniker.
        let hierarchy = inspector.get_diagnostics_hierarchy().await;
        let histograms_node = hierarchy
            .get_child_by_path(&vec!["stats", "histograms"])
            .expect("found histograms node");

        let matching_props: Vec<_> = histograms_node
            .properties
            .iter()
            .filter(|p| p.name() == "restarting-component")
            .collect();

        assert_eq!(
            matching_props.len(),
            1,
            "Expected exactly 1 histogram property for moniker, found {}",
            matching_props.len()
        );
    }
}
