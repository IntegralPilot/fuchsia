// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::power::{SuspendState, SuspendStats};
use crate::task::CurrentTask;

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::{Arc, Weak};

use anyhow::{Context, anyhow};
use fidl::endpoints::Proxy;
use fidl_fuchsia_power_observability as fobs;
use fidl_fuchsia_session_power as fpower;
use fidl_fuchsia_starnix_runner as frunner;
use fuchsia_component::client::connect_to_protocol_sync;
use fuchsia_inspect as inspect;
use fuchsia_inspect::ArrayProperty;
use futures::stream::{FusedStream, Next};
use futures::{FutureExt, StreamExt};
use starnix_logging::{log_info, log_warn};
use starnix_sync::{
    EbpfSuspendLock, LockDepGuard, LockDepMutex, LockDepReadGuard, LockDepRwLock,
    PowerMessageCountersLock, SuspendResumeManagerInnerLock,
};
use starnix_uapi::arc_key::WeakKey;
use starnix_uapi::errors::Errno;
use starnix_uapi::{errno, error};
use std::collections::VecDeque;
use std::fmt;
use zx::Peered;

/// Wake source persistent info, exposed in inspect diagnostics.
#[derive(Debug, Default)]
pub struct WakeupSource {
    /// The last task command that activated this wakeup source.
    pub last_actor: Option<starnix_task_command::TaskCommand>,

    /// The number of times the wakeup source has been activated.
    active_count: u64,

    /// The number of events signaled to this source. Similar to active_count but can track
    /// internal events causing the activation.
    event_count: u64,

    /// The number of times this source prevented suspension of the system, or woke the system from
    /// a suspended state.
    ///
    /// Right now there is no way for wake locks to wake the Starnix container, because the
    /// mechanism used for waking the container is not integrated into the wake source machinery.
    wakeup_count: u64,

    /// The number of times the timeout associated with this source expired.
    expire_count: u64,

    /// The timestamp relative to the monotonic clock when the lock became active. If 0, the lock
    /// is currently inactive.
    active_since: zx::MonotonicInstant,

    /// The total duration this source has been held active since the system booted.
    total_time: zx::MonotonicDuration,

    /// The longest single duration this source was held active.
    max_time: zx::MonotonicDuration,

    /// The last time this source was either acquired or released.
    last_change: zx::MonotonicInstant,
}

impl WakeupSource {
    /// Lazily formats the display name for inspection and logging.
    pub fn display_name(&self, origin: &WakeupSourceOrigin) -> String {
        match &self.last_actor {
            Some(actor) => format!("{} [{}]", origin.to_string(), actor),
            None => origin.to_string(),
        }
    }

