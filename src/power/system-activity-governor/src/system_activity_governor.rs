// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::cpu_manager::{
    CpuManager, SuspendBlockManager, SuspendResult, SuspendResumeListener, SuspendStatsUpdater,
};
use crate::events::{SagEvent, SagEventLogger};
use anyhow::Result;
use async_trait::async_trait;
use async_utils::hanging_get::server::{HangingGet, Publisher};
use fidl::endpoints::{Proxy, ServerEnd, create_endpoints};
use fidl_fuchsia_feedback as ffeedback;
use fidl_fuchsia_hardware_power_statecontrol as fstatecontrol;
use fidl_fuchsia_power_broker as fbroker;
use fidl_fuchsia_power_cpu_manager as fcpumanager;
use fidl_fuchsia_power_observability as fobs;
use fidl_fuchsia_power_suspend as fsuspend;
use fidl_fuchsia_power_system::{
    self as fsystem, ApplicationActivityLevel, CpuLevel, ExecutionStateLevel,
};
use fuchsia_async::{self as fasync, TimeoutExt};
use fuchsia_inspect::{
    ArrayProperty, BoolProperty as IBool, Inspector, IntProperty as IInt, LazyNode, Node as INode,
    NumericProperty, Property, UintProperty as IUint,
};

use futures::channel::oneshot;
use futures::future::FutureExt;
use futures::lock::Mutex;
use futures::prelude::*;
use futures::stream::StreamExt;
use power_broker_client::PowerElementContext;
use sag_config::Config;
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

pub struct StoredWakeLease {
    pub name: String,
    pub server_token: fsystem::LeaseToken,
    pub is_unmonitored: bool,
}

pub struct StoredSuspendBlocker {
    pub name: String,
    pub suspend_blocker: fsystem::SuspendBlockerProxy,
    pub server_token: fsystem::LeaseToken,
}

pub struct StoredApplicationActivityLease {
    pub name: String,
    pub server_token: fsystem::LeaseToken,
}

// TODO(https://fxbug.dev/491840509): Allow configurable timeouts when needed.
const RESUME_SUSPENDING_LEASE_DROP_DELAY: std::time::Duration =
    std::time::Duration::from_millis(100);
const SUSPEND_BLOCKER_WARNING_TIMEOUT: fasync::MonotonicDuration =
    fasync::MonotonicDuration::from_seconds(10);
const NO_SUSPEND_CRASH_SIGNATURE: &str = "fuchsia-no-suspend-in-5-application-activity-cycles";
const SUSPEND_LOOP_SIGNATURE: &str = "fuchsia-sag-suspend-callback-loop-detected";

type NotifyFn = Box<dyn Fn(&fsuspend::SuspendStats, fsuspend::StatsWatchResponder) -> bool>;
type StatsHangingGet = HangingGet<fsuspend::SuspendStats, fsuspend::StatsWatchResponder, NotifyFn>;
type StatsPublisher = Publisher<fsuspend::SuspendStats, fsuspend::StatsWatchResponder, NotifyFn>;

#[derive(Copy, Clone)]
enum BootControlLevel {
    Inactive,
    Active,
}

impl From<BootControlLevel> for fbroker::PowerLevel {
    fn from(bc: BootControlLevel) -> Self {
        match bc {
            BootControlLevel::Inactive => 0,
            BootControlLevel::Active => 1,
        }
    }
}

pub struct SuspendStatsManager {
    /// The hanging get handler used to notify subscribers of changes to suspend stats.
    hanging_get: RefCell<StatsHangingGet>,
    /// The publisher used to push changes to suspend stats.
    stats_publisher: StatsPublisher,
    /// The inspect node for suspend stats.
    inspect_node: INode,
    /// The inspect node that contains the number of successful suspend attempts.
    success_count_node: IUint,
    /// The inspect node that contains the number of failed suspend attempts.
    fail_count_node: IUint,
    /// The inspect node that contains the error code of the last failed suspend attempt.
    last_failed_error_node: IInt,
    /// The inspect node that contains the duration the platform spent in suspension in the last
    /// attempt.
    last_time_in_suspend_node: IInt,
    /// The inspect node that contains the total time the platform spent in suspension since boot.
    total_time_in_suspend_node: IInt,
    /// The inspect node that contains the duration the platform spent transitioning to a suspended
    /// state in the last attempt.
    last_time_in_suspend_operations_node: IInt,
}

impl SuspendStatsManager {
    fn new(inspect_node: INode) -> Self {
        let stats = fsuspend::SuspendStats {
            success_count: Some(0),
            fail_count: Some(0),
            total_time_in_suspend: Some(0),
            ..Default::default()
        };

        let success_count_node = inspect_node
            .create_uint(fobs::SUSPEND_SUCCESS_COUNT, *stats.success_count.as_ref().unwrap_or(&0));
        let fail_count_node = inspect_node
            .create_uint(fobs::SUSPEND_FAIL_COUNT, *stats.fail_count.as_ref().unwrap_or(&0));
        let last_failed_error_node = inspect_node.create_int(
            fobs::SUSPEND_LAST_FAILED_ERROR,
            (*stats.last_failed_error.as_ref().unwrap_or(&0i32)).into(),
        );
        let last_time_in_suspend_node = inspect_node.create_int(
            fobs::SUSPEND_LAST_TIMESTAMP,
            *stats.last_time_in_suspend.as_ref().unwrap_or(&-1i64),
        );
        let total_time_in_suspend_node = inspect_node.create_int(
            fobs::SUSPEND_CUMULATIVE_DURATION,
            *stats.total_time_in_suspend.as_ref().unwrap_or(&0i64),
        );
        let last_time_in_suspend_operations_node = inspect_node.create_int(
            fobs::SUSPEND_LAST_DURATION,
            *stats.last_time_in_suspend_operations.as_ref().unwrap_or(&-1i64),
        );

        let hanging_get = StatsHangingGet::new(
            stats,
            Box::new(
                |stats: &fsuspend::SuspendStats, res: fsuspend::StatsWatchResponder| -> bool {
                    if let Err(error) = res.send(stats) {
                        log::warn!(error:?; "Failed to send suspend stats to client");
                    }
                    true
                },
            ),
        );

        let stats_publisher = hanging_get.new_publisher();

        Self {
            hanging_get: RefCell::new(hanging_get),
            stats_publisher,
            inspect_node,
            success_count_node,
            fail_count_node,
            last_failed_error_node,
            last_time_in_suspend_node,
            total_time_in_suspend_node,
            last_time_in_suspend_operations_node,
        }
    }
}

impl SuspendStatsUpdater for SuspendStatsManager {
    fn update<'a>(
        &self,
        update: Box<dyn FnOnce(&mut Option<fsuspend::SuspendStats>) -> bool + 'a>,
    ) {
        self.stats_publisher.update(|stats_opt: &mut Option<fsuspend::SuspendStats>| {
            let success = update(stats_opt);

            self.inspect_node.atomic_update(|_| {
                let stats = stats_opt.as_ref().expect("stats is uninitialized");
                self.success_count_node.set(*stats.success_count.as_ref().unwrap_or(&0));
                self.fail_count_node.set(*stats.fail_count.as_ref().unwrap_or(&0));
                self.last_failed_error_node
                    .set((*stats.last_failed_error.as_ref().unwrap_or(&0i32)).into());
                self.last_time_in_suspend_node
                    .set(*stats.last_time_in_suspend.as_ref().unwrap_or(&-1i64));
                self.total_time_in_suspend_node
                    .set(*stats.total_time_in_suspend.as_ref().unwrap_or(&0i64));
                self.last_time_in_suspend_operations_node
                    .set(*stats.last_time_in_suspend_operations.as_ref().unwrap_or(&-1i64));
            });

            log::info!(success:?, stats_opt:?; "Updating suspend stats");
            success
        });
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum LeaseStatus {
    Satisfied,
    AwaitingSatisfaction,
    FailedSatisfaction,
}

impl LeaseStatus {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Satisfied => fobs::WAKE_LEASE_ITEM_STATUS_SATISFIED,
            Self::AwaitingSatisfaction => fobs::WAKE_LEASE_ITEM_STATUS_AWAITING_SATISFACTION,
            Self::FailedSatisfaction => fobs::WAKE_LEASE_ITEM_STATUS_FAILED_SATISFACTION,
        }
    }
}

/// Helper struct for LeaseManager.
struct ActiveWakeLease {
    name: String,
    lease_type: &'static str,
    is_unmonitored: bool,
    client_token_koid: u64,
    server_token_koid: u64,
    status: RefCell<LeaseStatus>,
    error: RefCell<Option<String>>,
    timer_task: RefCell<Option<fasync::Task<()>>>,
    created_at: zx::BootInstant,
    history_node: RefCell<Option<INode>>,
}

/// Manager of leases that block execution state.
///
/// Used to facilitate the `AcquireWakeLease()` and `TakeApplicationActivityLease()`
/// functionality of `fuchsia.power.system.ActivityGovernor`.
///
/// A wake lease blocks suspension by requiring the power level of the Execution
/// State to be at least [`ExecutionStateLevel::Suspending`].
///
/// An application activity lease requires Application Activity to be at least
/// [`ApplicationActivityLevel::Active`].
struct LeaseManager {
    /// The inspect node for active wake leases.
    _wake_leases_node: LazyNode,
    /// The active wake leases, ordered by lease ID. Because lease IDs are assigned
    /// sequentially, this is also ordered by lease creation time.
    active_wake_leases: Rc<RefCell<BTreeMap<u64, Rc<ActiveWakeLease>>>>,
    /// Logger for system-wide activity governor events.
    sag_event_logger: SagEventLogger,
    /// Proxy to the power topology to create power elements.
    topology: fbroker::TopologyProxy,
    /// Proxy to the lessor for the Execution State power element.
    execution_state_lessor: fbroker::LessorProxy,
    /// Proxy to the lease control service for Execution State.
    /// This lease is owned by async Tasks spawned inside of LeaseManager.
    /// When all Tasks complete, this lease is dropped.
    execution_state_suspending_lease:
        Rc<Mutex<std::rc::Weak<(Option<fbroker::LeaseControlProxy>, Result<()>)>>>,
    /// Dependency token for Application Activity.
    application_activity_assertive_dependency_token: fbroker::DependencyToken,
    /// Used to block suspension in CpuManager while a lease is in-flight but not yet satisfied.
    suspend_block_manager: Rc<SuspendBlockManager>,
    /// The maximum lease ID that has been assigned.
    max_lease_id: AtomicU64,
    /// A shared receiver that is set when the system is about to suspend.
    before_suspend_notifier: Rc<RefCell<Option<futures::future::Shared<oneshot::Receiver<()>>>>>,
    /// Sender for crash reports.
    report_sender: futures::channel::mpsc::UnboundedSender<CrashReportMessage>,
    /// Detector for long-lived wake leases that are not obtained as Application Activity Lease
    /// or Long Wake Lease.
    long_lease_detector: Option<LongLeaseDetector>,
    /// Count of active unmonitored leases.
    active_unmonitored_lease_count: Rc<Cell<u32>>,
}

impl LeaseManager {
    pub fn new(
        parent_node: &INode,
        sag_event_logger: SagEventLogger,
        topology: fbroker::TopologyProxy,
        execution_state_lessor: fbroker::LessorProxy,
        application_activity_assertive_dependency_token: fbroker::DependencyToken,
        suspend_blocker: Rc<SuspendBlockManager>,
        before_suspend_notifier: Rc<
            RefCell<Option<futures::future::Shared<oneshot::Receiver<()>>>>,
        >,
        max_wake_leases_to_log: usize,
        report_sender: futures::channel::mpsc::UnboundedSender<CrashReportMessage>,
        long_lease_threshold: Option<fasync::MonotonicDuration>,
    ) -> Self {
        let active_wake_leases = Rc::new(RefCell::new(BTreeMap::<u64, Rc<ActiveWakeLease>>::new()));
        let active_wake_leases_clone = active_wake_leases.clone();

        let long_lease_detector = long_lease_threshold.map(|threshold| {
            LongLeaseDetector::new(
                parent_node.create_child("long_wake_leases"),
                threshold,
                report_sender.clone(),
            )
        });

        // The wake_leases node is created lazily so we don't have to keep it up to date every time
        // a new lease is created or dropped.
        let callback = move || {
            let active_leases = active_wake_leases_clone.clone();
            async move {
                let inspector = Inspector::default();
                let active = active_leases.borrow();
                inspector.root().record_uint("active_count", active.len() as u64);

                let oldest_active_node = inspector.root().create_child("oldest_active");
                for (lease_id, lease) in active.iter().take(max_wake_leases_to_log) {
                    let lease_node =
                        oldest_active_node.create_child(lease.server_token_koid.to_string());

                    lease_node.record_int(
                        fobs::WAKE_LEASE_ITEM_NODE_CREATED_AT,
                        lease.created_at.into_nanos(),
                    );
                    lease_node.record_string(fobs::WAKE_LEASE_ITEM_NAME, lease.name.clone());
                    lease_node.record_string(fobs::WAKE_LEASE_ITEM_TYPE, lease.lease_type);
                    if lease.is_unmonitored {
                        lease_node.record_bool("is_unmonitored_lease", true);
                    }
                    lease_node.record_uint(
                        fobs::WAKE_LEASE_ITEM_CLIENT_TOKEN_KOID,
                        lease.client_token_koid,
                    );
                    lease_node.record_uint(fobs::WAKE_LEASE_ITEM_ID, *lease_id);
                    lease_node.record_string(
                        fobs::WAKE_LEASE_ITEM_STATUS,
                        lease.status.borrow().as_str(),
                    );
                    if let Some(error) = lease.error.borrow().as_deref() {
                        lease_node.record_string(fobs::WAKE_LEASE_ITEM_ERROR, error);
                    }
                    oldest_active_node.record(lease_node);
                }
                inspector.root().record(oldest_active_node);
                Ok(inspector)
            }
            .boxed_local()
        };
        let wake_leases_node =
            parent_node.create_lazy_child_with_thread_local(fobs::WAKE_LEASES_NODE, callback);

        Self {
            _wake_leases_node: wake_leases_node,
            active_wake_leases,
            sag_event_logger,
            topology,
            execution_state_lessor,
            execution_state_suspending_lease: Rc::new(Mutex::new(std::rc::Weak::new())),
            application_activity_assertive_dependency_token,
            suspend_block_manager: suspend_blocker,
            max_lease_id: AtomicU64::new(0),
            before_suspend_notifier,
            report_sender,
            long_lease_detector,
            active_unmonitored_lease_count: Rc::new(Cell::new(0)),
        }
    }

