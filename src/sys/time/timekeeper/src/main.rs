// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![warn(missing_docs)]

//! `timekeeper` is responsible for external time synchronization in Fuchsia.

mod clock_manager;
mod diagnostics;
mod enums;
mod estimator;
mod power_topology_integration;
mod reachability;
mod rtc;
mod rtc_testing;
mod time_source;
mod time_source_manager;

use alarms;

use crate::clock_manager::ClockManager;
use crate::diagnostics::{
    CobaltDiagnostics, CompositeDiagnostics, Diagnostics, Event, InspectDiagnostics,
};
use crate::enums::{InitialClockState, InitializeRtcOutcome, Role, StartClockSource, Track};
use crate::rtc::{Rtc, RtcCreationError, RtcImpl};
use crate::time_source::{TimeSource, TimeSourceLauncher};
use crate::time_source_manager::TimeSourceManager;
use anyhow::{Context as _, Result};
use chrono::prelude::*;
use fidl_fuchsia_net_reachability as ffnr;
use fidl_fuchsia_time as ftime;
use fidl_fuchsia_time_alarms as fta;
use fidl_fuchsia_time_external as ffte;
use fidl_fuchsia_time_test as fftt;
use fuchsia_async as fasync;
use fuchsia_component::server::ServiceFs;
use fuchsia_inspect::health;
use fuchsia_inspect::health::Reporter;
use fuchsia_runtime::{UtcClock, UtcClockDetails, UtcClockUpdate, UtcDuration, UtcTimeline};
use futures::SinkExt as _;
use futures::channel::mpsc;
use futures::future::{self, FutureExt, OptionFuture};
use futures::stream::StreamExt as _;
use log::{debug, error, info, warn};
use serde::Deserialize;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use time_adjust::Command;
use time_metrics_registry::TimeMetricDimensionExperiment;
use zx::BootTimeline;

type UtcTransform = time_util::Transform<BootTimeline, UtcTimeline>;

/// The type union of FIDL messages served by Timekeeper.
pub enum Rpcs {
    /// Time test protocol commands.
    TimeTest(fftt::RtcRequestStream),

    /// Client request for scheduling alarms.
    Wake(fta::WakeAlarmsRequestStream),

    /// Client request for adjusting the UTC estimate.
    Adjust(ffte::AdjustRequestStream),
}

/// Timekeeper config, populated from build-time generated structured config.
#[derive(Debug)]
pub struct Config {
    source_config: timekeeper_config::Config,
}

/// The policy for how to handle RTC readings that are in the past with respect
/// to the current boot clock.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RtcInitializationPolicy {
    /// No change to existing behavior.
    #[default]
    Default,
    /// Apply the RTC reading even if it is in the past.
    ApplyMaybeStale,
}

const MILLION: u64 = 1_000_000;

impl From<timekeeper_config::Config> for Config {
    fn from(source_config: timekeeper_config::Config) -> Self {
        Config { source_config }
    }
}

impl Config {
    fn get_primary_time_source_url(&self) -> String {
        self.source_config.primary_time_source_url.clone()
    }

    fn get_monitor_time_source_url(&self) -> Option<String> {
        Some(self.source_config.monitor_time_source_url.clone()).filter(|s| !s.is_empty())
    }

    fn get_oscillator_error_std_dev_ppm(&self) -> f64 {
        self.source_config.oscillator_error_std_dev_ppm as f64
    }

    fn get_oscillator_error_variance(&self) -> f64 {
        (self.source_config.oscillator_error_std_dev_ppm as f64 / MILLION as f64).powi(2)
    }

    fn get_max_frequency_error(&self) -> f64 {
        self.source_config.max_frequency_error_ppm as f64 / MILLION as f64
    }

    fn get_rtc_initialization_policy(&self) -> RtcInitializationPolicy {
        serde_json::from_value(self.source_config.rtc_allow_setting_past_utc.clone().into())
            .unwrap_or_else(|err| {
                warn!(
                    "Failed to parse rtc_allow_setting_past_utc value: {}. Error: {}. Using Default.",
                    self.source_config.rtc_allow_setting_past_utc, err
                );
                RtcInitializationPolicy::Default
            })
    }

    fn get_disable_delays(&self) -> bool {
        self.source_config.disable_delays
    }

    fn get_initial_frequency(&self) -> f64 {
        self.source_config.initial_frequency_ppm as f64 / MILLION as f64
    }

    fn get_monitor_uses_pull(&self) -> bool {
        self.source_config.monitor_uses_pull
    }

    fn get_back_off_time_between_pull_samples(&self) -> zx::MonotonicDuration {
        zx::MonotonicDuration::from_seconds(
            self.source_config.back_off_time_between_pull_samples_sec,
        )
    }

    fn get_first_sampling_delay(&self) -> zx::MonotonicDuration {
        zx::MonotonicDuration::from_seconds(self.source_config.first_sampling_delay_sec)
    }

    fn get_primary_uses_pull(&self) -> bool {
        self.source_config.primary_uses_pull
    }

    fn get_utc_start_at_startup(&self) -> bool {
        self.source_config.utc_start_at_startup
    }

    fn get_utc_start_at_startup_when_invalid_rtc(&self) -> bool {
        self.source_config.utc_start_at_startup_when_invalid_rtc
    }

    fn get_early_exit(&self) -> bool {
        self.source_config.early_exit
    }

    fn power_topology_integration_enabled(&self) -> bool {
        self.source_config.power_topology_integration_enabled
    }

    fn serve_test_protocols(&self) -> bool {
        self.source_config.serve_test_protocols
    }

    fn has_rtc(&self) -> bool {
        self.source_config.has_real_time_clock
    }

    fn has_always_on_counter(&self) -> bool {
        self.source_config.has_always_on_counter
    }

    fn serve_fuchsia_time_alarms(&self) -> bool {
        self.source_config.serve_fuchsia_time_alarms
    }

    fn serve_fuchsia_time_external_adjust(&self) -> bool {
        self.source_config.serve_fuchsia_time_external_adjust
    }

    fn max_window_width_past(&self) -> UtcDuration {
        assert!(self.source_config.utc_max_allowed_delta_past_sec >= 0);
        UtcDuration::from_seconds(self.source_config.utc_max_allowed_delta_past_sec)
    }

    fn max_window_width_future(&self) -> UtcDuration {
        assert!(self.source_config.utc_max_allowed_delta_future_sec >= 0);
        UtcDuration::from_seconds(self.source_config.utc_max_allowed_delta_future_sec)
    }

    fn use_connectivity(&self) -> bool {
        self.source_config.use_connectivity
    }

    fn get_min_utc_reference_to_backstop_diff(&self) -> UtcDuration {
        UtcDuration::from_minutes(
            self.source_config.min_utc_reference_to_backstop_diff_minutes as i64,
        )
    }

    fn get_min_acceptable_boot_time_for_utc_update(&self) -> zx::BootInstant {
        zx::BootInstant::ZERO
            + zx::BootDuration::from_minutes(
                self.source_config.min_acceptable_boot_time_for_utc_update_minutes as i64,
            )
    }