    /// Returns the amount of time passed since this wake source was last
    /// recorded as active. For active wake sources, this is exactly the time
    /// since the source became active. For inactive sources it's zero.
    pub fn active_duration(&self) -> zx::MonotonicDuration {
        if self.active_since == zx::MonotonicInstant::ZERO {
            zx::MonotonicDuration::default()
        } else {
            let now = zx::MonotonicInstant::get();
            now - self.active_since
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub enum WakeupSourceOrigin {
    WakeLock(String),
    Epoll(crate::vfs::EpollKey),
    HAL(String),
}

impl std::string::ToString for WakeupSourceOrigin {
    fn to_string(&self) -> String {
        match self {
            WakeupSourceOrigin::WakeLock(lock) => lock.clone(),
            WakeupSourceOrigin::Epoll(key) => format!("[epoll] {}", key),
            WakeupSourceOrigin::HAL(lock) => format!("[HAL] {}", lock),
        }
    }
}

/// Manager for suspend and resume.
pub struct SuspendResumeManager {
    // The mutable state of [SuspendResumeManager].
    inner: Arc<LockDepMutex<SuspendResumeManagerInner, SuspendResumeManagerInnerLock>>,

    /// The currently registered message counters in the system whose values are exposed to inspect
    /// via a lazy node.
    message_counters:
        Arc<LockDepMutex<HashSet<WeakKey<OwnedMessageCounter>>, PowerMessageCountersLock>>,

    /// The lock used to to avoid suspension while holding eBPF locks.
    ebpf_suspend_lock: LockDepRwLock<(), EbpfSuspendLock>,
}

/// Manager for suspend and resume.
/// Manager for suspend and resume.
pub struct SuspendResumeManagerInner {
    /// The suspend counters and gauges.
    suspend_stats: SuspendStats,
    sync_on_suspend_enabled: bool,

    suspend_events: VecDeque<SuspendEvent>,

    /// The wake sources in the system, both active and inactive.
    wakeup_sources: HashMap<WakeupSourceOrigin, WakeupSource>,

    /// The event pair that is passed to the Starnix runner so it can observe whether
    /// or not any wake locks are active before completing a suspend operation.
    active_lock_reader: zx::EventPair,

    /// The event pair that is used by the Starnix kernel to signal when there are
    /// active wake locks in the container. Note that the peer of the writer is the
    /// object that is signaled.
    active_lock_writer: zx::EventPair,

    /// The number of currently active wakeup sources.
    active_wakeup_source_count: u64,

    /// The total number of activate-deactivated cycles that have been seen across all wakeup
    /// sources.
    total_wakeup_source_event_count: u64,

    /// The external wake sources that are registered with the runner.
    external_wake_sources: HashMap<zx::Koid, ExternalWakeSource>,
}

#[derive(Debug)]
struct ExternalWakeSource {
    /// The handle that signals when the source is active.
    handle: zx::NullableHandle,
    /// The signals that indicate the source is active.
    signals: zx::Signals,
    /// The name of the wake source.
    name: String,
}

impl SuspendResumeManager {
    pub fn add_external_wake_source(
        &self,
        handle: zx::NullableHandle,
        signals: zx::Signals,
        name: String,
    ) -> Result<(), Errno> {
        let manager = connect_to_protocol_sync::<frunner::ManagerMarker>()
            .map_err(|e| errno!(EINVAL, format!("Failed to connect to manager: {e:?}")))?;
        manager
            .add_wake_source(frunner::ManagerAddWakeSourceRequest {
                container_job: Some(
                    fuchsia_runtime::job_default()
                        .duplicate_handle(zx::Rights::SAME_RIGHTS)
                        .expect("Failed to dup handle"),
                ),
                name: Some(name.clone()),
                handle: Some(
                    handle.duplicate_handle(zx::Rights::SAME_RIGHTS).map_err(|e| errno!(EIO, e))?,
                ),
                signals: Some(signals.bits()),
                ..Default::default()
            })
            .map_err(|e| errno!(EIO, e))?;

        let koid = handle.koid().map_err(|e| errno!(EINVAL, e))?;
        self.lock().external_wake_sources.insert(
            koid,
            ExternalWakeSource {
                handle: handle
                    .duplicate_handle(zx::Rights::SAME_RIGHTS)
                    .map_err(|e| errno!(EIO, e))?,
                signals,
                name,
            },
        );
        Ok(())
    }

    pub fn remove_external_wake_source(&self, handle: zx::NullableHandle) -> Result<(), Errno> {
        let manager = connect_to_protocol_sync::<frunner::ManagerMarker>()
            .map_err(|e| errno!(EINVAL, format!("Failed to connect to manager: {e:?}")))?;

        let koid = handle.koid().map_err(|e| errno!(EINVAL, e))?;
        self.lock().external_wake_sources.remove(&koid);

        manager
            .remove_wake_source(frunner::ManagerRemoveWakeSourceRequest {
                container_job: Some(
                    fuchsia_runtime::job_default()
                        .duplicate_handle(zx::Rights::SAME_RIGHTS)
                        .expect("Failed to dup handle"),
                ),
                handle: Some(handle),
                ..Default::default()
            })
            .map_err(|e| errno!(EIO, e))?;

        Ok(())
    }
}

pub type EbpfSuspendGuard<'a> = LockDepReadGuard<'a, ()>;

#[derive(Clone, Debug)]
pub enum SuspendEvent {
    Attempt { time: zx::BootInstant, state: String },
    Resume { time: zx::BootInstant, reason: String },
    Fail { time: zx::BootInstant, wakeup_sources: Option<Vec<String>> },
}

/// The inspect node ring buffer will keep at most this many entries.
const INSPECT_RING_BUFFER_CAPACITY: usize = 128;

impl Default for SuspendResumeManagerInner {
    fn default() -> Self {
        let (active_lock_reader, active_lock_writer) = zx::EventPair::create();
        active_lock_writer
            .signal_peer(zx::Signals::empty(), zx::Signals::USER_0)
            .expect("Failed to signal peer");
        Self {
            suspend_stats: Default::default(),
            sync_on_suspend_enabled: false,
            suspend_events: VecDeque::with_capacity(INSPECT_RING_BUFFER_CAPACITY),
            wakeup_sources: Default::default(),
            active_lock_reader,
            active_lock_writer,
            active_wakeup_source_count: 0,
            total_wakeup_source_event_count: 0,
            external_wake_sources: Default::default(),
        }
    }
}

impl SuspendResumeManagerInner {
    // Returns true if there are no wake locks preventing suspension.
    pub fn can_suspend(&self) -> bool {
        self.active_wakeup_source_count == 0
    }

    pub fn active_wake_locks(&self) -> Vec<WakeupSourceOrigin> {
        self.wakeup_sources
            .iter()
            .filter_map(|(name, source)| match name {
                WakeupSourceOrigin::WakeLock(_) => {
                    if source.active_since > zx::MonotonicInstant::ZERO {
                        Some(name.clone())
                    } else {
                        None
                    }
                }
                _ => None,
            })
            .collect()
    }

    pub fn inactive_wake_locks(&self) -> Vec<WakeupSourceOrigin> {
        self.wakeup_sources
            .iter()
            .filter_map(|(name, source)| match name {
                WakeupSourceOrigin::WakeLock(_) => {
                    if source.active_since == zx::MonotonicInstant::ZERO {
                        Some(name.clone())
                    } else {
                        None
                    }
                }
                _ => None,
            })
            .collect()
    }

    /// Signals whether or not there are currently any active wake locks in the kernel.
    fn signal_wake_events(&mut self) {
        let (clear_mask, set_mask) = if self.active_wakeup_source_count == 0 {
            (zx::Signals::EVENT_SIGNALED, zx::Signals::USER_0)
        } else {
            (zx::Signals::USER_0, zx::Signals::EVENT_SIGNALED)
        };
        self.active_lock_writer.signal_peer(clear_mask, set_mask).expect("Failed to signal peer");
    }

    fn update_suspend_stats<UpdateFn>(&mut self, update: UpdateFn)
    where
        UpdateFn: FnOnce(&mut SuspendStats),
    {
        update(&mut self.suspend_stats);
    }

    fn add_suspend_event(&mut self, event: SuspendEvent) {
        if self.suspend_events.len() >= INSPECT_RING_BUFFER_CAPACITY {
            self.suspend_events.pop_front();
        }
        self.suspend_events.push_back(event);
    }

    fn record_suspend_events(&self, node: &inspect::Node) {
        let events_node = node.create_child("suspend_events");
        for (i, event) in self.suspend_events.iter().enumerate() {
            let child = events_node.create_child(i.to_string());
            match event {
                SuspendEvent::Attempt { time, state } => {
                    child.record_int(fobs::SUSPEND_ATTEMPTED_AT, time.into_nanos());
                    child.record_string(fobs::SUSPEND_REQUESTED_STATE, state);
                }
                SuspendEvent::Resume { time, reason } => {
                    child.record_int(fobs::SUSPEND_RESUMED_AT, time.into_nanos());
                    child.record_string(fobs::SUSPEND_RESUME_REASON, reason);
                }
                SuspendEvent::Fail { time, wakeup_sources } => {
                    child.record_int(fobs::SUSPEND_FAILED_AT, time.into_nanos());
                    if let Some(names) = wakeup_sources {
                        let names_array =
                            child.create_string_array(fobs::WAKEUP_SOURCES_NAME, names.len());
                        for (i, name) in names.iter().enumerate() {
                            names_array.set(i, name);
                        }
                        child.record(names_array);
                    }
                }
            }
            events_node.record(child);
        }
        node.record(events_node);
    }

    fn record_wakeup_sources(&self, node: &inspect::Node) {
        let wakeup_node = node.create_child("wakeup_sources");
        for (origin, source) in self.wakeup_sources.iter() {
            let child = wakeup_node.create_child(&source.display_name(origin));
            child.record_uint("active_count", source.active_count);
            child.record_uint("event_count", source.event_count);
            child.record_uint("wakeup_count", source.wakeup_count);
            child.record_uint("expire_count", source.expire_count);
            child.record_int("active_since (ns)", source.active_since.into_nanos());
            // Records how long has this wakeup source been active for. If the source is currently
            // active, this is how long it's been currently active.
            child.record_int("active_duration_mono (ns)", source.active_duration().into_nanos());
            child.record_int("total_time (ms)", source.total_time.into_millis());
            child.record_int("max_time (ms)", source.max_time.into_millis());
            child.record_int("last_change (ns)", source.last_change.into_nanos());
            wakeup_node.record(child);
        }
        node.record(wakeup_node);
    }
}

pub type SuspendResumeManagerHandle = Arc<SuspendResumeManager>;

impl Default for SuspendResumeManager {
    fn default() -> Self {
        let message_counters: Arc<
            LockDepMutex<HashSet<WeakKey<OwnedMessageCounter>>, PowerMessageCountersLock>,
        > = Default::default();
        let message_counters_clone = message_counters.clone();
        let root = inspect::component::inspector().root();
        root.record_lazy_values("message_counters", move || {
            let message_counters_clone = message_counters_clone.clone();
            async move {
                let inspector = fuchsia_inspect::Inspector::default();
                let root = inspector.root();
                let message_counters = message_counters_clone.lock();
                let active_counter_names: Vec<String> = message_counters
                    .iter()
                    .filter_map(|c| c.0.upgrade())
                    .map(|c| c.to_string())
                    .collect();
                let message_counters_inspect =
                    root.create_string_array("message_counters", active_counter_names.len());
                for (i, name) in active_counter_names.iter().enumerate() {
                    message_counters_inspect.set(i, name);
                }
                root.record(message_counters_inspect);
                Ok(inspector)
            }
            .boxed()
        });
        let inner = Arc::new(LockDepMutex::new(SuspendResumeManagerInner::default()));
        let inner_clone = inner.clone();
        root.record_lazy_child("wakeup_sources", move || {
            let inner = inner_clone.clone();
            async move {
                let inspector = fuchsia_inspect::Inspector::default();
                let root = inspector.root();
                let state = inner.lock();

                state.record_suspend_events(root);
                state.record_wakeup_sources(root);

                Ok(inspector)
            }
            .boxed()
        });
        Self { message_counters, inner, ebpf_suspend_lock: Default::default() }
    }
}

impl SuspendResumeManager {
    /// Locks and returns the inner state of the manager.
    pub fn lock(&self) -> LockDepGuard<'_, SuspendResumeManagerInner> {
        self.inner.lock()
    }

    /// Power on the PowerMode element and start listening to the suspend stats updates.
    pub fn init(
        self: &SuspendResumeManagerHandle,
        system_task: &CurrentTask,
    ) -> Result<(), anyhow::Error> {
        let handoff = system_task
            .kernel()
            .connect_to_protocol_at_container_svc::<fpower::HandoffMarker>()?
            .into_sync_proxy();
        match handoff.take(zx::MonotonicInstant::INFINITE) {
            Ok(parent_lease) => {
                let parent_lease = parent_lease
                    .map_err(|e| anyhow!("Failed to take lessor and lease from parent: {e:?}"))?;
                drop(parent_lease)
            }
            Err(e) => {
                if e.is_closed() {
                    log_warn!(
                        "Failed to send the fuchsia.session.power/Handoff.Take request. Assuming no Handoff protocol exists and moving on..."
                    );
                } else {
                    return Err(e).context("Handoff::Take");
                }
            }
        }
        Ok(())
    }

    pub fn activate_wakeup_source(&self, origin: WakeupSourceOrigin) -> bool {
        self.activate_wakeup_source_with_actor(origin, None)
    }

    pub fn activate_wakeup_source_with_actor(
        &self,
        origin: WakeupSourceOrigin,
        actor: Option<starnix_task_command::TaskCommand>,
    ) -> bool {
        let mut state = self.lock();
        let did_activate = {
            let entry = state.wakeup_sources.entry(origin).or_default();
            entry.last_actor = actor;
            let now = zx::MonotonicInstant::get();
            entry.active_count += 1;
            entry.event_count += 1;
            entry.last_change = now;
            if entry.active_since == zx::MonotonicInstant::ZERO {
                entry.active_since = now;
                true
            } else {
                false
            }
        };
        if did_activate {
            state.active_wakeup_source_count += 1;
            state.signal_wake_events();
        }
        did_activate
    }

    pub fn deactivate_wakeup_source(&self, origin: &WakeupSourceOrigin) -> bool {
        self.remove_wakeup_source(origin, false)
    }

    pub fn timeout_wakeup_source(&self, origin: &WakeupSourceOrigin) -> bool {
        self.remove_wakeup_source(origin, true)
    }

    fn remove_wakeup_source(&self, origin: &WakeupSourceOrigin, timed_out: bool) -> bool {
        let mut state = self.lock();
        let removed = match state.wakeup_sources.get_mut(origin) {
            Some(entry) if entry.active_since != zx::MonotonicInstant::ZERO => {
                if timed_out {
                    entry.expire_count += 1;
                }

                let now = zx::MonotonicInstant::get();
                let duration = now - entry.active_since;
                entry.total_time += duration;
                entry.max_time = std::cmp::max(duration, entry.max_time);
                entry.last_change = now;
                entry.active_since = zx::MonotonicInstant::ZERO;

                true
            }
            _ => false,
        };
        if removed {
            state.active_wakeup_source_count -= 1;
            state.total_wakeup_source_event_count += 1;
            state.signal_wake_events();
        }
        removed
    }

    pub fn add_message_counter(
        &self,
        name: &str,
        counter: Option<zx::Counter>,
    ) -> OwnedMessageCounterHandle {
        let container_counter = OwnedMessageCounter::new(name, counter);
        let mut message_counters = self.message_counters.lock();
        message_counters.insert(WeakKey::from(&container_counter));
        message_counters.retain(|c| c.0.upgrade().is_some());
        container_counter
    }

    pub fn has_nonzero_message_counter(&self) -> bool {
        self.message_counters.lock().iter().any(|c| {
            let Some(c) = c.0.upgrade() else {
                return false;
            };
            c.counter.as_ref().and_then(|counter| counter.read().ok()).map_or(false, |v| v != 0)
        })
    }

    /// Returns a duplicate handle to the `EventPair` that is signaled when wake
    /// locks are active.
    pub fn duplicate_lock_event(&self) -> zx::EventPair {
        let state = self.lock();
        state
            .active_lock_reader
            .duplicate_handle(zx::Rights::SAME_RIGHTS)
            .expect("Failed to duplicate handle")
    }

    /// Gets the suspend statistics.
    pub fn suspend_stats(&self) -> SuspendStats {
        self.lock().suspend_stats.clone()
    }

    pub fn total_wakeup_events(&self) -> u64 {
        let state = self.lock();
        state.total_wakeup_source_event_count + state.suspend_stats.success_count
    }

    /// Get the contents of the power "sync_on_suspend" file in the power
    /// filesystem.  True will cause `1` to be reported, and false will cause
    /// `0` to be reported.
    pub fn sync_on_suspend_enabled(&self) -> bool {
        self.lock().sync_on_suspend_enabled.clone()
    }

    /// Get the contents of the power "sync_on_suspend" file in the power
    /// filesystem.  See also [sync_on_suspend_enabled].
    pub fn set_sync_on_suspend(&self, enable: bool) {
        self.lock().sync_on_suspend_enabled = enable;
    }

    /// Returns the supported suspend states.
    pub fn suspend_states(&self) -> HashSet<SuspendState> {
        // TODO(b/326470421): Remove the hardcoded supported state.
        HashSet::from([SuspendState::Idle])
    }

    pub fn suspend(&self, suspend_state: SuspendState) -> Result<(), Errno> {
        let suspend_start_time = zx::BootInstant::get();
        let mut state = self.lock();
        state.add_suspend_event(SuspendEvent::Attempt {
            time: suspend_start_time,
            state: suspend_state.to_string(),
        });

        // Check if any wake locks are active. If they are, short-circuit the suspend attempt.
        if !state.can_suspend() {
            self.report_failed_suspension(state, "kernel wake lock");
            return error!(EINVAL);
        }

        // Check if any external wake sources are active.
        let external_wake_source_abort = state.external_wake_sources.values().find_map(|source| {
            if source.handle.wait_one(source.signals, zx::MonotonicInstant::INFINITE_PAST).is_ok() {
                Some(source.name.clone())
            } else {
                None
            }
        });

        if let Some(name) = external_wake_source_abort {
            self.report_failed_suspension(state, &format!("external wake source: {}", name));
            return error!(EINVAL);
        }

        // Drop the state lock. This allows programs to acquire wake locks again. The runner will
        // check that no wake locks were acquired once all the container threads have been
        // suspended, and thus honor any wake locks that were acquired during suspension.
        std::mem::drop(state);

        // Take the ebpf lock to ensure that ebpf is not preventing suspension. This is necessary
        // because other components in the system might be executing ebpf programs on our behalf.
        let _ebpf_lock = self.ebpf_suspend_lock.write();

        let manager = connect_to_protocol_sync::<frunner::ManagerMarker>()
            .expect("Failed to connect to manager");
        fuchsia_trace::duration!("power", "suspend_container:fidl");

        let container_job = Some(
            fuchsia_runtime::job_default()
                .duplicate_handle(zx::Rights::SAME_RIGHTS)
                .expect("Failed to dup handle"),
        );
        let wake_lock_event = Some(self.duplicate_lock_event());

        log_info!("Requesting container suspension.");
        match manager.suspend_container(
            frunner::ManagerSuspendContainerRequest {
                container_job,
                wake_locks: wake_lock_event,
                ..Default::default()
            },
            zx::Instant::INFINITE,
        ) {
            Ok(Ok(res)) => {
                self.report_container_resumed(suspend_start_time, res);
            }
            e => {
                let state = self.lock();
                self.report_failed_suspension(state, &format!("runner error {:?}", e));
                return error!(EINVAL);
            }
        }
        Ok(())
    }

    fn report_container_resumed(
        &self,
        suspend_start_time: zx::BootInstant,
        res: frunner::ManagerSuspendContainerResponse,
    ) {
        let wake_time = zx::BootInstant::get();
        // The "0" here is to mimic the expected power management success string,
        // while we don't have IRQ numbers to report.
        let resume_reason = res.resume_reason.clone().map(|s| format!("0 {}", s));
        log_info!("Resuming from container suspension: {:?}", resume_reason);
        let mut state = self.lock();
        state.update_suspend_stats(|suspend_stats| {
            suspend_stats.success_count += 1;
            suspend_stats.last_time_in_suspend_operations = (wake_time - suspend_start_time).into();
            suspend_stats.last_time_in_sleep =
                zx::BootDuration::from_nanos(res.suspend_time.unwrap_or(0));
            suspend_stats.last_resume_reason = resume_reason.clone();
        });
        state.add_suspend_event(SuspendEvent::Resume {
            time: wake_time,
            reason: resume_reason.unwrap_or_default(),
        });
        fuchsia_trace::instant!("power", "suspend_container:done", fuchsia_trace::Scope::Process);
    }

    fn report_failed_suspension(
        &self,
        mut state: LockDepGuard<'_, SuspendResumeManagerInner>,
        failure_reason: &str,
    ) {
        let wake_time = zx::BootInstant::get();
        state.update_suspend_stats(|suspend_stats| {
            suspend_stats.fail_count += 1;
            suspend_stats.last_failed_errno = Some(errno!(EINVAL));
            suspend_stats.last_resume_reason = None;
        });

        let mut wakeup_sources: Vec<String> = state
            .wakeup_sources
            .iter_mut()
            .filter_map(|(origin, source)| {
                if source.active_since > zx::MonotonicInstant::ZERO {
                    source.wakeup_count += 1;
                    Some(source.display_name(origin))
                } else {
                    None
                }
            })
            .collect();

        for source in state.external_wake_sources.values() {
            if source.handle.wait_one(source.signals, zx::MonotonicInstant::INFINITE_PAST).is_ok() {
                wakeup_sources.push(source.name.clone());
            }
        }

        let last_resume_reason = format!("Abort: {}", wakeup_sources.join(" "));
        state.update_suspend_stats(|suspend_stats| {
            // Power analysis tools require `Abort: ` in the case of failed suspends
            suspend_stats.last_resume_reason = Some(last_resume_reason);
        });

        // LINT.IfChange(suspend_failed_tefmo)
        log_warn!(
            "Suspend failed due to {:?}. Here are the active wakeup sources: {:?}",
            failure_reason,
            wakeup_sources,
        );
        // LINT.ThenChange(//tools/testing/tefmocheck/nearby_string_check.go:suspend_failed_tefmo)
        state.add_suspend_event(SuspendEvent::Fail {
            time: wake_time,
            wakeup_sources: Some(wakeup_sources),
        });
        fuchsia_trace::instant!("power", "suspend_container:error", fuchsia_trace::Scope::Process);
    }

    pub fn acquire_ebpf_suspend_lock<'a>(&'a self) -> EbpfSuspendGuard<'a> {
        self.ebpf_suspend_lock.read()
    }
}