    // When an unmonitored lease is acquired, cancel the timers for all monitored leases.
    fn handle_unmonitored_lease_acquired(
        is_unmonitored: bool,
        active_unmonitored_lease_count: &Rc<Cell<u32>>,
        active_wake_leases: &Rc<RefCell<BTreeMap<u64, Rc<ActiveWakeLease>>>>,
    ) {
        if is_unmonitored {
            let count = active_unmonitored_lease_count.get() + 1;
            active_unmonitored_lease_count.set(count);
            log::debug!("Unmonitored lease acquired. Active count: {}", count);

            if count == 1 {
                log::debug!("Unmonitored lease active, cancelling regular lease timers.");
                for lease in active_wake_leases.borrow().values() {
                    if !lease.is_unmonitored {
                        lease.timer_task.borrow_mut().take();
                    }
                }
            }
        }
    }

    fn record_duration(
        lease_id: u64,
        active_wake_leases: &Rc<RefCell<BTreeMap<u64, Rc<ActiveWakeLease>>>>,
    ) {
        if let Some(lease) = active_wake_leases.borrow().get(&lease_id) {
            if let Some(node) = lease.history_node.borrow().as_ref() {
                let duration =
                    fasync::BootInstant::now().into_nanos() - lease.created_at.into_nanos();
                node.record_int("duration_ns", duration);
            }
        }
    }

    // When an unmonitored lease is dropped, restart the timers for all monitored leases.
    fn handle_unmonitored_lease_dropped(
        is_unmonitored: bool,
        active_unmonitored_lease_count: &Rc<Cell<u32>>,
        active_wake_leases: &Rc<RefCell<BTreeMap<u64, Rc<ActiveWakeLease>>>>,
        long_lease_detector: &Option<LongLeaseDetector>,
    ) {
        if is_unmonitored {
            let old_count = active_unmonitored_lease_count.get();
            if old_count > 0 {
                let count = old_count - 1;
                active_unmonitored_lease_count.set(count);
                log::debug!("Unmonitored lease dropped. Active count: {}", count);

                if count == 0 {
                    log::debug!("All unmonitored leases dropped, restarting regular lease timers.");
                    for (id, lease) in active_wake_leases.borrow().iter() {
                        if !lease.is_unmonitored && lease.history_node.borrow().is_none() {
                            let timer_task = long_lease_detector.as_ref().map(|detector| {
                                detector.start_timer(
                                    lease.name.clone(),
                                    *id,
                                    active_wake_leases.clone(),
                                    lease.created_at,
                                )
                            });
                            *lease.timer_task.borrow_mut() = timer_task;
                        }
                    }
                }
            } else {
                log::error!("Unmonitored lease dropped but count was already 0");
            }
        }
    }

    async fn create_application_activity_lease(
        &self,
        name: String,
        loop_detector: Option<std::rc::Rc<NoSuspendDetector>>,
    ) -> Result<fsystem::LeaseToken> {
        let (server_token, client_token) = fsystem::LeaseToken::create();
        self.create_application_activity_lease_using_token(name, server_token, loop_detector)
            .await?;
        Ok(client_token)
    }

    async fn create_application_activity_lease_using_token(
        &self,
        name: String,
        server_token: fsystem::LeaseToken,
        loop_detector: Option<std::rc::Rc<NoSuspendDetector>>,
    ) -> Result<()> {
        let lease_token = server_token.duplicate_handle(zx::Rights::SAME_RIGHTS)?;

        log::debug!("Acquiring application activity lease for '{}'", name);
        let sag_event_logger = self.sag_event_logger.clone();
        let lease_id = self.max_lease_id.fetch_add(1, Ordering::Relaxed);
        sag_event_logger.log(SagEvent::WakeLeaseCreated { name: name.clone(), id: lease_id });

        self.topology
            .lease(fbroker::LeaseSchema {
                lease_token: Some(lease_token),
                lease_name: Some(name.clone()),
                dependencies: Some(vec![fbroker::LeaseDependency {
                    requires_token: Some(
                        self.application_activity_assertive_dependency_token
                            .duplicate_handle(zx::Rights::SAME_RIGHTS)?,
                    ),
                    requires_level: Some(ApplicationActivityLevel::Active.into_primitive()),
                    ..Default::default()
                }]),
                ..Default::default()
            })
            .await
            .map_err(|e| anyhow::anyhow!("FIDL error while leasing application activity: {e}"))?
            .map_err(|e| {
                sag_event_logger.log(SagEvent::WakeLeaseSatisfactionFailed {
                    name: name.clone(),
                    id: lease_id,
                    error: format!("{e:?}"),
                });
                anyhow::anyhow!("Lease error while leasing application activity: {e:?}")
            })?;

        sag_event_logger.log(SagEvent::WakeLeaseSatisfied { name: name.clone(), id: lease_id });

        let token_info = server_token.basic_info()?;
        let related_koid = token_info.related_koid.raw_koid();

        if let Some(detector) = &loop_detector {
            match detector.on_lease_taken() {
                ReportAction::ShouldReport => {
                    log::info!(
                        "No-suspend loop detected for client '{}'; filing crash report",
                        name
                    );
                    let report = ffeedback::CrashReport {
                        program_name: Some("system".to_string()),
                        crash_signature: Some(NO_SUSPEND_CRASH_SIGNATURE.to_string()),
                        is_fatal: Some(false),
                        ..Default::default()
                    };
                    let message = CrashReportMessage { report, reboot_reason: None };
                    if let Err(e) = self.report_sender.unbounded_send(message) {
                        log::warn!("Failed to send crash report to channel: {:?}", e);
                    }
                }
                ReportAction::DoNotReport => {}
            }
        }

        let loop_detector = loop_detector.clone();

        let active_lease = Rc::new(ActiveWakeLease {
            name: name.clone(),
            lease_type: fobs::WAKE_LEASE_ITEM_TYPE_APPLICATION_ACTIVITY,
            is_unmonitored: true,
            client_token_koid: related_koid,
            server_token_koid: token_info.koid.raw_koid(),
            status: RefCell::new(LeaseStatus::Satisfied),
            error: RefCell::new(None),
            timer_task: RefCell::new(None),
            created_at: fasync::BootInstant::now().into_zx(),
            history_node: RefCell::new(None),
        });
        self.active_wake_leases.borrow_mut().insert(lease_id, active_lease);

        let active_wake_leases = self.active_wake_leases.clone();
        let active_unmonitored_lease_count = self.active_unmonitored_lease_count.clone();
        let long_lease_detector = self.long_lease_detector.clone();

        Self::handle_unmonitored_lease_acquired(
            true,
            &active_unmonitored_lease_count,
            &active_wake_leases,
        );

        fasync::Task::local(async move {
            // Keep lease alive for as long as the client keeps it alive.
            let _ = fasync::OnSignals::new(server_token, zx::Signals::EVENTPAIR_PEER_CLOSED).await;
            log::debug!("Dropping application activity lease for '{}'", name);

            sag_event_logger.log(SagEvent::WakeLeaseDropped { name: name.clone(), id: lease_id });
            active_wake_leases.borrow_mut().remove(&lease_id);

            Self::handle_unmonitored_lease_dropped(
                true,
                &active_unmonitored_lease_count,
                &active_wake_leases,
                &long_lease_detector,
            );

            if let Some(detector) = loop_detector {
                detector.on_lease_dropped();
            }
        })
        .detach();

        Ok(())
    }

    async fn create_wake_lease_using_token(
        &self,
        name: String,
        server_token: fsystem::LeaseToken,
        is_unmonitored: bool,
    ) -> Result<()> {
        let suspend_blocker = match self.suspend_block_manager.try_get_blocker() {
            None => {
                log::info!(
                    "Acquisition of wake lease '{}' temporarily blocked by suspend attempt",
                    name
                );
                self.suspend_block_manager.get_blocker().await
            }
            Some(blocker) => blocker,
        };

        let execution_state_suspending_lease = self.execution_state_suspending_lease.clone();
        let before_suspend_notifier = self.before_suspend_notifier.clone();
        let active_wake_leases = self.active_wake_leases.clone();
        let execution_state_lessor = self.execution_state_lessor.clone();
        let long_lease_detector = self.long_lease_detector.clone();

        log::debug!("Acquiring wake lease for '{}'", name);
        let sag_event_logger = self.sag_event_logger.clone();
        let lease_id = self.max_lease_id.fetch_add(1, Ordering::Relaxed);
        sag_event_logger.log(SagEvent::WakeLeaseCreated { name: name.clone(), id: lease_id });

        let active_unmonitored_lease_count = self.active_unmonitored_lease_count.clone();

        Self::handle_unmonitored_lease_acquired(
            is_unmonitored,
            &active_unmonitored_lease_count,
            &active_wake_leases,
        );

        fasync::Task::local(async move {
            let token_info = server_token.basic_info().expect("zx_object_get_info failed");
            let related_koid = token_info.related_koid.raw_koid();
            let created_at = fasync::BootInstant::now().into_zx();

            // Permitted unmonitored wake leases should not have their timers started.
            let timer_task = if !is_unmonitored && active_unmonitored_lease_count.get() == 0 {
                long_lease_detector.as_ref().map(|detector| {
                    detector.start_timer(
                        name.clone(),
                        lease_id,
                        active_wake_leases.clone(),
                        created_at,
                    )
                })
            } else {
                None
            };

            let active_lease = Rc::new(ActiveWakeLease {
                name: name.clone(),
                lease_type: fobs::WAKE_LEASE_ITEM_TYPE_WAKE,
                is_unmonitored,
                client_token_koid: related_koid,
                server_token_koid: token_info.koid.raw_koid(),
                status: RefCell::new(LeaseStatus::AwaitingSatisfaction),
                error: RefCell::new(None),
                timer_task: RefCell::new(timer_task),
                created_at,
                history_node: RefCell::new(None),
            });

            active_wake_leases.borrow_mut().insert(lease_id, active_lease.clone());

            // If a suspend transition is currently in progress, race the wake lease token's
            // close signal with the BeforeSuspend callbacks. If the wake lease token is dropped
            // by the client before the callbacks complete, then the power broker lease which
            // backs the wake lease should NOT be requested. Without this logic, SAG's power
            // broker lease request would preempt the race to suspension by preventing SAG's
            // power elements and their dependents from fully powering down.
            let rx_opt = before_suspend_notifier.borrow().clone();
            if let Some(rx) = rx_opt {
                let dup_token = server_token
                    .duplicate_handle(zx::Rights::SAME_RIGHTS)
                    .expect("duplicate handle failed");
                let token_closed = fasync::OnSignals::new(
                    dup_token, zx::Signals::EVENTPAIR_PEER_CLOSED).fuse();
                let before_suspend_done = rx.fuse();

                futures::pin_mut!(token_closed);
                futures::pin_mut!(before_suspend_done);

                // Race the two futures. If the token is dropped by the client, skip the power
                // broker lease request.
                let dropped_before_complete = futures::select! {
                    _ = before_suspend_done => false,
                    _ = token_closed => true,
                };

                if dropped_before_complete {
                    log::debug!(
                        "Wake lease '{}' dropped before BeforeSuspend completed, skipping power broker lease request",
                        name);
                    sag_event_logger.log(SagEvent::WakeLeaseDropped { name: name.clone(), id: lease_id });
                    Self::record_duration(lease_id, &active_wake_leases);
                    active_wake_leases.borrow_mut().remove(&lease_id);
                    Self::handle_unmonitored_lease_dropped(
                        is_unmonitored,
                        &active_unmonitored_lease_count,
                        &active_wake_leases,
                        &long_lease_detector,
                    );
                    return;
                }
            }

            let lease = {
                let mut lease_guard = execution_state_suspending_lease.lock().await;
                let lease_opt = lease_guard.upgrade();

                match lease_opt {
                    Some(lease) => lease,
                    None => {
                        Self::create_execution_state_lease(
                            &execution_state_lessor,
                            &mut lease_guard,
                        )
                        .await
                    }
                }
            };

            match &lease.1 {
                Ok(_) => {
                    *active_lease.status.borrow_mut() = LeaseStatus::Satisfied;
                    sag_event_logger
                        .log(SagEvent::WakeLeaseSatisfied { name: name.clone(), id: lease_id });
                }
                // If there is an error while waiting for lease satisfaction, `suspend_blocker`
                // will still prevent suspension until the client drops its token.
                Err(e) => {
                    log::error!(
                        "Waiting for satisfaction of wake lease with client_token_koid {} failed: \
                    {:?}. SAG will block suspension internally for the lifetime of the client \
                    token.",
                        related_koid,
                        e
                    );
                    *active_lease.status.borrow_mut() = LeaseStatus::FailedSatisfaction;
                    *active_lease.error.borrow_mut() = Some(e.to_string());
                    sag_event_logger.log(SagEvent::WakeLeaseSatisfactionFailed {
                        name: name.clone(),
                        id: lease_id,
                        error: e.to_string(),
                    });
                }
            }

            // Keep wake lease alive for as long as the client keeps it alive.
            // The power element lease will be dropped once all references to lease have
            // been been dropped.
            let _ = fasync::OnSignals::new(server_token, zx::Signals::EVENTPAIR_PEER_CLOSED).await;

            log::debug!("Dropping wake lease for '{}'", name);
            sag_event_logger.log(SagEvent::WakeLeaseDropped { name: name.clone(), id: lease_id });

            Self::record_duration(lease_id, &active_wake_leases);

            active_wake_leases.borrow_mut().remove(&lease_id);

            Self::handle_unmonitored_lease_dropped(
                is_unmonitored,
                &active_unmonitored_lease_count,
                &active_wake_leases,
                &long_lease_detector,
            );

            // Drop `suspend_blocker` before `lease` to avoid the possibility (however unlikely)
            // that the lease drop leads to a suspend attempt before the suspend blocker is removed.
            drop(suspend_blocker);

            // Before dropping `lease`, yield to other async tasks. If another wake lease request
            // is queued, this can prevent unnecessary recreation of the Execution State lease. The
            // practical performance impact is unclear, but in benchmarks that rapidly acquire and
            // drop leases, this does prevent Inspect errors due to max heap utilization.
            fasync::yield_now().await;

            drop(lease);
        })
        .detach();
        Ok(())
    }

