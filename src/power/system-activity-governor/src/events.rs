// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_power_observability as fobs;
use fuchsia_inspect::stats::InspectorExt;
use fuchsia_inspect::{ArrayProperty, LazyNode as ILazyNode, Node as INode};
use fuchsia_sync::Mutex;
use futures::FutureExt;
use inspect_format::constants::DEFAULT_VMO_SIZE_BYTES;
use state_recorder::{EnumStateRecorder, RecorderOptions};
use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque, btree_map};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use strum_macros::{Display, EnumIter};

const SUSPEND_EVENT_BUFFER_SIZE_BYTES: usize = (2.5f32 * DEFAULT_VMO_SIZE_BYTES as f32) as usize; // 640K buffer

static INSPECT_FIELD_EVENT_CAPACITY: &str = "event_capacity";
static INSPECT_FIELD_HISTORY_DURATION: &str = "history_duration_seconds";
static INSPECT_FIELD_HISTORY_DURATION_WHEN_FULL: &str = "at_capacity_history_duration_seconds";

/// An event logged by system-activity-governor.
#[derive(Clone, Debug)]
pub enum SagEvent {
    /// Suspend is being attempted.
    SuspendAttempted,
    /// Suspend was entered and exited successfully and the system is resuming.
    SuspendResumed { suspend_duration: i64, cumulative_duration: i64 },
    /// Suspend attempt was requested but is not allowed due to an unmet
    /// precondition, e.g. active wake leases, CPU power element is active.
    SuspendAttemptBlocked,
    /// Suspend attempt failed.
    SuspendFailed,
    /// A suspend blocker has been acquired, so suspend is blocked.
    SuspendBlockerAcquired,
    /// A suspend blocker has been dropped, so suspend is no longer blocked.
    SuspendBlockerDropped,
    /// A suspend lock has been acquired, so an uninterruptible suspend attempt
    /// is imminent.
    SuspendLockAcquired,
    /// A suspend lock has been dropped, so a suspend attempt has completed.
    SuspendLockDropped,
    /// A wake lease was created.
    WakeLeaseCreated { name: String, id: u64 },
    /// The underlying power broker lease for a wake lease failed to be satisfied.
    WakeLeaseSatisfactionFailed { name: String, id: u64, error: String },
    /// The underlying power broker lease for a wake lease was satisfied.
    WakeLeaseSatisfied { name: String, id: u64 },
    /// A wake lease was dropped and is no longer active.
    WakeLeaseDropped { name: String, id: u64 },
    /// A consolidated wake lease sample.
    WakeLeaseSample {
        name: String,
        id: u64,
        sample_end_ns: i64,
        active_fraction: f64,
        sample_duration_ns: i64,
    },
    /// Reported reasons of the last wake, or prevented sleep.
    WakeReasons { reasons: Vec<String> },
    /// Suspend callback processing started.
    SuspendCallbackPhaseStarted,
    /// Suspend callback processing ended.
    SuspendCallbackPhaseEnded,
    /// Resume callback processing started.
    ResumeCallbackPhaseStarted,
    /// Resume callback processing ended.
    ResumeCallbackPhaseEnded,
}

// Threshold duration for continuous holding boundaries, averaging of pulsing activity, and
// truncation due to inactivity, as described by WakeLeaseSampler.
const SAMPLING_THRESHOLD_NS: i64 = zx::BootDuration::from_minutes(1).into_nanos();

/// `WakeLeaseSampler` aggregates wake lease activity into consolidated sample entries to minimize
/// Inspect log traffic.
///
/// 1. Sample Boundaries & Behavioral Transitions:
///    A sample ends and a new sample begins upon any of the following boundaries:

///    - Continuous Holding Boundaries: Periods during which a lease is held continuously without
///      state changes (`active_count > 0`) for >= `SAMPLING_THRESHOLD_NS` are represented as a
///      single unbroken sample spanning the entire duration of the hold, even if it exceeds the
///      sampling threshold. For example, with the current one-minute threshold, activity over
///      interval [30s, 210s], yields a sample with duration = 180s, active_fraction = 1.0. If
///      pulsing activity preceded the continuous hold, a sample boundary is retroactively
///      established at the start of the hold.
///    - Pulsing Activity Duration Cap: Regular pulsing activity (acquire/drop cycles) continuing
///      past `SAMPLING_THRESHOLD_NS` is chunked in intervals of length `SAMPLING_THRESHOLD_NS`.
///    - External Triggers: Inspect snapshot collection flushes in-flight samples.
///
/// 2. Inspect Output Format:
///    Each consolidated sample entry records:
///    - `wake_lease_item_name`: The lease / `SuspendBlocker` name.
///    - `wake_lease_item_id`: The wake lease ID.
///    - `sample_end_ns`: End timestamp of the sample.
///    - `sample_duration_ns`: Total duration of the sample in nanoseconds.
///    - `active_fraction`: Fraction of the sample interval during which one or more leases were
///      active. (range [0.0, 1.0]).
#[derive(Clone, Debug, Default)]
struct WakeLeaseSampler {
    samples: BTreeMap<u64, InFlightSample>,
}

impl WakeLeaseSampler {
    fn new() -> Self {
        Self::default()
    }