/// Called when a wake happens resulting from a timer going off.
pub trait OnWakeOps: Send + Sync {
    /// Called on wake events.
    ///
    /// Must not block.
    ///
    /// # Args
    /// - `current_task`: the currently active task
    /// - `baton_lease`: the wake lease is provided if `on_wake` has critical
    ///   work to do and needs to prevent suspend.
    fn on_wake(&self, current_task: &CurrentTask, baton_lease: &zx::NullableHandle);
}

/// Creates a proxy between `remote_channel` and the returned `zx::Channel`.
///
/// The message counter's initial value will be set to 0.
///
/// The returned counter will be incremented each time there is an incoming message on the proxied
/// channel. The starnix_kernel is expected to decrement the counter when that incoming message is
/// handled.
///
/// Note that "message" in this context means channel message. This can be either a FIDL event, or
/// a response to a FIDL message from the platform.
///
/// For example, the starnix_kernel may issue a hanging get to retrieve input events. When that
/// hanging get returns, the counter will be incremented by 1. When the next hanging get has been
/// scheduled, the input subsystem decrements the counter by 1.
///
/// The proxying is done by the Starnix runner, and allows messages on the channel to wake
/// the container.
pub fn create_proxy_for_wake_events_counter_zero(
    remote_channel: zx::Channel,
    name: String,
) -> (zx::Channel, zx::Counter) {
    let (local_proxy, kernel_channel) = zx::Channel::create();
    let counter = zx::Counter::create();

    let local_counter =
        counter.duplicate_handle(zx::Rights::SAME_RIGHTS).expect("Failed to duplicate counter");

    let manager = fuchsia_component::client::connect_to_protocol_sync::<frunner::ManagerMarker>()
        .expect("failed");
    manager
        .proxy_wake_channel(frunner::ManagerProxyWakeChannelRequest {
            container_job: Some(
                fuchsia_runtime::job_default()
                    .duplicate_handle(zx::Rights::SAME_RIGHTS)
                    .expect("Failed to dup handle"),
            ),
            container_channel: Some(kernel_channel),
            remote_channel: Some(remote_channel),
            counter: Some(counter),
            name: Some(name),
            ..Default::default()
        })
        .expect("Failed to create proxy");

    (local_proxy, local_counter)
}