    async fn create_wake_lease(
        &self,
        name: String,
        is_unmonitored: bool,
    ) -> Result<fsystem::LeaseToken> {
        let (server_token, client_token) = fsystem::LeaseToken::create();
        self.create_wake_lease_using_token(name, server_token, is_unmonitored).await?;
        Ok(client_token)
    }

    async fn create_execution_state_lease(
        execution_state_lessor: &fbroker::LessorProxy,
        lease_guard: &mut std::rc::Weak<(Option<fbroker::LeaseControlProxy>, Result<()>)>,
    ) -> Rc<(Option<fbroker::LeaseControlProxy>, Result<()>)> {
        match execution_state_lessor.lease(ExecutionStateLevel::Suspending.into_primitive()).await {
            Ok(Ok(lease_client_end)) => {
                let lease = lease_client_end.into_proxy();
                let mut status = fbroker::LeaseStatus::Unknown;
                let lease_result = loop {
                    match lease.watch_status(status).await {
                        Ok(fbroker::LeaseStatus::Satisfied) => break Ok(()),
                        Ok(new_status) => {
                            status = new_status;
                        }
                        Err(e) => break Err(anyhow::anyhow!(e)),
                    }
                };

                let result = Rc::new((Some(lease), lease_result));
                *lease_guard = Rc::downgrade(&result);
                result
            }
            Ok(Err(e)) => Rc::new((
                None,
                Err(anyhow::anyhow!("Failed to lease execution state for wake lease: {e:?})")),
            )),
            Err(e) => Rc::new((
                None,
                Err(anyhow::anyhow!(
                    "Failed to contact power broker to lease execution state for wake lease: {e:?})"
                )),
            )),
        }
    }
}

type SuspendBlockerVector = Vec<(u64, fsystem::SuspendBlockerProxy, String)>;

pub struct SuspendBlockerIdGenerator {
    next_id: AtomicU64,
}

impl Default for SuspendBlockerIdGenerator {
    fn default() -> Self {
        Self { next_id: AtomicU64::new(0) }
    }
}

impl SuspendBlockerIdGenerator {
    pub fn next_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }
}

#[derive(Debug, PartialEq, Eq)]
enum ReportAction {
    ShouldReport,
    DoNotReport,
}

/// Detects situations where the system repeatedly takes leases and drops them
/// without entering suspend, potentially indicating a bug or an infinite loop.
/// It reports when the lease is taken for the `CYCLE_THRESHOLD`th time.
struct NoSuspendDetector {
    active_count: std::cell::Cell<u32>,
    cycle_count: std::cell::Cell<u32>,
}

impl NoSuspendDetector {
    const CYCLE_THRESHOLD: u32 = 5;

    fn new() -> Self {
        Self { active_count: std::cell::Cell::new(0), cycle_count: std::cell::Cell::new(0) }
    }

    fn on_lease_taken(&self) -> ReportAction {
        let mut action = ReportAction::DoNotReport;
        let active = self.active_count.get();
        if active == 0 {
            let cycles = self.cycle_count.get();
            if cycles == Self::CYCLE_THRESHOLD {
                action = ReportAction::ShouldReport;
            }
            self.cycle_count.set(cycles + 1);
        }
        self.active_count.set(active + 1);
        action
    }

    fn on_lease_dropped(&self) {
        let active = self.active_count.get();
        if active > 0 {
            self.active_count.set(active - 1);
        }
    }

    fn on_suspend_success(&self) {
        if self.cycle_count.get() > 0 {
            log::debug!("Resetting cycle count for loop detector");
            self.cycle_count.set(0);
        }
    }
}

/// Detects situations where the system repeatedly runs suspend and resume
/// callbacks in a loop without suspension handled.
struct SuspendLoopDetector {
    count: std::cell::Cell<u32>,
    max_attempts: u32,
}

impl SuspendLoopDetector {
    fn new(max_attempts: u32) -> Self {
        Self { count: std::cell::Cell::new(0), max_attempts }
    }

    fn on_suspend_attempt(&self) {
        if self.max_attempts == 0 {
            return;
        }
        let count = self.count.get() + 1;
        self.count.set(count);
    }

    fn should_report(&self) -> ReportAction {
        if self.max_attempts == 0 {
            return ReportAction::DoNotReport;
        }
        let count = self.count.get();

        if count == self.max_attempts {
            ReportAction::ShouldReport
        } else {
            ReportAction::DoNotReport
        }
    }

    fn reset(&self) {
        self.count.set(0);
    }
}

/// Message sent to report crash and optionally trigger reboot.
struct CrashReportMessage {
    report: ffeedback::CrashReport,
    /// Reason to initiate reboot. If None, we don't reboot.
    reboot_reason: Option<fstatecontrol::ShutdownReason>,
}

/// Represents the tracking record for all long wake leases with a given name.
/// This record serves two purposes:
/// 1. Maintains an in-memory count (`report_count`) to rate-limit crash report generation (max 2)
///    for this specific lease name.
/// 2. Holds the Inspect property (`total_count`) to expose the total violation count
///    for this lease name in diagnostics.
struct LongLeaseRecord {
    report_count: u32,
    total_count: IUint,
}

/// The state managed by `LongLeaseDetector`. Inside a thread-local context, it records
/// occurrence counts and updates the circular buffer history, filing crash reports
/// for long-lived wake leases.
struct LongLeaseDetectorInner {
    timeout: fasync::MonotonicDuration,
    report_sender: futures::channel::mpsc::UnboundedSender<CrashReportMessage>,
    _node: INode,
    occurrences_node: INode,
    long_leases: RefCell<BTreeMap<String, LongLeaseRecord>>,
    history: RefCell<fuchsia_inspect_contrib::nodes::BoundedListNode>,
}

/// Detects wake leases held for an unusually long time (e.g. > 5 minutes).
#[derive(Clone)]
struct LongLeaseDetector {
    inner: Rc<LongLeaseDetectorInner>,
}

impl LongLeaseDetector {
    const CRASH_SIGNATURE: &'static str = "fuchsia-wake-lease-held-long";

    fn new(
        node: INode,
        timeout: fasync::MonotonicDuration,
        report_sender: futures::channel::mpsc::UnboundedSender<CrashReportMessage>,
    ) -> Self {
        let occurrences_node = node.create_child("occurrences");
        let history = fuchsia_inspect_contrib::nodes::BoundedListNode::new(
            node.create_child("latest_long_wake_leases"),
            10,
        );
        Self {
            inner: Rc::new(LongLeaseDetectorInner {
                timeout,
                report_sender,
                _node: node,
                occurrences_node,
                long_leases: RefCell::new(BTreeMap::new()),
                history: RefCell::new(history),
            }),
        }
    }

    fn start_timer(
        &self,
        name: String,
        lease_id: u64,
        active_wake_leases: Rc<RefCell<BTreeMap<u64, Rc<ActiveWakeLease>>>>,
        start_time: zx::BootInstant,
    ) -> fasync::Task<()> {
        let inner = self.inner.clone();
        fasync::Task::local(async move {
            fasync::Timer::new(inner.timeout).await;

            // Record in the circular buffer first for (name, start_time_ns)
            let weak_node = inner
                .history
                .borrow_mut()
                .add_entry(|node| {
                    node.record_string("name", &name);
                    node.record_int("start_time_ns", start_time.into_nanos());
                })
                .clone_weak();

            if let Some(lease) = active_wake_leases.borrow().get(&lease_id) {
                *lease.history_node.borrow_mut() = Some(weak_node);
            }

            let mut long_leases = inner.long_leases.borrow_mut();
            if let Some(state) = long_leases.get_mut(&name) {
                // Increment count per lease name in Inspect
                state.total_count.add(1);

                if state.report_count >= 2 {
                    log::info!(
                        "Wake lease '{}' (id={}) held for longer than {:?}; crash report suppressed",
                        name,
                        lease_id,
                        inner.timeout
                    );
                    return;
                }
                state.report_count += 1;
            } else {
                // First violation: create property and record count = 1
                let total_count = inner.occurrences_node.create_uint(&name, 1);
                long_leases.insert(name.clone(), LongLeaseRecord { report_count: 1, total_count });
            }
            log::info!(
                "Wake lease '{}' (id={}) held for longer than {:?}; filing crash report",
                name,
                lease_id,
                inner.timeout
            );
            let report = ffeedback::CrashReport {
                program_name: Some("system".to_string()),
                crash_signature: Some(Self::CRASH_SIGNATURE.to_string()),
                is_fatal: Some(false),
                ..Default::default()
            };
            let message = CrashReportMessage { report, reboot_reason: None };
            if let Err(e) = inner.report_sender.unbounded_send(message) {
                log::warn!("Failed to send crash report to channel: {:?}", e);
            }
        })
    }
}

/// SystemActivityGovernor runs the server for fuchsia.power.suspend and fuchsia.power.system FIDL
/// APIs.
pub struct SystemActivityGovernor {
    /// The context used to manage the execution state power element.
    execution_state: PowerElementContext,
    /// The context used to manage the application activity power element.
    application_activity: PowerElementContext,
    /// The manager used to report suspend stats to inspect and clients of
    /// fuchsia.power.suspend.Stats.
    suspend_stats: SuspendStatsManager,
    /// The manager used to create and report wake and activity application leases.
    lease_manager: LeaseManager,
    /// The collection of fsystem::SuspendBlockerProxy that have
    /// been registered through
    /// fuchsia.power.system.ActivityGovernor/RegisterSuspendBlocker.
    suspend_blockers: Rc<RefCell<SuspendBlockerVector>>,
    /// The collection of fsystem::SuspendBlockerProxy that have
    /// been registered through
    /// fuchsia.power.system.ActivityGovernor/RegisterSuspendBlocker but are not
    /// active yet. Suspend blockers are moved to `suspend_blockers` at the
    /// beginning of the next suspend cycle to prevent unnecessary
    /// notifications during resume procedures.
    pending_suspend_blockers: Rc<RefCell<SuspendBlockerVector>>,
    /// The manager used to modify cpu power element and trigger suspend.
    cpu_manager: Rc<CpuManager>,
    /// The context used to manage the boot_control power element.
    boot_control: Rc<PowerElementContext>,
    /// The collection of information about PowerElements managed
    /// by system-activity-governor.
    element_power_level_names: Vec<fbroker::ElementPowerLevelNames>,
    /// The signal which is set when the power elements are configured and
    /// FIDL handlers can run. This is required because a newly constructed
    /// SystemActivityGovernor initializes and runs power elements
    /// asynchronously. This signal prevents exposing uninitialized power
    /// element state to external clients.
    is_running_signal: async_lock::OnceCell<()>,
    /// The flag used to track whether the system is shutting down.
    is_shutting_down: Rc<Cell<bool>>,
    /// The flag used to synchronize the resume_control_lease.
    /// It's unset when a resume_control_lease is created and is set
    /// when it needs to be dropped.
    // TODO(https://fxbug.dev/372695129): Optimize resume_control_lease.
    es_activation_after_resume_signal: Rc<RefCell<async_lock::OnceCell<()>>>,
    /// The lease which hold execution_state at suspending state temporarily
    /// after suspension.
    resume_control_lease: Rc<RefCell<Option<fbroker::LeaseControlProxy>>>,
    /// The token for the CPU frequency boost.
    boost_token: Rc<RefCell<Option<zx::EventPair>>>,
    /// The proxy used to call Boost.
    boost_proxy: fcpumanager::BoostProxy,
    /// Temporarily holds the boot_control lease.
    booting_lease: Rc<RefCell<Option<fidl::endpoints::ClientEnd<fbroker::LeaseControlMarker>>>>,
    /// Logger for system-wide activity governor events.
    sag_event_logger: SagEventLogger,
    /// ElementRunner ServerEnds for the Execution State, Application Activity and Boot Control
    /// elements.
    execution_state_runner: Rc<RefCell<Option<ServerEnd<fbroker::ElementRunnerMarker>>>>,
    application_activity_runner: Rc<RefCell<Option<ServerEnd<fbroker::ElementRunnerMarker>>>>,
    suspend_blocker_id_generator: Rc<SuspendBlockerIdGenerator>,
    _suspend_blockers_node: LazyNode,
    before_suspend_notifier: Rc<RefCell<Option<futures::future::Shared<oneshot::Receiver<()>>>>>,
    stuck_warning_timeout: fasync::MonotonicDuration,
    report_sender: futures::channel::mpsc::UnboundedSender<CrashReportMessage>,
    loop_detector: Option<std::rc::Rc<NoSuspendDetector>>,
    suspend_loop_detector: std::rc::Rc<SuspendLoopDetector>,
    reboot_on_stalled_suspend_blocker: bool,
}