    fn get_periodic_rtc_update_interval(&self) -> Option<zx::BootDuration> {
        let update_interval_minutes =
            self.source_config.periodic_rtc_update_interval_minutes as i64;
        // Intentional interval of zero is really an operator error, so make that a "None" value,
        // to avoid catastrophic errors.
        let update_interval = if update_interval_minutes == 0 {
            None
        } else {
            Some(zx::BootDuration::from_minutes(update_interval_minutes))
        };
        debug!("Periodic RTC update inerval set to: {update_interval:?}");
        update_interval
    }
}

/// A definition which time sources to install, along with the URL and child names for each.
struct TimeSourceUrls {
    primary: TimeSourceDetails,
    monitor: Option<TimeSourceDetails>,
}

/// Describes the timesource to be installed.
struct TimeSourceDetails {
    url: String,
    name: String,
}

/// Instantiates a [TimeSource::Push] or [TimeSource::Pull] depending on
/// `use_pull`.
fn new_time_source(use_pull: bool, details: &TimeSourceDetails, is_monitor: bool) -> TimeSource {
    let launcher = TimeSourceLauncher::new(&details.url, &details.name, is_monitor);
    if use_pull {
        info!("time source {} uses pull", details.name);
        TimeSource::Pull(launcher.into())
    } else {
        info!("time source {} uses push", details.name);
        TimeSource::Push(launcher.into())
    }
}

/// The experiment to record on Cobalt events.
const COBALT_EXPERIMENT: TimeMetricDimensionExperiment = TimeMetricDimensionExperiment::None;

/// The information required to maintain UTC for the primary track.
struct PrimaryTrack {
    time_source: TimeSource,
    clock: Arc<UtcClock>,
}

/// The information required to maintain UTC for the monitor track.
struct MonitorTrack {
    time_source: TimeSource,
    clock: Arc<UtcClock>,
}

fn koid_of(c: &UtcClock) -> u64 {
    c.as_handle_ref().koid().expect("infallible").raw_koid()
}

