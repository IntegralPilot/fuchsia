// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod rollback;

use std::pin::{Pin, pin};
use std::time::Duration;

use cobalt_client::traits::AsEventCode as _;
use fidl_fuchsia_metrics as fmetrics;
use fidl_fuchsia_net_stackmigrationdeprecated as fnet_migration;
use fidl_fuchsia_power_internal as fpower;
use fuchsia_async::Task;
use fuchsia_component::server::{ServiceFs, ServiceFsDir};
use fuchsia_inspect::Property as _;
use futures::channel::mpsc;
use futures::{FutureExt as _, Stream, StreamExt as _};
use log::{error, info, warn};
use networking_metrics_registry::networking_metrics_registry as metrics_registry;

const DEFAULT_NETSTACK: NetstackVersion = NetstackVersion::Netstack3;

/// The number of times a device is allowed to fail to migrate to Netstack3,
/// before abandoning migration attempts during the `CURRENT_EPOCH`.
///
/// Based on the timing parameters in [rollback], a full rollback cycle takes
/// about 2 days. Limiting the number of rollbacks to 180 should allow migration
/// attempts for approximately 1 year. In a pragmatic sense, this effectively
/// disables the "abandon migration" logic.
const MAX_ROLLBACKS_PER_EPOCH: u8 = 180;

/// The "epoch" is an arbitrary interval used to limit the number of attempts a
/// device will make to migrate to Netstack3. Once the epoch increases, the
/// device will resume attempts to migrate to Netstack3.
///
/// Pragmatically the intention is to increment this number every time a Fuchsia
/// Release is cut that contains a known fix to a Netstack3 bug.
const CURRENT_EPOCH: u32 = 3;

/// The number of failed healthchecks at which we begin generating crash
/// reports.
///
/// By setting this to one less than the `rollback::MAX_FAILED_HEALTHCHECKS`, we
/// ensure each rollback event will have two "failed healthcheck" reports from
/// the previous boot.
const HEALTHCHECK_FAILURE_THRESHOLD_FOR_CRASH_REPORTS: usize =
    crate::rollback::MAX_FAILED_HEALTHCHECKS - 1;

#[derive(Debug, Clone, Copy, serde::Deserialize, serde::Serialize, Eq, PartialEq)]
enum NetstackVersion {
    Netstack2,
    Netstack3,
}

impl NetstackVersion {
    fn inspect_uint_value(&self) -> u64 {
        match self {
            Self::Netstack2 => 2,
            Self::Netstack3 => 3,
        }
    }

    fn optional_inspect_uint_value(o: &Option<Self>) -> u64 {
        o.as_ref().map(Self::inspect_uint_value).unwrap_or(0)
    }
}

impl From<fnet_migration::NetstackVersion> for NetstackVersion {
    fn from(value: fnet_migration::NetstackVersion) -> Self {
        match value {
            fnet_migration::NetstackVersion::Netstack2 => NetstackVersion::Netstack2,
            fnet_migration::NetstackVersion::Netstack3 => NetstackVersion::Netstack3,
        }
    }
}

impl From<NetstackVersion> for fnet_migration::NetstackVersion {
    fn from(value: NetstackVersion) -> Self {
        match value {
            NetstackVersion::Netstack2 => fnet_migration::NetstackVersion::Netstack2,
            NetstackVersion::Netstack3 => fnet_migration::NetstackVersion::Netstack3,
        }
    }
}

impl From<NetstackVersion> for Box<fnet_migration::VersionSetting> {
    fn from(value: NetstackVersion) -> Self {
        Box::new(fnet_migration::VersionSetting { version: value.into() })
    }
}

#[derive(Debug, PartialEq)]
enum RollbackNetstackVersion {
    Netstack2,
    // The automated setting requested Netstack3, but the persisted state
    // indicates that the previous boot failed to migrate to Netstack3 (had too
    // many health check failure). Forcibly use Netstack2 for this boot.
    ForceNetstack2,
    // The automated setting requested Netstack3, but we've already failed too
    // many migration attempts in the `CURRENT_EPOCH`. Forcibly Use Netstack2
    // until the next epoch.
    ForceNetstack2Indefinitely,
    Netstack3,
}

impl RollbackNetstackVersion {
    // Convert into a `NetstackVersion`, while honoring the forced setting.
    fn version(&self) -> NetstackVersion {
        match self {
            Self::Netstack2 | Self::ForceNetstack2 | Self::ForceNetstack2Indefinitely => {
                NetstackVersion::Netstack2
            }
            Self::Netstack3 => NetstackVersion::Netstack3,
        }
    }

    // Convert into a `NetstackVersion`, while ignoring the forced setting.
    fn version_ignoring_force(&self) -> NetstackVersion {
        match self {
            Self::Netstack2 => NetstackVersion::Netstack2,
            Self::Netstack3 | Self::ForceNetstack2 | Self::ForceNetstack2Indefinitely => {
                NetstackVersion::Netstack3
            }
        }
    }
}

impl From<NetstackVersion> for RollbackNetstackVersion {
    fn from(version: NetstackVersion) -> Self {
        match version {
            NetstackVersion::Netstack2 => RollbackNetstackVersion::Netstack2,
            NetstackVersion::Netstack3 => RollbackNetstackVersion::Netstack3,
        }
    }
}

#[derive(Default, Debug, serde::Deserialize, serde::Serialize)]
#[cfg_attr(test, derive(Eq, PartialEq))]
struct Persisted {
    automated: Option<NetstackVersion>,
    user: Option<NetstackVersion>,
    rollback: Option<rollback::Persisted>,
    rollback_history: Option<RollbackHistory>,
}

impl Persisted {
    fn load<R: std::io::Read>(r: R) -> Self {
        serde_json::from_reader(std::io::BufReader::new(r)).unwrap_or_else(|e| {
            error!("error loading persisted config {e:?}, using defaults");
            Persisted::default()
        })
    }

    fn save<W: std::io::Write>(&self, w: W) {
        serde_json::to_writer(w, self).unwrap_or_else(|e: serde_json::Error| {
            error!("error persisting configuration {self:?}: {e:?}")
        })
    }

    // Determine the desired NetstackVersion based on the persisted values
    fn desired_netstack_version(&self) -> RollbackNetstackVersion {
        match self {
            // Always prefer the user setting, if present.
            Persisted { user: Some(user), automated: _, rollback: _, rollback_history: _ } => {
                (*user).into()
            }
            // If the automated setting is Netstack2, honor it unconditionally.
            Persisted {
                user: None,
                automated: Some(NetstackVersion::Netstack2),
                rollback: _,
                rollback_history: _,
            } => RollbackNetstackVersion::Netstack2,
            // If the automated setting is Netstack3, we need to first check the
            // rollback state.
            Persisted {
                user: None,
                automated: Some(NetstackVersion::Netstack3),
                rollback,
                rollback_history,
            } => {
                let rollback_count = match rollback_history {
                    None => 0,
                    Some(RollbackHistory { epoch, count }) => {
                        if *epoch == CURRENT_EPOCH {
                            *count
                        } else {
                            // Ignore failed rollbacks in an different epoch.
                            0
                        }
                    }
                };
                let healthcheck_failure_count = match rollback {
                    None => 0,
                    Some(rollback::Persisted::Success) => 0,
                    Some(rollback::Persisted::HealthcheckFailures(count)) => *count,
                };

                if rollback_count >= MAX_ROLLBACKS_PER_EPOCH {
                    // If we've failed all of our allotted migration attempts
                    // abandon further migration attempts in this epoch.
                    RollbackNetstackVersion::ForceNetstack2Indefinitely
                } else if healthcheck_failure_count >= rollback::MAX_FAILED_HEALTHCHECKS {
                    // If we've failed all the healthchecks in the current
                    // migration attempt, temporarily rollback to Netstack2.
                    RollbackNetstackVersion::ForceNetstack2
                } else {
                    // Otherwise, proceed as normal with Netstack3.
                    RollbackNetstackVersion::Netstack3
                }
            }
            // Use the default version if nothing is set.
            Persisted { user: None, automated: None, rollback: _, rollback_history: _ } => {
                DEFAULT_NETSTACK.into()
            }
        }
    }
}

/// A history of rollbacks that have occurred on the device.
#[derive(Debug, Copy, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct RollbackHistory {
    /// The epoch during which the rollback occurred.
    epoch: u32,
    /// The number of rollbacks that took place.
    count: u8,
}

impl Default for RollbackHistory {
    fn default() -> Self {
        RollbackHistory { epoch: CURRENT_EPOCH, count: 0 }
    }
}

enum ServiceRequest {
    Control(fnet_migration::ControlRequest),
    State(fnet_migration::StateRequest),
}

struct Migration<P, CR, R> {
    current_boot: RollbackNetstackVersion,
    persisted: Persisted,
    persistence: P,
    collaborative_reboot: CollaborativeReboot<CR>,
    crash_reporter: R,
}

trait PersistenceProvider {
    type Writer: std::io::Write;
    type Reader: std::io::Read;

    fn open_writer(&mut self) -> std::io::Result<Self::Writer>;
    fn open_reader(&self) -> std::io::Result<Self::Reader>;
}

fn try_persist<P: PersistenceProvider>(provider: &mut P, data: &Persisted) {
    let w = match provider.open_writer() {
        Ok(w) => w,
        Err(e) => {
            error!("failed to open writer to persist settings: {e:?}");
            return;
        }
    };
    data.save(w);
}

struct DataPersistenceProvider {}

const PERSISTED_FILE_PATH: &'static str = "/data/config.json";

impl PersistenceProvider for DataPersistenceProvider {
    type Writer = std::fs::File;
    type Reader = std::fs::File;

    fn open_writer(&mut self) -> std::io::Result<Self::Writer> {
        std::fs::File::create(PERSISTED_FILE_PATH)
    }

    fn open_reader(&self) -> std::io::Result<Self::Reader> {
        std::fs::File::open(PERSISTED_FILE_PATH)
    }
}

struct CollaborativeReboot<CR> {
    scheduler: CR,
    /// `Some(<cancellation_token>)` if there's an outstanding collaborative
    /// reboot scheduled.
    scheduled_req: Option<zx::EventPair>,
}

impl<CR: CollaborativeRebootScheduler> CollaborativeReboot<CR> {
    /// Schedules a collaborative reboot.
    ///
    /// No-Op if there's already a reboot scheduled.
    async fn schedule(&mut self) {
        let Self { scheduler, scheduled_req } = self;
        if scheduled_req.is_some() {
            // We already have an outstanding request.
            return;
        }

        info!("Scheduling collaborative reboot");
        let (mine, theirs) = zx::EventPair::create();
        *scheduled_req = Some(mine);
        scheduler
            .schedule(fpower::CollaborativeRebootReason::NetstackMigration, Some(theirs))
            .await;
    }