impl SystemActivityGovernor {
    pub async fn new(
        topology: &fbroker::TopologyProxy,
        inspect_root: INode,
        sag_event_logger: SagEventLogger,
        cpu_manager: Rc<CpuManager>,
        execution_state_dependencies: Vec<fbroker::LevelDependency>,
        is_shutting_down: Rc<Cell<bool>>,
        crash_reporter: ffeedback::CrashReporterProxy,
        boost_proxy: fcpumanager::BoostProxy,
        admin_proxy: Option<fstatecontrol::AdminProxy>,
        config: &Config,
    ) -> Result<Rc<Self>> {
        let (report_sender, mut report_receiver) =
            futures::channel::mpsc::unbounded::<CrashReportMessage>();
        let crash_reporter_clone = crash_reporter.clone();
        let admin_proxy_clone = admin_proxy.clone();
        fasync::Task::local(async move {
            while let Some(message) = report_receiver.next().await {
                match crash_reporter_clone.file_report(message.report).await {
                    Ok(Ok(result)) => log::info!("Crash report filed: {:?}", result),
                    Ok(Err(e)) => log::warn!("Failed to file crash report: {:?}", e),
                    Err(e) => log::warn!("Failed to call FileReport: {:?}", e),
                }
                let Some(reason) = message.reboot_reason else {
                    continue;
                };
                let Some(admin) = admin_proxy_clone.as_ref() else {
                    continue;
                };
                log::info!("Initiating reboot due to stuck suspend/resume watchdog firing...");
                let options = fstatecontrol::ShutdownOptions {
                    action: Some(fstatecontrol::ShutdownAction::Reboot),
                    reasons: Some(vec![reason]),
                    ..Default::default()
                };
                match admin.shutdown(&options).await {
                    Ok(Ok(())) => {
                        log::info!("Shutdown/Reboot requested successfully")
                    }
                    Ok(Err(e)) => {
                        log::error!("Shutdown/Reboot failed: {:?}", e)
                    }
                    Err(e) => log::error!("Failed to call Shutdown: {:?}", e),
                }
            }
        })
        .detach();

        let mut element_power_level_names: Vec<fbroker::ElementPowerLevelNames> = Vec::new();

        element_power_level_names.push(generate_element_power_level_names(
            "cpu",
            vec![
                (CpuLevel::Inactive.into_primitive(), "Inactive".to_string()),
                (CpuLevel::Active.into_primitive(), "Active".to_string()),
            ],
        ));

        let (es_element_runner_client, execution_state_runner) =
            create_endpoints::<fbroker::ElementRunnerMarker>();
        let execution_state = PowerElementContext::builder(
            topology,
            "execution_state",
            &[
                ExecutionStateLevel::Inactive.into_primitive(),
                ExecutionStateLevel::Suspending.into_primitive(),
                ExecutionStateLevel::Active.into_primitive(),
            ],
            es_element_runner_client,
        )
        .dependencies(execution_state_dependencies)
        .build()
        .await
        .expect("PowerElementContext encountered error while building execution_state");

        element_power_level_names.push(generate_element_power_level_names(
            "execution_state",
            vec![
                (ExecutionStateLevel::Inactive.into_primitive(), "Inactive".to_string()),
                (ExecutionStateLevel::Suspending.into_primitive(), "Suspending".to_string()),
                (ExecutionStateLevel::Active.into_primitive(), "Active".to_string()),
            ],
        ));

        let (aa_element_runner_client, application_activity_runner) =
            create_endpoints::<fbroker::ElementRunnerMarker>();
        let application_activity = PowerElementContext::builder(
            topology,
            "application_activity",
            &[
                ApplicationActivityLevel::Inactive.into_primitive(),
                ApplicationActivityLevel::Active.into_primitive(),
            ],
            aa_element_runner_client,
        )
        .dependencies(vec![fbroker::LevelDependency {
            dependent_level: Some(ApplicationActivityLevel::Active.into_primitive()),
            requires_token: Some(
                execution_state.assertive_dependency_token().expect("token not registered"),
            ),
            requires_level_by_preference: Some(vec![ExecutionStateLevel::Active.into_primitive()]),
            ..Default::default()
        }])
        .build()
        .await
        .expect("PowerElementContext encountered error while building application_activity");

        let before_suspend_notifier = Rc::new(RefCell::new(None));
        let long_lease_threshold = if config.long_wake_lease_timeout == 0 {
            None
        } else {
            Some(fasync::MonotonicDuration::from_seconds(config.long_wake_lease_timeout as i64))
        };
        let lease_manager = LeaseManager::new(
            &inspect_root,
            sag_event_logger.clone(),
            topology.clone(),
            execution_state.lessor.clone(),
            application_activity.assertive_dependency_token().expect("token not registered"),
            cpu_manager.suspend_block_manager().await,
            before_suspend_notifier.clone(),
            config.max_active_wake_leases_to_log as usize,
            report_sender.clone(),
            long_lease_threshold,
        );

        element_power_level_names.push(generate_element_power_level_names(
            "application_activity",
            vec![
                (ApplicationActivityLevel::Inactive.into_primitive(), "Inactive".to_string()),
                (ApplicationActivityLevel::Active.into_primitive(), "Active".to_string()),
            ],
        ));

        let (bc_element_runner_client, boot_control_runner) =
            create_endpoints::<fbroker::ElementRunnerMarker>();
        let boot_control = Rc::new(
            PowerElementContext::builder(
                topology,
                "boot_control",
                &[BootControlLevel::Inactive.into(), BootControlLevel::Active.into()],
                bc_element_runner_client,
            )
            .dependencies(vec![fbroker::LevelDependency {
                dependent_level: Some(BootControlLevel::Active.into()),
                requires_token: Some(
                    execution_state.assertive_dependency_token().expect("token not registered"),
                ),
                requires_level_by_preference: Some(vec![
                    ExecutionStateLevel::Active.into_primitive(),
                ]),
                ..Default::default()
            }])
            .build()
            .await
            .expect("PowerElementContext encountered error while building boot_control"),
        );
        let bc_context = boot_control.clone();
        fasync::Task::local(async move {
            bc_context
                .run(boot_control_runner, None /* inspect_node */, None /* update_fun */)
                .await;
        })
        .detach();

        element_power_level_names.push(generate_element_power_level_names(
            "boot_control",
            vec![
                (BootControlLevel::Inactive.into(), "Inactive".to_string()),
                (BootControlLevel::Active.into(), "Active".to_string()),
            ],
        ));

        let suspend_stats =
            SuspendStatsManager::new(inspect_root.create_child(fobs::SUSPEND_STATS_NODE));

        // Create fields to record suspend blockers and the lazy Inspect node that reports their
        // names.
        let suspend_blockers = Rc::new(RefCell::new(SuspendBlockerVector::new()));
        let pending_suspend_blockers = Rc::new(RefCell::new(SuspendBlockerVector::new()));
        let suspend_blockers_clone = suspend_blockers.clone();
        let pending_suspend_blockers_clone = pending_suspend_blockers.clone();
        let callback = move || {
            let suspend_blockers = suspend_blockers_clone.clone();
            let pending_suspend_blockers = pending_suspend_blockers_clone.clone();
            async move {
                let inspector = Inspector::default();
                let suspend_blockers = suspend_blockers.borrow();
                let pending_suspend_blockers = pending_suspend_blockers.borrow();
                let mut names: Vec<_> = suspend_blockers
                    .iter()
                    .chain(pending_suspend_blockers.iter())
                    .map(|(_, _, name)| name.as_str())
                    .collect();
                names.sort();

                let names_property = inspector.root().create_string_array("names", names.len());
                for (i, name) in names.into_iter().enumerate() {
                    names_property.set(i, name);
                }
                inspector.root().record(names_property);
                Ok(inspector)
            }
            .boxed_local()
        };
        let suspend_blockers_node =
            inspect_root.create_lazy_child_with_thread_local("suspend_blockers", callback);

        let loop_detector = if config.use_suspender {
            Some(std::rc::Rc::new(NoSuspendDetector::new()))
        } else {
            None
        };
        let suspend_loop_detector =
            std::rc::Rc::new(SuspendLoopDetector::new(config.suspend_loop_max_attempts));

        Ok(Rc::new(Self {
            execution_state,
            application_activity,
            suspend_stats,
            lease_manager,
            suspend_blockers,
            pending_suspend_blockers,
            cpu_manager,
            boot_control,
            element_power_level_names,
            reboot_on_stalled_suspend_blocker: config.reboot_on_stalled_suspend_blocker,
            es_activation_after_resume_signal: Rc::new(RefCell::new(async_lock::OnceCell::new())),
            resume_control_lease: Rc::new(RefCell::new(None)),
            boost_token: Rc::new(RefCell::new(None)),
            boost_proxy,
            is_running_signal: async_lock::OnceCell::new(),
            is_shutting_down,
            booting_lease: Rc::new(RefCell::new(None)),
            sag_event_logger,
            execution_state_runner: Rc::new(RefCell::new(Some(execution_state_runner))),
            application_activity_runner: Rc::new(RefCell::new(Some(application_activity_runner))),
            suspend_blocker_id_generator: Rc::new(SuspendBlockerIdGenerator::default()),
            _suspend_blockers_node: suspend_blockers_node,
            before_suspend_notifier,
            stuck_warning_timeout: fasync::MonotonicDuration::from_seconds(
                config.suspend_resume_stuck_warning_timeout.into(),
            ),
            report_sender,
            loop_detector,
            suspend_loop_detector,
        }))
    }

    /// Runs a FIDL server to handle fuchsia.power.suspend and fuchsia.power.system API requests.
    pub async fn run(self: &Rc<Self>, elements_node: &INode) -> Result<()> {
        log::info!("Handling power elements");

        self.run_execution_state(
            self.execution_state_runner.take().expect("execution_state_runner not set"),
            &elements_node,
        );
        log::info!("System is booting. Acquiring boot control lease.");
        let boot_control_lease = self
            .boot_control
            .lessor
            .lease(BootControlLevel::Active.into())
            .await
            .expect("Failed to request boot control lease")
            .expect("Failed to acquire boot control lease")
            .into_proxy();

        // TODO(https://fxbug.dev/333947976): Use RequiredLevel when LeaseStatus is removed.
        let mut lease_status = fbroker::LeaseStatus::Unknown;
        while lease_status != fbroker::LeaseStatus::Satisfied {
            lease_status = boot_control_lease.watch_status(lease_status).await.unwrap();
        }

        let booting_lease =
            boot_control_lease.into_client_end().expect("failed to convert to ClientEnd");
        let _ = self.booting_lease.borrow_mut().insert(booting_lease);

        self.run_application_activity(
            self.application_activity_runner.take().expect("application_activity_runner not set"),
            &elements_node,
        );

        let _ = self.is_running_signal.set(()).await;
        Ok(())
    }

    fn run_application_activity(
        self: &Rc<Self>,
        element_runner: ServerEnd<fbroker::ElementRunnerMarker>,
        inspect_node: &INode,
    ) {
        let application_activity_node = inspect_node.create_child("application_activity");
        let this = self.clone();

        fasync::Task::local(async move {
            this.application_activity
                .run(element_runner, Some(application_activity_node), None /* update_fn */)
                .await;
        })
        .detach();
    }