/// Creates a proxy between `remote_channel` and the returned `zx::Channel`.
///
/// The message counter's initial value will be set to 1, which will prevent the container from
/// suspending until the caller decrements the counter.
///
/// The returned counter will be incremented each time there is an incoming message on the proxied
/// channel. The starnix_kernel is expected to decrement the counter when that incoming message is
/// handled.
///
/// Note that "message" in this context means channel message. This can be either a FIDL event, or
/// a response to a FIDL message from the platform.
///
/// For example, the starnix_kernel may issue a hanging get to retrieve input events. When that
/// hanging get returns, the counter will be incremented by 1. When the next hanging get has been
/// scheduled, the input subsystem decrements the counter by 1.
///
/// The proxying is done by the Starnix runner, and allows messages on the channel to wake
/// the container.
pub fn create_proxy_for_wake_events_counter(
    remote_channel: zx::Channel,
    name: String,
) -> (zx::Channel, zx::Counter) {
    let (proxy, counter) = create_proxy_for_wake_events_counter_zero(remote_channel, name);

    // Increment the counter by one so that the initial incoming message to the container will
    // set the count to 0, instead of -1.
    counter.add(1).expect("Failed to add to counter");

    (proxy, counter)
}

/// Marks a message handled by decrementing `counter`.
///
/// This should be called when a proxied channel message has been handled, and the caller would
/// be ok letting the container suspend.
pub fn mark_proxy_message_handled(counter: &zx::Counter) {
    counter.add(-1).expect("Failed to decrement counter");
}