    /// Cancels the currently scheduled collaborative Reboot.
    ///
    /// No-Op if there's none scheduled.
    fn cancel(&mut self) {
        if let Some(cancel) = self.scheduled_req.take() {
            info!("Canceling collaborative reboot request. It's no longer necessary.");
            // Dropping the eventpair cancels the request.
            std::mem::drop(cancel);
        }
    }
}

/// An abstraction over the `fpower::CollaborativeRebootScheduler` FIDL API.
trait CollaborativeRebootScheduler {
    async fn schedule(
        &mut self,
        reason: fpower::CollaborativeRebootReason,
        cancel: Option<zx::EventPair>,
    );
}

/// An implementation of `CollaborativeRebootScheduler` that connects to the
/// API over FIDL.
struct Scheduler {}

impl CollaborativeRebootScheduler for Scheduler {
    async fn schedule(
        &mut self,
        reason: fpower::CollaborativeRebootReason,
        cancel: Option<zx::EventPair>,
    ) {
        let proxy = match fuchsia_component::client::connect_to_protocol::<
            fpower::CollaborativeRebootSchedulerMarker,
        >() {
            Ok(proxy) => proxy,
            Err(e) => {
                error!("Failed to connect to collaborative reboot scheduler: {e:?}");
                return;
            }
        };
        match proxy.schedule_reboot(reason, cancel).await {
            Ok(()) => {}
            Err(e) => error!("Failed to schedule collaborative reboot: {e:?}"),
        }
    }
}

/// The reason to file a crash report.
#[derive(Debug, Clone, PartialEq, Eq)]
enum CrashReportReason {
    FailedHealthcheck,
    RolledBack,
}

/// An abstraction over the `fidl_fuchsia_feedback::CrashReporter` FIDL API.
trait CrashReporter {
    /// The amount of time after startup to wait before filling a rollback
    /// induced crash report.
    const ROLLBACK_CRASH_REPORT_DELAY: Duration;

    fn file_report(&mut self, reason: CrashReportReason);
}

/// An implementation of `CrashReporter` that connects to the API over FIDL.
struct Reporter {}

impl CrashReporter for Reporter {
    const ROLLBACK_CRASH_REPORT_DELAY: Duration = Duration::from_secs(180);

    fn file_report(&mut self, reason: CrashReportReason) {
        const PROGRAM_NAME: &str = "netstack_migration";
        const FAILED_HEALTHCHECK_SIGNATURE: &str = "fuchsia-netstack3-healthcheck-failed";
        const ROLLED_BACK_SIGNATURE: &str = "fuchsia-netstack3-rolled-back";

        info!("Generating a crash report for reason: {reason:?}");

        let proxy = match fuchsia_component::client::connect_to_protocol::<
            fidl_fuchsia_feedback::CrashReporterMarker,
        >() {
            Ok(proxy) => proxy,
            Err(e) => {
                error!("Failed to connect to CrashReport proxy: {e:?}");
                return;
            }
        };

        let signature = match reason {
            CrashReportReason::FailedHealthcheck => FAILED_HEALTHCHECK_SIGNATURE,
            CrashReportReason::RolledBack => ROLLED_BACK_SIGNATURE,
        };
        let report = fidl_fuchsia_feedback::CrashReport {
            program_name: Some(PROGRAM_NAME.to_string()),
            program_uptime: Some(zx::MonotonicInstant::get().into_nanos()),
            crash_signature: Some(signature.to_string()),
            is_fatal: Some(false),
            ..Default::default()
        };

        // File the crash report in a background task. It may take a while and
        // we don't want to block the migration worker from serving other
        // operations.
        Task::spawn(proxy.file_report(report).map(|result| match result {
            Ok(Ok(status)) => info!("Successfully filed crash report. Status={status:?}"),
            Ok(Err(e)) => error!("Failed to file crash report: {e:?}"),
            Err(e) => error!("Failed to request crash report: {e:?}"),
        }))
        .detach();
    }
}

impl<P: PersistenceProvider, CR: CollaborativeRebootScheduler, R: CrashReporter>
    Migration<P, CR, R>
{
    fn new(mut persistence: P, cr_scheduler: CR, crash_reporter: R) -> Self {
        let mut persisted = persistence.open_reader().map(Persisted::load).unwrap_or_else(|e| {
            warn!("could not open persistence reader: {e:?}. using defaults");
            Persisted::default()
        });

        // Routine to update the rollback history on startup based on persisted
        // values from the previous boot.
        //
        // This performs two important operations:
        //   1) reset the failure count if the epoch has advanced, and
        //   2) increment the failure count if the previous boot failed to
        //      migrate.
        let mut new = match persisted.rollback_history {
            None => RollbackHistory::default(),
            Some(RollbackHistory { epoch, count }) if epoch != CURRENT_EPOCH => {
                if count > 0 {
                    info!("Reseting Rollback History for new epoch");
                }
                RollbackHistory::default()
            }
            Some(history) => history,
        };
        if let Some(rollback::Persisted::HealthcheckFailures(failures)) = persisted.rollback {
            if failures >= rollback::MAX_FAILED_HEALTHCHECKS {
                new.count = new.count.saturating_add(1);
            }
        }

        if Some(new) != persisted.rollback_history {
            persisted.rollback_history = Some(new);
            try_persist(&mut persistence, &persisted);
        }

        info!("Netstack Migration starting with persisted state: {persisted:?}");

        let current_boot = persisted.desired_netstack_version();

        match current_boot {
            RollbackNetstackVersion::ForceNetstack2 => {
                warn!(
                    "Previous boot failed to migrate to Netstack3. \
                    Ignoring automated setting and forcibly using Netstack2 \
                    for the current boot."
                );
            }
            RollbackNetstackVersion::ForceNetstack2Indefinitely => {
                warn!(
                    "Previous boot failed to migrate to Netstack3 and the \
                    rollback limit has been exceeded. Ignoring automated \
                    setting and forcibly using Netstack2 until the next epoch."
                );
            }
            RollbackNetstackVersion::Netstack2 | RollbackNetstackVersion::Netstack3 => {}
        }

        Self {
            current_boot,
            persisted,
            persistence,
            collaborative_reboot: CollaborativeReboot {
                scheduler: cr_scheduler,
                scheduled_req: None,
            },
            crash_reporter,
        }
    }

    fn persist(&mut self) {
        let Self {
            current_boot: _,
            persisted,
            persistence,
            collaborative_reboot: _,
            crash_reporter: _,
        } = self;
        try_persist(persistence, persisted);
    }

    fn map_version_setting(
        version: Option<Box<fnet_migration::VersionSetting>>,
    ) -> Option<NetstackVersion> {
        version.map(|v| {
            let fnet_migration::VersionSetting { version } = &*v;
            (*version).into()
        })
    }

    async fn update_collaborative_reboot(&mut self) {
        let Self {
            current_boot,
            persisted,
            persistence: _,
            collaborative_reboot,
            crash_reporter: _,
        } = self;
        if persisted.desired_netstack_version().version() != current_boot.version() {
            // When the current boot differs from our desired version, schedule
            // a reboot (if there's not already one).
            collaborative_reboot.schedule().await
        } else {
            // When the current_boot matches our desired version, we no longer
            // need reboot. Cancel the outstanding request (if any)
            collaborative_reboot.cancel()
        }
    }

    async fn update_rollback_state(&mut self, new_state: rollback::Persisted) {
        if self.persisted.rollback != Some(new_state) {
            self.persisted.rollback = Some(new_state);
            self.update_collaborative_reboot().await;
            self.persist();

            if let rollback::Persisted::HealthcheckFailures(f) = new_state {
                if f >= HEALTHCHECK_FAILURE_THRESHOLD_FOR_CRASH_REPORTS {
                    self.crash_reporter.file_report(CrashReportReason::FailedHealthcheck);
                }
            }
        }
    }

    async fn handle_control_request(
        &mut self,
        req: fnet_migration::ControlRequest,
    ) -> Result<(), fidl::Error> {
        match req {
            fnet_migration::ControlRequest::SetAutomatedNetstackVersion { version, responder } => {
                let version = Self::map_version_setting(version);
                let Self {
                    current_boot: _,
                    persisted: Persisted { automated, user: _, rollback: _, rollback_history: _ },
                    persistence: _,
                    collaborative_reboot: _,
                    crash_reporter: _,
                } = self;
                if version != *automated {
                    info!("automated netstack version switched to {version:?}");
                    *automated = version;
                    self.persist();
                    self.update_collaborative_reboot().await;
                }
                responder.send()
            }
            fnet_migration::ControlRequest::SetUserNetstackVersion { version, responder } => {
                let version = Self::map_version_setting(version);
                let Self {
                    current_boot: _,
                    persisted: Persisted { automated: _, user, rollback: _, rollback_history: _ },
                    persistence: _,
                    collaborative_reboot: _,
                    crash_reporter: _,
                } = self;
                if version != *user {
                    info!("user netstack version switched to {version:?}");
                    *user = version;
                    self.persist();
                    self.update_collaborative_reboot().await;
                }
                responder.send()
            }
        }
    }

    fn handle_state_request(&self, req: fnet_migration::StateRequest) -> Result<(), fidl::Error> {
        let Migration {
            current_boot,
            persisted: Persisted { user, automated, rollback: _, rollback_history: _ },
            persistence: _,
            collaborative_reboot: _,
            crash_reporter: _,
        } = self;
        match req {
            fnet_migration::StateRequest::GetNetstackVersion { responder } => {
                responder.send(&fnet_migration::InEffectVersion {
                    current_boot: current_boot.version().into(),
                    user: (*user).map(Into::into),
                    automated: (*automated).map(Into::into),
                })
            }
        }
    }

    async fn handle_request(&mut self, req: ServiceRequest) -> Result<(), fidl::Error> {
        match req {
            ServiceRequest::Control(r) => self.handle_control_request(r).await,
            ServiceRequest::State(r) => self.handle_state_request(r),
        }
    }
}

struct InspectNodes {
    automated_setting: fuchsia_inspect::UintProperty,
    user_setting: fuchsia_inspect::UintProperty,
    rollback_state: fuchsia_inspect::StringProperty,
}

impl InspectNodes {
    fn new<P, CR, F>(inspector: &fuchsia_inspect::Inspector, m: &Migration<P, CR, F>) -> Self {
        let root = inspector.root();
        let Migration {
            current_boot,
            persisted: Persisted { automated, user, rollback, rollback_history },
            ..
        } = m;
        let automated_setting = root.create_uint(
            "automated_setting",
            NetstackVersion::optional_inspect_uint_value(automated),
        );
        let user_setting =
            root.create_uint("user_setting", NetstackVersion::optional_inspect_uint_value(user));

        let rollback_state = root.create_string("rollback_state", format!("{rollback:?}"));

        // Record immutable state once, instead of keeping track of a property node.
        root.record_uint("current_boot", current_boot.version().inspect_uint_value());
        let forced_netstack2 = match current_boot {
            RollbackNetstackVersion::Netstack2 | RollbackNetstackVersion::Netstack3 => false,
            RollbackNetstackVersion::ForceNetstack2
            | RollbackNetstackVersion::ForceNetstack2Indefinitely => true,
        };
        root.record_bool("forced_netstack2", forced_netstack2);
        root.record_uint("current_epoch", CURRENT_EPOCH.into());
        root.record_string("rollback_history", format!("{rollback_history:?}"));

        Self { automated_setting, user_setting, rollback_state }
    }