    fn run_execution_state(
        self: &Rc<Self>,
        element_runner: ServerEnd<fbroker::ElementRunnerMarker>,
        inspect_node: &INode,
    ) {
        let execution_state_node = inspect_node.create_child("execution_state");
        let this = self.clone();
        let this_clone = this.clone();

        fasync::Task::local(async move {
            let previous_power_level =
                Rc::new(Cell::new(ExecutionStateLevel::Inactive.into_primitive()));

            this.execution_state
                .run(
                    element_runner,
                    Some(execution_state_node),
                    Some(Box::new(move |new_power_level: fbroker::PowerLevel| {
                        let previous_power_level = previous_power_level.clone();
                        let this = this_clone.clone();

                        async move {
                            // Call suspend callback before ExecutionState power level changes.
                            if new_power_level == ExecutionStateLevel::Inactive.into_primitive() {
                                if this.is_shutting_down.get() {
                                    log::info!("System is shutting down, halting execution_state power element transitions");
                                    futures::future::pending::<()>().await;
                                }

                                this.notify_on_suspend().await;
                            } else if previous_power_level.get()
                                == ExecutionStateLevel::Inactive.into_primitive()
                            {
                                this.notify_on_resume().await;
                            }

                            // If entering Active, SAG drops the resume control lease to re-enable
                            // suspension.
                            if new_power_level == ExecutionStateLevel::Active.into_primitive() {
                                let _ =
                                    this.es_activation_after_resume_signal.borrow().set(()).await;
                            }

                            previous_power_level.set(new_power_level);
                        }
                        .boxed_local()
                    })),
                )
                .await;
        })
        .detach();
    }

    async fn get_status_endpoints(&self) -> Vec<fbroker::ElementStatusEndpoint> {
        let mut endpoints = Vec::new();

        register_element_status_endpoint("execution_state", &self.execution_state, &mut endpoints);

        register_element_status_endpoint(
            "application_activity",
            &self.application_activity,
            &mut endpoints,
        );

        register_element_status_endpoint(
            "cpu",
            self.cpu_manager.cpu().await.as_ref(),
            &mut endpoints,
        );

        register_element_status_endpoint("boot_control", &self.boot_control, &mut endpoints);
        endpoints
    }

    pub async fn handle_activity_governor_stream(
        self: Rc<Self>,
        mut stream: fsystem::ActivityGovernorRequestStream,
    ) {
        // Before handling requests, ensure power elements are initialized and handlers are running.
        self.is_running_signal.wait().await;
        while let Some(request) = stream.next().await {
            match request {
                Ok(fsystem::ActivityGovernorRequest::GetPowerElements { responder }) => {
                    self.handle_get_power_elements(responder);
                }
                Ok(fsystem::ActivityGovernorRequest::TakeApplicationActivityLease {
                    responder,
                    name,
                }) => {
                    self.handle_take_application_activity_lease(responder, name).await;
                }
                Ok(fsystem::ActivityGovernorRequest::AcquireWakeLease { responder, name }) => {
                    self.handle_acquire_wake_lease(responder, name).await;
                }
                Ok(fsystem::ActivityGovernorRequest::AcquireWakeLeaseWithToken {
                    responder,
                    name,
                    server_token,
                }) => {
                    self.handle_acquire_wake_lease_with_token(responder, name, server_token).await;
                }
                Ok(fsystem::ActivityGovernorRequest::AcquireUnmonitoredWakeLease {
                    responder,
                    name,
                }) => {
                    self.handle_acquire_unmonitored_wake_lease(responder, name).await;
                }
                Ok(fsystem::ActivityGovernorRequest::RegisterSuspendBlocker {
                    responder,
                    payload,
                }) => {
                    self.handle_register_suspend_blocker(responder, payload).await;
                }
                Ok(fsystem::ActivityGovernorRequest::_UnknownMethod { ordinal, .. }) => {
                    log::warn!(ordinal:?; "Unknown ActivityGovernorRequest method");
                }
                Err(error) => {
                    log::error!(error:?; "Error handling ActivityGovernor request stream");
                }
            }
        }
    }

    pub async fn handle_execution_state_manager_stream(
        self: Rc<Self>,
        mut stream: fsystem::ExecutionStateManagerRequestStream,
    ) {
        // Before handling requests, ensure power elements are initialized and handlers are running.
        self.is_running_signal.wait().await;
        while let Some(request) = stream.next().await {
            match request {
                Ok(fsystem::ExecutionStateManagerRequest::GetExecutionStateDependencyToken {
                    responder,
                }) => {
                    self.handle_get_execution_state_dependency_token(responder);
                }
                Ok(fsystem::ExecutionStateManagerRequest::AddApplicationActivityDependency {
                    payload,
                    responder,
                }) => {
                    self.handle_add_application_activity_dependency(responder, payload).await;
                }
                Ok(fsystem::ExecutionStateManagerRequest::_UnknownMethod { ordinal, .. }) => {
                    log::warn!(ordinal:?; "Unknown ExecutionStateManagerRequest method");
                }
                Err(error) => {
                    log::error!(error:?; "Error handling ExecutionStateManager request stream");
                }
            }
        }
    }

    pub(crate) fn handle_get_power_elements(
        &self,
        responder: fsystem::ActivityGovernorGetPowerElementsResponder,
    ) {
        let result = responder.send(fsystem::PowerElements {
            application_activity: Some(fsystem::ApplicationActivity {
                assertive_dependency_token: Some(
                    self.application_activity
                        .assertive_dependency_token()
                        .expect("token not registered"),
                ),
                ..Default::default()
            }),
            ..Default::default()
        });

        if let Err(error) = result {
            log::warn!(
                error:?;
                "Encountered error while responding to GetPowerElements request"
            );
        }
    }

    pub(crate) fn handle_get_execution_state_dependency_token(
        &self,
        responder: fsystem::ExecutionStateManagerGetExecutionStateDependencyTokenResponder,
    ) {
        let result = responder.send(fsystem::ExecutionState {
            dependency_token: Some(
                self.execution_state.assertive_dependency_token().expect("token not registered"),
            ),
            ..Default::default()
        });

        if let Err(error) = result {
            log::warn!(
                error:?;
                "Encountered error while responding to GetExecutionStateDependencyToken request"
            );
        }
    }

    async fn handle_take_application_activity_lease(
        &self,
        responder: fsystem::ActivityGovernorTakeApplicationActivityLeaseResponder,
        name: String,
    ) {
        let client_token = match self
            .lease_manager
            .create_application_activity_lease(name, self.loop_detector.clone())
            .await
        {
            Ok(client_token) => client_token,
            Err(error) => {
                log::warn!(
                    error:?;
                    "Encountered error while registering application activity lease"
                );
                return;
            }
        };

        if let Err(error) = responder.send(client_token) {
            log::warn!(
                error:?;
                "Encountered error while responding to TakeApplicationActivity request"
            );
        }
    }

    async fn acquire_wake_lease_common(
        &self,
        name: String,
        is_unmonitored: bool,
    ) -> Result<fsystem::LeaseToken, fsystem::AcquireWakeLeaseError> {
        if name.is_empty() {
            log::warn!("Received invalid name while acquiring wake lease");
            Err(fsystem::AcquireWakeLeaseError::InvalidName)
        } else {
            self.lease_manager
                .create_wake_lease(name, is_unmonitored)
                .await
                .and_then(|client_token| {
                    client_token
                        .replace_handle(
                            zx::Rights::TRANSFER | zx::Rights::DUPLICATE | zx::Rights::WAIT,
                        )
                        .map_err(|status| {
                            anyhow::anyhow!("Failed to replace client token handle: {status}")
                        })
                })
                .or_else(|error| {
                    log::warn!(
                        error:?;
                        "Encountered error while registering wake lease"
                    );

                    Err(fsystem::AcquireWakeLeaseError::Internal)
                })
        }
    }

    async fn handle_acquire_wake_lease(
        &self,
        responder: fsystem::ActivityGovernorAcquireWakeLeaseResponder,
        name: String,
    ) {
        let client_token_res = self.acquire_wake_lease_common(name, false).await;
        if let Err(error) = responder.send(client_token_res) {
            log::warn!(
                error:?;
                "Encountered error while responding to AcquireWakeLease request"
            );
        }
    }

    async fn handle_acquire_unmonitored_wake_lease(
        &self,
        responder: fsystem::ActivityGovernorAcquireUnmonitoredWakeLeaseResponder,
        name: String,
    ) {
        let client_token_res = self.acquire_wake_lease_common(name, true).await;
        if let Err(error) = responder.send(client_token_res) {
            log::warn!(
                error:?;
                "Encountered error while responding to AcquireUnmonitoredWakeLease request"
            );
        }
    }

    async fn handle_acquire_wake_lease_with_token(
        &self,
        responder: fsystem::ActivityGovernorAcquireWakeLeaseWithTokenResponder,
        name: String,
        server_token: fsystem::LeaseToken,
    ) {
        // Check if the peer closed the other side before making a wake lease.
        let client_token_res = match server_token
            .wait_one(zx::Signals::EVENTPAIR_PEER_CLOSED, zx::MonotonicInstant::INFINITE_PAST)
        {
            zx::WaitResult::Ok(_) => {
                log::debug!("Token already closed for '{}', skipping wake lease creation", name);
                Ok(())
            }
            _ => {
                if name.is_empty() {
                    log::warn!("Received invalid name while acquiring wake lease");
                    Err(fsystem::AcquireWakeLeaseError::InvalidName)
                } else {
                    self.lease_manager
                        .create_wake_lease_using_token(name, server_token, false)
                        .await
                        .or_else(|error| {
                            log::warn!(
                                error:?;
                                "Encountered error while registering wake lease"
                            );

                            Err(fsystem::AcquireWakeLeaseError::Internal)
                        })
                }
            }
        };

        if let Err(error) = responder.send(client_token_res) {
            log::warn!(
                error:?;
                "Encountered error while responding to AcquireWakeLease request"
            );
        }
    }

    async fn handle_register_suspend_blocker(
        &self,
        responder: fsystem::ActivityGovernorRegisterSuspendBlockerResponder,
        payload: fsystem::ActivityGovernorRegisterSuspendBlockerRequest,
    ) {
        let res = match (payload.suspend_blocker, payload.name) {
            (Some(suspend_blocker), Some(name)) => {
                if name.is_empty() {
                    log::warn!("Received invalid name while registering suspend blocker");
                    let _ = responder.send(Err(fsystem::RegisterSuspendBlockerError::InvalidArgs));
                    return;
                }

                self.lease_manager
                    .create_wake_lease(name.clone(), false)
                    .await
                    .and_then(|client_token| {
                        client_token
                            .replace_handle(
                                zx::Rights::TRANSFER | zx::Rights::DUPLICATE | zx::Rights::WAIT,
                            )
                            .map_err(|status| {
                                anyhow::anyhow!("Failed to replace client token handle: {status}")
                            })
                    })
                    .and_then(|client_token| {
                        let proxy = suspend_blocker.into_proxy();
                        let id = self.suspend_blocker_id_generator.next_id();
                        self.pending_suspend_blockers.borrow_mut().push((id, proxy.clone(), name));

                        let suspend_blockers = self.suspend_blockers.clone();
                        let pending_suspend_blockers = self.pending_suspend_blockers.clone();
                        fasync::Task::local(async move {
                            let _ = proxy.on_closed().await;
                            suspend_blockers.borrow_mut().retain(|(_, p, _)| !p.is_closed());
                            pending_suspend_blockers
                                .borrow_mut()
                                .retain(|(_, p, _)| !p.is_closed());
                        })
                        .detach();

                        Ok(client_token)
                    })
                    .or_else(|error| {
                        log::warn!(
                            error:?;
                            "Encountered error while registering wake lease"
                        );

                        Err(fsystem::RegisterSuspendBlockerError::Internal)
                    })
            }
            (None, Some(_)) => {
                log::warn!("No suspend blocker provided in request");
                Err(fsystem::RegisterSuspendBlockerError::InvalidArgs)
            }
            (Some(_), None) => {
                log::warn!("No name provided in request");
                Err(fsystem::RegisterSuspendBlockerError::InvalidArgs)
            }
            (None, None) => {
                log::warn!("No arguments provided in request");
                Err(fsystem::RegisterSuspendBlockerError::InvalidArgs)
            }
        };
        let _ = responder.send(res);
    }

    async fn handle_add_application_activity_dependency(
        &self,
        responder: fsystem::ExecutionStateManagerAddApplicationActivityDependencyResponder,
        payload: fsystem::ExecutionStateManagerAddApplicationActivityDependencyRequest,
    ) {
        let res = match (payload.dependency_token, payload.power_level) {
            (Some(dependency_token), Some(power_level)) => {
                match self
                    .application_activity
                    .element_control
                    .add_dependency(fbroker::LevelDependency {
                        dependent_level: Some(
                            fsystem::ApplicationActivityLevel::Active.into_primitive(),
                        ),
                        requires_token: Some(dependency_token),
                        requires_level_by_preference: Some(vec![power_level]),
                        remove_with_required_element: Some(true),
                        ..Default::default()
                    })
                    .await
                {
                    Ok(Ok(())) => Ok(()),
                    Ok(Err(e)) => {
                        log::warn!("Failed to add dependency; Power Broker error: {:?}", e);
                        let err = match e {
                            fbroker::ModifyDependencyError::AlreadyExists => {
                                fsystem::AddApplicationActivityDependencyError::AlreadyExists
                            }
                            fbroker::ModifyDependencyError::Invalid => {
                                fsystem::AddApplicationActivityDependencyError::Invalid
                            }
                            _ => fsystem::AddApplicationActivityDependencyError::Internal,
                        };
                        Err(err)
                    }
                    Err(e) => {
                        log::warn!("FIDL error adding dependency: {:?}", e);
                        Err(fsystem::AddApplicationActivityDependencyError::Internal)
                    }
                }
            }
            _ => Err(fsystem::AddApplicationActivityDependencyError::Invalid),
        };
        let _ = responder.send(res);
    }