#[fuchsia::main(logging_tags=["time", "timekeeper"])]
async fn main() -> Result<()> {
    fuchsia_trace_provider::trace_provider_create_with_fdio();
    // The root inspect node in the inspect hierarchy.
    let inspector_root = diagnostics::INSPECTOR.root();

    let structured_config_node = inspector_root.create_child("structured_config");
    let structured_config = timekeeper_config::Config::take_from_startup_handle();
    structured_config.record_inspect(&structured_config_node);

    let config: Arc<Config> = Arc::new(structured_config.into());

    // If we don't get this, timekeeper probably didn't even start.
    debug!("starting timekeeper: config: {:?}", config);

    info!("retrieving UTC clock handle");
    let time_maintainer =
        fuchsia_component::client::connect_to_protocol::<ftime::MaintenanceMarker>().unwrap();
    let utc_clock = time_maintainer
        .get_writable_utc_clock()
        .await
        .context("failed to get UTC clock from maintainer")?
        .cast();
    let utc_clock_for_alarms =
        utc_clock.duplicate_handle(zx::Rights::SAME_RIGHTS).expect("failed to duplicate clock");
    debug!("utc_clock handle with koid: {}", koid_of(&utc_clock));

    let time_source_urls = TimeSourceUrls {
        primary: TimeSourceDetails {
            url: config.get_primary_time_source_url(),
            name: Role::Primary.to_string(),
        },
        monitor: config
            .get_monitor_time_source_url()
            .map(|url| TimeSourceDetails { url, name: Role::Monitor.to_string() }),
    };

    info!("constructing time sources");
    let primary_track = PrimaryTrack {
        time_source: new_time_source(
            config.get_primary_uses_pull(),
            &time_source_urls.primary,
            false,
        ),
        clock: Arc::new(utc_clock),
    };
    let monitor_track = time_source_urls.monitor.map(|details| MonitorTrack {
        time_source: new_time_source(config.get_monitor_uses_pull(), &details, true),
        clock: Arc::new(create_monitor_clock(&primary_track.clock)),
    });

    let enable_user_utc_adjustment = config.serve_fuchsia_time_external_adjust();

    info!("initializing diagnostics and serving inspect on servicefs");
    let cobalt_experiment = COBALT_EXPERIMENT;
    let diagnostics = Arc::new(CompositeDiagnostics::new(
        InspectDiagnostics::new(
            inspector_root,
            &primary_track,
            &monitor_track,
            enable_user_utc_adjustment,
        ),
        CobaltDiagnostics::new(cobalt_experiment),
    ));

    let persistence_node = inspector_root.create_child("persistence");
    let persistence_health = Rc::new(RefCell::new(health::Node::new(&persistence_node)));
    persistence_health.borrow_mut().set_ok();

    let persistent_state = Rc::new(RefCell::new(
        time_persistence::State::read_and_update()
            .map_err(|e| {
                persistence_health
                    .borrow_mut()
                    .set_unhealthy(&format!("at startup: {:?}; use #DEBUG logs for details", e))
            })
            .unwrap_or_else(|_| Default::default()),
    ));

    // Capacity is set to >1 since we may have more than one unit of work to insert into the
    // channel before we start.
    let (cmd_send, cmd_rcv) = mpsc::channel(10);

    let cmd_send_clone = cmd_send.clone();
    let serve_test_protocols = config.serve_test_protocols();
    let serve_fuchsia_time_alarms = config.serve_fuchsia_time_alarms();
    let use_connectivity = config.use_connectivity();
    let ps = persistent_state.clone();

    let scope = fasync::Scope::new();

    let hrtimer_proxy = if config.has_always_on_counter() || serve_fuchsia_time_alarms {
        match alarms::connect_to_hrtimer_async().await {
            Ok(proxy) => Some(proxy),
            Err(err) => {
                if serve_fuchsia_time_alarms {
                    return Err(err).context("could not connect to hrtimer for wake alarms");
                }
                warn!(
                    "could not connect to fuchsia.hardware.hrtimer/Device, falling back to non-persistent RTC: {err:?}"
                );
                None
            }
        }
    } else {
        None
    };

    if config.has_always_on_counter() {
        // A read only RTC implementation using an always-on counter.
        let read_only_rtc = rtc::new_read_only_rtc(
            persistent_state.clone(),
            hrtimer_proxy.clone(),
            config.get_rtc_initialization_policy(),
        );
        scope.spawn_local(async move {
            maintain_utc(
                primary_track,
                monitor_track,
                Some(read_only_rtc),
                diagnostics,
                config,
                cmd_send_clone,
                cmd_rcv,
                ps,
            )
            .await;
        });
    } else {
        // A conventional (PC-like) RTC implementation.
        let optional_rtc: Option<RtcImpl> = match RtcImpl::only_device(config.has_rtc()).await {
            Ok(rtc) => {
                debug!("RTC found.");
                Some(rtc)
            }
            Err(err) => {
                match err {
                    RtcCreationError::NoDevices => info!("no RTC devices found."),
                    _ => warn!("failed to connect to RTC: {}", err),
                };
                diagnostics.record(Event::InitializeRtc { outcome: err.into(), time: None });
                None
            }
        };
        scope.spawn_local(async move {
            maintain_utc(
                primary_track,
                monitor_track,
                optional_rtc,
                diagnostics,
                config,
                cmd_send_clone,
                cmd_rcv,
                ps,
            )
            .await;
        });
    }

    let loop_inspect = inspector_root.create_child("wake_alarms");

    let _inspect_server_task = inspect_runtime::publish(
        &diagnostics::INSPECTOR,
        inspect_runtime::PublishOptions::default(),
    );

    let mut fs = ServiceFs::new();
    if serve_test_protocols {
        fs.dir("svc").add_fidl_service(Rpcs::TimeTest);
        info!("serving test protocols: fuchsia.test.time/RTC");
    }

    fs.dir("svc").add_fidl_service(Rpcs::Wake);
    info!("serving protocol: fuchsia.time.alarms/Wake");
    if enable_user_utc_adjustment {
        fs.dir("svc").add_fidl_service(Rpcs::Adjust);
        info!("serving protocol: fuchsia.time.alarms/Adjust");
    }
    fs.take_and_serve_directory_handle()?;

    // Ensures that only one handler of fuchsia.time.test/RPC is active at
    // any one time.
    let time_test_mutex: Rc<RefCell<()>> = Rc::new(RefCell::new(()));

    let timer_loop = if serve_fuchsia_time_alarms {
        // Instantiate connections to wake alarms. Wait for hrtimer device to enumerate before
        // proceeding and exit if it is failed to be found.
        let proxy = hrtimer_proxy
            .expect("hrtimer_proxy must be present if serve_fuchsia_time_alarms is true");
        Rc::new(alarms::Loop::new(scope.to_handle(), proxy, loop_inspect, utc_clock_for_alarms))
    } else {
        // Emulate wake alarms. This is used on platforms that do not have
        // power management, and will *not* actually sleep.
        Rc::new(alarms::Loop::new_emulated(scope.to_handle(), loop_inspect, utc_clock_for_alarms))
    };

    // Look for this text to know whether connections have succeeded.
    info!("now starting to serve exported FIDL protocols");

    // The `MAX_CONCURRENT_HANDLERS` must be one larger than the largest number
    // of timer clients the system may have.
    const MAX_CONCURRENT_HANDLERS: usize = 100;

    let rtc_test_server = Rc::new(rtc_testing::Server::new(persistence_health, persistent_state));

    let adjust_server = Rc::new(if enable_user_utc_adjustment {
        let cmd_send_clone = cmd_send.clone();
        Some(time_adjust::Server::new(cmd_send_clone))
    } else {
        None
    });

    if use_connectivity {
        // Start the connectivity monitor loop. The loop may fail, in which case,
        // we will effectively be disabling reachability and external time sync.
        if let Ok(proxy) = fuchsia_component::client::connect_to_protocol::<ffnr::MonitorMarker>() {
            let cmd = cmd_send.clone();
            let mut monitor = reachability::Monitor::new(cmd);
            fasync::Task::local(async move {
                if let Err(result) = monitor.serve(proxy).await {
                    error!("error on fuchsia.net.reachability/Monitor: {:?}", result);
                }
            })
            .detach();
        } else {
            warn!(
                "no connection to fuchsia.net.reachability/Monitor: sampling time sources is turned off."
            );
        }
    }

    // fuchsia::main can only return () or Result<()>.
    let result = fs
        .for_each_concurrent(MAX_CONCURRENT_HANDLERS, |request: Rpcs| {
            let rtc_test_server = rtc_test_server.clone();
            let time_test_mutex = time_test_mutex.clone();
            let timer_loop = timer_loop.clone();
            let adjust_server = adjust_server.clone();
            let scope = scope.to_handle();
            fuchsia_trace::instant!("timekeeper", "request", fuchsia_trace::Scope::Process);
            async move {
                match request {
                    Rpcs::TimeTest(stream) => {
                        // Accepts only one client for fuchsia.time.test/RPC at a time.
                        // This is because conflicting instructions from different clients
                        // can end up being confusing for the test fixture.
                        if let Ok(_only_one_please) = time_test_mutex.try_borrow_mut() {
                            rtc_test_server
                                .serve(stream)
                                .await
                                .map_err(|e| {
                                    log::error!("while serving fuchsia.time.test/RPC: {:?}", e)
                                })
                                .unwrap_or(());
                        } else {
                            // Your request to connect was rejected.
                            //
                            // Not a bug in this code, but could be a bug in the
                            // test fixture that calls into here.
                            warn!("prevented a second client for fuchsia.time.test/RPC");
                        }
                    }
                    Rpcs::Wake(stream) => {
                        scope.spawn_local(alarms::serve(timer_loop.clone(), stream));
                    },
                    Rpcs::Adjust(stream) => match *adjust_server {
                        Some(ref server) => {
                            let _log_and_discard = server.serve(stream).await.map_err(|e| {
                                error!("error while serving fuchsia.time.external/Adjust: {}", e)
                            });
                        }
                        None => {
                            // We must never have spurious connections to this endpoint.
                            // If we do, then something is off.
                            error!(
                                "IMPORTANT! not serving fuchsia.time.external/Adjust, but got a connection"
                            );
                        }
                    },
                };
            }
        })
        .await;
    Ok(result)
}

/// Creates a new userspace clock for use in the monitor track, set to the same backstop time as
/// the supplied primary clock.
fn create_monitor_clock(primary_clock: &UtcClock) -> UtcClock {
    // Note: Failure should not be possible from a valid UtcClock.
    let backstop = primary_clock.get_details().expect("failed to get UTC clock details").backstop;
    // Note: Only failure mode is an OOM which we handle via panic.
    UtcClock::create(zx::ClockOpts::empty(), Some(backstop))
        .expect("failed to create new monitor clock")
}

/// Determines whether the supplied clock has previously been set.
/// Returns the clock state and the backstop.
fn initial_clock_state(utc_clock: &UtcClock) -> (InitialClockState, UtcClockDetails) {
    // Note: Failure should not be possible from a valid UtcClock.
    let clock_details = utc_clock.get_details().expect("failed to get UTC clock details");
    // When the clock is first initialized to the backstop time, its synthetic offset should
    // be identical. Once the clock is updated, this is no longer true.
    if clock_details.backstop == clock_details.ticks_to_synthetic.synthetic_offset {
        (InitialClockState::NotSet, clock_details)
    } else {
        (InitialClockState::PreviouslySet, clock_details)
    }
}