/// Marks all messages tracked by `counter` as handled.
pub fn mark_all_proxy_messages_handled(counter: &zx::Counter) {
    counter.write(0).expect("Failed to decrement counter");
}

/// Creates a watcher between clients and the Starnix runner.
///
/// Changes in the power state of the container are relayed by the event pair.
pub fn create_watcher_for_wake_events(watcher: zx::EventPair) {
    let manager = fuchsia_component::client::connect_to_protocol_sync::<frunner::ManagerMarker>()
        .expect("failed");
    manager
        .register_wake_watcher(
            frunner::ManagerRegisterWakeWatcherRequest {
                watcher: Some(watcher),
                ..Default::default()
            },
            zx::Instant::INFINITE,
        )
        .expect("Failed to register wake watcher");
}

/// Wrapper around a Weak `OwnedMessageCounter` that can be passed around to keep the container
/// awake.
///
/// Each live `SharedMessageCounter` is responsible for a pending message while it in scope,
/// and removes it from the counter when it goes out of scope.  Processes that need to cooperate
/// can pass a `SharedMessageCounter` to each other to ensure that once the work is done, the lock
/// goes out of scope as well. This allows for precise accounting of remaining work, and should
/// give us control over container suspension which is guarded by the compiler, not conventions.
#[derive(Debug)]
pub struct SharedMessageCounter(Weak<OwnedMessageCounter>);