    /// Before a new event at time `now` is ingested, this function propagates the SampleStatus from
    /// `active_since_ns` to `now` and flushes any complete samples.
    fn propagate_previous_status_and_flush_complete_samples(
        id: u64,
        sample: &mut InFlightSample,
        now: i64,
        buffer: &mut SagEventBuffer,
    ) {
        // If a lease has been held continuously without state changes for >=
        // `SAMPLING_THRESHOLD_NS`, retroactively split any preceding pulsing activity and emit the
        // continuous hold as an unbroken single sample.
        if let SampleStatus::LeaseActive { active_count, active_since_ns } = sample.status {
            let continuously_active_duration = now.saturating_sub(active_since_ns);
            if continuously_active_duration >= SAMPLING_THRESHOLD_NS {
                if sample.sample_start_ns < active_since_ns {
                    // 1. Emit preceding pulsing sample up to active_since_ns
                    let preceding_total_dur =
                        active_since_ns.saturating_sub(sample.sample_start_ns);
                    let preceding_active_dur = sample.active_duration_ns;
                    let active_fraction =
                        (preceding_active_dur as f64 / preceding_total_dur as f64).clamp(0.0, 1.0);

                    buffer.push(
                        active_since_ns,
                        SagEvent::WakeLeaseSample {
                            name: sample.name.clone(),
                            id,
                            sample_end_ns: active_since_ns,
                            active_fraction,
                            sample_duration_ns: preceding_total_dur,
                        },
                    );
                }

                // 2. Emit the continuous hold sample [active_since_ns, now]
                buffer.push(
                    now,
                    SagEvent::WakeLeaseSample {
                        name: sample.name.clone(),
                        id,
                        sample_end_ns: now,
                        active_fraction: 1.0,
                        sample_duration_ns: continuously_active_duration,
                    },
                );

                sample.sample_start_ns = now;
                sample.active_duration_ns = 0;
                sample.status = SampleStatus::LeaseActive { active_count, active_since_ns: now };
                return;
            }
        }
        // Only apply pulsing chunk boundaries while the lease is actively held. Inactive periods
        // are not chunked mid-silence; they either remain in-flight until active pulses resume, or
        // (TODO(https://fxbug.dev/554025327): complete this feature) are truncated upon reaching
        // the silence threshold.
        let SampleStatus::LeaseActive { active_count, mut active_since_ns } = sample.status else {
            return;
        };

        while now >= sample.sample_start_ns + SAMPLING_THRESHOLD_NS {
            let chunk_end_ns = sample.sample_start_ns + SAMPLING_THRESHOLD_NS;

            // Include the latest interval of activity in the total active time.
            let mut total_active_ns = sample.active_duration_ns;
            if active_since_ns < chunk_end_ns {
                total_active_ns += chunk_end_ns - active_since_ns;
            }

            let active_fraction =
                (total_active_ns as f64 / SAMPLING_THRESHOLD_NS as f64).clamp(0.0, 1.0);

            buffer.push(
                chunk_end_ns,
                SagEvent::WakeLeaseSample {
                    name: sample.name.clone(),
                    id,
                    sample_end_ns: chunk_end_ns,
                    active_fraction,
                    sample_duration_ns: SAMPLING_THRESHOLD_NS,
                },
            );

            sample.sample_start_ns = chunk_end_ns;
            sample.active_duration_ns = 0;
            active_since_ns = active_since_ns.max(chunk_end_ns);
            sample.status = SampleStatus::LeaseActive { active_count, active_since_ns };
        }
    }

    fn on_lease_acquired(&mut self, id: u64, name: &str, now: i64, buffer: &mut SagEventBuffer) {
        match self.samples.entry(id) {
            btree_map::Entry::Vacant(vacant) => {
                vacant.insert(InFlightSample::new_active(name.to_string(), now));
            }
            btree_map::Entry::Occupied(mut occupied) => {
                let sample = occupied.get_mut();
                Self::propagate_previous_status_and_flush_complete_samples(id, sample, now, buffer);
                match &mut sample.status {
                    SampleStatus::LeaseActive { active_count, .. } => {
                        *active_count = active_count.saturating_add(1);
                    }
                    SampleStatus::LeaseInactive { .. } => {
                        sample.status = SampleStatus::LeaseActive {
                            active_count: std::num::NonZeroUsize::MIN,
                            active_since_ns: now,
                        };
                    }
                }
            }
        }
    }

    fn on_lease_dropped(&mut self, id: u64, _name: &str, now: i64, buffer: &mut SagEventBuffer) {
        let btree_map::Entry::Occupied(mut occupied) = self.samples.entry(id) else {
            return;
        };
        let sample = occupied.get_mut();
        Self::propagate_previous_status_and_flush_complete_samples(id, sample, now, buffer);

        if let SampleStatus::LeaseActive { active_count, active_since_ns } = sample.status {
            if let Some(new_count) = std::num::NonZeroUsize::new(active_count.get() - 1) {
                // There's still at least one active lease.
                sample.status =
                    SampleStatus::LeaseActive { active_count: new_count, active_since_ns };
            } else {
                // The last lease was just dropped.
                let hold_start = active_since_ns;
                let hold_duration = now.saturating_sub(hold_start);
                sample.active_duration_ns += hold_duration;
                sample.status = SampleStatus::LeaseInactive { inactive_since_ns: now };
            }
        }
    }

    fn on_snapshot_collection(&mut self, now: i64, buffer: &mut SagEventBuffer) {
        // For each sample, perform propagation and flushing in the usual way. Then flush any
        // remaining in-flight activity into a sample for the snapshot, and purge any entries
        // with inactive leases.
        self.samples.retain(|&id, sample| {
            Self::propagate_previous_status_and_flush_complete_samples(id, sample, now, buffer);
            Self::end_sample_at_boundary_event(
                id,
                sample,
                now,
                SampleBoundaryEvent::SnapshotCollection,
                buffer,
            ) == SampleAction::Keep
        });
    }