    pub(crate) async fn process_accumulated_requests(
        &self,
        wake_leases: Vec<StoredWakeLease>,
        suspend_blockers: Vec<StoredSuspendBlocker>,
        application_activity_leases: Vec<StoredApplicationActivityLease>,
        add_application_activity_dependencies: Vec<(
            fsystem::ExecutionStateManagerAddApplicationActivityDependencyRequest,
            fsystem::ExecutionStateManagerAddApplicationActivityDependencyResponder,
        )>,
    ) {
        log::info!("Processing accumulated requests in SAG...");

        // Check if the peer closed the other side before making a wake lease.
        for lease in wake_leases {
            match lease
                .server_token
                .wait_one(zx::Signals::EVENTPAIR_PEER_CLOSED, zx::MonotonicInstant::INFINITE_PAST)
            {
                zx::WaitResult::Ok(_) => {
                    log::debug!(
                        "Token already closed for accumulated lease '{}', skipping",
                        lease.name
                    );
                    continue;
                }
                _ => {}
            }

            let res = self
                .lease_manager
                .create_wake_lease_using_token(
                    lease.name.clone(),
                    lease.server_token,
                    lease.is_unmonitored,
                )
                .await;
            if let Err(ref error) = res {
                log::warn!(
                    error:?;
                    "Encountered error while registering accumulated wake lease for {0}", lease.name
                );
            }
        }

        for blocker in suspend_blockers {
            let proxy = blocker.suspend_blocker;
            let name = blocker.name;
            let server_token = blocker.server_token;

            let res = self
                .lease_manager
                .create_wake_lease_using_token(name.clone(), server_token, false)
                .await
                .map(|()| {
                    let id = self.suspend_blocker_id_generator.next_id();
                    self.pending_suspend_blockers.borrow_mut().push((
                        id,
                        proxy.clone(),
                        name.clone(),
                    ));

                    let suspend_blockers = self.suspend_blockers.clone();
                    let pending_suspend_blockers = self.pending_suspend_blockers.clone();
                    fasync::Task::local(async move {
                        let _ = proxy.on_closed().await;
                        suspend_blockers.borrow_mut().retain(|(_, p, _)| !p.is_closed());
                        pending_suspend_blockers.borrow_mut().retain(|(_, p, _)| !p.is_closed());
                    })
                    .detach();
                });

            if let Err(ref error) = res {
                log::warn!(
                    error:?;
                    "Encountered error while registering accumulated suspend blocker for '{0}'", name
                );
            }
        }

        for lease in application_activity_leases {
            let res = self
                .lease_manager
                .create_application_activity_lease_using_token(
                    lease.name.clone(),
                    lease.server_token,
                    self.loop_detector.clone(),
                )
                .await;
            if let Err(ref error) = res {
                log::warn!(
                    error:?;
                    "Encountered error while registering accumulated application activity lease for {0}",
                    lease.name
                );
            }
        }

        for (payload, responder) in add_application_activity_dependencies {
            self.handle_add_application_activity_dependency(responder, payload).await;
        }
    }

    pub async fn handle_boot_control_stream(
        self: Rc<Self>,
        mut stream: fsystem::BootControlRequestStream,
        booting_node: Rc<IBool>,
    ) {
        // Before handling requests, ensure power elements are initialized and handlers are running.
        self.is_running_signal.wait().await;

        while let Ok(Some(request)) = stream.try_next().await {
            match request {
                fsystem::BootControlRequest::SetBootComplete { responder } => {
                    if self.booting_lease.borrow().is_some() {
                        log::info!("System has booted. Dropping boot control lease.");
                        self.booting_lease.borrow_mut().take();
                        booting_node.set(false);
                    }
                    responder.send().unwrap();
                }
                fsystem::BootControlRequest::_UnknownMethod { ordinal, .. } => {
                    log::warn!(ordinal:?; "Unknown StatsRequest method");
                }
            }
        }
    }

    pub async fn handle_stats_stream(self: Rc<Self>, mut stream: fsuspend::StatsRequestStream) {
        // Before handling requests, ensure power elements are initialized and handlers are running.
        self.is_running_signal.wait().await;
        let sub = self.suspend_stats.hanging_get.borrow_mut().new_subscriber();

        while let Ok(Some(request)) = stream.try_next().await {
            match request {
                fsuspend::StatsRequest::Watch { responder } => {
                    if let Err(error) = sub.register(responder) {
                        log::warn!(error:?; "Failed to register for Watch call");
                    }
                }
                fsuspend::StatsRequest::_UnknownMethod { ordinal, .. } => {
                    log::warn!(ordinal:?; "Unknown StatsRequest method");
                }
            }
        }
    }

    pub async fn handle_element_info_provider_stream(
        self: Rc<Self>,
        mut stream: fbroker::ElementInfoProviderRequestStream,
    ) {
        // Before handling requests, ensure power elements are initialized and handlers are running.
        self.is_running_signal.wait().await;
        while let Ok(Some(request)) = stream.try_next().await {
            match request {
                fbroker::ElementInfoProviderRequest::GetElementPowerLevelNames { responder } => {
                    let result = responder.send(Ok(&self.element_power_level_names));
                    if let Err(error) = result {
                        log::warn!(
                            error:?;
                            "Encountered error while responding to GetElementPowerLevelNames request"
                        );
                    }
                }
                fbroker::ElementInfoProviderRequest::GetStatusEndpoints { responder } => {
                    let result = responder.send(Ok(self.get_status_endpoints().await));
                    if let Err(error) = result {
                        log::warn!(
                            error:?;
                            "Encountered error while responding to GetStatusEndpoints request"
                        );
                    }
                }
                fbroker::ElementInfoProviderRequest::_UnknownMethod { ordinal, .. } => {
                    log::warn!(ordinal:?; "Unknown ElementInfoProviderRequest method");
                }
            }
        }
    }

    async fn update_suspend_blockers(&self, is_suspending: bool) {
        if is_suspending {
            self.suspend_blockers
                .borrow_mut()
                .append(&mut self.pending_suspend_blockers.borrow_mut());
        }

        // A client may call RegisterSuspendBlocker which may cause another
        // mutable borrow of suspend_blockers. Clone suspend_blockers to prevent this.
        let suspend_blockers = self.suspend_blockers.borrow().clone();

        let dead_blocker_ids = Rc::new(RefCell::new(Vec::new()));

        // LINT.IfChange(no_response_tefmo)
        let method_name = if is_suspending { "BeforeSuspend" } else { "AfterResume " };
        log::info!("Running {method_name} for {} SuspendBlockers", suspend_blockers.len());

        let timeout = self.stuck_warning_timeout;
        let report_sender = self.report_sender.clone();
        let reboot_on_stalled_suspend_blocker = self.reboot_on_stalled_suspend_blocker;
        // Queue a task to warn if the entire suspend or resume operation gets stuck and file crash
        // report.
        let _outer_warn_task = fasync::Task::local(async move {
            fasync::Timer::new(timeout).await;
            let phase = if is_suspending { "Suspend" } else { "Resume" };
            let report = ffeedback::CrashReport {
                program_name: Some("system".to_string()),
                crash_signature: Some(format!(
                    "fuchsia-system-activity-governor-{}-stuck",
                    phase.to_lowercase()
                )),
                is_fatal: Some(false),
                ..Default::default()
            };

            log::info!("Sending crash report and optional reboot request for stuck {phase} phase");
            let reboot_reason = if reboot_on_stalled_suspend_blocker {
                Some(fstatecontrol::ShutdownReason::SuspensionFailure)
            } else {
                None
            };
            let message = CrashReportMessage { report, reboot_reason };
            if let Err(e) = report_sender.unbounded_send(message) {
                log::warn!("Failed to send crash report and reboot request: {:?}", e);
            }

            for count in 1.. {
                let phase = if is_suspending { "Suspend" } else { "Resume" };
                log::warn!(
                    "{phase} has been stuck (warning #{count}); device is likely unresponsive"
                );
                fasync::Timer::new(timeout).await;
            }
        });

        futures::stream::iter(suspend_blockers)
            .enumerate()
            .for_each_concurrent(None, |(i, (id, suspend_blocker, name))| {
                let dead_blocker_ids = dead_blocker_ids.clone();

                async move {
                    let name2 = name.clone();
                    let _warn_task = fasync::Task::local(async move {
                        // Log every 10 seconds that a given callback has not completed.
                        for count in 1.. {
                            fasync::Timer::new(SUSPEND_BLOCKER_WARNING_TIMEOUT).await;
                            let seconds = count * 10;
                            log::warn!(
                                "No response to {method_name} from SuspendBlocker '{name2}' ({i}) after {seconds} seconds!"
                            );
                            // LINT.ThenChange(//tools/testing/tefmocheck/string_in_log_check.go:no_response_tefmo)
                        }
                    });

                    if is_suspending {
                        if let Err(e) = suspend_blocker.before_suspend().await {
                            log::warn!(
                                "Failed to call BeforeSuspend on SuspendBlocker '{name}' ({i}): {e:?}"
                            );
                            dead_blocker_ids.borrow_mut().push(id);
                        }
                    } else {
                        if let Err(e) = suspend_blocker.after_resume().await {
                            log::warn!(
                                "Failed to call AfterResume on SuspendBlocker '{name}' ({i}): {e:?}"
                            );
                            dead_blocker_ids.borrow_mut().push(id);
                        }
                    }
            }}).await;

        // Remove suspend blockers that failed.
        let dead_ids = dead_blocker_ids.borrow();
        if !dead_ids.is_empty() {
            self.suspend_blockers.borrow_mut().retain(|(id, _, _)| !dead_ids.contains(id));
        }
    }
}

#[async_trait(?Send)]
impl SuspendResumeListener for SystemActivityGovernor {
    fn suspend_stats(&self) -> &dyn SuspendStatsUpdater {
        &self.suspend_stats
    }

    async fn on_suspend_ended(&self, result: SuspendResult) {
        log::debug!("on_suspend_ended: result={:?}", result);
        if result == SuspendResult::Success {
            if let Some(detector) = &self.loop_detector {
                detector.on_suspend_success();
            }
        }
        self.suspend_loop_detector.reset();
        // Reset Execution State activation signal at each resume transition.
        let _ = self.es_activation_after_resume_signal.borrow_mut().take();

        let lease = self
            .execution_state
            .lessor
            .lease(ExecutionStateLevel::Suspending.into_primitive())
            .await
            .expect("Failed to request ExecutionState lease")
            .expect("Failed to acquire ExecutionState lease")
            .into_proxy();

        // TODO(https://fxbug.dev/333947976): Use RequiredLevel when LeaseStatus is removed.
        let mut lease_status = fbroker::LeaseStatus::Unknown;
        while lease_status != fbroker::LeaseStatus::Satisfied {
            lease_status = lease.watch_status(lease_status).await.unwrap();
        }

        if self.es_activation_after_resume_signal.borrow().is_initialized() {
            log::info!("System already Active, dropping boost token immediately");
            drop(self.boost_token.borrow_mut().take());
        } else {
            let _ = self.resume_control_lease.borrow_mut().insert(lease);
            let resume_control_lease = self.resume_control_lease.clone();
            let es_activation_after_resume_signal = self.es_activation_after_resume_signal.clone();
            let boost_token = self.boost_token.clone();

            fasync::Task::local(async move {
                let _ = es_activation_after_resume_signal
                    .borrow()
                    .wait()
                    .on_timeout(RESUME_SUSPENDING_LEASE_DROP_DELAY, || {
                        log::info!("Dropping resume control lease due to timeout");
                        &()
                    })
                    .await;
                drop(resume_control_lease.borrow_mut().take());
                drop(boost_token.borrow_mut().take());
            })
            .detach();
        }
    }

    async fn notify_on_suspend(&self) {
        let (tx, rx) = oneshot::channel();
        *self.before_suspend_notifier.borrow_mut() = Some(rx.shared());

        fuchsia_trace::duration!("power", "system-activity-governor:suspend_callbacks");
        log::debug!("notify_on_suspend");

        self.suspend_loop_detector.on_suspend_attempt();

        let boost_fut = async {
            log::info!("Calling Boost protocol");
            match self.boost_proxy.boost().await {
                Ok(Ok(token)) => {
                    log::info!("Boost successful");
                    let _ = self.boost_token.borrow_mut().insert(token);
                }
                Ok(Err(e)) => {
                    log::warn!("Boost failed: {e:?}");
                }
                Err(e) => {
                    log::warn!("FIDL error during Boost call: {e:?}");
                }
            }
        };

        let update_fut = async {
            self.sag_event_logger.log(SagEvent::SuspendCallbackPhaseStarted);
            self.update_suspend_blockers(true).await;
            self.sag_event_logger.log(SagEvent::SuspendCallbackPhaseEnded);
            drop(tx);
            *self.before_suspend_notifier.borrow_mut() = None;
        };

        futures::join!(boost_fut, update_fut);
        log::debug!("update_suspend_blockers(true) done");
    }