/// Attempts to initialize a userspace clock from the current value of the real time clock.
/// sending progress to diagnostics as appropriate.
///
/// Args:
/// - `force_start`: if set, the clock will be force-started, even though RTC
///   is present.  However, if the RTC reading shows a timestamp before backstop,
///   we will be snapped to backstop before setting the UTC clock. This way,
///   RTC setting remains broken, but UTC does not get set to time before
///   backstop.
async fn set_clock_from_rtc<R: Rtc, D: Diagnostics>(
    rtc: &R,
    clock: &UtcClock,
    diagnostics: Arc<D>,
    force_start: bool,
) {
    info!("reading initial RTC time.");
    let mono_before = zx::BootInstant::get();
    let mut rtc_time = match rtc.get().await {
        Err(err) => {
            error!("failed to read RTC time: {}", err);
            diagnostics.record(Event::InitializeRtc {
                outcome: InitializeRtcOutcome::ReadFailed,
                time: None,
            });
            return;
        }
        Ok(time) => time,
    };
    let mono_after = zx::BootInstant::get();
    let mono_time = mono_before + (mono_after - mono_before) / 2;

    let mut rtc_chrono = Utc.timestamp_nanos(rtc_time.into_nanos());
    let backstop = clock.get_details().expect("failed to get UTC clock details").backstop;
    let backstop_chrono = Utc.timestamp_nanos(backstop.into_nanos());
    if rtc_time < backstop {
        warn!("initial RTC time {} is before backstop: {}", rtc_chrono, backstop_chrono);
        diagnostics.record(Event::InitializeRtc {
            outcome: InitializeRtcOutcome::InvalidBeforeBackstop,
            time: Some(rtc_time),
        });
        if force_start {
            // If we must start the clock from RTC when we know RTC is before
            // backstop, we snap the RTC reading to backstop before starting
            // the clock. We leave the RTC reading update for later.
            info!("Snapping RTC reading (but not RTC content) to backstop: {}", backstop_chrono);
            rtc_time = backstop;
            rtc_chrono = backstop_chrono;
        } else {
            return;
        }
    } else {
        debug!("RTC time {} is ahead of backstop {}, as expected", rtc_chrono, backstop_chrono);
    }

    diagnostics.record(Event::InitializeRtc {
        outcome: InitializeRtcOutcome::Succeeded,
        time: Some(rtc_time),
    });
    if let Err(status) = clock.update(UtcClockUpdate::builder().absolute_value(mono_time, rtc_time))
    {
        error!("failed to start UTC clock from RTC at time {}: {}", rtc_chrono, status);
    } else {
        diagnostics
            .record(Event::StartClock { track: Track::Primary, source: StartClockSource::Rtc });
        info!("started UTC clock from RTC at time: {}", rtc_chrono);

        // Refresh the UTC immediately, to ensure that next reboot always
        // gets a reference from this boot. Otherwise, confusion around
        // reference will exist.
        if let Ok(utc_instant) =
            clock.read().inspect_err(|err| log::error!("could not read UTC: {err:?}"))
        {
            let _ = rtc
                .set(utc_instant)
                .await
                .map_err(|err| log::error!("could not reconfirm UTC: {err:?}"));
        }

        if let Err(status) = clock.signal(
            zx::Signals::NONE,
            zx::Signals::from_bits(ftime::SIGNAL_UTC_CLOCK_LOGGING_QUALITY).unwrap(),
        ) {
            // Since userspace depends on this signal, we probably can not recover if
            // we can not signal.
            panic!("Failed to signal clock logging quality: {}", status);
        } else {
            debug!("sent SIGNAL_UTC_CLOCK_LOGGING_QUALITY");
        }
    }
}

/// The top-level control loop for time synchronization.
///
/// Maintains the utc clock using updates received over the `fuchsia.time.external` protocols.
async fn maintain_utc<R: Rtc, D: 'static>(
    primary: PrimaryTrack,
    optional_monitor: Option<MonitorTrack>,
    optional_rtc: Option<R>,
    diagnostics: Arc<D>,
    config: Arc<Config>,
    cmd_send: mpsc::Sender<Command>,
    cmd_recv: mpsc::Receiver<Command>,
    persistent_state: Rc<RefCell<time_persistence::State>>,
) where
    D: Diagnostics,
{
    info!("record the state at initialization.");
    let (initial_clock_state, clock_details) = initial_clock_state(&primary.clock);
    diagnostics.record(Event::Initialized { clock_state: initial_clock_state });

    if let Some(rtc) = optional_rtc.as_ref() {
        match initial_clock_state {
            InitialClockState::NotSet => {
                set_clock_from_rtc(
                    rtc,
                    &primary.clock,
                    Arc::clone(&diagnostics),
                    config.get_utc_start_at_startup_when_invalid_rtc(),
                )
                .await;
            }
            InitialClockState::PreviouslySet => {
                diagnostics.record(Event::InitializeRtc {
                    outcome: InitializeRtcOutcome::ReadNotAttempted,
                    time: None,
                });
            }
        }
    }
    info!("launching time source managers...");
    let time_source_fn = match config.get_disable_delays() {
        true => TimeSourceManager::new_with_delays_disabled,
        false => TimeSourceManager::new,
    };

    debug!("checking whether to start UTC from RTC or not");
    if optional_rtc.is_none() && config.get_utc_start_at_startup() {
        // Legacy programs assume that UTC clock is always running.  If config allows it,
        // we start the clock from backstop and hope for the best.
        let backstop = &clock_details.backstop;
        // Not possible to start at backstop, so we start just a bit after.
        let b1 = *backstop + UtcDuration::from_nanos(1);
        let mono = zx::BootInstant::get();
        info!("starting the UTC clock from backstop time, to handle legacy programs");
        debug!("`- synthetic (backstop+1): {:?}, reference (monotonic): {:?}", b1, mono);
        if let Err(status) =
            primary.clock.update(UtcClockUpdate::builder().absolute_value(mono, b1))
        {
            warn!("failed to start UTC clock from backstop time: {}", status);
            // If we got here, the UTC clock is not started yet. We might have better luck with
            // time sources, provided that we have network access.
        } else {
            // Yay, the clock is started!  Announce to the world.
            diagnostics.record(Event::InitializeRtc {
                outcome: InitializeRtcOutcome::StartedFromBackstop,
                time: Some(b1),
            });
        }
    }
    if config.get_early_exit() {
        log::info!(
            "early_exit=true: exiting early per request from configuration. UTC clock will not be managed"
        );
        return;
    }
    let primary_source_manager = time_source_fn(
        clock_details.backstop,
        Role::Primary,
        primary.time_source,
        Arc::clone(&diagnostics),
    );
    let monitor_source_manager_and_clock = optional_monitor.map(|monitor| {
        let source_manager = time_source_fn(
            clock_details.backstop,
            Role::Monitor,
            monitor.time_source,
            Arc::clone(&diagnostics),
        );
        (source_manager, monitor.clock)
    });

    info!("launching clock managers...");
    let fut1 = ClockManager::execute(
        primary.clock,
        primary_source_manager,
        optional_rtc,
        Arc::clone(&diagnostics),
        Track::Primary,
        Arc::clone(&config),
        cmd_recv,
        persistent_state,
    );
    let (_, r2) = mpsc::channel(1);
    let fut2_cfg_clone = config.clone();
    let fut2: OptionFuture<_> = monitor_source_manager_and_clock
        .map(|(source_manager, clock)| {
            ClockManager::<R, D>::execute(
                clock,
                source_manager,
                None,
                diagnostics,
                Track::Monitor,
                fut2_cfg_clone,
                r2,
                Rc::new(RefCell::new(time_persistence::State::new(true))),
            )
        })
        .into();

    let pte = config.power_topology_integration_enabled();
    let cmd_send_oneshot = cmd_send.clone();
    let oneshot = Box::pin(async move {
        info!("power_topology_integration_enabled: {}", pte);
        if pte {
            power_topology_integration::manage(cmd_send_oneshot)
                .await
                .context("(timekeeper will ignore this error and just turn the integration off)")
                .map_err(|e| error!("power management integration: {:#}", e))
                .unwrap_or_else(|_| fasync::Task::local(async {}))
                .await;
        }
    });

    let rtc_updater = match config.get_periodic_rtc_update_interval() {
        Some(interval) => periodic_rtc_update(cmd_send.clone(), interval).boxed(),
        None => future::ready(()).boxed(),
    }
    .boxed();

    future::join4(fut1, fut2, oneshot, rtc_updater).await;
}