    /// Finalizes and closes an in-flight sample when a boundary event occurs.
    fn end_sample_at_boundary_event(
        id: u64,
        sample: &mut InFlightSample,
        end_ns: i64,
        // TODO(https://fxbug.dev/554025327): Use this parameter once truncation on long silence
        // is supported.
        _end_reason: SampleBoundaryEvent,
        buffer: &mut SagEventBuffer,
    ) -> SampleAction {
        let start_ns = sample.sample_start_ns;
        let mut total_active_ns = sample.active_duration_ns;
        if let SampleStatus::LeaseActive { active_since_ns, .. } = sample.status {
            total_active_ns += end_ns.saturating_sub(active_since_ns);
        }

        let duration_ns = end_ns.saturating_sub(start_ns);

        if duration_ns > 0 {
            let active_fraction = (total_active_ns as f64 / duration_ns as f64).clamp(0.0, 1.0);

            buffer.push(
                end_ns,
                SagEvent::WakeLeaseSample {
                    name: sample.name.clone(),
                    id,
                    sample_end_ns: end_ns,
                    active_fraction,
                    sample_duration_ns: duration_ns,
                },
            );
        }

        if let SampleStatus::LeaseActive { active_count, .. } = sample.status {
            sample.sample_start_ns = end_ns;
            sample.active_duration_ns = 0;
            sample.status = SampleStatus::LeaseActive { active_count, active_since_ns: end_ns };
            SampleAction::Keep
        } else {
            SampleAction::Eject
        }
    }
}

#[derive(Clone, Debug)]
struct InFlightSample {
    name: String,
    sample_start_ns: i64,
    active_duration_ns: i64,
    status: SampleStatus,
}