    fn update<P, CR, F>(&self, m: &Migration<P, CR, F>) {
        let Migration {
            persisted: Persisted { automated, user, rollback, rollback_history: _ },
            ..
        } = m;
        let Self { automated_setting, user_setting, rollback_state } = self;
        automated_setting.set(NetstackVersion::optional_inspect_uint_value(automated));
        user_setting.set(NetstackVersion::optional_inspect_uint_value(user));
        rollback_state.set(&format!("{rollback:?}"));
    }
}

/// Wraps communication with metrics (cobalt) server.
struct MetricsLogger {
    logger: Option<fmetrics::MetricEventLoggerProxy>,
}

impl MetricsLogger {
    async fn new() -> Self {
        let (logger, server_end) =
            fidl::endpoints::create_proxy::<fmetrics::MetricEventLoggerMarker>();

        let factory = match fuchsia_component::client::connect_to_protocol::<
            fmetrics::MetricEventLoggerFactoryMarker,
        >() {
            Ok(f) => f,
            Err(e) => {
                warn!("can't connect to logger factory {e:?}");
                return Self { logger: None };
            }
        };

        match factory
            .create_metric_event_logger(
                &fmetrics::ProjectSpec {
                    customer_id: Some(metrics_registry::CUSTOMER_ID),
                    project_id: Some(metrics_registry::PROJECT_ID),
                    ..Default::default()
                },
                server_end,
            )
            .await
        {
            Ok(Ok(())) => Self { logger: Some(logger) },
            Ok(Err(e)) => {
                warn!("can't create event logger {e:?}");
                Self { logger: None }
            }
            Err(e) => {
                warn!("error connecting to metric event logger {e:?}");
                Self { logger: None }
            }
        }
    }

    /// Logs metrics from `migration` to the metrics server.
    async fn log_metrics<P, CR, F>(&self, migration: &Migration<P, CR, F>) {
        let logger = if let Some(logger) = self.logger.as_ref() {
            logger
        } else {
            // Silently don't log metrics if we didn't manage to create a
            // logger, warnings are emitted upon creation.
            return;
        };

        let current_boot = match migration.current_boot {
            RollbackNetstackVersion::Netstack2
            | RollbackNetstackVersion::ForceNetstack2
            | RollbackNetstackVersion::ForceNetstack2Indefinitely => {
                metrics_registry::StackMigrationCurrentBootMetricDimensionNetstackVersion::Netstack2
            }
            RollbackNetstackVersion::Netstack3 => {
                metrics_registry::StackMigrationCurrentBootMetricDimensionNetstackVersion::Netstack3
            }
        }
        .as_event_code();
        let user = match migration.persisted.user {
            None => metrics_registry::StackMigrationUserSettingMetricDimensionNetstackVersion::NoSelection,
            Some(NetstackVersion::Netstack2) => metrics_registry::StackMigrationUserSettingMetricDimensionNetstackVersion::Netstack2,
            Some(NetstackVersion::Netstack3) => metrics_registry::StackMigrationUserSettingMetricDimensionNetstackVersion::Netstack3,
        }
        .as_event_code();
        let automated = match migration.persisted.automated {
            None => metrics_registry::StackMigrationAutomatedSettingMetricDimensionNetstackVersion::NoSelection,
            Some(NetstackVersion::Netstack2) => metrics_registry::StackMigrationAutomatedSettingMetricDimensionNetstackVersion::Netstack2,
            Some(NetstackVersion::Netstack3) => metrics_registry::StackMigrationAutomatedSettingMetricDimensionNetstackVersion::Netstack3,
        }.as_event_code();
        let rollback_state = compute_state_metric(migration).as_event_code();
        for (metric_id, event_code) in [
            (metrics_registry::STACK_MIGRATION_CURRENT_BOOT_METRIC_ID, current_boot),
            (metrics_registry::STACK_MIGRATION_USER_SETTING_METRIC_ID, user),
            (metrics_registry::STACK_MIGRATION_AUTOMATED_SETTING_METRIC_ID, automated),
            (metrics_registry::STACK_MIGRATION_STATE_METRIC_ID, rollback_state),
        ] {
            let occurrence_count = 1;
            logger
                .log_occurrence(metric_id, occurrence_count, &[event_code][..])
                .await
                .map(|r| {
                    r.unwrap_or_else(|e| warn!("error reported logging metric {metric_id} {e:?}"))
                })
                .unwrap_or_else(|fidl_error| {
                    warn!("error logging metric {metric_id} {fidl_error:?}")
                });
        }
    }
}

fn compute_state_metric<P, CR, F>(
    migration: &Migration<P, CR, F>,
) -> metrics_registry::StackMigrationStateMetricDimensionMigrationState {
    use metrics_registry::StackMigrationStateMetricDimensionMigrationState as state_metric;
    let Migration {
        current_boot,
        persisted: Persisted { automated, user: _, rollback, rollback_history: _ },
        persistence: _,
        collaborative_reboot: _,
        crash_reporter: _,
    } = migration;

    match (current_boot, automated, rollback) {
        (RollbackNetstackVersion::Netstack2, Some(NetstackVersion::Netstack2) | None, _) => {
            state_metric::NotStarted
        }
        (RollbackNetstackVersion::Netstack2, Some(NetstackVersion::Netstack3), _) => {
            state_metric::Scheduled
        }
        (RollbackNetstackVersion::ForceNetstack2Indefinitely, _, _) => state_metric::Abandoned,
        (RollbackNetstackVersion::ForceNetstack2, Some(NetstackVersion::Netstack3), _) => {
            state_metric::RolledBack
        }
        // NB: If a device observes the automated setting switch to Netstack2
        // while it's current boot is `ForceNetstack2` it won't have any need
        // to schedule a reboot, and it will perpetually stay in
        // `ForceNetstack`. Rather that perpetually reporting
        // `state_metric::RolledBack`, it's more useful from a diagnostics
        // perspective to consider this device `state_metric::NotStarted`.
        (RollbackNetstackVersion::ForceNetstack2, Some(NetstackVersion::Netstack2) | None, _) => {
            state_metric::NotStarted
        }
        (RollbackNetstackVersion::Netstack3, Some(NetstackVersion::Netstack2) | None, _) => {
            state_metric::Canceled
        }
        (RollbackNetstackVersion::Netstack3, Some(NetstackVersion::Netstack3), None) => {
            state_metric::InProgress
        }
        (
            RollbackNetstackVersion::Netstack3,
            Some(NetstackVersion::Netstack3),
            Some(rollback::Persisted::HealthcheckFailures(f)),
        ) => {
            if *f >= rollback::MAX_FAILED_HEALTHCHECKS {
                state_metric::Failed
            } else {
                state_metric::InProgress
            }
        }
        (
            RollbackNetstackVersion::Netstack3,
            Some(NetstackVersion::Netstack3),
            Some(rollback::Persisted::Success),
        ) => state_metric::Success,
    }
}

#[fuchsia::main]
pub async fn main() {
    info!("running netstack migration service");

    let mut fs = ServiceFs::new();
    let _: &mut ServiceFsDir<'_, _> = fs
        .dir("svc")
        .add_fidl_service(|rs: fnet_migration::ControlRequestStream| {
            rs.map(|req| req.map(ServiceRequest::Control)).left_stream()
        })
        .add_fidl_service(|rs: fnet_migration::StateRequestStream| {
            rs.map(|req| req.map(ServiceRequest::State)).right_stream()
        });
    let _: &mut ServiceFs<_> =
        fs.take_and_serve_directory_handle().expect("failed to take out directory handle");

    let mut migration = Migration::new(DataPersistenceProvider {}, Scheduler {}, Reporter {});
    main_inner(
        &mut migration,
        fs.fuse().flatten_unordered(None),
        rollback::FidlHttpFetcher::new(),
        rollback::new_healthcheck_stream(),
    )
    .await
}

async fn main_inner<
    P: PersistenceProvider,
    CR: CollaborativeRebootScheduler,
    R: CrashReporter,
    H: rollback::HttpFetcher + Send + 'static,
    T: Stream<Item = ()> + Send + 'static,
    SR: Stream<Item = Result<ServiceRequest, fidl::Error>>,