async fn periodic_rtc_update(mut cmd_send: mpsc::Sender<Command>, interval: zx::BootDuration) {
    info!("periodic_rtc_update: starting periodic RTC updates, update interval: {interval:?}");
    loop {
        fasync::Timer::new(fasync::BootInstant::after(interval)).await;
        debug!("periodic_rtc_update: sending command");
        if let Err(e) = cmd_send.send(Command::UpdateRtc).await {
            // Reported as "error" since if this does not work, it generates
            // bug reports.
            error!("failed to send UpdateRtc command: {e:?}");
            break;
        }
    }
    warn!("periodic_rtc_update: exiting. This should not happen in production.");
}

// Reexport test config creation to be used in other tests.
#[cfg(test)]
use tests::{
    clone_system_time, make_test_config, make_test_config_with_delay, make_test_config_with_fn,
    run_in_fake_time,
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostics::FakeDiagnostics;
    use crate::enums::WriteRtcOutcome;
    use crate::rtc::FakeRtc;
    use crate::time_source::{Event as TimeSourceEvent, FakePushTimeSource, Sample};
    use fidl_fuchsia_time_external as ftexternal;
    use fuchsia_runtime::{UtcClockUpdate, UtcInstant};
    use futures::Future;
    use std::matches;
    use std::pin::pin;
    use std::sync::LazyLock;
    use std::task::Poll;
    use test_case::test_case;
    use test_util::assert_leq;

    const NANOS_PER_SECOND: i64 = 1_000_000_000;
    const OFFSET: zx::BootDuration = zx::BootDuration::from_seconds(1111_000);
    const OFFSET_2: zx::BootDuration = zx::BootDuration::from_seconds(1111_333);
    const STD_DEV: zx::BootDuration = zx::BootDuration::from_millis(44);
    const INVALID_RTC_TIME: UtcInstant = UtcInstant::from_nanos(111111 * NANOS_PER_SECOND);
    const BACKSTOP_TIME: UtcInstant = UtcInstant::from_nanos(222222 * NANOS_PER_SECOND);
    const VALID_RTC_TIME: UtcInstant = UtcInstant::from_nanos(333333 * NANOS_PER_SECOND);

    static CLOCK_OPTS: LazyLock<zx::ClockOpts> = LazyLock::new(zx::ClockOpts::empty);

    // Sets up the given `executor` to the current system time.
    //
    // Most importantly, handles possible discrepancy between the boot and monotonic
    // timelines, which can make a difference in the case of timekeeping tests.
    //
    // # Returns
    // - The applied monotonic and boot instants as a tuple, in that order.
    pub fn clone_system_time(
        executor: &mut fasync::TestExecutor,
    ) -> (zx::MonotonicInstant, zx::BootInstant) {
        let real_now = zx::MonotonicInstant::get();
        let real_boot_now = zx::BootInstant::get();
        let offset_nanos = real_boot_now.into_nanos() - real_now.into_nanos();

        // Offset is nonnegative by definition, since the boot time reading is at least
        // the monotonic time reading.
        assert!(offset_nanos >= 0, "offset can not be negative: {:?}", offset_nanos);

        executor.set_fake_time(real_now.into());
        let offset = zx::BootDuration::from_nanos(offset_nanos);
        executor.set_fake_boot_to_mono_offset(offset);
        (real_now, real_boot_now)
    }

    // Run the future `main_fut` in fake time.  The fake time is being advanced
    // in relatively small increments until a specified `total_duration` has
    // elapsed.
    //
    // This complication is needed to ensure that any expired
    // timers are awoken in the correct sequence because the test executor does
    // not automatically wake the timers. For the fake time execution to
    // be comparable to a real time execution, we need each timer to have the
    // chance of waking up, so that we can properly process the consequences
    // of that timer firing.
    //
    // We require that `main_fut` has completed at `total_duration`, and panic
    // if it has not.  This ensures that we never block forever in fake time.
    //
    // This method could possibly be implemented in [TestExecutor] for those
    // test executor users who do not care to wake the timers in any special
    // way.
    pub fn run_in_fake_time<F>(
        executor: &mut fasync::TestExecutor,
        main_fut: &mut F,
        total_duration: fasync::MonotonicDuration,
    ) -> Poll<()>
    where
        F: Future<Output = ()> + Unpin,
    {
        const INCREMENT: zx::MonotonicDuration = zx::MonotonicDuration::from_millis(1);
        // Run the loop for a bit longer than the fake time needed to pump all
        // the events, to allow the event queue to drain.
        let mut current = zx::MonotonicDuration::from_millis(0);
        let mut poll_status = Poll::Pending;

        // We run until either the future completes or the timeout is reached,
        // whichever comes first.
        // Running the future after it returns Poll::Ready is not allowed, so
        // we must exit the loop then.
        while current < total_duration && poll_status == Poll::Pending {
            let fake_time = executor.now() + INCREMENT;
            executor.set_fake_time(fake_time.into());
            executor.wake_expired_timers();
            poll_status = executor.run_until_stalled(main_fut);
            current = current + INCREMENT;
        }
        poll_status
    }

    fn new_state_for_test(value: bool) -> Rc<RefCell<time_persistence::State>> {
        Rc::new(RefCell::new(time_persistence::State::new(value)))
    }

    /// Creates and starts a new clock with default options, returning a tuple of the clock and its
    /// initial update time in ticks.
    fn create_clock() -> (Arc<UtcClock>, zx::BootTicks) {
        let clock = UtcClock::create(*CLOCK_OPTS, Some(BACKSTOP_TIME)).unwrap();
        clock.update(UtcClockUpdate::builder().approximate_value(BACKSTOP_TIME)).unwrap();
        let initial_update_ticks = clock.get_details().unwrap().last_value_update_ticks;
        (Arc::new(clock), initial_update_ticks)
    }

    pub fn make_test_config_with_fn<T>(adjust_fn: T) -> Arc<Config>
    where
        T: FnOnce(timekeeper_config::Config) -> timekeeper_config::Config,
    {
        let config = timekeeper_config::Config {
            disable_delays: true,
            oscillator_error_std_dev_ppm: 15,
            max_frequency_error_ppm: 10,
            primary_time_source_url: "".to_string(),
            initial_frequency_ppm: 1_000_000,
            monitor_uses_pull: false,
            back_off_time_between_pull_samples_sec: 0,
            first_sampling_delay_sec: 0,
            monitor_time_source_url: "".to_string(),
            primary_uses_pull: false,
            utc_start_at_startup: false,
            utc_start_at_startup_when_invalid_rtc: false,
            early_exit: false,
            power_topology_integration_enabled: false,
            has_real_time_clock: true,
            serve_fuchsia_time_alarms: false,
            has_always_on_counter: false,
            serve_fuchsia_time_external_adjust: false,
            utc_max_allowed_delta_future_sec: 0,
            utc_max_allowed_delta_past_sec: 0,
            serve_test_protocols: false,
            use_connectivity: false,
            min_utc_reference_to_backstop_diff_minutes: 0,
            min_acceptable_boot_time_for_utc_update_minutes: 0,
            periodic_rtc_update_interval_minutes: 0,
            rtc_allow_setting_past_utc: "default".to_string(),
        };
        Arc::new(Config::from(adjust_fn(config)))
    }

    pub fn make_test_config_with_params(
        delay: i64,
        serve_test_protocols: bool,
        force_start: bool,
    ) -> Arc<Config> {
        make_test_config_with_fn(|mut config| {
            config.first_sampling_delay_sec = delay;
            config.serve_test_protocols = serve_test_protocols;
            config.utc_start_at_startup = force_start;
            config.utc_start_at_startup_when_invalid_rtc = force_start;
            config
        })
    }

    pub fn make_test_config_with_delay(delay: i64) -> Arc<Config> {
        make_test_config_with_params(
            delay, /*serve_test_protocols=*/ false, /*force_start=*/ false,
        )
    }

    pub fn make_test_config() -> Arc<Config> {
        make_test_config_with_delay(0)
    }

    pub fn make_test_config_with_test_protocols() -> Arc<Config> {
        make_test_config_with_params(
            /*delay=*/ 0, /*serve_test_protocols=*/ true, /*force_start=*/ false,
        )
    }

    pub fn make_test_config_with_force_start() -> Arc<Config> {
        make_test_config_with_params(
            /*delay=*/ 0, /*serve_test_protocols=*/ true, /* force_start=*/ true,
        )
    }

    #[fuchsia::test]
    fn successful_update_with_monitor() {
        let mut executor = fasync::TestExecutor::new();
        let (primary_clock, primary_ticks) = create_clock();
        let (monitor_clock, monitor_ticks) = create_clock();
        let rtc = FakeRtc::valid(INVALID_RTC_TIME);
        let diagnostics = Arc::new(FakeDiagnostics::new());
        let config = make_test_config();

        let boot_ref = zx::BootInstant::get();

        let (s, r) = mpsc::channel(1);
        let b = new_state_for_test(true);

        // Maintain UTC until no more work remains
        let mut fut = pin!(maintain_utc(
            PrimaryTrack {
                clock: Arc::clone(&primary_clock),
                time_source: FakePushTimeSource::events(vec![
                    TimeSourceEvent::StatusChange { status: ftexternal::Status::Ok },
                    TimeSourceEvent::from(Sample::new(
                        UtcInstant::from_nanos((boot_ref + OFFSET).into_nanos()),
                        boot_ref,
                        STD_DEV,
                    )),
                ])
                .into(),
            },
            Some(MonitorTrack {
                clock: Arc::clone(&monitor_clock),
                time_source: FakePushTimeSource::events(vec![
                    TimeSourceEvent::StatusChange { status: ftexternal::Status::Network },
                    TimeSourceEvent::StatusChange { status: ftexternal::Status::Ok },
                    TimeSourceEvent::from(Sample::new(
                        UtcInstant::from_nanos((boot_ref + OFFSET_2).into_nanos()),
                        boot_ref,
                        STD_DEV,
                    )),
                ])
                .into(),
            }),
            Some(rtc.clone()),
            Arc::clone(&diagnostics),
            Arc::clone(&config),
            s,
            r,
            b,
        ));
        let _ = executor.run_until_stalled(&mut fut);

        // Check that the clocks are set.
        assert!(primary_clock.get_details().unwrap().last_value_update_ticks > primary_ticks);
        assert!(monitor_clock.get_details().unwrap().last_value_update_ticks > monitor_ticks);
        assert!(rtc.last_set().is_some());

        // Check that the correct diagnostic events were logged.
        diagnostics.assert_events(&[
            Event::Initialized { clock_state: InitialClockState::NotSet },
            Event::InitializeRtc {
                outcome: InitializeRtcOutcome::InvalidBeforeBackstop,
                time: Some(INVALID_RTC_TIME),
            },
            Event::TimeSourceStatus { role: Role::Primary, status: ftexternal::Status::Ok },
            Event::KalmanFilterUpdated {
                track: Track::Primary,
                reference: boot_ref,
                utc: UtcInstant::from_nanos((boot_ref + OFFSET).into_nanos()),
                sqrt_covariance: STD_DEV,
            },
            Event::StartClock {
                track: Track::Primary,
                source: StartClockSource::External(Role::Primary),
            },
            Event::WriteRtc { outcome: WriteRtcOutcome::Succeeded },
            Event::TimeSourceStatus { role: Role::Monitor, status: ftexternal::Status::Network },
            Event::TimeSourceStatus { role: Role::Monitor, status: ftexternal::Status::Ok },
            Event::KalmanFilterUpdated {
                track: Track::Monitor,
                reference: boot_ref,
                utc: UtcInstant::from_nanos((boot_ref + OFFSET_2).into_nanos()),
                sqrt_covariance: STD_DEV,
            },
            Event::StartClock {
                track: Track::Monitor,
                source: StartClockSource::External(Role::Monitor),
            },
        ]);
    }

    #[test_case(0; "no pause")]
    #[test_case(1; "one second pause")]
    #[fuchsia::test]
    fn successful_update_with_delay(delay: i64) {
        let mut executor = fasync::TestExecutor::new();
        let (primary_clock, primary_ticks) = create_clock();
        let rtc = FakeRtc::valid(INVALID_RTC_TIME);
        let diagnostics = Arc::new(FakeDiagnostics::new());
        let config = make_test_config_with_delay(delay);

        let boot_ref = zx::BootInstant::get();
        let (s, r) = mpsc::channel(1);
        let b = new_state_for_test(true);

        // Maintain UTC until no more work remains
        let mut fut = pin!(maintain_utc(
            PrimaryTrack {
                clock: Arc::clone(&primary_clock),
                time_source: FakePushTimeSource::events(vec![
                    TimeSourceEvent::StatusChange { status: ftexternal::Status::Ok },
                    TimeSourceEvent::from(Sample::new(
                        UtcInstant::from_nanos((boot_ref + OFFSET).into_nanos()),
                        boot_ref,
                        STD_DEV,
                    )),
                ])
                .into(),
            },
            None,
            Some(rtc.clone()),
            Arc::clone(&diagnostics),
            Arc::clone(&config),
            s,
            r,
            b,
        ));

        // This is slightly silly, but allows us to run the clock maintenance
        // in fake time, waking up appropriate delay timers along the way.
        // Tests running in fake time always have similar silliness where
        // timer wakeups are involved.
        let _ = executor.run_until_stalled(&mut fut); // Get to the first delay.

        // This will wake the delay timer *if* one exists. This is a non-obvious
        // feature of run_until_stalled: it does *not* wake timers. So without
        // this the "one second delay" will never get out of the pause and the
        // test will fail, proving that the pause does exist.
        // On the other hand, the fact that both "no pause" and
        // "one second pause" have an identical result means that the delay does
        // not affect the normal operation of the clock manager.
        executor.wake_next_timer();
        let _ = executor.run_until_stalled(&mut fut); // Finish clock update work.

        // Check that the clocks are set.
        let last_value = primary_clock.get_details().unwrap().last_value_update_ticks;
        assert!(
            last_value > primary_ticks,
            "wanted: last_value: {:?} > primary_ticks: {:?}",
            last_value,
            primary_ticks
        );
        assert!(rtc.last_set().is_some());

        // Check that the correct diagnostic events were logged.
        diagnostics.assert_events_prefix(&[
            Event::Initialized { clock_state: InitialClockState::NotSet },
            Event::InitializeRtc {
                outcome: InitializeRtcOutcome::InvalidBeforeBackstop,
                time: Some(INVALID_RTC_TIME),
            },
            Event::TimeSourceStatus { role: Role::Primary, status: ftexternal::Status::Ok },
            Event::KalmanFilterUpdated {
                track: Track::Primary,
                reference: boot_ref,
                utc: UtcInstant::from_nanos((boot_ref + OFFSET).into_nanos()),
                sqrt_covariance: STD_DEV,
            },
            Event::StartClock {
                track: Track::Primary,
                source: StartClockSource::External(Role::Primary),
            },
            Event::WriteRtc { outcome: WriteRtcOutcome::Succeeded },
        ]);
    }

    #[fuchsia::test]
    fn fail_when_no_delays() {
        let mut executor = fasync::TestExecutor::new_with_fake_time();
        // Ensure that we don't hit hard limitations such as backstop but that
        // we still run in fake time.
        clone_system_time(&mut executor);

        let (primary_clock, primary_ticks) = create_clock();
        let rtc = FakeRtc::valid(INVALID_RTC_TIME);
        let diagnostics = Arc::new(FakeDiagnostics::new());
        let config = make_test_config_with_delay(1);

        let boot_ref = executor.boot_now();
        let (s, r) = mpsc::channel(1);
        let b = new_state_for_test(true);

        // Maintain UTC until no more work remains
        let mut fut = pin!(maintain_utc(
            PrimaryTrack {
                clock: Arc::clone(&primary_clock),
                time_source: FakePushTimeSource::events(vec![
                    TimeSourceEvent::StatusChange { status: ftexternal::Status::Ok },
                    TimeSourceEvent::from(Sample::new(
                        UtcInstant::from_nanos((boot_ref + OFFSET).into_nanos()),
                        boot_ref.into(),
                        STD_DEV,
                    )),
                ])
                .into(),
            },
            None,
            Some(rtc),
            Arc::clone(&diagnostics),
            Arc::clone(&config),
            s,
            r,
            b,
        ));

        let start_time = executor.now();
        let _ = executor.run_until_stalled(&mut fut);

        // Half a second in, nothing to wake.
        executor.set_fake_time(start_time + fasync::MonotonicDuration::from_millis(550));
        let _ = executor.run_until_stalled(&mut fut);
        assert_eq!(false, executor.wake_expired_timers());

        // One second in, there is something to wake.
        executor.set_fake_time(start_time + fasync::MonotonicDuration::from_millis(1050));
        assert_eq!(true, executor.wake_expired_timers());
        let _ = executor.run_until_stalled(&mut fut);

        // And our clock was updated, too!
        assert!(primary_clock.get_details().unwrap().last_value_update_ticks > primary_ticks);
    }

    #[fuchsia::test]
    fn no_update_invalid_rtc() {
        let mut executor = fasync::TestExecutor::new_with_fake_time();
        clone_system_time(&mut executor);

        let (clock, initial_update_ticks) = create_clock();
        let rtc = FakeRtc::valid(INVALID_RTC_TIME);
        let diagnostics = Arc::new(FakeDiagnostics::new());
        let config = make_test_config();

        let time_source = FakePushTimeSource::events(vec![TimeSourceEvent::StatusChange {
            status: ftexternal::Status::Network,
        }])
        .into();
        let (s, r) = mpsc::channel(1);
        let b = new_state_for_test(true);

        // Maintain UTC until no more work remains
        let mut fut = pin!(maintain_utc(
            PrimaryTrack { clock: Arc::clone(&clock), time_source },
            None,
            Some(rtc.clone()),
            Arc::clone(&diagnostics),
            Arc::clone(&config),
            s,
            r,
            b,
        ));
        let _ignore = run_in_fake_time(
            &mut executor,
            &mut fut,
            // Only run long enough to drain `time_source` above.
            fasync::MonotonicDuration::from_millis(1),
        );

        // Checking that the clock has not been updated yet
        assert_eq!(initial_update_ticks, clock.get_details().unwrap().last_value_update_ticks);
        assert_eq!(rtc.last_set(), None);

        // Checking that the correct diagnostic events were logged.
        diagnostics.assert_events_prefix(&[
            Event::Initialized { clock_state: InitialClockState::NotSet },
            Event::InitializeRtc {
                outcome: InitializeRtcOutcome::InvalidBeforeBackstop,
                time: Some(INVALID_RTC_TIME),
            },
            Event::TimeSourceStatus { role: Role::Primary, status: ftexternal::Status::Network },
        ]);
    }

    #[fuchsia::test]
    fn no_update_invalid_rtc_force_start() {
        let mut executor = fasync::TestExecutor::new_with_fake_time();
        clone_system_time(&mut executor);

        let (clock, initial_update_ticks) = create_clock();
        let rtc = FakeRtc::valid(INVALID_RTC_TIME);
        let diagnostics = Arc::new(FakeDiagnostics::new());

        // Force start from backstop even if RTC is present.
        let config = make_test_config_with_force_start();

        let time_source = FakePushTimeSource::events(vec![TimeSourceEvent::StatusChange {
            status: ftexternal::Status::Network,
        }])
        .into();
        let (s, r) = mpsc::channel(1);
        let b = new_state_for_test(true);

        // Maintain UTC until no more work remains
        let mut fut = pin!(maintain_utc(
            PrimaryTrack { clock: Arc::clone(&clock), time_source },
            None,
            Some(rtc.clone()),
            Arc::clone(&diagnostics),
            Arc::clone(&config),
            s,
            r,
            b,
        ));
        // Run beyond first sample update.
        let _ignore_poll =
            run_in_fake_time(&mut executor, &mut fut, zx::MonotonicDuration::from_millis(1));

        // Checking that the clock has not been updated yet
        let last_value_update_ticks = clock.get_details().unwrap().last_value_update_ticks;
        assert_leq!(initial_update_ticks, last_value_update_ticks);

        // Checking that the correct diagnostic events were logged.
        diagnostics.assert_events_prefix(&[
            Event::Initialized { clock_state: InitialClockState::NotSet },
            // RTC time was invalid...
            Event::InitializeRtc {
                outcome: InitializeRtcOutcome::InvalidBeforeBackstop,
                time: Some(INVALID_RTC_TIME),
            },
            // ...But we initialized from backstop.
            Event::InitializeRtc {
                outcome: InitializeRtcOutcome::Succeeded,
                time: Some(BACKSTOP_TIME),
            },
            Event::StartClock { track: Track::Primary, source: StartClockSource::Rtc },
            Event::TimeSourceStatus { role: Role::Primary, status: ftexternal::Status::Network },
        ]);
    }

    #[fuchsia::test]
    fn no_update_valid_rtc() {
        // Start from the system time. Required to work around backstop time issues.
        let mut executor = fasync::TestExecutor::new_with_fake_time();
        clone_system_time(&mut executor);

        let (clock, initial_update_ticks) = create_clock();
        let rtc = FakeRtc::valid(VALID_RTC_TIME);
        let diagnostics = Arc::new(FakeDiagnostics::new());
        let config = make_test_config();

        let time_source = FakePushTimeSource::events(vec![TimeSourceEvent::StatusChange {
            status: ftexternal::Status::Network,
        }])
        .into();
        let (s, r) = mpsc::channel(1);
        let b = new_state_for_test(true);

        // Maintain UTC until no more work remains
        let mut fut = pin!(maintain_utc(
            PrimaryTrack { clock: Arc::clone(&clock), time_source },
            None,
            Some(rtc.clone()),
            Arc::clone(&diagnostics),
            Arc::clone(&config),
            s,
            r,
            b,
        ));
        let _ignore =
            run_in_fake_time(&mut executor, &mut fut, fasync::MonotonicDuration::from_millis(1));

        // Checking that the clock was updated to use the valid RTC time.
        assert!(clock.get_details().unwrap().last_value_update_ticks > initial_update_ticks);
        assert!(clock.read().unwrap() >= VALID_RTC_TIME);

        // Checking that the correct diagnostic events were logged.
        diagnostics.assert_events_prefix(&[
            Event::Initialized { clock_state: InitialClockState::NotSet },
            Event::InitializeRtc {
                outcome: InitializeRtcOutcome::Succeeded,
                time: Some(VALID_RTC_TIME),
            },
            Event::StartClock { track: Track::Primary, source: StartClockSource::Rtc },
            Event::TimeSourceStatus { role: Role::Primary, status: ftexternal::Status::Network },
        ]);
    }

    #[fuchsia::test]
    fn no_update_clock_already_running() {
        let mut executor = fasync::TestExecutor::new_with_fake_time();
        let (_, _) = clone_system_time(&mut executor);

        // Create a clock and set it slightly after backstop
        let (clock, _) = create_clock();
        clock
            .update(
                UtcClockUpdate::builder()
                    .approximate_value(BACKSTOP_TIME + UtcDuration::from_millis(1)),
            )
            .unwrap();
        let initial_update_ticks = clock.get_details().unwrap().last_value_update_ticks;
        let rtc = FakeRtc::valid(VALID_RTC_TIME);
        let diagnostics = Arc::new(FakeDiagnostics::new());
        let config = make_test_config();

        let time_source = FakePushTimeSource::events(vec![TimeSourceEvent::StatusChange {
            status: ftexternal::Status::Network,
        }])
        .into();

        let (s, r) = mpsc::channel(1);
        let b = new_state_for_test(true);

        // Maintain UTC until no more work remains
        let mut fut = pin!(maintain_utc(
            PrimaryTrack { clock: Arc::clone(&clock), time_source },
            None,
            Some(rtc.clone()),
            Arc::clone(&diagnostics),
            Arc::clone(&config),
            s,
            r,
            b,
        ));
        let _ignore = run_in_fake_time(
            &mut executor,
            &mut fut,
            // Only run long enough to drain the single sample from `time_source`.
            fasync::MonotonicDuration::from_millis(1),
        );

        // Checking that neither the clock nor the RTC were updated.
        assert_eq!(clock.get_details().unwrap().last_value_update_ticks, initial_update_ticks);
        assert_eq!(rtc.last_set(), None);

        // Checking that the correct diagnostic events were logged.
        // After these events, we expect a number of bogus status reports.
        diagnostics.assert_events_prefix(&[
            Event::Initialized { clock_state: InitialClockState::PreviouslySet },
            Event::InitializeRtc { outcome: InitializeRtcOutcome::ReadNotAttempted, time: None },
            Event::TimeSourceStatus { role: Role::Primary, status: ftexternal::Status::Network },
        ]);
    }

    #[fuchsia::test]
    fn test_periodic_rtc_update() {
        let mut executor = fasync::TestExecutor::new_with_fake_time();
        let (tx, mut rx) = mpsc::channel(1);
        let interval = zx::BootDuration::from_minutes(10);
        let mut fut = pin!(periodic_rtc_update(tx, interval));

        // Let it run until it stalls on the first timer.
        assert!(executor.run_until_stalled(&mut fut).is_pending());

        // Advance by 10 minutes.
        executor.set_fake_time(fasync::MonotonicInstant::after(
            zx::MonotonicDuration::from_minutes(interval.into_minutes()),
        ));
        assert!(executor.wake_expired_timers());

        // It should now send the command and then stall on the next timer.
        assert!(executor.run_until_stalled(&mut fut).is_pending());

        // Check if command was received.
        let mut rx_fut = pin!(rx.next());
        match executor.run_until_stalled(&mut rx_fut) {
            Poll::Ready(Some(Command::UpdateRtc)) => {}
            _ => panic!("Expected Command::UpdateRtc"),
        }
    }

    #[fuchsia::test]
    fn test_initial_clock_state() {
        let clock =
            UtcClock::create(zx::ClockOpts::empty(), Some(UtcInstant::from_nanos(1_000))).unwrap();
        // The clock must be started with an initial value.
        clock
            .update(UtcClockUpdate::builder().approximate_value(UtcInstant::from_nanos(1_000)))
            .unwrap();
        let (state, _) = initial_clock_state(&clock);
        assert!(matches!(state, InitialClockState::NotSet));

        // Update the clock, which is already running.
        clock
            .update(UtcClockUpdate::builder().approximate_value(UtcInstant::from_nanos(1_000_000)))
            .unwrap();
        let (state, _) = initial_clock_state(&clock);
        assert_eq!(state, InitialClockState::PreviouslySet);
    }
}