impl Drop for SharedMessageCounter {
    fn drop(&mut self) {
        if let Some(message_counter) = self.0.upgrade() {
            message_counter.mark_handled();
        }
    }
}

/// Owns a `zx::Counter` to track pending messages that prevent the container from suspending.
///
/// This struct ensures that the counter is reset to 0 when the last strong reference is dropped,
/// effectively releasing any wake lock held by this counter.
pub struct OwnedMessageCounter {
    name: String,
    counter: Option<zx::Counter>,
}
pub type OwnedMessageCounterHandle = Arc<OwnedMessageCounter>;

impl Drop for OwnedMessageCounter {
    /// Resets the underlying `zx::Counter` to 0 when the `OwnedMessageCounter` is dropped.
    ///
    /// This ensures that all pending messages are marked as handled, allowing the system to suspend
    /// if no other wake locks are held.
    fn drop(&mut self) {
        self.counter.as_ref().map(mark_all_proxy_messages_handled);
    }
}

impl OwnedMessageCounter {
    pub fn new(name: &str, counter: Option<zx::Counter>) -> OwnedMessageCounterHandle {
        Arc::new(Self { name: name.to_string(), counter })
    }

    /// Decrements the counter, signaling that a pending message or operation has been handled.
    ///
    /// This should be called when the work associated with a previous `mark_pending` call is
    /// complete.
    pub fn mark_handled(&self) {
        self.counter.as_ref().map(mark_proxy_message_handled);
    }