    async fn notify_on_resume(&self) {
        fuchsia_trace::duration!("power", "system-activity-governor:resume_callbacks");
        log::debug!("notify_on_resume");
        self.sag_event_logger.log(SagEvent::ResumeCallbackPhaseStarted);
        self.update_suspend_blockers(false).await;
        self.sag_event_logger.log(SagEvent::ResumeCallbackPhaseEnded);
        log::debug!("update_suspend_blockers(false) done");

        if self.suspend_loop_detector.should_report() == ReportAction::ShouldReport {
            log::warn!("Suspend loop detected; filing crash report");
            let report = ffeedback::CrashReport {
                program_name: Some("system".to_string()),
                crash_signature: Some(SUSPEND_LOOP_SIGNATURE.to_string()),
                is_fatal: Some(false),
                ..Default::default()
            };
            let message = CrashReportMessage { report, reboot_reason: None };
            if let Err(e) = self.report_sender.unbounded_send(message) {
                log::warn!("Failed to send crash report to channel: {:?}", e);
            }
        }
    }
}

fn register_element_status_endpoint(
    name: &str,
    element: &PowerElementContext,
    endpoints: &mut Vec<fbroker::ElementStatusEndpoint>,
) {
    let (status_client, status_server) = create_endpoints::<fbroker::StatusMarker>();
    match element.element_control.open_status_channel(status_server) {
        Ok(_) => {
            endpoints.push(fbroker::ElementStatusEndpoint {
                identifier: Some(name.into()),
                status: Some(status_client),
                ..Default::default()
            });
        }
        Err(error) => {
            log::warn!(error:?; "Failed to register a Status channel for {}", name)
        }
    }
}