>(
    migration: &mut Migration<P, CR, R>,
    service_request_stream: SR,
    http_fetcher: H,
    healthcheck_tick: T,
) {
    let inspector = fuchsia_inspect::component::inspector();
    let _inspect_server =
        inspect_runtime::publish(inspector, inspect_runtime::PublishOptions::default())
            .expect("failed to serve inspector");
    let inspect_nodes = InspectNodes::new(inspector, &migration);

    let metrics_logger = MetricsLogger::new().await;

    let (desired_version_sender, desired_version_receiver) = mpsc::unbounded();
    let (rollback_state_sender, rollback_state_receiver) = mpsc::unbounded();

    // NB: Check if we failed to migrate to NS3 in the previous boot.
    //
    // It's important to perform this check before the rollback state is reset
    // below.
    let current_boot_is_rollback = match migration.persisted.rollback {
        Some(rollback::Persisted::Success) | None => false,
        Some(rollback::Persisted::HealthcheckFailures(f)) => f >= rollback::MAX_FAILED_HEALTHCHECKS,
    };

    let rollback_state =
        rollback::State::new(migration.persisted.rollback, migration.current_boot.version());

    // Update rollback persistence immediately in case the device reboots before
    // the rollback module has time to send an asynchronous update. This is
    // required for correctness if Netstack3 is crashing on startup, or in the
    // following case:
    //
    // 1. Device fails to migrate to Netstack3 and persists
    //    HealthcheckFailures(MAX_FAILED_HEALTHCHECKS), which will force
    //    Netstack2 on subsequent boots.
    // 2. Device reboots into Netstack2, sees that it should be running
    //    Netstack3, and schedules a reboot without clearing the failures.
    // 3. Device reboots back into Netstack3 and sees that it should schedule
    //    a reboot because the persisted failures are above the limit.
    migration.update_rollback_state(rollback_state.persisted()).await;

    Task::spawn(async move {
        rollback::run(
            rollback_state,
            http_fetcher,
            desired_version_receiver,
            rollback_state_sender,
            pin!(healthcheck_tick),
        )
        .await
    })
    .detach();

    enum Action {
        ServiceRequest(Result<ServiceRequest, fidl::Error>),
        LogMetrics,
        UpdateRollbackState(rollback::Persisted),
        FileRollbackCrashReport,
    }

    let metrics_logging_interval = fuchsia_async::MonotonicDuration::from_hours(1);
    let mut stream: futures::stream::SelectAll<Pin<Box<dyn Stream<Item = Action>>>> =
        futures::stream::SelectAll::new();

    // Always log metrics once on startup then periodically log new values so
    // the aggregation window always contains one sample of the current
    // settings.
    stream.push(Box::pin(Box::new(
        futures::stream::once(futures::future::ready(()))
            .chain(fuchsia_async::Interval::new(metrics_logging_interval))
            .map(|()| Action::LogMetrics),
    )));
    stream.push(Box::pin(Box::new(Box::pin(service_request_stream.map(Action::ServiceRequest)))));
    stream.push(Box::pin(rollback_state_receiver.map(|state| Action::UpdateRollbackState(state))));

    // If the current boot is a rollback, schedule a crash report to be filed
    // soon. Don't file it right away, as during early boot there is not much
    // useful diagnostics data available.
    if current_boot_is_rollback {
        stream.push(Box::pin(
            futures::stream::once(fuchsia_async::Timer::new(R::ROLLBACK_CRASH_REPORT_DELAY))
                .map(|()| Action::FileRollbackCrashReport),
        ));
    }

    while let Some(action) = stream.next().await {
        match action {
            Action::ServiceRequest(req) => {
                let result = match req {
                    Ok(req) => migration.handle_request(req).await,
                    Err(e) => Err(e),
                };
                // Always update inspector state after handling a request.
                inspect_nodes.update(&migration);

                // Send the desired netstack version to the rollback mechanism,
                // but ignore the "forced" setting. The "forced" setting comes
                // from the rollback mechanism, and sending that signal back
                // into it would cause a the mechanism to incorrectly detect
                // a cancelation.
                match desired_version_sender.unbounded_send(
                    migration.persisted.desired_netstack_version().version_ignoring_force(),
                ) {
                    Ok(()) => (),
                    Err(e) => {
                        error!("error sending update to rollback module: {:?}", e);
                    }
                }

                match result {
                    Ok(()) => (),
                    Err(e) => {
                        if !e.is_closed() {
                            error!("error processing FIDL request {:?}", e)
                        }
                    }
                }
            }
            Action::LogMetrics => {
                metrics_logger.log_metrics(&migration).await;
            }
            Action::UpdateRollbackState(new_state) => {
                migration.update_rollback_state(new_state).await;
                // Always update inspector state when the rollback state
                // changes.
                inspect_nodes.update(&migration);
            }
            Action::FileRollbackCrashReport => {
                migration.crash_reporter.file_report(CrashReportReason::RolledBack);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use async_utils::event::{Event, EventWait};
    use diagnostics_assertions::assert_data_tree;
    use fidl::Peered as _;
    use fidl_fuchsia_net_http as fnet_http;
    use fuchsia_async::TimeoutExt;
    use futures::FutureExt;
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::time::Duration;
    use test_case::test_case;

    #[derive(Default, Clone)]
    struct InMemory {
        file: Rc<RefCell<Option<Vec<u8>>>>,
    }

    impl InMemory {
        fn with_persisted(p: Persisted) -> Self {
            let mut s = Self::default();
            p.save(s.open_writer().unwrap());
            s
        }
    }

    impl PersistenceProvider for InMemory {
        type Writer = Self;
        type Reader = std::io::Cursor<Vec<u8>>;

        fn open_writer(&mut self) -> std::io::Result<Self::Writer> {
            *self.file.borrow_mut() = Some(Vec::new());
            Ok(self.clone())
        }

        fn open_reader(&self) -> std::io::Result<Self::Reader> {
            self.file
                .borrow()
                .clone()
                .map(std::io::Cursor::new)
                .ok_or_else(|| std::io::ErrorKind::NotFound.into())
        }
    }

    impl std::io::Write for InMemory {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            let r = self.file.borrow_mut().as_mut().expect("no file open").write(buf);
            r
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    struct NoCollaborativeReboot;

    impl CollaborativeRebootScheduler for NoCollaborativeReboot {
        async fn schedule(
            &mut self,
            _reason: fpower::CollaborativeRebootReason,
            _cancel: Option<zx::EventPair>,
        ) {
            panic!("unexpectedly attempted to schedule a collaborative reboot");
        }
    }

    #[derive(Default)]
    struct FakeCollaborativeReboot {
        req: Option<zx::EventPair>,
    }

    impl CollaborativeRebootScheduler for FakeCollaborativeReboot {
        async fn schedule(
            &mut self,
            reason: fpower::CollaborativeRebootReason,
            cancel: Option<zx::EventPair>,
        ) {
            assert_eq!(reason, fpower::CollaborativeRebootReason::NetstackMigration);
            let cancel = cancel.expect("cancellation signal must be provided");
            assert_eq!(self.req.replace(cancel), None, "attempted to schedule multiple reboots");
        }
    }

    struct NoCrashReports;

    impl CrashReporter for NoCrashReports {
        const ROLLBACK_CRASH_REPORT_DELAY: Duration = Duration::ZERO;
        fn file_report(&mut self, reason: CrashReportReason) {
            panic!("unexpectedly attemped to file a crash report: {reason:?}")
        }
    }

    #[derive(Default)]
    struct FakeCrashReporter {
        reports: Vec<CrashReportReason>,
    }

    impl CrashReporter for FakeCrashReporter {
        const ROLLBACK_CRASH_REPORT_DELAY: Duration = Duration::from_millis(1);
        fn file_report(&mut self, reason: CrashReportReason) {
            self.reports.push(reason)
        }
    }

    /// Signals an `EventWait` once the `target` crash reports have been filed.
    struct AwaitCrashReports {
        target: Vec<CrashReportReason>,
        reports: Vec<CrashReportReason>,
        event: Event,
    }

    impl AwaitCrashReports {
        fn new(target: Vec<CrashReportReason>) -> (Self, EventWait) {
            let event = Event::new();
            let wait = event.wait();
            (Self { target, reports: Vec::new(), event }, wait)
        }
    }

    impl CrashReporter for AwaitCrashReports {
        const ROLLBACK_CRASH_REPORT_DELAY: Duration = Duration::from_millis(1);
        fn file_report(&mut self, reason: CrashReportReason) {
            self.reports.push(reason);
            if self.reports == self.target {
                assert!(self.event.signal());
            }
        }
    }

    fn serve_migration<
        P: PersistenceProvider,
        CR: CollaborativeRebootScheduler,
        R: CrashReporter,
    >(
        migration: Migration<P, CR, R>,
    ) -> (
        impl futures::Future<Output = Migration<P, CR, R>>,
        fnet_migration::ControlProxy,
        fnet_migration::StateProxy,
    ) {
        let (control, control_server) =
            fidl::endpoints::create_proxy_and_stream::<fnet_migration::ControlMarker>();
        let (state, state_server) =
            fidl::endpoints::create_proxy_and_stream::<fnet_migration::StateMarker>();

        let fut = {
            let control =
                control_server.map(|req| ServiceRequest::Control(req.expect("control error")));
            let state = state_server.map(|req| ServiceRequest::State(req.expect("state error")));
            futures::stream::select(control, state).fold(migration, |mut migration, req| async {
                migration.handle_request(req).await.expect("handling request");
                migration
            })
        };
        (fut, control, state)
    }

    #[test_case(Persisted{
        user: Some(NetstackVersion::Netstack2),
        automated: None,
        rollback: None,
        rollback_history: None,
    }; "user_netstack2")]
    #[test_case(Persisted{
        user: Some(NetstackVersion::Netstack3),
        automated: None,
        rollback: None,
        rollback_history: None,
    }; "user_netstack3")]
    #[test_case(Persisted{
        user: None,
        automated: None,
        rollback: None,
        rollback_history: None,
    }; "none")]
    #[test_case(Persisted{
        user: None,
        automated: Some(NetstackVersion::Netstack2),
        rollback: None,
        rollback_history: None,
    }; "automated_netstack2")]
    #[test_case(Persisted{
        user: None,
        automated: Some(NetstackVersion::Netstack3),
        rollback: None,
        rollback_history: None,
    }; "automated_netstack3")]
    #[test_case(Persisted{
        user: None,
        automated: None,
        rollback: Some(rollback::Persisted::Success),
        rollback_history: None,
    }; "rollback_success")]
    #[test_case(Persisted{
        user: None,
        automated: None,
        rollback: Some(rollback::Persisted::HealthcheckFailures(0)),
        rollback_history: Some(RollbackHistory {epoch: CURRENT_EPOCH, count: 3}),
    }; "rollback_history")]
    #[test_case(Persisted{
        user: Some(NetstackVersion::Netstack2),
        automated: Some(NetstackVersion::Netstack3),
        rollback: Some(rollback::Persisted::HealthcheckFailures(5)),
        rollback_history: Some(RollbackHistory {epoch: CURRENT_EPOCH, count: 3}),
    }; "all")]
    #[fuchsia::test(add_test_attr = false)]
    fn persist_save_load(v: Persisted) {
        let mut m = InMemory::default();
        v.save(m.open_writer().unwrap());
        assert_eq!(Persisted::load(m.open_reader().unwrap()), v);
    }

    #[fuchsia::test]
    fn uses_defaults_if_no_persistence() {
        let m = Migration::new(InMemory::default(), NoCollaborativeReboot, NoCrashReports);
        let Migration {
            current_boot,
            persisted: Persisted { user, automated, rollback: _, rollback_history: _ },
            persistence: _,
            collaborative_reboot: _,
            crash_reporter: _,
        } = m;
        assert_eq!(current_boot.version(), DEFAULT_NETSTACK);
        assert_eq!(user, None);
        assert_eq!(automated, None);
    }

    #[test_case(
        None, Some(NetstackVersion::Netstack3), None, None, NetstackVersion::Netstack3;
        "automated_ns3")]
    #[test_case(
        None, Some(NetstackVersion::Netstack2), None, None, NetstackVersion::Netstack2;
        "automated_ns2")]
    #[test_case(
        Some(NetstackVersion::Netstack3),
        Some(NetstackVersion::Netstack2),
        Some(rollback::Persisted::HealthcheckFailures(rollback::MAX_FAILED_HEALTHCHECKS)),
        Some(RollbackHistory{epoch: CURRENT_EPOCH, count: MAX_ROLLBACKS_PER_EPOCH}),
        NetstackVersion::Netstack3;
        "user_ns3_override")]
    #[test_case(
        Some(NetstackVersion::Netstack2),
        Some(NetstackVersion::Netstack3),
        Some(rollback::Persisted::Success),
        None,
        NetstackVersion::Netstack2;
        "user_ns2_override")]
    #[test_case(
        Some(NetstackVersion::Netstack2),
        None,
        None,
        None,
        NetstackVersion::Netstack2; "user_ns2")]
    #[test_case(
        Some(NetstackVersion::Netstack3),
        None,
        None,
        None,
        NetstackVersion::Netstack3; "user_ns3")]
    #[test_case(
        None,
        Some(NetstackVersion::Netstack3),
        Some(rollback::Persisted::HealthcheckFailures(rollback::MAX_FAILED_HEALTHCHECKS)),
        None,
        NetstackVersion::Netstack2; "rollback_to_ns2")]
    #[test_case(
        None,
        Some(NetstackVersion::Netstack3),
        Some(rollback::Persisted::HealthcheckFailures(0)),
        Some(RollbackHistory{epoch: CURRENT_EPOCH, count: MAX_ROLLBACKS_PER_EPOCH}),
        NetstackVersion::Netstack2; "indefinitely_rolled_back_to_ns2")]
    #[test_case(
        None,
        Some(NetstackVersion::Netstack3),
        Some(rollback::Persisted::HealthcheckFailures(0)),
        Some(RollbackHistory{epoch: CURRENT_EPOCH - 1, count: MAX_ROLLBACKS_PER_EPOCH}),
        NetstackVersion::Netstack3; "ignore_rollback_history_from_old_epoch")]
    #[test_case(None, None, None, None, DEFAULT_NETSTACK; "default")]
    #[fuchsia::test]
    async fn get_netstack_version(
        p_user: Option<NetstackVersion>,
        p_automated: Option<NetstackVersion>,
        p_rollback: Option<rollback::Persisted>,
        p_history: Option<RollbackHistory>,
        expect: NetstackVersion,
    ) {
        let m = Migration::new(
            InMemory::with_persisted(Persisted {
                user: p_user,
                automated: p_automated,
                rollback: p_rollback,
                rollback_history: p_history,
            }),
            NoCollaborativeReboot,
            NoCrashReports,
        );
        let Migration {
            current_boot,
            persisted: Persisted { user, automated, rollback: _, rollback_history: _ },
            persistence: _,
            collaborative_reboot: _,
            crash_reporter: _,
        } = &m;
        assert_eq!(current_boot.version(), expect);
        assert_eq!(*user, p_user);
        assert_eq!(*automated, p_automated);

        let (serve, _, state) = serve_migration(m);
        let fut = async move {
            let fnet_migration::InEffectVersion { current_boot, user, automated } =
                state.get_netstack_version().await.expect("get netstack version");
            let expect = expect.into();
            let p_user = p_user.map(Into::into);
            let p_automated = p_automated.map(Into::into);
            assert_eq!(current_boot, expect);
            assert_eq!(user, p_user);
            assert_eq!(automated, p_automated);
        };
        let (_, ()): (Migration<_, _, _>, _) = futures::future::join(serve, fut).await;
    }

    #[derive(Debug, Copy, Clone)]
    enum SetMechanism {
        User,
        Automated,
    }

    #[test_case(SetMechanism::User, NetstackVersion::Netstack2; "set_user_ns2")]
    #[test_case(SetMechanism::User, NetstackVersion::Netstack3; "set_user_ns3")]
    #[test_case(SetMechanism::Automated, NetstackVersion::Netstack2; "set_automated_ns2")]
    #[test_case(SetMechanism::Automated, NetstackVersion::Netstack3; "set_automated_ns3")]
    #[fuchsia::test]
    async fn set_netstack_version(mechanism: SetMechanism, set_version: NetstackVersion) {
        let m = Migration::new(
            InMemory::with_persisted(Default::default()),
            FakeCollaborativeReboot::default(),
            NoCrashReports,
        );
        let (serve, control, _) = serve_migration(m);
        let fut = async move {
            let setting = fnet_migration::VersionSetting { version: set_version.into() };
            match mechanism {
                SetMechanism::User => control
                    .set_user_netstack_version(Some(&setting))
                    .await
                    .expect("set user netstack version"),
                SetMechanism::Automated => control
                    .set_automated_netstack_version(Some(&setting))
                    .await
                    .expect("set automated netstack version"),
            }
        };
        let (migration, ()) = futures::future::join(serve, fut).await;

        let validate_versions = |m: &Migration<_, _, _>, current| {
            let Migration {
                current_boot,
                persisted: Persisted { user, automated, rollback: _, rollback_history: _ },
                persistence: _,
                collaborative_reboot: _,
                crash_reporter: _,
            } = m;
            assert_eq!(current_boot.version(), current);
            match mechanism {
                SetMechanism::User => {
                    assert_eq!(*user, Some(set_version));
                    assert_eq!(*automated, None);
                }
                SetMechanism::Automated => {
                    assert_eq!(*user, None);
                    assert_eq!(*automated, Some(set_version));
                }
            }
        };

        validate_versions(&migration, DEFAULT_NETSTACK);
        // There should only be a collaborative reboot request if the new
        // version differs from the default version.
        let cr_req = &migration.collaborative_reboot.scheduler.req;
        if set_version != DEFAULT_NETSTACK {
            assert_eq!(Ok(false), cr_req.as_ref().expect("there should be a request").is_closed());
        } else {
            assert_eq!(cr_req, &None);
        }

        // Check that the setting was properly persisted.
        let migration = Migration::new(
            migration.persistence,
            migration.collaborative_reboot.scheduler,
            migration.crash_reporter,
        );
        validate_versions(&migration, set_version);
    }

    #[fuchsia::test]
    async fn update_rollback_state() {
        let mut migration = Migration::new(
            InMemory::with_persisted(Persisted {
                automated: Some(NetstackVersion::Netstack3),
                user: None,
                rollback: None,
                rollback_history: None,
            }),
            FakeCollaborativeReboot::default(),
            FakeCrashReporter::default(),
        );

        assert_eq!(migration.current_boot.version(), NetstackVersion::Netstack3);
        assert!(migration.collaborative_reboot.scheduler.req.is_none());

        // The first update shouldn't schedule a reboot because we haven't
        // passed the healthcheck threshold yet.
        migration.update_rollback_state(rollback::Persisted::HealthcheckFailures(1)).await;
        assert_matches!(
            migration.persisted.rollback,
            Some(rollback::Persisted::HealthcheckFailures(1))
        );
        assert!(migration.collaborative_reboot.scheduler.req.is_none());

        // This second update should schedule a reboot because we've passed
        // the healthcheck limit and want to roll back to Netstack2.
        migration
            .update_rollback_state(rollback::Persisted::HealthcheckFailures(
                rollback::MAX_FAILED_HEALTHCHECKS,
            ))
            .await;
        assert_matches!(
            migration.persisted.rollback,
            Some(rollback::Persisted::HealthcheckFailures(rollback::MAX_FAILED_HEALTHCHECKS))
        );
        assert_eq!(
            migration
                .collaborative_reboot
                .scheduler
                .req
                .as_ref()
                .expect("reboot was not scheduled")
                .is_closed()
                .unwrap(),
            false
        );

        // It should have also filed a crash report
        assert_eq!(migration.crash_reporter.reports, vec![CrashReportReason::FailedHealthcheck]);

        // This emulates seeing a healthcheck success before rebooting, in which
        // case we should see the reboot get canceled.
        migration.update_rollback_state(rollback::Persisted::Success).await;
        assert_matches!(migration.persisted.rollback, Some(rollback::Persisted::Success));
        assert!(
            migration.collaborative_reboot.scheduler.req.as_ref().unwrap().is_closed().unwrap()
        );

        // Ensure that the changes were persisted successfully.
        let migration = Migration::new(
            migration.persistence,
            migration.collaborative_reboot.scheduler,
            migration.crash_reporter,
        );
        assert_matches!(migration.persisted.rollback, Some(rollback::Persisted::Success));
    }

    #[test_case(SetMechanism::User, Some(NetstackVersion::Netstack2), false)]
    #[test_case(SetMechanism::User, Some(NetstackVersion::Netstack3), true)]
    #[test_case(SetMechanism::User, None, false)]
    #[test_case(SetMechanism::Automated, Some(NetstackVersion::Netstack2), false)]
    #[test_case(SetMechanism::Automated, Some(NetstackVersion::Netstack3), true)]
    #[test_case(SetMechanism::Automated, None, true)]
    #[fuchsia::test]
    async fn cancel_collaborative_reboot(
        mechanism: SetMechanism,
        version: Option<NetstackVersion>,
        expect_canceled: bool,
    ) {
        let migration = Migration::new(
            InMemory::with_persisted(Persisted {
                user: None,
                automated: None,
                rollback: None,
                rollback_history: None,
            }),
            FakeCollaborativeReboot::default(),
            NoCrashReports,
        );

        // Start of by updating the automated setting to Netstack2; this ensures
        // there is a pending request to cancel.
        let (serve, control, _) = serve_migration(migration);
        let fut = async move {
            control
                .set_automated_netstack_version(Some(&fnet_migration::VersionSetting {
                    version: fnet_migration::NetstackVersion::Netstack2,
                }))
                .await
                .expect("set automated netstack version");
        };
        let (migration, ()) = futures::future::join(serve, fut).await;
        let cancel = migration
            .collaborative_reboot
            .scheduler
            .req
            .as_ref()
            .expect("there should be a request");
        assert_eq!(Ok(false), cancel.is_closed());

        // Update the setting based on the test parameters
        let (serve, control, _) = serve_migration(migration);
        let fut = async move {
            let setting = version.map(|v| fnet_migration::VersionSetting { version: v.into() });
            match mechanism {
                SetMechanism::User => control
                    .set_user_netstack_version(setting.as_ref())
                    .await
                    .expect("set user netstack version"),
                SetMechanism::Automated => control
                    .set_automated_netstack_version(setting.as_ref())
                    .await
                    .expect("set automated netstack version"),
            }
        };
        let (migration, ()) = futures::future::join(serve, fut).await;

        let cancel = migration
            .collaborative_reboot
            .scheduler
            .req
            .as_ref()
            .expect("there should be a request");
        assert_eq!(Ok(expect_canceled), cancel.is_closed());
    }

    #[test_case(SetMechanism::User)]
    #[test_case(SetMechanism::Automated)]
    #[fuchsia::test]
    async fn clear_netstack_version(mechanism: SetMechanism) {
        const PREVIOUS_VERSION: NetstackVersion = NetstackVersion::Netstack2;
        let m = Migration::new(
            InMemory::with_persisted(Persisted {
                user: Some(PREVIOUS_VERSION),
                automated: Some(PREVIOUS_VERSION),
                rollback: None,
                rollback_history: None,
            }),
            NoCollaborativeReboot,
            NoCrashReports,
        );
        let (serve, control, _) = serve_migration(m);
        let fut = async move {
            match mechanism {
                SetMechanism::User => control
                    .set_user_netstack_version(None)
                    .await
                    .expect("set user netstack version"),
                SetMechanism::Automated => control
                    .set_automated_netstack_version(None)
                    .await
                    .expect("set automated netstack version"),
            }
        };
        let (migration, ()) = futures::future::join(serve, fut).await;

        let validate_versions = |m: &Migration<_, _, _>| {
            let Migration {
                current_boot,
                persisted: Persisted { user, automated, rollback: _, rollback_history: _ },
                persistence: _,
                collaborative_reboot: _,
                crash_reporter: _,
            } = m;
            assert_eq!(current_boot.version(), PREVIOUS_VERSION);
            match mechanism {
                SetMechanism::User => {
                    assert_eq!(*user, None);
                    assert_eq!(*automated, Some(PREVIOUS_VERSION));
                }
                SetMechanism::Automated => {
                    assert_eq!(*user, Some(PREVIOUS_VERSION));
                    assert_eq!(*automated, None);
                }
            }
        };

        validate_versions(&migration);
        // Check that the setting was properly persisted.
        let migration = Migration::new(
            migration.persistence,
            migration.collaborative_reboot.scheduler,
            migration.crash_reporter,
        );
        validate_versions(&migration);
    }

    #[fuchsia::test]
    async fn inspect() {
        let mut m = Migration::new(
            InMemory::with_persisted(Persisted {
                user: Some(NetstackVersion::Netstack2),
                automated: Some(NetstackVersion::Netstack3),
                rollback: None,
                rollback_history: None,
            }),
            NoCollaborativeReboot,
            NoCrashReports,
        );
        let inspector = fuchsia_inspect::component::inspector();
        let nodes = InspectNodes::new(inspector, &m);
        assert_data_tree!(inspector,
            root: {
                current_boot: 2u64,
                user_setting: 2u64,
                automated_setting: 3u64,
                rollback_state: "None",
                forced_netstack2: false,
                current_epoch: CURRENT_EPOCH,
                rollback_history: format!(
                    "Some(RollbackHistory {{ epoch: {CURRENT_EPOCH}, count: 0 }})"
                ),
            }
        );

        m.persisted = Persisted {
            user: None,
            automated: Some(NetstackVersion::Netstack2),
            rollback: None,
            rollback_history: None,
        };
        nodes.update(&m);
        assert_data_tree!(inspector,
            root: {
                current_boot: 2u64,
                user_setting: 0u64,
                automated_setting: 2u64,
                rollback_state: "None",
                forced_netstack2: false,
                current_epoch: CURRENT_EPOCH,
                rollback_history: format!(
                    "Some(RollbackHistory {{ epoch: {CURRENT_EPOCH}, count: 0 }})"
                ),
            }
        );
    }

    #[fuchsia::test]
    async fn inspect_rollback() {
        let mut m = Migration::new(
            InMemory::with_persisted(Persisted {
                user: None,
                automated: Some(NetstackVersion::Netstack3),
                rollback: Some(rollback::Persisted::HealthcheckFailures(
                    rollback::MAX_FAILED_HEALTHCHECKS,
                )),
                rollback_history: None,
            }),
            NoCollaborativeReboot,
            NoCrashReports,
        );
        let inspector = fuchsia_inspect::component::inspector();
        let nodes = InspectNodes::new(inspector, &m);
        assert_data_tree!(inspector,
            root: {
                current_boot: 2u64,
                user_setting: 0u64,
                automated_setting: 3u64,
                rollback_state: "Some(HealthcheckFailures(5))",
                forced_netstack2: true,
                current_epoch: CURRENT_EPOCH,
                rollback_history: format!(
                    "Some(RollbackHistory {{ epoch: {CURRENT_EPOCH}, count: 1 }})"
                ),
            }
        );

        m.persisted.rollback =
            Some(rollback::Persisted::HealthcheckFailures(rollback::MAX_FAILED_HEALTHCHECKS + 1));
        nodes.update(&m);
        assert_data_tree!(inspector,
            root: {
                current_boot: 2u64,
                user_setting: 0u64,
                automated_setting: 3u64,
                rollback_state: "Some(HealthcheckFailures(6))",
                forced_netstack2: true,
                current_epoch: CURRENT_EPOCH,
                rollback_history: format!(
                    "Some(RollbackHistory {{ epoch: {CURRENT_EPOCH}, count: 1 }})"
                ),
            }
        );
        m.persisted.rollback = Some(rollback::Persisted::Success);
        nodes.update(&m);
        assert_data_tree!(inspector,
            root: {
                current_boot: 2u64,
                user_setting: 0u64,
                automated_setting: 3u64,
                rollback_state: "Some(Success)",
                forced_netstack2: true,
                current_epoch: CURRENT_EPOCH,
                rollback_history: format!(
                    "Some(RollbackHistory {{ epoch: {CURRENT_EPOCH}, count: 1 }})"
                ),
            }
        );
    }

    #[test_case(CURRENT_EPOCH, 2, true; "ns2_with_failures_in_current_epoch")]
    #[test_case(CURRENT_EPOCH - 1, 3, false; "ns3_with_failures_in_old_epoch")]
    #[fuchsia::test]
    async fn inspect_rollback_history(epoch: u32, current_boot: u64, forced_netstack2: bool) {
        let m = Migration::new(
            InMemory::with_persisted(Persisted {
                user: None,
                automated: Some(NetstackVersion::Netstack3),
                rollback: Some(rollback::Persisted::HealthcheckFailures(0)),
                rollback_history: Some(RollbackHistory {
                    epoch: epoch,
                    count: MAX_ROLLBACKS_PER_EPOCH,
                }),
            }),
            NoCollaborativeReboot,
            NoCrashReports,
        );
        let inspector = fuchsia_inspect::component::inspector();
        let _nodes = InspectNodes::new(inspector, &m);
        assert_data_tree!(inspector,
            root: {
                current_boot: current_boot,
                user_setting: 0u64,
                automated_setting: 3u64,
                rollback_state: "Some(HealthcheckFailures(0))",
                forced_netstack2: forced_netstack2,
                current_epoch: CURRENT_EPOCH,
                rollback_history: format!(
                    "Some(RollbackHistory {{ epoch: {}, count: {} }})",
                    CURRENT_EPOCH,
                    // Check if the history should have been reset.
                    if epoch == CURRENT_EPOCH { MAX_ROLLBACKS_PER_EPOCH } else {0},
                ),
            }
        );
    }

    #[test_case::test_matrix(
    [
        (RollbackNetstackVersion::Netstack2, metrics_registry::StackMigrationCurrentBootMetricDimensionNetstackVersion::Netstack2),
        (RollbackNetstackVersion::Netstack3, metrics_registry::StackMigrationCurrentBootMetricDimensionNetstackVersion::Netstack3),
    ],
    [
        (None, metrics_registry::StackMigrationUserSettingMetricDimensionNetstackVersion::NoSelection),
        (Some(NetstackVersion::Netstack2), metrics_registry::StackMigrationUserSettingMetricDimensionNetstackVersion::Netstack2),
        (Some(NetstackVersion::Netstack3), metrics_registry::StackMigrationUserSettingMetricDimensionNetstackVersion::Netstack3),
    ],
    [
        (None, metrics_registry::StackMigrationAutomatedSettingMetricDimensionNetstackVersion::NoSelection),
        (Some(NetstackVersion::Netstack2), metrics_registry::StackMigrationAutomatedSettingMetricDimensionNetstackVersion::Netstack2),
        (Some(NetstackVersion::Netstack3), metrics_registry::StackMigrationAutomatedSettingMetricDimensionNetstackVersion::Netstack3),
    ]
    )]
    #[fuchsia::test]
    async fn metrics_logger(
        current_boot: (
            RollbackNetstackVersion,
            metrics_registry::StackMigrationCurrentBootMetricDimensionNetstackVersion,
        ),
        user: (
            Option<NetstackVersion>,
            metrics_registry::StackMigrationUserSettingMetricDimensionNetstackVersion,
        ),
        automated: (
            Option<NetstackVersion>,
            metrics_registry::StackMigrationAutomatedSettingMetricDimensionNetstackVersion,
        ),
    ) {
        let (current_boot, current_boot_expect) = current_boot;
        let (user, user_expect) = user;
        let (automated, automated_expect) = automated;
        let mut m = Migration::new(
            InMemory::with_persisted(Persisted {
                user,
                automated,
                rollback: None,
                rollback_history: None,
            }),
            NoCollaborativeReboot,
            NoCrashReports,
        );
        m.current_boot = current_boot;
        let (logger, mut logger_stream) =
            fidl::endpoints::create_proxy_and_stream::<fmetrics::MetricEventLoggerMarker>();

        let metrics_logger = MetricsLogger { logger: Some(logger) };

        let ((), ()) = futures::future::join(metrics_logger.log_metrics(&m), async {
            let expect = [
                (
                    metrics_registry::STACK_MIGRATION_CURRENT_BOOT_METRIC_ID,
                    Some(current_boot_expect.as_event_code()),
                ),
                (
                    metrics_registry::STACK_MIGRATION_USER_SETTING_METRIC_ID,
                    Some(user_expect.as_event_code()),
                ),
                (
                    metrics_registry::STACK_MIGRATION_AUTOMATED_SETTING_METRIC_ID,
                    Some(automated_expect.as_event_code()),
                ),
                (
                    metrics_registry::STACK_MIGRATION_STATE_METRIC_ID,
                    // Note: The rollback state doesn't have a flat expectation.
                    // Don't assert on its value here, and instead we directly
                    // test it in a separate test case.
                    None,
                ),
            ];
            for (id, ev) in expect {
                let (metric, occurences, codes, responder) = logger_stream
                    .next()
                    .await
                    .unwrap()
                    .unwrap()
                    .into_log_occurrence()
                    .expect("bad request");
                assert_eq!(metric, id);
                assert_eq!(occurences, 1);
                if let Some(ev) = ev {
                    assert_eq!(codes, vec![ev]);
                }
                responder.send(Ok(())).unwrap();
            }
        })
        .await;
    }

    #[test_case(
        RollbackNetstackVersion::Netstack2, None, None =>
        metrics_registry::StackMigrationStateMetricDimensionMigrationState::NotStarted;
        "not_started_none"
    )]
    #[test_case(
        RollbackNetstackVersion::Netstack2, Some(NetstackVersion::Netstack2), None =>
        metrics_registry::StackMigrationStateMetricDimensionMigrationState::NotStarted;
        "not_started_ns2"
    )]
    #[test_case(
        RollbackNetstackVersion::Netstack2, Some(NetstackVersion::Netstack3), None =>
        metrics_registry::StackMigrationStateMetricDimensionMigrationState::Scheduled;
        "scheduled"
    )]
    #[test_case(
        RollbackNetstackVersion::Netstack3, Some(NetstackVersion::Netstack3), None =>
        metrics_registry::StackMigrationStateMetricDimensionMigrationState::InProgress;
        "in_progress_none"
    )]
    #[test_case(
        RollbackNetstackVersion::Netstack3, Some(NetstackVersion::Netstack3),
        Some(rollback::Persisted::HealthcheckFailures(rollback::MAX_FAILED_HEALTHCHECKS - 1)) =>
        metrics_registry::StackMigrationStateMetricDimensionMigrationState::InProgress;
        "in_progress_some"
    )]
    #[test_case(
        RollbackNetstackVersion::Netstack3, Some(NetstackVersion::Netstack3),
        Some(rollback::Persisted::HealthcheckFailures(rollback::MAX_FAILED_HEALTHCHECKS)) =>
        metrics_registry::StackMigrationStateMetricDimensionMigrationState::Failed;
        "failed_exact"
    )]
    #[test_case(
        RollbackNetstackVersion::Netstack3, Some(NetstackVersion::Netstack3),
        Some(rollback::Persisted::HealthcheckFailures(rollback::MAX_FAILED_HEALTHCHECKS + 1)) =>
        metrics_registry::StackMigrationStateMetricDimensionMigrationState::Failed;
        "failed_more"
    )]
    #[test_case(
        RollbackNetstackVersion::Netstack3, Some(NetstackVersion::Netstack3),
        Some(rollback::Persisted::Success) =>
        metrics_registry::StackMigrationStateMetricDimensionMigrationState::Success;
        "success"
    )]
    #[test_case(
        RollbackNetstackVersion::Netstack3, None, None =>
        metrics_registry::StackMigrationStateMetricDimensionMigrationState::Canceled;
        "canceled_none"
    )]
    #[test_case(
        RollbackNetstackVersion::Netstack3, Some(NetstackVersion::Netstack2), None =>
        metrics_registry::StackMigrationStateMetricDimensionMigrationState::Canceled;
        "canceled_ns2"
    )]
    #[test_case(
        RollbackNetstackVersion::ForceNetstack2, Some(NetstackVersion::Netstack3), None =>
        metrics_registry::StackMigrationStateMetricDimensionMigrationState::RolledBack;
        "rolled_back"
    )]
    #[test_case(
        RollbackNetstackVersion::ForceNetstack2, Some(NetstackVersion::Netstack2), None =>
        metrics_registry::StackMigrationStateMetricDimensionMigrationState::NotStarted;
        "rolled_back_becomes_not_started"
    )]
    #[test_case(
        RollbackNetstackVersion::ForceNetstack2Indefinitely, None, None =>
        metrics_registry::StackMigrationStateMetricDimensionMigrationState::Abandoned;
        "abandoned"
    )]
    #[fuchsia::test]
    fn test_state_metric(
        current_boot: RollbackNetstackVersion,
        automated: Option<NetstackVersion>,
        rollback: Option<rollback::Persisted>,
    ) -> metrics_registry::StackMigrationStateMetricDimensionMigrationState {
        let mut migration = Migration::new(
            InMemory::with_persisted(Persisted {
                user: None,
                automated,
                rollback,
                rollback_history: None,
            }),
            NoCollaborativeReboot,
            NoCrashReports,
        );
        migration.current_boot = current_boot;
        compute_state_metric(&migration)
    }

    /// An in-memory mock-persistence that triggers an event once the target
    /// state has been persisted.
    #[derive(Clone)]
    struct AwaitPersisted {
        file: Rc<RefCell<Option<Vec<u8>>>>,
        target: Vec<u8>,
        event: Event,
    }

    impl AwaitPersisted {
        fn with_persisted(start: Persisted, target: &Persisted) -> (Self, EventWait) {
            let event = Event::new();
            let wait = event.wait();
            let target_bytes = serde_json::to_vec(target).expect("failed to serialize target");
            let mut s = Self { file: Default::default(), target: target_bytes, event };
            start.save(s.open_writer().unwrap());
            (s, wait)
        }
    }

    impl PersistenceProvider for AwaitPersisted {
        type Writer = Self;
        type Reader = std::io::Cursor<Vec<u8>>;

        fn open_writer(&mut self) -> std::io::Result<Self::Writer> {
            *self.file.borrow_mut() = Some(Vec::new());
            Ok(self.clone())
        }

        fn open_reader(&self) -> std::io::Result<Self::Reader> {
            self.file
                .borrow()
                .clone()
                .map(std::io::Cursor::new)
                .ok_or_else(|| std::io::ErrorKind::NotFound.into())
        }
    }

    impl std::io::Write for AwaitPersisted {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            let r = self.file.borrow_mut().as_mut().expect("no file open").write(buf);
            if self.file.borrow().as_ref().expect("no_file_open") == &self.target {
                let _: bool = self.event.signal();
            }
            r
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[fuchsia::test]
    async fn migrate_to_ns3_success() {
        let start = Persisted {
            user: None,
            automated: Some(NetstackVersion::Netstack3),
            rollback: None,
            rollback_history: None,
        };
        let target = Persisted {
            user: None,
            automated: Some(NetstackVersion::Netstack3),
            rollback: Some(rollback::Persisted::Success),
            rollback_history: Some(RollbackHistory { epoch: CURRENT_EPOCH, count: 0 }),
        };

        let (persistence, mut wait) = AwaitPersisted::with_persisted(start, &target);
        let mut migration = Migration::new(persistence, NoCollaborativeReboot, NoCrashReports);
        // No service requests.
        let service_request_stream = futures::stream::pending();
        // A health check that always succeeds.
        let mock_healthcheck = rollback::testutil::MockHttpRequester(|| {
            Ok(fnet_http::Response { error: None, status_code: Some(204), ..Default::default() })
        });
        let healthcheck_tick = futures::stream::once(futures::future::ready(()));

        {
            let main_fut = main_inner(
                &mut migration,
                service_request_stream,
                mock_healthcheck,
                healthcheck_tick,
            )
            .fuse();
            futures::pin_mut!(main_fut);
            futures::select!(
                () = main_fut => unreachable!("main fut should never exit"),
                () = wait => {}
            );
        }

        assert_eq!(migration.persisted.rollback, Some(rollback::Persisted::Success));
    }

    #[test_case[0; "first_failure"]]
    #[test_case[MAX_ROLLBACKS_PER_EPOCH-1; "last_failure"]]
    #[fuchsia::test]
    async fn migrate_to_ns3_fails(rollbacks_in_history: u8) {
        let start = Persisted {
            user: None,
            automated: Some(NetstackVersion::Netstack3),
            rollback: None,
            rollback_history: Some(RollbackHistory {
                epoch: CURRENT_EPOCH,
                count: rollbacks_in_history,
            }),
        };
        let target = Persisted {
            user: None,
            automated: Some(NetstackVersion::Netstack3),
            rollback: Some(rollback::Persisted::HealthcheckFailures(
                rollback::MAX_FAILED_HEALTHCHECKS,
            )),
            rollback_history: Some(RollbackHistory {
                epoch: CURRENT_EPOCH,
                count: rollbacks_in_history,
            }),
        };

        let (persistence, mut wait) = AwaitPersisted::with_persisted(start, &target);
        let mut migration = Migration::new(
            persistence,
            FakeCollaborativeReboot::default(),
            FakeCrashReporter::default(),
        );
        // No service requests.
        let service_request_stream = futures::stream::pending();
        // A health check that always fails.
        let mock_healthcheck = rollback::testutil::MockHttpRequester(|| {
            Ok(fnet_http::Response { error: None, status_code: Some(500), ..Default::default() })
        });
        // Use a non-zero interval so that the Healthcheck code doesn't hog the scheduler.
        let healthcheck_tick = fuchsia_async::Interval::new(Duration::from_millis(1).into());

        {
            let main_fut = main_inner(
                &mut migration,
                service_request_stream,
                mock_healthcheck,
                healthcheck_tick,
            )
            .fuse();
            futures::pin_mut!(main_fut);
            futures::select!(
                () = main_fut => unreachable!("main fut should never exit"),
                () = wait => {}
            );
        }

        let failure_count = assert_matches!(
            migration.persisted.rollback,
            Some(rollback::Persisted::HealthcheckFailures(f)) => f
        );
        assert!(
            failure_count >= rollback::MAX_FAILED_HEALTHCHECKS,
            "failure_count={failure_count}"
        );

        // Verify a failed migration schedules a collaborative reboot.
        let cr_req = &migration.collaborative_reboot.scheduler.req;
        assert_eq!(Ok(false), cr_req.as_ref().expect("there should be a request").is_closed());

        // Verify crash reports were filed for each failed healthcheck above
        // the threshold.
        let expected_num_crash_reports =
            failure_count - HEALTHCHECK_FAILURE_THRESHOLD_FOR_CRASH_REPORTS + 1;
        assert_eq!(
            migration.crash_reporter.reports,
            vec![CrashReportReason::FailedHealthcheck; expected_num_crash_reports]
        )
    }

    // Regression test for https://fxbug.dev/395913604.
    //
    // The original bug would reset the number of failed healthchecks in
    // persistence from `rollback::MAX_FAILED_HEALTHCHECKS` to 0, if an inbound
    // service request was received.
    //
    // Verify this is no longer the case by triggering a failed healthcheck,
    // pushing the total to `rollback::MAX_FAILED_HEALTHCHECKS`, then sending
    // a `fuchsia.net.migration/State.GetNetstackVersion` request.
    #[fuchsia::test]
    async fn migrate_to_ns3_rollback_regression_test() {
        let start = Persisted {
            user: None,
            automated: Some(NetstackVersion::Netstack3),
            rollback: Some(rollback::Persisted::HealthcheckFailures(
                rollback::MAX_FAILED_HEALTHCHECKS - 1,
            )),
            rollback_history: None,
        };
        let target = Persisted {
            user: None,
            automated: Some(NetstackVersion::Netstack3),
            rollback: Some(rollback::Persisted::HealthcheckFailures(0)),
            rollback_history: Some(RollbackHistory { epoch: CURRENT_EPOCH, count: 0 }),
        };

        let (persistence, wait) = AwaitPersisted::with_persisted(start, &target);
        let mut migration = Migration::new(
            persistence,
            FakeCollaborativeReboot::default(),
            FakeCrashReporter::default(),
        );
        // A health check that always fails.
        let mock_healthcheck = rollback::testutil::MockHttpRequester(|| {
            Ok(fnet_http::Response { error: None, status_code: Some(500), ..Default::default() })
        });
        let healthcheck_tick = futures::stream::once(futures::future::ready(()));
        // Send "get" requests, to trigger the bug.
        let (client, server) =
            fidl::endpoints::create_proxy_and_stream::<fnet_migration::StateMarker>();
        let service_request_stream = server.map(|r| r.map(ServiceRequest::State));
        let client_fut = async move {
            // Send multiple get requests, to ensure that at least one would occur after the failed
            // healthcheck.
            let mut stream = fuchsia_async::Interval::new(Duration::from_millis(1).into());
            while let Some(()) = stream.next().await {
                let _ =
                    client.get_netstack_version().await.expect("failed to get netstack version");
            }
        }
        .fuse();

        // If wait were to fire, the bug has occurred. Instead expect a timeout.
        // Use 1 second to keep the test runtime short; If CQ has a hiccup and
        // pauses execution, we'd see a false negative, which isn't a big deal.
        let wait_fut = wait
            .map(|()| panic!("unexpectedly observed the persisted healthcheck failures reset to 0"))
            .on_timeout(Duration::from_secs(1), || ())
            .fuse();

        {
            let main_fut = main_inner(
                &mut migration,
                service_request_stream,
                mock_healthcheck,
                healthcheck_tick,
            )
            .fuse();
            futures::pin_mut!(main_fut);
            futures::pin_mut!(client_fut);
            futures::pin_mut!(wait_fut);
            futures::select!(
                () = main_fut => unreachable!("main fut should never exit"),
                () = client_fut => unreachable!("client fut should never exit"),
                () = wait_fut => {}
            );
        }
    }

    #[fuchsia::test]
    async fn startup_after_first_rollback() {
        let start = Persisted {
            user: None,
            automated: Some(NetstackVersion::Netstack3),
            rollback: Some(rollback::Persisted::HealthcheckFailures(
                rollback::MAX_FAILED_HEALTHCHECKS,
            )),
            rollback_history: None,
        };
        let target = Persisted {
            user: None,
            automated: Some(NetstackVersion::Netstack3),
            rollback: Some(rollback::Persisted::HealthcheckFailures(0)),
            rollback_history: Some(RollbackHistory { epoch: CURRENT_EPOCH, count: 1 }),
        };

        let (persistence, persistence_wait) = AwaitPersisted::with_persisted(start, &target);
        let (crash_reporter, crash_reporter_wait) =
            AwaitCrashReports::new(vec![CrashReportReason::RolledBack]);
        let mut migration =
            Migration::new(persistence, FakeCollaborativeReboot::default(), crash_reporter);
        // No service requests.
        let service_request_stream = futures::stream::pending();
        // No Healthchecks.
        let healthcheck_tick = futures::stream::pending();
        let mock_healthcheck = rollback::testutil::MockHttpRequester(|| {
            panic!("This test isn't exercising healthchecks");
        });
        {
            let main_fut = main_inner(
                &mut migration,
                service_request_stream,
                mock_healthcheck,
                healthcheck_tick,
            )
            .fuse();
            futures::pin_mut!(main_fut);
            let mut wait = futures::future::join(persistence_wait, crash_reporter_wait);
            futures::select!(
                () = main_fut => unreachable!("main fut should never exit"),
                ((), ()) = wait => {}
            );
        }

        // The number of Healthcheck Failures should have been reset in
        // preparation for the next migration attempt.
        assert_eq!(migration.persisted.rollback, Some(rollback::Persisted::HealthcheckFailures(0)));
        // The rollback history should have been updated to reflect this
        // failure.
        assert_eq!(
            migration.persisted.rollback_history,
            Some(RollbackHistory { epoch: CURRENT_EPOCH, count: 1 })
        );
        // A Collaborative Reboot request should be scheduled, so that we can
        // attempt the migration again.
        let cr_req = &migration.collaborative_reboot.scheduler.req;
        assert_eq!(Ok(false), cr_req.as_ref().expect("there should be a request").is_closed());
        // The current_boot should reflect that we've rolled back to NS2
        // temporarily.
        assert_eq!(migration.current_boot, RollbackNetstackVersion::ForceNetstack2);
        // A rollback crash report should have been filed.
        assert_eq!(migration.crash_reporter.reports, vec![CrashReportReason::RolledBack],);
    }

    #[fuchsia::test]
    async fn startup_after_final_rollback() {
        let start = Persisted {
            user: None,
            automated: Some(NetstackVersion::Netstack3),
            rollback: Some(rollback::Persisted::HealthcheckFailures(
                rollback::MAX_FAILED_HEALTHCHECKS,
            )),
            rollback_history: Some(RollbackHistory {
                epoch: CURRENT_EPOCH,
                count: MAX_ROLLBACKS_PER_EPOCH - 1,
            }),
        };
        let target = Persisted {
            user: None,
            automated: Some(NetstackVersion::Netstack3),
            rollback: Some(rollback::Persisted::HealthcheckFailures(0)),
            rollback_history: Some(RollbackHistory {
                epoch: CURRENT_EPOCH,
                count: MAX_ROLLBACKS_PER_EPOCH,
            }),
        };

        let (persistence, persistence_wait) = AwaitPersisted::with_persisted(start, &target);
        let (crash_reporter, crash_reporter_wait) =
            AwaitCrashReports::new(vec![CrashReportReason::RolledBack]);
        let mut migration =
            Migration::new(persistence, FakeCollaborativeReboot::default(), crash_reporter);
        // No service requests.
        let service_request_stream = futures::stream::pending();
        // No Healthchecks.
        let healthcheck_tick = futures::stream::pending();
        let mock_healthcheck = rollback::testutil::MockHttpRequester(|| {
            panic!("This test isn't exercising healthchecks");
        });
        {
            let main_fut = main_inner(
                &mut migration,
                service_request_stream,
                mock_healthcheck,
                healthcheck_tick,
            )
            .fuse();
            futures::pin_mut!(main_fut);
            let mut wait = futures::future::join(persistence_wait, crash_reporter_wait);
            futures::select!(
                () = main_fut => unreachable!("main fut should never exit"),
                ((), ()) = wait => {}
            );
        }

        // The number of Healthcheck Failures should have been reset in
        // preparation for the next migration attempt (in a later epoch).
        assert_eq!(migration.persisted.rollback, Some(rollback::Persisted::HealthcheckFailures(0)));
        // The rollback history should have been updated to reflect this
        // failure.
        assert_eq!(
            migration.persisted.rollback_history,
            Some(RollbackHistory { epoch: CURRENT_EPOCH, count: MAX_ROLLBACKS_PER_EPOCH })
        );
        // A collaborative Reboot request should NOT be scheduled. We're not
        // going to re-attempt the migration until the epoch advances.
        assert_eq!(migration.collaborative_reboot.scheduler.req, None);
        // The current_boot should reflect that we've rolled back to NS2
        // indefinitely.
        assert_eq!(migration.current_boot, RollbackNetstackVersion::ForceNetstack2Indefinitely);
        // A rollback crash report should have been filed.
        assert_eq!(migration.crash_reporter.reports, vec![CrashReportReason::RolledBack]);
    }

    // This test simulates the device starting up while we were already
    // in the `ForceNetstack2Indefinitely` state. E.g. the current boot isn't
    // a rollback, rather it's continuing to use Netstack2, as established in
    // a previous boot.
    #[fuchsia::test]
    async fn startup_already_with_max_rollbacks_per_epoch() {
        let persistence = InMemory::with_persisted(Persisted {
            user: None,
            automated: Some(NetstackVersion::Netstack3),
            rollback: Some(rollback::Persisted::HealthcheckFailures(0)),
            rollback_history: Some(RollbackHistory {
                epoch: CURRENT_EPOCH,
                count: MAX_ROLLBACKS_PER_EPOCH,
            }),
        });
        // No crash report should get filed because this boot isn't a rollback.
        let crash_reporter = NoCrashReports;
        let mut migration =
            Migration::new(persistence, FakeCollaborativeReboot::default(), crash_reporter);
        // No service requests.
        let service_request_stream = futures::stream::pending();
        // No Healthchecks.
        let healthcheck_tick = futures::stream::pending();
        let mock_healthcheck = rollback::testutil::MockHttpRequester(|| {
            panic!("This test isn't exercising healthchecks");
        });

        // Drive the main future for a fixed timeout. It will never exit, but
        // we want to give the migration machinery enough time to apply any
        // changes it wants to make (it shouldn't make any).
        main_inner(&mut migration, service_request_stream, mock_healthcheck, healthcheck_tick)
            .map(Err)
            .on_timeout(Duration::from_secs(1), || Ok(()))
            .await
            .expect("main fut should never exit");

        // None of the persisted state should have changed.
        assert_eq!(migration.persisted.rollback, Some(rollback::Persisted::HealthcheckFailures(0)));
        assert_eq!(
            migration.persisted.rollback_history,
            Some(RollbackHistory { epoch: CURRENT_EPOCH, count: MAX_ROLLBACKS_PER_EPOCH })
        );
        // A collaborative Reboot request should NOT be scheduled.
        assert_eq!(migration.collaborative_reboot.scheduler.req, None);
        // The current_boot should reflect that we've rolled back to NS2
        // indefinitely.
        assert_eq!(migration.current_boot, RollbackNetstackVersion::ForceNetstack2Indefinitely);
    }

    #[fuchsia::test]
    async fn startup_with_new_epoch() {
        let start = Persisted {
            user: None,
            automated: Some(NetstackVersion::Netstack3),
            rollback: Some(rollback::Persisted::HealthcheckFailures(0)),
            rollback_history: Some(RollbackHistory {
                epoch: CURRENT_EPOCH - 1,
                count: MAX_ROLLBACKS_PER_EPOCH,
            }),
        };
        let target = Persisted {
            user: None,
            automated: Some(NetstackVersion::Netstack3),
            rollback: Some(rollback::Persisted::HealthcheckFailures(1)),
            rollback_history: Some(RollbackHistory { epoch: CURRENT_EPOCH, count: 0 }),
        };

        let (persistence, mut wait) = AwaitPersisted::with_persisted(start, &target);
        let mut migration =
            Migration::new(persistence, FakeCollaborativeReboot::default(), NoCrashReports);
        // No service requests.
        let service_request_stream = futures::stream::pending();
        // No Healthchecks.
        let healthcheck_tick = futures::stream::pending();
        let mock_healthcheck = rollback::testutil::MockHttpRequester(|| {
            panic!("This test isn't exercising healthchecks");
        });
        {
            let main_fut = main_inner(
                &mut migration,
                service_request_stream,
                mock_healthcheck,
                healthcheck_tick,
            )
            .fuse();
            futures::pin_mut!(main_fut);
            futures::select!(
                () = main_fut => unreachable!("main fut should never exit"),
                () = wait => {}
            );
        }

        // The number of Healthcheck Failures should have been incremented,
        // indicating the rollback mechanism is trying to migrate to NS3.
        assert_eq!(migration.persisted.rollback, Some(rollback::Persisted::HealthcheckFailures(1)));
        // The rollback history should have been updated to reflect the new
        // epoch.
        assert_eq!(
            migration.persisted.rollback_history,
            Some(RollbackHistory { epoch: CURRENT_EPOCH, count: 0 })
        );
        // A collaborative Reboot request should NOT be scheduled. We're already
        // running Nestack3!
        assert_eq!(migration.collaborative_reboot.scheduler.req, None);
        // The current_boot should reflect that we're retrying migrations to NS3.
        assert_eq!(migration.current_boot, RollbackNetstackVersion::Netstack3);
    }

    #[fuchsia::test]
    async fn noop_rollback_state_update() {
        let rollback_state = rollback::Persisted::HealthcheckFailures(
            HEALTHCHECK_FAILURE_THRESHOLD_FOR_CRASH_REPORTS,
        );
        let mut m = Migration::new(
            InMemory::with_persisted(Persisted {
                user: None,
                automated: Some(NetstackVersion::Netstack3),
                rollback: Some(rollback_state),
                rollback_history: None,
            }),
            // Expect not to see a Collaborative Reboot or a Crash Report.
            NoCollaborativeReboot,
            NoCrashReports,
        );
        m.update_rollback_state(rollback_state).await;
    }
}