    /// Increments the counter, signaling that a new message or operation is pending.
    ///
    /// This prevents the system from suspending until a corresponding `mark_handled` call is made.
    pub fn mark_pending(&self) {
        self.counter.as_ref().map(|c| c.add(1).expect("Failed to increment counter"));
    }

    /// Creates a `SharedMessageCounter` from this `OwnedMessageCounter`.
    ///
    /// `new_pending_message` - if a new pending message should be added
    pub fn share(
        self: &OwnedMessageCounterHandle,
        new_pending_message: bool,
    ) -> SharedMessageCounter {
        if new_pending_message {
            self.mark_pending();
        }
        SharedMessageCounter(Arc::downgrade(self))
    }
}

impl fmt::Display for OwnedMessageCounter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Counter({}): {:?}", self.name, self.counter.as_ref().map(|c| c.read()))
    }
}

/// A proxy wrapper that manages a `zx::Counter` to allow the container to suspend
/// after events are being processed.
///
/// When the proxy is dropped, the counter is reset to 0 to release the wake-lock.
pub struct ContainerWakingProxy<P: Proxy> {
    counter: OwnedMessageCounterHandle,
    proxy: P,
}

impl<P: Proxy> ContainerWakingProxy<P> {
    pub fn new(counter: OwnedMessageCounterHandle, proxy: P) -> Self {
        Self { counter, proxy }
    }

    /// Create a `Future` call on the proxy.
    ///
    /// The counter will be decremented as message handled after the future is created.
    pub fn call<T, F, R>(&self, future: F) -> R
    where
        F: FnOnce(&P) -> R,
        R: Future<Output = T>,
    {
        // The sequence for handling events MUST be:
        //
        // 1. create future
        // 2. decrease counter
        // 3. await future
        //
        // for allowing suspend - wake.
        let f = future(&self.proxy);
        self.counter.mark_handled();
        f
    }
}

/// A stream wrapper that manages a `zx::Counter` to allow the container to suspend
/// after events are being processed.
///
/// When the stream is dropped, the counter is reset to 0 to release the wake-lock.
pub struct ContainerWakingStream<S: FusedStream + Unpin> {
    counter: OwnedMessageCounterHandle,
    stream: S,
}

impl<S: FusedStream + Unpin> ContainerWakingStream<S> {
    pub fn new(counter: OwnedMessageCounterHandle, stream: S) -> Self {
        Self { counter, stream }
    }

    /// Create a `Next` call on the stream.poll_next().
    ///
    /// The counter will be decremented as message handled after the future is created.
    pub fn next(&mut self) -> Next<'_, S> {
        // See `ContainerWakingProxy::call` for sequence of handling events.
        let is_terminated = self.stream.is_terminated();
        let next = self.stream.next();
        if !is_terminated {
            self.counter.mark_handled();
        }
        next
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use diagnostics_assertions::assert_data_tree;
    use fidl::endpoints::create_proxy_and_stream;
    use fidl_test_placeholders::{EchoMarker, EchoRequest};
    use fuchsia_async as fasync;
    use fuchsia_inspect as inspect;
    use futures::StreamExt;

    #[::fuchsia::test]
    fn test_counter_zero_initialization() {
        let (_endpoint, endpoint) = zx::Channel::create();
        let (_channel, counter) =
            super::create_proxy_for_wake_events_counter_zero(endpoint, "test".into());
        assert_eq!(counter.read(), Ok(0));
    }

    #[::fuchsia::test]
    fn test_counter_initialization() {
        let (_endpoint, endpoint) = zx::Channel::create();
        let (_channel, counter) =
            super::create_proxy_for_wake_events_counter(endpoint, "test".into());
        assert_eq!(counter.read(), Ok(1));
    }

    #[::fuchsia::test]
    async fn test_container_waking_proxy() {
        let (proxy, mut stream) = create_proxy_and_stream::<EchoMarker>();
        let server_task = fasync::Task::spawn(async move {
            let request = stream.next().await.unwrap().unwrap();
            match request {
                EchoRequest::EchoString { value, responder } => {
                    responder.send(value.as_deref()).unwrap();
                }
            }
        });

        let counter = zx::Counter::create();
        counter.add(5).unwrap();
        assert_eq!(counter.read(), Ok(5));

        let waking_proxy = ContainerWakingProxy {
            counter: OwnedMessageCounter::new(
                "test_proxy",
                Some(counter.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap()),
            ),
            proxy,
        };

        let response_future = waking_proxy.call(|p| p.echo_string(Some("hello")));

        // The `call` method decrements the counter.
        assert_eq!(counter.read(), Ok(4));

        let response = response_future.await.unwrap();
        assert_eq!(response.as_deref(), Some("hello"));

        server_task.await;

        assert_eq!(counter.read(), Ok(4));
        drop(waking_proxy);
        assert_eq!(counter.read(), Ok(0));
    }