impl InFlightSample {
    fn new_active(name: String, now: i64) -> Self {
        Self {
            name,
            sample_start_ns: now,
            active_duration_ns: 0,
            status: SampleStatus::LeaseActive {
                active_count: std::num::NonZeroUsize::MIN,
                active_since_ns: now,
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum SampleStatus {
    LeaseActive { active_count: std::num::NonZeroUsize, active_since_ns: i64 },
    LeaseInactive { inactive_since_ns: i64 },
}

/// The action to take on an in-flight sample entry within [`WakeLeaseSampler`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SampleAction {
    /// Keep the sample entry in the tracking map to continue monitoring an active lease.
    Keep,
    /// Eject and remove the sample entry from the map because the sample has been
    /// finalized and the lease is inactive.
    Eject,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum SampleBoundaryEvent {
    SnapshotCollection,
}

/// The state of the system with respect to suspend.
#[derive(Clone, Copy, Debug, Display, Eq, Hash, PartialEq, EnumIter)]
#[repr(u8)]
pub enum SystemSuspendState {
    Suspended = 0,
    Active = 1,
}

impl From<SystemSuspendState> for u64 {
    fn from(s: SystemSuspendState) -> u64 {
        s as u64
    }
}

#[derive(Clone, Debug)]
struct SagWakeLeaseEvent {
    event_number: u64,    // Event number
    event_log_time: i64,  // Timestamp for the event
    event_info: SagEvent, // The event itself
}

#[derive(Debug)]
struct SagEventBuffer {
    events: VecDeque<SagWakeLeaseEvent>,
    event_number: u64,
    max_events: usize,
}

impl SagEventBuffer {
    fn new(max_events: usize) -> Self {
        Self { events: VecDeque::with_capacity(max_events), event_number: 0, max_events }
    }

    fn push(&mut self, event_log_time: i64, event_info: SagEvent) {
        if self.max_events == 0 {
            return;
        }
        if self.events.len() == self.max_events {
            self.events.pop_front();
        }
        self.events.push_back(SagWakeLeaseEvent {
            event_number: self.event_number,
            event_log_time,
            event_info,
        });
        self.event_number += 1;
    }
}

/// A logger for SagEvent objects that inserts the event into a circular buffer
/// in inspect.
#[derive(Clone, Debug)]
pub struct SagEventLogger {
    /// Internal ring buffer for event logging
    event_buffer: Arc<Mutex<SagEventBuffer>>,

    /// Sampler for high-rate wake lease events
    sampler: Arc<Mutex<WakeLeaseSampler>>,

    /// Inspect node that tracks wall-time history duration.
    /// Schema follows Power Broker's topology stats:
    ///   event_capacity: u64
    ///   history_duration_seconds: i64
    ///   at_capacity_history_duration_seconds: i64
    _event_log_stats: Rc<RefCell<ILazyNode>>,

    /// Inspect node that reports internally-logged wake lease events
    _internal_event_log_stats: Rc<RefCell<ILazyNode>>,

    /// Total time that the device has spent suspended since boot (or at least
    /// since this logger was created), in nanoseconds.
    cumulative_suspend_duration: Arc<AtomicI64>,

    /// State recorder for `SystemSuspendState`.
    system_suspend_state: Arc<Mutex<EnumStateRecorder<SystemSuspendState>>>,
}

impl SagEventLogger {
    pub fn new(node: &INode, max_suspend_events_to_log: usize) -> Self {
        let event_buffer = Arc::new(Mutex::new(SagEventBuffer::new(max_suspend_events_to_log)));
        let sampler = Arc::new(Mutex::new(WakeLeaseSampler::new()));

        let weak_event_buffer = Arc::downgrade(&event_buffer);
        let weak_sampler = Arc::downgrade(&sampler);

        // Create inspect node for logging suspend events. Events are stored in an internal ring
        // buffer and lazily converted/logged to Inspect to reduce Inspect memory usage.
        let value = weak_event_buffer.clone();
        let internal_event_log_stats =
            node.create_lazy_child(fobs::SUSPEND_EVENTS_NODE, move || {
                let weak_buffer = value.clone();
                let weak_sampler = weak_sampler.clone();

                async move {
                    let lazy_inspect_config = fuchsia_inspect::InspectorConfig::default()
                        .size(SUSPEND_EVENT_BUFFER_SIZE_BYTES);
                    let inspector = fuchsia_inspect::Inspector::new(lazy_inspect_config);
                    let root = inspector.root();
                    inspector.record_lazy_stats();

                    // Convert internally-logged events into Inspect nodes
                    if let (Some(event_buffer), Some(sampler)) =
                        (weak_buffer.upgrade(), weak_sampler.upgrade())
                    {
                        let mut buffer = event_buffer.lock();
                        let now = zx::BootInstant::get().into_nanos();
                        sampler.lock().on_snapshot_collection(now, &mut buffer);

                        for internal_event in buffer.events.iter() {
                            let time = internal_event.event_log_time;
                            let event = internal_event.event_info.clone();

                            root.record_child(internal_event.event_number.to_string(), |root| {
                                match event {
                                    SagEvent::SuspendAttempted => {
                                        root.record_int(fobs::SUSPEND_ATTEMPTED_AT, time);
                                    }
                                    SagEvent::SuspendResumed {
                                        suspend_duration,
                                        cumulative_duration,
                                    } => {
                                        root.record_int(fobs::SUSPEND_RESUMED_AT, time);
                                        root.record_int(
                                            fobs::SUSPEND_LAST_TIMESTAMP,
                                            suspend_duration,
                                        );
                                        root.record_int(
                                            fobs::SUSPEND_CUMULATIVE_DURATION,
                                            cumulative_duration,
                                        );
                                    }
                                    SagEvent::SuspendFailed => {
                                        root.record_int(fobs::SUSPEND_FAILED_AT, time);
                                    }
                                    SagEvent::SuspendAttemptBlocked => {
                                        root.record_int(fobs::SUSPEND_ATTEMPT_BLOCKED_AT, time);
                                    }
                                    SagEvent::SuspendBlockerAcquired => {
                                        root.record_int(fobs::SUSPEND_BLOCKER_ACQUIRED_AT, time);
                                    }
                                    SagEvent::SuspendBlockerDropped => {
                                        root.record_int(fobs::SUSPEND_BLOCKER_DROPPED_AT, time);
                                    }
                                    SagEvent::SuspendLockAcquired => {
                                        root.record_int(fobs::SUSPEND_LOCK_ACQUIRED_AT, time);
                                    }
                                    SagEvent::SuspendLockDropped => {
                                        root.record_int(fobs::SUSPEND_LOCK_DROPPED_AT, time);
                                    }
                                    SagEvent::WakeLeaseCreated { name, id } => {
                                        root.record_int(fobs::WAKE_LEASE_CREATED_AT, time);
                                        root.record_uint(fobs::WAKE_LEASE_ITEM_ID, id);
                                        root.record_string(fobs::WAKE_LEASE_ITEM_NAME, name);
                                    }
                                    SagEvent::WakeLeaseSatisfactionFailed { name, id, error } => {
                                        root.record_int(
                                            fobs::WAKE_LEASE_SATISFACTION_FAILED_AT,
                                            time,
                                        );
                                        root.record_uint(fobs::WAKE_LEASE_ITEM_ID, id);
                                        root.record_string(fobs::WAKE_LEASE_ITEM_NAME, name);
                                        root.record_string(fobs::WAKE_LEASE_ITEM_ERROR, error);
                                    }
                                    SagEvent::WakeLeaseSatisfied { name, id } => {
                                        root.record_int(fobs::WAKE_LEASE_SATISFIED_AT, time);
                                        root.record_uint(fobs::WAKE_LEASE_ITEM_ID, id);
                                        root.record_string(fobs::WAKE_LEASE_ITEM_NAME, name);
                                    }
                                    SagEvent::WakeLeaseDropped { name, id } => {
                                        root.record_int(fobs::WAKE_LEASE_DROPPED_AT, time);
                                        root.record_uint(fobs::WAKE_LEASE_ITEM_ID, id);
                                        root.record_string(fobs::WAKE_LEASE_ITEM_NAME, name);
                                    }
                                    SagEvent::WakeLeaseSample {
                                        name,
                                        id,
                                        sample_end_ns,
                                        active_fraction,
                                        sample_duration_ns,
                                    } => {
                                        root.record_string(fobs::WAKE_LEASE_ITEM_NAME, name);
                                        root.record_uint(fobs::WAKE_LEASE_ITEM_ID, id);
                                        root.record_int(fobs::WAKE_LEASE_SAMPLE_END, sample_end_ns);
                                        root.record_double(
                                            fobs::WAKE_LEASE_SAMPLE_ACTIVE_FRACTION,
                                            active_fraction,
                                        );
                                        root.record_int(
                                            fobs::WAKE_LEASE_SAMPLE_DURATION,
                                            sample_duration_ns,
                                        );
                                    }
                                    SagEvent::SuspendCallbackPhaseStarted => {
                                        root.record_int(
                                            fobs::SUSPEND_CALLBACK_PHASE_START_AT,
                                            time,
                                        );
                                    }
                                    SagEvent::SuspendCallbackPhaseEnded => {
                                        root.record_int(fobs::SUSPEND_CALLBACK_PHASE_END_AT, time);
                                    }
                                    SagEvent::ResumeCallbackPhaseStarted => {
                                        root.record_int(fobs::RESUME_CALLBACK_PHASE_START_AT, time);
                                    }
                                    SagEvent::ResumeCallbackPhaseEnded => {
                                        root.record_int(fobs::RESUME_CALLBACK_PHASE_END_AT, time);
                                    }
                                    SagEvent::WakeReasons { reasons } => {
                                        root.record_int(fobs::WAKE_REASONS_REPORTED_AT, time);
                                        let reason_array = root.create_string_array(
                                            fobs::WAKE_REASONS_WAKE_VECTOR_PREFIX,
                                            reasons.len(),
                                        );
                                        reasons.iter().enumerate().for_each(|(i, reason)| {
                                            reason_array.set(i, reason);
                                        });
                                        root.record(reason_array);
                                    }
                                };
                            });
                        }
                    }
                    Ok(inspector)
                }
                .boxed()
            });

        // Create Inspect node for suspend event stats
        let event_log_stats = node.create_lazy_child("suspend_events_stats", move || {
            let weak_buffer = weak_event_buffer.clone();

            async move {
                let inspector = fuchsia_inspect::Inspector::default();
                let root = inspector.root();

                if let Some(event_buffer) = weak_buffer.upgrade() {
                    let buffer = event_buffer.lock();
                    root.record_uint(INSPECT_FIELD_EVENT_CAPACITY, buffer.max_events as u64);

                    if !buffer.events.is_empty() {
                        // `events` may be slightly out of order, so we need to loop through the
                        // entries to compute the min and max timestamps.
                        let (min_ns, max_ns) = buffer
                            .events
                            .iter()
                            .map(|e| e.event_log_time)
                            .fold((i64::MAX, i64::MIN), |(min, max), t| (min.min(t), max.max(t)));
                        let duration = zx::BootDuration::from_nanos(max_ns - min_ns).into_seconds();
                        root.record_int(INSPECT_FIELD_HISTORY_DURATION, duration);

                        if buffer.events.len() == buffer.max_events {
                            root.record_int(INSPECT_FIELD_HISTORY_DURATION_WHEN_FULL, duration);
                        }
                    } else {
                        root.record_int(INSPECT_FIELD_HISTORY_DURATION, 0i64);
                    }
                }
                Ok(inspector)
            }
            .boxed()
        });

        // Create the system_suspend_state recorder
        let system_suspend_state = Arc::new(Mutex::new(
            EnumStateRecorder::new(
                "system_suspend_state".to_string(),
                c"power",
                // Current capacity would cover 3 hours with one suspend/resume cycle per
                // minute.
                RecorderOptions { lazy_record: true, capacity: 360, ..Default::default() },
            )
            .expect("Failed to create system_suspend_state recorder"),
        ));
        system_suspend_state.lock().record(SystemSuspendState::Active);

        Self {
            event_buffer,
            sampler,
            _internal_event_log_stats: Rc::new(RefCell::new(internal_event_log_stats)),
            _event_log_stats: Rc::new(RefCell::new(event_log_stats)),
            cumulative_suspend_duration: Arc::new(AtomicI64::new(0)),
            system_suspend_state,
        }
    }

    pub fn update_cumulative_suspend_duration(&self, suspend_duration: i64) -> i64 {
        let new_cumulative =
            self.cumulative_suspend_duration.fetch_add(suspend_duration, Ordering::SeqCst)
                + suspend_duration;
        new_cumulative
    }

    #[expect(dead_code)]
    pub fn log_sampled_wake_lease_created(&self, id: u64, name: &str) {
        let time = zx::BootInstant::get().into_nanos();
        let mut buffer = self.event_buffer.lock();
        self.sampler.lock().on_lease_acquired(id, name, time, &mut buffer);
    }

    #[expect(dead_code)]
    pub fn log_sampled_wake_lease_dropped(&self, id: u64, name: &str) {
        let time = zx::BootInstant::get().into_nanos();
        let mut buffer = self.event_buffer.lock();
        self.sampler.lock().on_lease_dropped(id, name, time, &mut buffer);
    }

    pub fn log(&self, event: SagEvent) {
        let time = zx::BootInstant::get().into_nanos();
        // Log event to internal ring buffer
        {
            let mut buffer = self.event_buffer.lock();
            buffer.push(time, event.clone());
        }

        // For Suspend events, additionally update the logged suspend state
        match event {
            SagEvent::SuspendAttempted => {
                self.system_suspend_state.lock().record(SystemSuspendState::Suspended);
            }
            SagEvent::SuspendResumed { suspend_duration: _, cumulative_duration: _ } => {
                self.system_suspend_state.lock().record(SystemSuspendState::Active);
            }
            SagEvent::SuspendFailed => {
                self.system_suspend_state.lock().record(SystemSuspendState::Active);
            }
            _ => {} // Ignore other events
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_util::assert_near;

    const FLOAT_TOLERANCE: f64 = 1e-8;

    macro_rules! assert_wake_lease_sample {
        ($event:expr, name: $name:expr, id: $id:expr, end_ns: $end_ns:expr, duration_ns: $dur_ns:expr, active_fraction: $frac:expr $(,)?) => {
            match $event {
                SagEvent::WakeLeaseSample {
                    name,
                    id,
                    sample_end_ns,
                    active_fraction,
                    sample_duration_ns,
                } => {
                    assert_eq!(name, $name);
                    assert_eq!(*id, $id);
                    assert_eq!(*sample_end_ns, $end_ns);
                    assert_eq!(*sample_duration_ns, $dur_ns);
                    assert_near!(*active_fraction, $frac, FLOAT_TOLERANCE);
                }
                other => panic!("Expected WakeLeaseSample, got {:?}", other),
            }
        };
        ($event:expr, $name:expr, $id:expr, $end_ns:expr, $dur_ns:expr, $frac:expr $(,)?) => {
            assert_wake_lease_sample!(
                $event,
                name: $name,
                id: $id,
                end_ns: $end_ns,
                duration_ns: $dur_ns,
                active_fraction: $frac,
            )
        };
    }

    #[fuchsia::test]
    fn test_sampler_snapshot_ends_sample() {
        let mut sampler = WakeLeaseSampler::new();
        let mut buffer = SagEventBuffer::new(100);

        // Lease acquired at T=0
        sampler.on_lease_acquired(1, "test_lease", 0, &mut buffer);
        // Lease dropped at T=30s
        sampler.on_lease_dropped(1, "test_lease", 30_000_000_000, &mut buffer);

        // Snapshot collection at T=40s flushes active sample
        sampler.on_snapshot_collection(40_000_000_000, &mut buffer);

        assert_eq!(buffer.events.len(), 1);
        assert_wake_lease_sample!(
            &buffer.events[0].event_info,
            name: "test_lease",
            id: 1,
            end_ns: 40_000_000_000,
            duration_ns: 40_000_000_000,
            active_fraction: 0.75,
        );
    }

    #[fuchsia::test]
    fn test_sampler_pulsing_chunked_to_60s_intervals() {
        let mut sampler = WakeLeaseSampler::new();
        let mut buffer = SagEventBuffer::new(100);

        // Simulate 100 Hz high-rate lease activity for 270 seconds (27,000 cycles).
        // Each cycle is 10 ms (10_000_000 ns): 5 ms active, 5 ms inactive.
        let cycle_period_ns = 10_000_000i64; // 10 ms
        let active_duration_ns = 5_000_000i64; // 5 ms
        let total_cycles = 27_000; // 270s

        for i in 0..total_cycles {
            let acquire_time = i * cycle_period_ns;
            let drop_time = acquire_time + active_duration_ns;

            sampler.on_lease_acquired(2, "100hz_lease", acquire_time, &mut buffer);
            sampler.on_lease_dropped(2, "100hz_lease", drop_time, &mut buffer);

            // Verify that before the first 60s boundary (6,000 cycles), no intermediate events are
            // logged.
            if (i as i64) < SAMPLING_THRESHOLD_NS / cycle_period_ns - 1 {
                assert!(buffer.events.is_empty());
            }
        }

        // During 270s of 100 Hz pulsing (27,000 cycles), 4 full 60s samples were emitted at t=60s,
        // 120s, 180s, 240s.
        assert_eq!(buffer.events.len(), 4);

        // Snapshot collection at T=270s flushes the final 30s sample.
        let snapshot_time_ns = 4 * SAMPLING_THRESHOLD_NS + SAMPLING_THRESHOLD_NS / 2;
        sampler.on_snapshot_collection(snapshot_time_ns, &mut buffer);

        // Expect 5 sample events with durations: [60s, 60s, 60s, 60s, 30s]
        assert_eq!(buffer.events.len(), 5);

        let expected_samples = [
            (SAMPLING_THRESHOLD_NS, SAMPLING_THRESHOLD_NS, 0.50f64),
            (2 * SAMPLING_THRESHOLD_NS, SAMPLING_THRESHOLD_NS, 0.50f64),
            (3 * SAMPLING_THRESHOLD_NS, SAMPLING_THRESHOLD_NS, 0.50f64),
            (4 * SAMPLING_THRESHOLD_NS, SAMPLING_THRESHOLD_NS, 0.50f64),
            (snapshot_time_ns, SAMPLING_THRESHOLD_NS / 2, 0.50f64),
        ];

        for (i, (end, duration, fraction)) in expected_samples.iter().enumerate() {
            assert_wake_lease_sample!(
                &buffer.events[i].event_info,
                name: "100hz_lease",
                id: 2,
                end_ns: *end,
                duration_ns: *duration,
                active_fraction: *fraction,
            );
        }
    }

    #[fuchsia::test]
    fn test_sampler_acquire_after_snapshot_collection() {
        let mut sampler = WakeLeaseSampler::new();
        let mut buffer = SagEventBuffer::new(100);

        // Lease acquired at T=0, dropped at T=10s
        sampler.on_lease_acquired(1, "test_lease", 0, &mut buffer);
        sampler.on_lease_dropped(1, "test_lease", 10_000_000_000, &mut buffer);

        // Snapshot collection at T=20s closes active sample (truncated to 10s)
        sampler.on_snapshot_collection(20_000_000_000, &mut buffer);
        assert_eq!(buffer.events.len(), 1);

        // New lease acquired at T=30s (< 60s since drop at 10s).
        sampler.on_lease_acquired(1, "test_lease", 30_000_000_000, &mut buffer);
        sampler.on_lease_dropped(1, "test_lease", 40_000_000_000, &mut buffer);

        // Second snapshot collection at T=40s flushes second sample
        sampler.on_snapshot_collection(40_000_000_000, &mut buffer);
        assert_eq!(buffer.events.len(), 2);
        assert_wake_lease_sample!(
            &buffer.events[1].event_info,
            name: "test_lease",
            id: 1,
            end_ns: 40_000_000_000,
            duration_ns: 10_000_000_000,
            active_fraction: 1.0,
        );
    }

    #[fuchsia::test]
    fn test_sampler_nested_overlapping_acquisitions() {
        let mut sampler = WakeLeaseSampler::new();
        let mut buffer = SagEventBuffer::new(100);

        // Acquire #1 at T=0
        sampler.on_lease_acquired(1, "nested_lease", 0, &mut buffer);
        // Acquire #2 at T=10s (active_count = 2)
        sampler.on_lease_acquired(1, "nested_lease", 10_000_000_000, &mut buffer);
        // Drop #1 at T=20s (active_count = 1, still active)
        sampler.on_lease_dropped(1, "nested_lease", 20_000_000_000, &mut buffer);
        // Drop #2 at T=30s (active_count = 0, inactive)
        sampler.on_lease_dropped(1, "nested_lease", 30_000_000_000, &mut buffer);

        sampler.on_snapshot_collection(30_000_000_000, &mut buffer);

        // The overlapping leases are grouped into a single sample with active_fraction 1.0.
        assert_eq!(buffer.events.len(), 1);
        assert_wake_lease_sample!(
            &buffer.events[0].event_info,
            name: "nested_lease",
            id: 1,
            end_ns: 30_000_000_000,
            duration_ns: 30_000_000_000,
            active_fraction: 1.0,
        );
    }

    #[fuchsia::test]
    fn test_sampler_multiple_distinct_leases() {
        let mut sampler = WakeLeaseSampler::new();
        let mut buffer = SagEventBuffer::new(100);

        sampler.on_lease_acquired(1, "lease_a", 0, &mut buffer);
        sampler.on_lease_acquired(2, "lease_b", 0, &mut buffer);

        sampler.on_lease_dropped(1, "lease_a", 20_000_000_000, &mut buffer);
        sampler.on_lease_dropped(2, "lease_b", 30_000_000_000, &mut buffer);

        // Snapshot at T=40s
        sampler.on_snapshot_collection(40_000_000_000, &mut buffer);

        assert_eq!(buffer.events.len(), 2);
        assert_wake_lease_sample!(
            &buffer.events[0].event_info,
            name: "lease_a",
            id: 1,
            end_ns: 40_000_000_000,
            duration_ns: 40_000_000_000,
            active_fraction: 0.5,
        );
        assert_wake_lease_sample!(
            &buffer.events[1].event_info,
            name: "lease_b",
            id: 2,
            end_ns: 40_000_000_000,
            duration_ns: 40_000_000_000,
            active_fraction: 0.75,
        );
    }

    #[fuchsia::test]
    fn test_sampler_pulsing_spanning_chunk_boundary() {
        let mut sampler = WakeLeaseSampler::new();
        let mut buffer = SagEventBuffer::new(100);

        // 1. Initial pulse: active [0s, 20s], then inactive [20s, 50s]
        sampler.on_lease_acquired(1, "span_lease", 0, &mut buffer);
        sampler.on_lease_dropped(1, "span_lease", 20_000_000_000, &mut buffer);
        assert!(buffer.events.is_empty());

        // 2. Second pulse acquired at T=50s and held across the 60s boundary until T=70s
        sampler.on_lease_acquired(1, "span_lease", 50_000_000_000, &mut buffer);
        sampler.on_lease_dropped(1, "span_lease", 70_000_000_000, &mut buffer);

        // The first 60s chunk [0s, 60s] was flushed when dropping at T=70s.
        // Active time in first chunk: 20s (0-20s) + 10s (50-60s) = 30s -> active_fraction = 0.5.
        assert_eq!(buffer.events.len(), 1);
        assert_wake_lease_sample!(
            &buffer.events[0].event_info,
            name: "span_lease",
            id: 1,
            end_ns: SAMPLING_THRESHOLD_NS,
            duration_ns: SAMPLING_THRESHOLD_NS,
            active_fraction: 0.5,
        );

        // 3. Snapshot collection at T=80s flushes the remaining interval [60s, 80s].
        // Active time in second chunk: 10s (60-70s) -> active_fraction = 10s / 20s = 0.5.
        sampler.on_snapshot_collection(80_000_000_000, &mut buffer);

        assert_eq!(buffer.events.len(), 2);
        assert_wake_lease_sample!(
            &buffer.events[1].event_info,
            name: "span_lease",
            id: 1,
            end_ns: 80_000_000_000,
            duration_ns: 20_000_000_000,
            active_fraction: 0.5,
        );
    }

    #[fuchsia::test]
    fn test_sampler_reacquire_before_silence_threshold_does_not_chunk() {
        let mut sampler = WakeLeaseSampler::new();
        let mut buffer = SagEventBuffer::new(100);

        // Lease acquired at T=0, dropped at T=15s.
        sampler.on_lease_acquired(1, "test_lease", 0, &mut buffer);
        sampler.on_lease_dropped(1, "test_lease", 15_000_000_000, &mut buffer);

        // Re-acquire at T=70s (silence is 55s < 60s).
        sampler.on_lease_acquired(1, "test_lease", 70_000_000_000, &mut buffer);

        // In-flight sample should remain open with no events emitted yet.
        assert!(buffer.events.is_empty());
    }

    #[fuchsia::test]
    fn test_sampler_reacquire_pulsing_spans_60s_boundary() {
        let mut sampler = WakeLeaseSampler::new();
        let mut buffer = SagEventBuffer::new(100);

        // t=0: Acquire
        sampler.on_lease_acquired(1, "test_lease", 0, &mut buffer);
        // t=40s: Drop
        sampler.on_lease_dropped(1, "test_lease", 40_000_000_000, &mut buffer);
        assert!(buffer.events.is_empty());

        // t=50s: Acquire (< 60s silence)
        sampler.on_lease_acquired(1, "test_lease", 50_000_000_000, &mut buffer);
        assert!(buffer.events.is_empty());

        // t=70s: Drop (crosses 60s boundary while active)
        sampler.on_lease_dropped(1, "test_lease", 70_000_000_000, &mut buffer);

        // Expect 60s chunk [0s, 60s] emitted with active time: 40s (0-40s) + 10s (50-60s) = 50s.
        assert_eq!(buffer.events.len(), 1);
        assert_wake_lease_sample!(
            &buffer.events[0].event_info,
            name: "test_lease",
            id: 1,
            end_ns: SAMPLING_THRESHOLD_NS,
            duration_ns: SAMPLING_THRESHOLD_NS,
            active_fraction: 50_000_000_000.0 / SAMPLING_THRESHOLD_NS as f64,
        );

        // Snapshot at t=80s flushes remaining [60s, 80s] sample (10s active in 60s-70s).
        sampler.on_snapshot_collection(80_000_000_000, &mut buffer);
        assert_eq!(buffer.events.len(), 2);
        assert_wake_lease_sample!(
            &buffer.events[1].event_info,
            name: "test_lease",
            id: 1,
            end_ns: 80_000_000_000,
            duration_ns: 20_000_000_000,
            active_fraction: 10.0 / 20.0,
        );
    }

    #[fuchsia::test]
    fn test_sampler_mixed_2hz_and_continuous() {
        let mut sampler = WakeLeaseSampler::new();
        let mut buffer = SagEventBuffer::new(100);

        // Phase 1: 30 seconds of 2 Hz activity (0s to 30s) -> 60 cycles of 500ms (250ms active,
        // 250ms inactive)
        let cycle_period_ns = 500_000_000i64;
        let active_duration_ns = 250_000_000i64;
        let phase1_duration_ns = SAMPLING_THRESHOLD_NS / 2;
        let cycles_per_phase = (phase1_duration_ns / cycle_period_ns) as usize;

        for i in 0..cycles_per_phase {
            let acquire_time = (i as i64) * cycle_period_ns;
            let drop_time = acquire_time + active_duration_ns;
            sampler.on_lease_acquired(3, "mixed_lease", acquire_time, &mut buffer);
            sampler.on_lease_dropped(3, "mixed_lease", drop_time, &mut buffer);
        }

        // Phase 2: 3 minutes (180s) of continuous active lease (30s to 210s)
        let continuous_duration_ns = 3 * SAMPLING_THRESHOLD_NS;
        let phase2_end_ns = phase1_duration_ns + continuous_duration_ns;
        sampler.on_lease_acquired(3, "mixed_lease", phase1_duration_ns, &mut buffer);
        sampler.on_lease_dropped(3, "mixed_lease", phase2_end_ns, &mut buffer);

        // Phase 3: 30 seconds of 2 Hz activity (210s to 240s) -> 60 cycles
        let phase3_duration_ns = SAMPLING_THRESHOLD_NS / 2;
        let phase3_end_ns = phase2_end_ns + phase3_duration_ns;
        for i in 0..cycles_per_phase {
            let acquire_time = phase2_end_ns + (i as i64) * cycle_period_ns;
            let drop_time = acquire_time + active_duration_ns;
            sampler.on_lease_acquired(3, "mixed_lease", acquire_time, &mut buffer);
            sampler.on_lease_dropped(3, "mixed_lease", drop_time, &mut buffer);
        }

        // Phase 4: Snapshot collection at 240s
        sampler.on_snapshot_collection(phase3_end_ns, &mut buffer);

        // EXACTLY 3 samples emitted: [0s, 30s], [30s, 210s], [210s, 240s]!
        assert_eq!(buffer.events.len(), 3);

        let expected_samples = [
            // Sample 1: 30s 2Hz pulsing [0s, 30s]
            (phase1_duration_ns, phase1_duration_ns, 0.50f64),
            // Sample 2: 3 minutes continuous hold [30s, 210s]
            (phase2_end_ns, continuous_duration_ns, 1.0f64),
            // Sample 3: 30s 2Hz pulsing [210s, 240s]
            (phase3_end_ns, phase3_duration_ns, 0.50f64),
        ];

        for (i, (end, duration, fraction)) in expected_samples.iter().enumerate() {
            assert_wake_lease_sample!(
                &buffer.events[i].event_info,
                name: "mixed_lease",
                id: 3,
                end_ns: *end,
                duration_ns: *duration,
                active_fraction: *fraction,
            );
        }
    }

    #[fuchsia::test]
    fn test_sampler_pure_continuous_hold_no_preceding_activity() {
        let mut sampler = WakeLeaseSampler::new();
        let mut buffer = SagEventBuffer::new(100);

        // Continuous lease held for 2 minutes from t=0 with no prior pulsing
        let hold_duration_ns = 2 * SAMPLING_THRESHOLD_NS;
        sampler.on_lease_acquired(1, "pure_hold", 0, &mut buffer);
        sampler.on_lease_dropped(1, "pure_hold", hold_duration_ns, &mut buffer);

        assert_eq!(buffer.events.len(), 1);
        assert_wake_lease_sample!(
            &buffer.events[0].event_info,
            name: "pure_hold",
            id: 1,
            end_ns: hold_duration_ns,
            duration_ns: hold_duration_ns,
            active_fraction: 1.0,
        );
    }

    #[fuchsia::test]
    fn test_sampler_snapshot_during_continuous_hold() {
        let mut sampler = WakeLeaseSampler::new();
        let mut buffer = SagEventBuffer::new(100);

        sampler.on_lease_acquired(1, "active_hold", 0, &mut buffer);

        // Snapshot at 1.5x threshold while lease is still actively held
        let snapshot_time_ns = SAMPLING_THRESHOLD_NS + SAMPLING_THRESHOLD_NS / 2;
        sampler.on_snapshot_collection(snapshot_time_ns, &mut buffer);

        assert_eq!(buffer.events.len(), 1);
        assert_wake_lease_sample!(
            &buffer.events[0].event_info,
            name: "active_hold",
            id: 1,
            end_ns: snapshot_time_ns,
            duration_ns: snapshot_time_ns,
            active_fraction: 1.0,
        );

        // Dropping 10s later at t=100s
        let additional_active_dur_ns = zx::BootDuration::from_seconds(10).into_nanos();
        let drop_time_ns = snapshot_time_ns + additional_active_dur_ns;
        sampler.on_lease_dropped(1, "active_hold", drop_time_ns, &mut buffer);
        sampler.on_snapshot_collection(drop_time_ns, &mut buffer);

        assert_eq!(buffer.events.len(), 2);
        assert_wake_lease_sample!(
            &buffer.events[1].event_info,
            name: "active_hold",
            id: 1,
            end_ns: drop_time_ns,
            duration_ns: additional_active_dur_ns,
            active_fraction: 1.0,
        );
    }

    #[fuchsia::test]
    fn test_sampler_continuous_hold_boundary_59s_vs_60s() {
        let mut sampler = WakeLeaseSampler::new();
        let mut buffer = SagEventBuffer::new(100);

        // 59 seconds: should not trigger continuous hold sample upon drop
        let sub_threshold_ns =
            SAMPLING_THRESHOLD_NS - zx::BootDuration::from_seconds(1).into_nanos();
        sampler.on_lease_acquired(1, "sub_threshold", 0, &mut buffer);
        sampler.on_lease_dropped(1, "sub_threshold", sub_threshold_ns, &mut buffer);
        assert!(buffer.events.is_empty());

        // Exactly 60 seconds: must trigger continuous hold sample upon drop
        let mut sampler = WakeLeaseSampler::new();
        let mut buffer = SagEventBuffer::new(100);

        sampler.on_lease_acquired(2, "exact_threshold", 0, &mut buffer);
        sampler.on_lease_dropped(2, "exact_threshold", SAMPLING_THRESHOLD_NS, &mut buffer);
        assert_eq!(buffer.events.len(), 1);
        assert_wake_lease_sample!(
            &buffer.events[0].event_info,
            name: "exact_threshold",
            id: 2,
            end_ns: SAMPLING_THRESHOLD_NS,
            duration_ns: SAMPLING_THRESHOLD_NS,
            active_fraction: 1.0,
        );
    }
}