fn generate_element_power_level_names(
    element_name: &str,
    power_levels_names: Vec<(fbroker::PowerLevel, String)>,
) -> fbroker::ElementPowerLevelNames {
    fbroker::ElementPowerLevelNames {
        identifier: Some(element_name.into()),
        levels: Some(
            power_levels_names
                .iter()
                .cloned()
                .map(|(level, name)| fbroker::PowerLevelName {
                    level: Some(level),
                    name: Some(name.into()),
                    ..Default::default()
                })
                .collect(),
        ),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use diagnostics_assertions::{AnyProperty, assert_data_tree};

    #[fuchsia::test]
    fn test_suspend_loop_detector() {
        // Create an executor so that fasync types (like MonotonicDuration) can be used
        // in this synchronous test without panicking ("Fuchsia Executor must be created first").
        let _executor = fasync::TestExecutor::new();
        let detector = SuspendLoopDetector::new(3);

        // First attempt
        detector.on_suspend_attempt();
        assert_eq!(detector.should_report(), ReportAction::DoNotReport);

        // Second attempt
        detector.on_suspend_attempt();
        assert_eq!(detector.should_report(), ReportAction::DoNotReport);

        // Third attempt (triggers)
        detector.on_suspend_attempt();
        assert_eq!(detector.should_report(), ReportAction::ShouldReport);

        // Fourth attempt (should not trigger because count != max_attempts)
        detector.on_suspend_attempt();
        assert_eq!(detector.should_report(), ReportAction::DoNotReport);

        // Reset when handled
        detector.reset();

        // Should not trigger now on first attempt after reset
        detector.on_suspend_attempt();
        assert_eq!(detector.should_report(), ReportAction::DoNotReport);
    }

    #[fuchsia::test]
    fn test_suspend_loop_detector_files_once_after_reset() {
        let detector = SuspendLoopDetector::new(3);

        // First sequence
        detector.on_suspend_attempt();
        detector.on_suspend_attempt();
        detector.on_suspend_attempt(); // count=3
        assert_eq!(detector.should_report(), ReportAction::ShouldReport);

        // Fourth attempt (should not trigger)
        detector.on_suspend_attempt();
        assert_eq!(detector.should_report(), ReportAction::DoNotReport);

        // Reset
        detector.reset();

        // Second sequence (should trigger again after reset)
        detector.on_suspend_attempt();
        detector.on_suspend_attempt();
        detector.on_suspend_attempt(); // count=3
        assert_eq!(detector.should_report(), ReportAction::ShouldReport);
    }

    #[fuchsia::test]
    fn test_no_suspend_detector() {
        let detector = NoSuspendDetector::new();

        // Cycles from 0 to CYCLE_THRESHOLD - 1 should not file a report.
        for _ in 0..NoSuspendDetector::CYCLE_THRESHOLD {
            assert_eq!(detector.on_lease_taken(), ReportAction::DoNotReport);
            detector.on_lease_dropped();
        }

        // The next cycle start (representing the CYCLE_THRESHOLD + 1 lease taken) triggers the report.
        assert_eq!(detector.on_lease_taken(), ReportAction::ShouldReport);
        detector.on_lease_dropped();

        // Subsequent cycles do not trigger the report.
        assert_eq!(detector.on_lease_taken(), ReportAction::DoNotReport);
    }

    #[fuchsia::test]
    fn test_long_lease_detector() {
        let mut executor = fasync::TestExecutor::new_with_fake_time();
        let (sender, mut receiver) = futures::channel::mpsc::unbounded::<CrashReportMessage>();
        let inspector = Inspector::default();
        let detector = LongLeaseDetector::new(
            inspector.root().create_child("long_wake_leases"),
            fasync::MonotonicDuration::from_seconds(5),
            sender,
        );

        let active_wake_leases = Rc::new(RefCell::new(BTreeMap::<u64, Rc<ActiveWakeLease>>::new()));
        let lease_id = 1u64;
        let start_time_ns = fasync::BootInstant::now().into_nanos();

        let active_lease = Rc::new(ActiveWakeLease {
            name: "test_lease".to_string(),
            lease_type: "wake",
            is_unmonitored: false,
            client_token_koid: 123,
            server_token_koid: 456,
            status: RefCell::new(LeaseStatus::Satisfied),
            error: RefCell::new(None),
            timer_task: RefCell::new(None),
            created_at: zx::BootInstant::from_nanos(start_time_ns),
            history_node: RefCell::new(None),
        });
        active_wake_leases.borrow_mut().insert(lease_id, active_lease.clone());

        let timer_task = detector.start_timer(
            "test_lease".to_string(),
            lease_id,
            active_wake_leases.clone(),
            active_lease.created_at,
        );
        *active_lease.timer_task.borrow_mut() = Some(timer_task);

        // Let the spawned task run and register the timer!
        let _ = executor.run_until_stalled(&mut futures::future::pending::<()>());

        // Move time forward to trigger timer!
        executor.set_fake_time(
            fasync::MonotonicInstant::now() + fasync::MonotonicDuration::from_seconds(6),
        );
        let _ = executor.run_until_stalled(&mut futures::future::pending::<()>());

        let report = receiver.try_next();
        assert!(report.is_ok());
        let report = report.unwrap();
        assert!(report.is_some());
        let report = report.unwrap();
        assert_eq!(report.report.crash_signature.unwrap(), LongLeaseDetector::CRASH_SIGNATURE);
    }

    #[fuchsia::test]
    fn test_long_lease_detector_rate_limiting() {
        let mut executor = fasync::TestExecutor::new_with_fake_time();
        let (sender, mut receiver) = futures::channel::mpsc::unbounded::<CrashReportMessage>();
        let inspector = Inspector::default();
        let detector = LongLeaseDetector::new(
            inspector.root().create_child("long_wake_leases"),
            fasync::MonotonicDuration::from_seconds(5),
            sender,
        );

        let active_wake_leases = Rc::new(RefCell::new(BTreeMap::<u64, Rc<ActiveWakeLease>>::new()));

        // Start timer for lease A three times
        for lease_id in 1..=3u64 {
            let start_time_ns = fasync::BootInstant::now().into_nanos();
            let active_lease = Rc::new(ActiveWakeLease {
                name: "test_lease_a".to_string(),
                lease_type: "wake",
                is_unmonitored: false,
                client_token_koid: 100 + lease_id,
                server_token_koid: 200 + lease_id,
                status: RefCell::new(LeaseStatus::Satisfied),
                error: RefCell::new(None),
                timer_task: RefCell::new(None),
                created_at: zx::BootInstant::from_nanos(start_time_ns),
                history_node: RefCell::new(None),
            });
            active_wake_leases.borrow_mut().insert(lease_id, active_lease.clone());

            let timer_task = detector.start_timer(
                "test_lease_a".to_string(),
                lease_id,
                active_wake_leases.clone(),
                active_lease.created_at,
            );
            *active_lease.timer_task.borrow_mut() = Some(timer_task);
        }

        // Start timer for lease B once
        let lease_id = 4u64;
        let start_time_ns = fasync::BootInstant::now().into_nanos();
        let active_lease = Rc::new(ActiveWakeLease {
            name: "test_lease_b".to_string(),
            lease_type: "wake",
            is_unmonitored: false,
            client_token_koid: 100 + lease_id,
            server_token_koid: 200 + lease_id,
            status: RefCell::new(LeaseStatus::Satisfied),
            error: RefCell::new(None),
            timer_task: RefCell::new(None),
            created_at: zx::BootInstant::from_nanos(start_time_ns),
            history_node: RefCell::new(None),
        });
        active_wake_leases.borrow_mut().insert(lease_id, active_lease.clone());

        let timer_task = detector.start_timer(
            "test_lease_b".to_string(),
            lease_id,
            active_wake_leases.clone(),
            active_lease.created_at,
        );
        *active_lease.timer_task.borrow_mut() = Some(timer_task);

        // Let the spawned tasks run and register the timers!
        let _ = executor.run_until_stalled(&mut futures::future::pending::<()>());

        // Move time forward to trigger timers!
        executor.set_fake_time(
            fasync::MonotonicInstant::now() + fasync::MonotonicDuration::from_seconds(6),
        );
        let _ = executor.run_until_stalled(&mut futures::future::pending::<()>());

        // We expect exactly 3 reports in the receiver (2 for A, 1 for B)
        let mut reports = Vec::new();
        while let Ok(Some(report)) = receiver.try_next() {
            reports.push(report);
        }
        assert_eq!(reports.len(), 3);
        for report in reports {
            assert_eq!(report.report.crash_signature.unwrap(), LongLeaseDetector::CRASH_SIGNATURE);
        }
    }

    #[fuchsia::test]
    fn test_unmonitored_lease_cancels_timer() {
        let mut executor = fasync::TestExecutor::new_with_fake_time();
        let (sender, mut receiver) = futures::channel::mpsc::unbounded::<CrashReportMessage>();
        let inspector = Inspector::default();
        let detector = LongLeaseDetector::new(
            inspector.root().create_child("long_wake_leases"),
            fasync::MonotonicDuration::from_seconds(5),
            sender,
        );

        let active_unmonitored_lease_count = Rc::new(Cell::new(0));
        let active_wake_leases = Rc::new(RefCell::new(BTreeMap::<u64, Rc<ActiveWakeLease>>::new()));
        let lease_id = 1u64;

        let start_time_ns = fasync::BootInstant::now().into_nanos();
        let active_lease = Rc::new(ActiveWakeLease {
            name: "test_lease".to_string(),
            lease_type: fobs::WAKE_LEASE_ITEM_TYPE_WAKE,
            is_unmonitored: false,
            client_token_koid: 123,
            server_token_koid: 456,
            status: RefCell::new(LeaseStatus::Satisfied),
            error: RefCell::new(None),
            timer_task: RefCell::new(None),
            created_at: zx::BootInstant::from_nanos(start_time_ns),
            history_node: RefCell::new(None),
        });
        active_wake_leases.borrow_mut().insert(lease_id, active_lease.clone());

        let timer_task = detector.start_timer(
            "test_lease".to_string(),
            lease_id,
            active_wake_leases.clone(),
            active_lease.created_at,
        );
        *active_lease.timer_task.borrow_mut() = Some(timer_task);

        // Let the spawned task run and register the timer!
        let _ = executor.run_until_stalled(&mut futures::future::pending::<()>());

        LeaseManager::handle_unmonitored_lease_acquired(
            true,
            &active_unmonitored_lease_count,
            &active_wake_leases,
        );

        assert!(active_lease.timer_task.borrow().is_none());

        // Move time forward to see if it fires (it shouldn't because cancelled!).
        executor.set_fake_time(
            fasync::MonotonicInstant::now() + fasync::MonotonicDuration::from_seconds(6),
        );
        let _ = executor.run_until_stalled(&mut futures::future::pending::<()>());

        let res = receiver.try_next();
        assert!(res.is_err() || res.unwrap().is_none(), "Expected no crash report to be filed");

        LeaseManager::handle_unmonitored_lease_dropped(
            true,
            &active_unmonitored_lease_count,
            &active_wake_leases,
            &Some(detector.clone()),
        );

        // Let the spawned task run and register the timer!
        let _ = executor.run_until_stalled(&mut futures::future::pending::<()>());

        assert!(active_lease.timer_task.borrow().is_some());

        // Move time forward to trigger timer!
        executor.set_fake_time(
            fasync::MonotonicInstant::now() + fasync::MonotonicDuration::from_seconds(6),
        );
        let _ = executor.run_until_stalled(&mut futures::future::pending::<()>());

        let report = receiver.try_next();
        assert!(report.is_ok());
        let report = report.unwrap();
        assert!(report.is_some());
        let report = report.unwrap();
        assert_eq!(report.report.crash_signature.unwrap(), LongLeaseDetector::CRASH_SIGNATURE);

        assert_data_tree!(@executor executor, inspector, root: {
            long_wake_leases: {
                occurrences: {
                    test_lease: 1u64,
                },
                latest_long_wake_leases: {
                    "0": {
                        name: "test_lease",
                        start_time_ns: start_time_ns,
                    }
                }
            }
        });
    }

    #[fuchsia::test]
    fn test_regular_lease_created_while_unmonitored_lease_active_starts_timer_on_drop() {
        let mut executor = fasync::TestExecutor::new_with_fake_time();
        let (sender, mut receiver) = futures::channel::mpsc::unbounded::<CrashReportMessage>();
        let inspector = Inspector::default();
        let detector = LongLeaseDetector::new(
            inspector.root().create_child("long_wake_leases"),
            fasync::MonotonicDuration::from_seconds(5),
            sender,
        );

        let active_unmonitored_lease_count = Rc::new(Cell::new(0));
        let active_wake_leases = Rc::new(RefCell::new(BTreeMap::<u64, Rc<ActiveWakeLease>>::new()));
        let lease_id = 1u64;

        // 1. Acquire unmonitored lease first.
        LeaseManager::handle_unmonitored_lease_acquired(
            true,
            &active_unmonitored_lease_count,
            &active_wake_leases,
        );

        // 2. Create normal lease. It should NOT have a timer started.
        let start_time_ns = fasync::BootInstant::now().into_nanos();
        let active_lease = Rc::new(ActiveWakeLease {
            name: "test_lease".to_string(),
            lease_type: fobs::WAKE_LEASE_ITEM_TYPE_WAKE,
            is_unmonitored: false,
            client_token_koid: 123,
            server_token_koid: 456,
            status: RefCell::new(LeaseStatus::Satisfied),
            error: RefCell::new(None),
            timer_task: RefCell::new(None), // No timer!
            created_at: zx::BootInstant::from_nanos(start_time_ns),
            history_node: RefCell::new(None),
        });
        active_wake_leases.borrow_mut().insert(lease_id, active_lease.clone());

        assert!(active_lease.timer_task.borrow().is_none());

        // Advance fake time by 2 seconds between creation and unmonitored drop.
        executor.set_fake_time(
            fasync::MonotonicInstant::now() + fasync::MonotonicDuration::from_seconds(2),
        );
        let _ = executor.run_until_stalled(&mut futures::future::pending::<()>());

        // 3. Drop unmonitored lease.
        LeaseManager::handle_unmonitored_lease_dropped(
            true,
            &active_unmonitored_lease_count,
            &active_wake_leases,
            &Some(detector.clone()),
        );

        // Let the spawned task run and register the timer!
        let _ = executor.run_until_stalled(&mut futures::future::pending::<()>());

        // 4. Verify timer is NOW started!
        assert!(active_lease.timer_task.borrow().is_some());

        // 5. Move time forward to trigger timer!
        executor.set_fake_time(
            fasync::MonotonicInstant::now() + fasync::MonotonicDuration::from_seconds(6),
        );

        // Run tasks until stalled to let the timer fire!
        let _ = executor.run_until_stalled(&mut futures::future::pending::<()>());

        // 6. Verify report received!
        let report = receiver.try_next();
        assert!(report.is_ok());
        let report = report.unwrap();
        assert!(report.is_some());
        let report = report.unwrap();
        assert_eq!(report.report.crash_signature.unwrap(), LongLeaseDetector::CRASH_SIGNATURE);

        assert_data_tree!(@executor executor, inspector, root: {
            long_wake_leases: {
                occurrences: {
                    test_lease: 1u64,
                },
                latest_long_wake_leases: {
                    "0": {
                        name: "test_lease",
                        start_time_ns: start_time_ns,
                    }
                }
            }
        });
    }

    #[fuchsia::test]
    fn test_unmonitored_leases_toggle_timer_already_fired() {
        let mut executor = fasync::TestExecutor::new_with_fake_time();
        let (sender, mut receiver) = futures::channel::mpsc::unbounded::<CrashReportMessage>();
        let inspector = Inspector::default();
        let detector = LongLeaseDetector::new(
            inspector.root().create_child("long_wake_leases"),
            fasync::MonotonicDuration::from_seconds(5),
            sender,
        );

        let active_unmonitored_lease_count = Rc::new(Cell::new(0));
        let active_wake_leases = Rc::new(RefCell::new(BTreeMap::<u64, Rc<ActiveWakeLease>>::new()));

        let lease_id = 1;

        // 1. Acquire an unmonitored lease first
        LeaseManager::handle_unmonitored_lease_acquired(
            true, // is_unmonitored
            &active_unmonitored_lease_count,
            &active_wake_leases,
        );

        // 2. Create normal lease.
        let start_time_ns = fasync::BootInstant::now().into_nanos();
        let active_lease = Rc::new(ActiveWakeLease {
            name: "test_lease".to_string(),
            lease_type: fobs::WAKE_LEASE_ITEM_TYPE_WAKE,
            is_unmonitored: false,
            client_token_koid: 123,
            server_token_koid: 456,
            status: RefCell::new(LeaseStatus::Satisfied),
            error: RefCell::new(None),
            timer_task: RefCell::new(None),
            created_at: zx::BootInstant::from_nanos(start_time_ns),
            history_node: RefCell::new(None),
        });
        active_wake_leases.borrow_mut().insert(lease_id, active_lease.clone());

        // 3. Drop unmonitored lease. This will start the timer.
        LeaseManager::handle_unmonitored_lease_dropped(
            true,
            &active_unmonitored_lease_count,
            &active_wake_leases,
            &Some(detector.clone()),
        );

        // Let the spawned task run and register the timer!
        let _ = executor.run_until_stalled(&mut futures::future::pending::<()>());
        assert!(active_lease.timer_task.borrow().is_some());

        // 4. Move time forward to trigger the timer (5 sec timeout + 1 sec buffer)
        executor.set_fake_time(
            fasync::MonotonicInstant::now() + fasync::MonotonicDuration::from_seconds(6),
        );
        let _ = executor.run_until_stalled(&mut futures::future::pending::<()>());

        // Verify report received and history node populated
        assert!(receiver.try_next().is_ok());
        assert!(active_lease.history_node.borrow().is_some());

        // 5. Acquire an unmonitored lease again. This cancels/cleans up timer_task.
        LeaseManager::handle_unmonitored_lease_acquired(
            true, // is_unmonitored
            &active_unmonitored_lease_count,
            &active_wake_leases,
        );
        assert!(active_lease.timer_task.borrow().is_none());

        // 6. Drop unmonitored lease again.
        LeaseManager::handle_unmonitored_lease_dropped(
            true,
            &active_unmonitored_lease_count,
            &active_wake_leases,
            &Some(detector.clone()),
        );
        let _ = executor.run_until_stalled(&mut futures::future::pending::<()>());

        // 7. Verify timer is NOT restarted!
        assert!(active_lease.timer_task.borrow().is_none());
    }

    #[fuchsia::test]
    fn test_long_lease_detector_inspect() {
        let mut executor = fasync::TestExecutor::new_with_fake_time();
        let (sender, _receiver) = futures::channel::mpsc::unbounded::<CrashReportMessage>();
        let inspector = Inspector::default();
        let detector = LongLeaseDetector::new(
            inspector.root().create_child("long_wake_leases"),
            fasync::MonotonicDuration::from_seconds(5),
            sender,
        );

        let active_wake_leases = Rc::new(RefCell::new(BTreeMap::<u64, Rc<ActiveWakeLease>>::new()));

        // Start timers for some leases to exceed capacity (10)
        for i in 1..=12 {
            let lease_name = format!("lease_{}", i);
            let start_time_ns = fasync::BootInstant::now().into_nanos();
            let active_lease = Rc::new(ActiveWakeLease {
                name: lease_name.clone(),
                lease_type: "wake",
                is_unmonitored: false,
                client_token_koid: 100 + i as u64,
                server_token_koid: 200 + i as u64,
                status: RefCell::new(LeaseStatus::Satisfied),
                error: RefCell::new(None),
                timer_task: RefCell::new(None),
                created_at: zx::BootInstant::from_nanos(start_time_ns),
                history_node: RefCell::new(None),
            });
            active_wake_leases.borrow_mut().insert(i as u64, active_lease.clone());
            let timer_task = detector.start_timer(
                lease_name,
                i as u64,
                active_wake_leases.clone(),
                active_lease.created_at,
            );
            *active_lease.timer_task.borrow_mut() = Some(timer_task);

            // Let the timer task register
            let _ = executor.run_until_stalled(&mut futures::future::pending::<()>());

            // Advance time by 1 second for the next lease, so they expire sequentially
            executor.set_fake_time(
                fasync::MonotonicInstant::now() + fasync::MonotonicDuration::from_seconds(1),
            );
        }

        // Advance time once more to let remaining timers fire
        // The last one expires at start_time + 5s + 11s = 16s
        executor.set_fake_time(
            fasync::MonotonicInstant::now() + fasync::MonotonicDuration::from_seconds(5),
        );
        let _ = executor.run_until_stalled(&mut futures::future::pending::<()>());

        // Verify Inspect data structure
        assert_data_tree!(@executor executor, inspector, root: {
            long_wake_leases: {
                occurrences: {
                    lease_1: 1u64,
                    lease_2: 1u64,
                    lease_3: 1u64,
                    lease_4: 1u64,
                    lease_5: 1u64,
                    lease_6: 1u64,
                    lease_7: 1u64,
                    lease_8: 1u64,
                    lease_9: 1u64,
                    lease_10: 1u64,
                    lease_11: 1u64,
                    lease_12: 1u64,
                },
                latest_long_wake_leases: {
                    "2": {
                        name: "lease_3",
                        start_time_ns: AnyProperty,
                    },
                    "3": {
                        name: "lease_4",
                        start_time_ns: AnyProperty,
                    },
                    "4": {
                        name: "lease_5",
                        start_time_ns: AnyProperty,
                    },
                    "5": {
                        name: "lease_6",
                        start_time_ns: AnyProperty,
                    },
                    "6": {
                        name: "lease_7",
                        start_time_ns: AnyProperty,
                    },
                    "7": {
                        name: "lease_8",
                        start_time_ns: AnyProperty,
                    },
                    "8": {
                        name: "lease_9",
                        start_time_ns: AnyProperty,
                    },
                    "9": {
                        name: "lease_10",
                        start_time_ns: AnyProperty,
                    },
                    "10": {
                        name: "lease_11",
                        start_time_ns: AnyProperty,
                    },
                    "11": {
                        name: "lease_12",
                        start_time_ns: AnyProperty,
                    },
                }
            }
        });
    }

    #[fuchsia::test]
    fn test_long_lease_detector_rate_limiting_history() {
        let mut executor = fasync::TestExecutor::new_with_fake_time();
        let (sender, mut receiver) = futures::channel::mpsc::unbounded::<CrashReportMessage>();
        let inspector = Inspector::default();
        let detector = LongLeaseDetector::new(
            inspector.root().create_child("long_wake_leases"),
            fasync::MonotonicDuration::from_seconds(5),
            sender,
        );

        let active_wake_leases = Rc::new(RefCell::new(BTreeMap::<u64, Rc<ActiveWakeLease>>::new()));

        // Trigger the same lease 'lease_A' 3 times sequentially
        for i in 1..=3 {
            let start_time_ns = fasync::BootInstant::now().into_nanos();
            let active_lease = Rc::new(ActiveWakeLease {
                name: "lease_A".to_string(),
                lease_type: "wake",
                is_unmonitored: false,
                client_token_koid: 100 + i as u64,
                server_token_koid: 200 + i as u64,
                status: RefCell::new(LeaseStatus::Satisfied),
                error: RefCell::new(None),
                timer_task: RefCell::new(None),
                created_at: zx::BootInstant::from_nanos(start_time_ns),
                history_node: RefCell::new(None),
            });
            active_wake_leases.borrow_mut().insert(i as u64, active_lease.clone());
            let timer_task = detector.start_timer(
                "lease_A".to_string(),
                i as u64,
                active_wake_leases.clone(),
                active_lease.created_at,
            );
            *active_lease.timer_task.borrow_mut() = Some(timer_task);

            // Let the timer task register
            let _ = executor.run_until_stalled(&mut futures::future::pending::<()>());

            // Advance time past the 5 second timeout to let the timer fire
            executor.set_fake_time(
                fasync::MonotonicInstant::now() + fasync::MonotonicDuration::from_seconds(6),
            );
            let _ = executor.run_until_stalled(&mut futures::future::pending::<()>());
        }

        // Verify that only 2 crash reports were received (suppressed on the 3rd)
        let mut report_count = 0;
        while let Ok(Some(_report)) = receiver.try_next() {
            report_count += 1;
        }
        assert_eq!(report_count, 2);

        // Verify Inspect hierarchy
        assert_data_tree!(@executor executor, inspector, root: {
            long_wake_leases: {
                occurrences: {
                    lease_A: 3u64,
                },
                latest_long_wake_leases: {
                    "0": {
                        name: "lease_A",
                        start_time_ns: AnyProperty,
                    },
                    "1": {
                        name: "lease_A",
                        start_time_ns: AnyProperty,
                    },
                    "2": {
                        name: "lease_A",
                        start_time_ns: AnyProperty,
                    },
                }
            }
        });
    }
}