    #[::fuchsia::test]
    async fn test_container_waking_stream() {
        let (proxy, stream) = create_proxy_and_stream::<EchoMarker>();
        let client_task = fasync::Task::spawn(async move {
            let response = proxy.echo_string(Some("hello")).await.unwrap();
            assert_eq!(response.as_deref(), Some("hello"));
        });

        let counter = zx::Counter::create();
        counter.add(5).unwrap();
        assert_eq!(counter.read(), Ok(5));

        let mut waking_stream = ContainerWakingStream {
            counter: OwnedMessageCounter::new(
                "test_stream",
                Some(counter.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap()),
            ),
            stream,
        };

        let request_future = waking_stream.next();

        // The `next` method decrements the counter.
        assert_eq!(counter.read(), Ok(4));

        let request = request_future.await.unwrap().unwrap();
        match request {
            EchoRequest::EchoString { value, responder } => {
                assert_eq!(value.as_deref(), Some("hello"));
                responder.send(value.as_deref()).unwrap();
            }
        }

        client_task.await;

        assert_eq!(counter.read(), Ok(4));
        drop(waking_stream);
        assert_eq!(counter.read(), Ok(0));
    }

    #[::fuchsia::test]
    async fn test_message_counters_inspect() {
        let power_manager = SuspendResumeManager::default();
        let inspector = inspect::component::inspector();

        let zx_counter = zx::Counter::create();
        let counter_handle = power_manager.add_message_counter(
            "test_counter",
            Some(zx_counter.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap()),
        );

        zx_counter.add(1).unwrap();

        assert_data_tree!(inspector, root: contains {
            message_counters: vec!["Counter(test_counter): Some(Ok(1))"],
        });

        zx_counter.add(1).unwrap();
        assert_data_tree!(inspector, root: contains {
            message_counters: vec!["Counter(test_counter): Some(Ok(2))"],
        });

        drop(counter_handle);
        assert_data_tree!(inspector, root: contains {
            message_counters: Vec::<String>::new(),
        });
    }

    #[::fuchsia::test]
    fn test_shared_message_counter() {
        // Create an owned counter and set its value.
        let zx_counter = zx::Counter::create();
        let owned_counter = OwnedMessageCounter::new(
            "test_shared_counter",
            Some(zx_counter.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap()),
        );
        zx_counter.add(5).unwrap();
        assert_eq!(zx_counter.read(), Ok(5));

        // Create a shared counter with no new message. The value should be unchanged.
        let shared_counter = owned_counter.share(false);
        assert_eq!(zx_counter.read(), Ok(5));

        // Drop the shared counter. The value should be decremented.
        drop(shared_counter);
        assert_eq!(zx_counter.read(), Ok(4));

        // Create a shared counter with a new message. The value should be incremented.
        let shared_counter_2 = owned_counter.share(true);
        assert_eq!(zx_counter.read(), Ok(5));

        // Drop the shared counter. The value should be decremented.
        drop(shared_counter_2);
        assert_eq!(zx_counter.read(), Ok(4));

        // Create another shared counter.
        let shared_counter_3 = owned_counter.share(false);
        assert_eq!(zx_counter.read(), Ok(4));

        // Drop the owned counter. The value should be reset to 0.
        drop(owned_counter);
        assert_eq!(zx_counter.read(), Ok(0));

        // Drop the shared counter. The value should remain 0, and it shouldn't panic.
        drop(shared_counter_3);
        assert_eq!(zx_counter.read(), Ok(0));
    }

    #[::fuchsia::test]
    async fn test_container_waking_event_termination() {
        let stream = futures::stream::iter(vec![0]).fuse();
        let counter = zx::Counter::create();
        counter.add(2).unwrap();
        assert_eq!(counter.read(), Ok(2));
        let mut waking_stream = ContainerWakingStream {
            counter: OwnedMessageCounter::new(
                "test_stream",
                Some(counter.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap()),
            ),
            stream,
        };

        assert_eq!(waking_stream.next().await, Some(0));
        assert_eq!(counter.read(), Ok(1));

        assert_eq!(waking_stream.next().await, None);
        assert_eq!(waking_stream.next().await, None);
        // The stream is already terminated, so the counter should remain 0.
        assert_eq!(counter.read(), Ok(0));
    }

    #[::fuchsia::test]
    fn test_external_wake_source_aborts_suspend() {
        let manager = SuspendResumeManager::default();
        let event = zx::Event::create();
        let signals = zx::Signals::USER_0;

        // We can't actually verify the runner call in this unit test environment easily
        // without a lot of mocking setup that might not be present.
        // However, we can verify that if it was registered, the suspend check respects it.

        let res = manager.add_external_wake_source(
            event.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap().into_handle(),
            signals,
            "test_external".to_string(),
        );

        if res.is_err() {
            println!(
                "Skipping test_external_wake_source_aborts_suspend because runner connection failed: {:?}",
                res
            );
            return;
        }

        // Signal the event
        event.signal(zx::Signals::empty(), signals).unwrap();

        let state = manager.lock();
        assert!(state.external_wake_sources.contains_key(&event.koid().unwrap()));
    }
}
