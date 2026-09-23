// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::diagnostics::{Diagnostics, Event};
use crate::enums::{
    ClockCorrectionStrategy, ClockUpdateReason, InitialClockState, StartClockSource, Track,
};
use anyhow::{Context as _, Error, format_err};
use cobalt_client::traits::AsEventCodes;
use fidl_contrib::ProtocolConnector;
use fidl_contrib::protocol_connector::{ConnectedProtocol, ProtocolSender};
use fidl_fuchsia_metrics::{
    MetricEvent, MetricEventLoggerFactoryMarker, MetricEventLoggerProxy, ProjectSpec,
};
use fuchsia_async as fasync;
use fuchsia_cobalt_builders::MetricEventExt;
use fuchsia_component::client::connect_to_protocol;
use fuchsia_runtime::UtcDuration;
use fuchsia_sync::Mutex;
use futures::{FutureExt as _, future};
use time_metrics_registry::{
    PROJECT_ID, REAL_TIME_CLOCK_EVENTS_MIGRATED_METRIC_ID,
    RealTimeClockEventsMigratedMetricDimensionEventType as RtcEvent,
    TIMEKEEPER_CLOCK_CORRECTION_MIGRATED_METRIC_ID,
    TIMEKEEPER_FREQUENCY_ABS_ESTIMATE_MIGRATED_METRIC_ID,
    TIMEKEEPER_LIFECYCLE_EVENTS_MIGRATED_METRIC_ID, TIMEKEEPER_SQRT_COVARIANCE_MIGRATED_METRIC_ID,
    TIMEKEEPER_TIME_SOURCE_EVENTS_MIGRATED_METRIC_ID, TIMEKEEPER_TRACK_EVENTS_MIGRATED_METRIC_ID,
    TimeMetricDimensionDirection as Direction, TimeMetricDimensionExperiment as Experiment,
    TimeMetricDimensionIteration as Iteration, TimeMetricDimensionRole as CobaltRole,
    TimeMetricDimensionTrack as CobaltTrack,
    TimekeeperLifecycleEventsMigratedMetricDimensionEventType as LifecycleEvent,
    TimekeeperTimeSourceEventsMigratedMetricDimensionEventType as TimeSourceEvent,
    TimekeeperTrackEventsMigratedMetricDimensionEventType as TrackEvent,
};

/// The number of parts in a million.
const ONE_MILLION: i64 = 1_000_000;

struct CobaltConnectedService;
impl ConnectedProtocol for CobaltConnectedService {
    type Protocol = MetricEventLoggerProxy;
    type ConnectError = Error;
    type Message = MetricEvent;
    type SendError = Error;

    fn get_protocol<'a>(
        &'a mut self,
    ) -> future::BoxFuture<'a, Result<MetricEventLoggerProxy, Error>> {
        async {
            let (logger_proxy, server_end) = fidl::endpoints::create_proxy();
            let metric_event_logger_factory =
                connect_to_protocol::<MetricEventLoggerFactoryMarker>()
                    .context("Failed to connect to fuchsia::metrics::MetricEventLoggerFactory")?;

            metric_event_logger_factory
                .create_metric_event_logger(
                    &ProjectSpec { project_id: Some(PROJECT_ID), ..Default::default() },
                    server_end,
                )
                .await?
                .map_err(|e| format_err!("Connection to MetricEventLogger refused {e:?}"))?;
            Ok(logger_proxy)
        }
        .boxed()
    }

    fn send_message<'a>(
        &'a mut self,
        protocol: &'a MetricEventLoggerProxy,
        msg: MetricEvent,
    ) -> future::BoxFuture<'a, Result<(), Error>> {
        async move {
            let fut = protocol.log_metric_events(&[msg]);
            fut.await?.map_err(|e| format_err!("Failed to log timekeeper metric {e:?}"))?;
            Ok(())
        }
        .boxed()
    }
}

/// A connection to the real Cobalt service.
pub struct CobaltDiagnostics {
    /// The ProtocolSender for MetricEvent's used to log metrics.
    sender: Mutex<ProtocolSender<MetricEvent>>,
    /// The experiment to record on all experiment-based events.
    experiment: Experiment,
    // TODO(https://fxbug.dev/42135549): Move back to an owned fasync::Task instead of detaching the spawned
    // Task once the lifecycle of timekeeper ensures CobaltDiagnostics objects will last long enough
    // to finish their logging.
}

impl CobaltDiagnostics {
    /// Constructs a new `CobaltDiagnostics` instance.
    pub(crate) fn new(experiment: Experiment) -> Self {
        let (sender, fut) = ProtocolConnector::new(CobaltConnectedService).serve_and_log_errors();
        fasync::Task::spawn(fut).detach();
        Self { sender: Mutex::new(sender), experiment }
    }

    /// Records an update to the Kalman filter state, including an event and a covariance report.
    fn record_kalman_filter_update(&self, track: Track, sqrt_covariance: zx::BootDuration) {
        let mut locked_sender = self.sender.lock();
        let cobalt_track = Into::<CobaltTrack>::into(track);
        locked_sender.send(
            MetricEvent::builder(TIMEKEEPER_TRACK_EVENTS_MIGRATED_METRIC_ID)
                .with_event_codes(
                    (TrackEvent::EstimatedOffsetUpdated, cobalt_track, self.experiment)
                        .as_event_codes(),
                )
                .as_occurrence(1),
        );
        locked_sender.send(
            MetricEvent::builder(TIMEKEEPER_SQRT_COVARIANCE_MIGRATED_METRIC_ID)
                .with_event_codes(
                    (Into::<CobaltTrack>::into(track), self.experiment).as_event_codes(),
                )
                // This metric is configured to be microseconds, which does not follow the
                // standard of nanoseconds everywhere.
                .as_integer(sqrt_covariance.into_micros()),
        );
    }

    /// Records an update to the estimated frequency.
    fn record_frequency_update(&self, track: Track, rate_adjust_ppm: i32, window_count: u32) {
        let iteration = match window_count {
            1 => Iteration::First,
            2 => Iteration::Second,
            _ => Iteration::Subsequent,
        };
        // Frequency arrives as a deviation in ppm from a 1Hz clock which can be positive or
        // negative, but to keep the events we report to cobalt positive we output an absolute
        // utc parts per million monotonic parts.
        self.sender.lock().send(
            MetricEvent::builder(TIMEKEEPER_FREQUENCY_ABS_ESTIMATE_MIGRATED_METRIC_ID)
                .with_event_codes(
                    (iteration, Into::<CobaltTrack>::into(track), self.experiment).as_event_codes(),
                )
                .as_integer(rate_adjust_ppm as i64 + ONE_MILLION),
        );
    }

    /// Records a clock correction, including an event and a report on the magnitude of change.
    fn record_clock_correction(
        &self,
        track: Track,
        correction: UtcDuration,
        strategy: ClockCorrectionStrategy,
    ) {
        let mut locked_sender = self.sender.lock();
        let cobalt_track = Into::<CobaltTrack>::into(track);
        let direction =
            if correction.into_nanos() >= 0 { Direction::Positive } else { Direction::Negative };
        locked_sender.send(
            MetricEvent::builder(TIMEKEEPER_TRACK_EVENTS_MIGRATED_METRIC_ID)
                .with_event_codes(
                    (Into::<TrackEvent>::into(strategy), cobalt_track, self.experiment)
                        .as_event_codes(),
                )
                .as_occurrence(1),
        );
        locked_sender.send(
            MetricEvent::builder(TIMEKEEPER_CLOCK_CORRECTION_MIGRATED_METRIC_ID)
                .with_event_codes((direction, cobalt_track, self.experiment).as_event_codes())
                .as_integer(correction.into_micros().abs()),
        );
    }

    /// Records relevant information following a clock update.
    ///
    /// All updates record the reason, an update to the monitor track additionally records the
    /// difference between the monitor and primary clocks.
    fn record_clock_update(&self, track: Track, reason: ClockUpdateReason) {
        let mut locked_sender = self.sender.lock();
        locked_sender.send(
            MetricEvent::builder(TIMEKEEPER_TRACK_EVENTS_MIGRATED_METRIC_ID)
                .with_event_codes(
                    (
                        Into::<TrackEvent>::into(reason),
                        Into::<CobaltTrack>::into(track),
                        self.experiment,
                    )
                        .as_event_codes(),
                )
                .as_occurrence(1),
        );
    }
}

impl Diagnostics for CobaltDiagnostics {
    fn record(&self, event: Event) {
        match event {
            Event::Initialized { clock_state } => {
                let event = match clock_state {
                    InitialClockState::NotSet => LifecycleEvent::InitializedBeforeUtcStart,
                    InitialClockState::PreviouslySet => LifecycleEvent::InitializedAfterUtcStart,
                };
                self.sender.lock().send(
                    MetricEvent::builder(TIMEKEEPER_LIFECYCLE_EVENTS_MIGRATED_METRIC_ID)
                        .with_event_codes(event.as_event_codes())
                        .as_occurrence(1),
                );
            }
            Event::InitializeRtc { outcome, .. } => {
                self.sender.lock().send(
                    MetricEvent::builder(REAL_TIME_CLOCK_EVENTS_MIGRATED_METRIC_ID)
                        .with_event_codes(Into::<RtcEvent>::into(outcome).as_event_codes())
                        .as_occurrence(1),
                );
            }
            Event::TimeSourceFailed { role, error } => {
                let event = Into::<TimeSourceEvent>::into(error);
                self.sender.lock().send(
                    MetricEvent::builder(TIMEKEEPER_TIME_SOURCE_EVENTS_MIGRATED_METRIC_ID)
                        .with_event_codes(
                            (event, Into::<CobaltRole>::into(role), self.experiment)
                                .as_event_codes(),
                        )
                        .as_occurrence(1),
                );
            }
            Event::TimeSourceStatus { .. } => {}
            Event::SampleRejected { role, error } => {
                let event = Into::<TimeSourceEvent>::into(error);
                self.sender.lock().send(
                    MetricEvent::builder(TIMEKEEPER_TIME_SOURCE_EVENTS_MIGRATED_METRIC_ID)
                        .with_event_codes(
                            (event, Into::<CobaltRole>::into(role), self.experiment)
                                .as_event_codes(),
                        )
                        .as_occurrence(1),
                );
            }
            Event::FrequencyWindowDiscarded { track, reason } => {
                if let Some(event) = Into::<Option<TrackEvent>>::into(reason) {
                    self.sender.lock().send(
                        MetricEvent::builder(TIMEKEEPER_TRACK_EVENTS_MIGRATED_METRIC_ID)
                            .with_event_codes(
                                (event, Into::<CobaltTrack>::into(track), self.experiment)
                                    .as_event_codes(),
                            )
                            .as_occurrence(1),
                    );
                }
            }
            Event::KalmanFilterUpdated { track, sqrt_covariance, .. } => {
                self.record_kalman_filter_update(track, sqrt_covariance);
            }
            Event::FrequencyUpdated { track, rate_adjust_ppm, window_count, .. } => {
                self.record_frequency_update(track, rate_adjust_ppm, window_count);
            }
            Event::ClockCorrection { track, correction, strategy } => {
                self.record_clock_correction(track, correction, strategy);
            }
            Event::WriteRtc { outcome } => {
                self.sender.lock().send(
                    MetricEvent::builder(REAL_TIME_CLOCK_EVENTS_MIGRATED_METRIC_ID)
                        .with_event_codes(Into::<RtcEvent>::into(outcome).as_event_codes())
                        .as_occurrence(1),
                );
            }
            Event::StartClock { track, source } => {
                if track == Track::Primary {
                    let event = match source {
                        StartClockSource::Rtc => LifecycleEvent::StartedUtcFromRtc,
                        StartClockSource::External(_) => LifecycleEvent::StartedUtcFromTimeSource,
                    };
                    self.sender.lock().send(
                        MetricEvent::builder(TIMEKEEPER_LIFECYCLE_EVENTS_MIGRATED_METRIC_ID)
                            .with_event_codes(event.as_event_codes())
                            .as_occurrence(1),
                    );
                }
            }
            Event::UpdateClock { track, reason } => {
                self.record_clock_update(track, reason);
            }
            Event::UserAdjustUtc { .. } => {
                // Don't record anything to Cobalt.
            }
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::enums::{
        FrequencyDiscardReason, InitializeRtcOutcome, Role, SampleValidationError, TimeSourceError,
        WriteRtcOutcome,
    };
    use fidl_fuchsia_metrics::MetricEventPayload;
    use fuchsia_runtime::UtcInstant;
    use futures::StreamExt;
    use futures::channel::mpsc;

    const TEST_EXPERIMENT: Experiment = Experiment::B;

    /// Creates a test CobaltDiagnostics and an mpsc receiver that may be used to verify the
    /// events it sends.
    fn create_test_object() -> (CobaltDiagnostics, mpsc::Receiver<MetricEvent>) {
        let (mpsc_sender, mpsc_receiver) = futures::channel::mpsc::channel(1);
        let sender = ProtocolSender::new(mpsc_sender);
        let diagnostics =
            CobaltDiagnostics { sender: Mutex::new(sender), experiment: TEST_EXPERIMENT };
        (diagnostics, mpsc_receiver)
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn record_initialization_events() {
        let (diagnostics, mut mpsc_receiver) = create_test_object();

        diagnostics.record(Event::Initialized { clock_state: InitialClockState::NotSet });
        assert_eq!(
            mpsc_receiver.next().await,
            Some(MetricEvent {
                metric_id: time_metrics_registry::TIMEKEEPER_LIFECYCLE_EVENTS_MIGRATED_METRIC_ID,
                event_codes: vec![LifecycleEvent::InitializedBeforeUtcStart as u32],
                payload: MetricEventPayload::Count(1),
            })
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn record_clock_events() {
        let (diagnostics, mut mpsc_receiver) = create_test_object();

        diagnostics.record(Event::StartClock {
            track: Track::Monitor,
            source: StartClockSource::External(Role::Monitor),
        });
        diagnostics.record(Event::StartClock {
            track: Track::Primary,
            source: StartClockSource::External(Role::Primary),
        });
        assert_eq!(
            mpsc_receiver.next().await,
            Some(MetricEvent {
                metric_id: time_metrics_registry::TIMEKEEPER_LIFECYCLE_EVENTS_MIGRATED_METRIC_ID,
                event_codes: vec![LifecycleEvent::StartedUtcFromTimeSource as u32],
                payload: MetricEventPayload::Count(1),
            })
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn record_rtc_events() {
        let (diagnostics, mut mpsc_receiver) = create_test_object();

        diagnostics
            .record(Event::InitializeRtc { outcome: InitializeRtcOutcome::Succeeded, time: None });
        assert_eq!(
            mpsc_receiver.next().await,
            Some(MetricEvent {
                metric_id: time_metrics_registry::REAL_TIME_CLOCK_EVENTS_MIGRATED_METRIC_ID,
                event_codes: vec![RtcEvent::ReadSucceeded as u32],
                payload: MetricEventPayload::Count(1),
            })
        );

        diagnostics.record(Event::WriteRtc { outcome: WriteRtcOutcome::Failed });
        assert_eq!(
            mpsc_receiver.next().await,
            Some(MetricEvent {
                metric_id: time_metrics_registry::REAL_TIME_CLOCK_EVENTS_MIGRATED_METRIC_ID,
                event_codes: vec![RtcEvent::WriteFailed as u32],
                payload: MetricEventPayload::Count(1),
            })
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn record_time_source_events() {
        let (diagnostics, mut mpsc_receiver) = create_test_object();

        diagnostics.record(Event::SampleRejected {
            role: Role::Primary,
            error: SampleValidationError::ReferenceInstantTooOld,
        });
        assert_eq!(
            mpsc_receiver.next().await,
            Some(MetricEvent {
                metric_id: time_metrics_registry::TIMEKEEPER_TIME_SOURCE_EVENTS_MIGRATED_METRIC_ID,
                event_codes: vec![
                    TimeSourceEvent::SampleRejectedMonotonicTooOld as u32,
                    CobaltRole::Primary as u32,
                    TEST_EXPERIMENT as u32
                ],
                payload: MetricEventPayload::Count(1),
            })
        );

        diagnostics.record(Event::TimeSourceFailed {
            role: Role::Monitor,
            error: TimeSourceError::CallFailed,
        });
        assert_eq!(
            mpsc_receiver.next().await,
            Some(MetricEvent {
                metric_id: time_metrics_registry::TIMEKEEPER_TIME_SOURCE_EVENTS_MIGRATED_METRIC_ID,
                event_codes: vec![
                    TimeSourceEvent::RestartedCallFailed as u32,
                    CobaltRole::Monitor as u32,
                    TEST_EXPERIMENT as u32
                ],
                payload: MetricEventPayload::Count(1),
            })
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn record_time_track_events() {
        let (diagnostics, mut mpsc_receiver) = create_test_object();

        diagnostics.record(Event::FrequencyWindowDiscarded {
            track: Track::Primary,
            reason: FrequencyDiscardReason::InsufficientSamples,
        });
        assert_eq!(
            mpsc_receiver.next().await,
            Some(MetricEvent {
                metric_id: time_metrics_registry::TIMEKEEPER_TRACK_EVENTS_MIGRATED_METRIC_ID,
                event_codes: vec![
                    TrackEvent::FrequencyWindowDiscardedSampleCount as u32,
                    CobaltTrack::Primary as u32,
                    TEST_EXPERIMENT as u32
                ],
                payload: MetricEventPayload::Count(1),
            })
        );

        diagnostics.record(Event::KalmanFilterUpdated {
            track: Track::Primary,
            reference: zx::BootInstant::from_nanos(333_000_000_000),
            utc: UtcInstant::from_nanos(4455445544_000_000_000),
            sqrt_covariance: zx::BootDuration::from_micros(55555),
        });
        assert_eq!(
            mpsc_receiver.next().await,
            Some(MetricEvent {
                metric_id: time_metrics_registry::TIMEKEEPER_TRACK_EVENTS_MIGRATED_METRIC_ID,
                event_codes: vec![
                    TrackEvent::EstimatedOffsetUpdated as u32,
                    CobaltTrack::Primary as u32,
                    TEST_EXPERIMENT as u32
                ],
                payload: MetricEventPayload::Count(1),
            })
        );
        assert_eq!(
            mpsc_receiver.next().await,
            Some(MetricEvent {
                metric_id: time_metrics_registry::TIMEKEEPER_SQRT_COVARIANCE_MIGRATED_METRIC_ID,
                event_codes: vec![CobaltTrack::Primary as u32, TEST_EXPERIMENT as u32],
                payload: MetricEventPayload::IntegerValue(55555),
            })
        );

        diagnostics.record(Event::FrequencyUpdated {
            track: Track::Primary,
            reference: zx::BootInstant::from_nanos(888_000_000_000),
            rate_adjust_ppm: -4,
            window_count: 7,
        });
        assert_eq!(
            mpsc_receiver.next().await,
            Some(MetricEvent {
                metric_id:
                    time_metrics_registry::TIMEKEEPER_FREQUENCY_ABS_ESTIMATE_MIGRATED_METRIC_ID,
                event_codes: vec![
                    Iteration::Subsequent as u32,
                    CobaltTrack::Primary as u32,
                    TEST_EXPERIMENT as u32
                ],
                payload: MetricEventPayload::IntegerValue(1_000_000 - 4),
            })
        );

        diagnostics.record(Event::ClockCorrection {
            track: Track::Monitor,
            correction: UtcDuration::from_micros(-777),
            strategy: ClockCorrectionStrategy::NominalRateSlew,
        });
        assert_eq!(
            mpsc_receiver.next().await,
            Some(MetricEvent {
                metric_id: time_metrics_registry::TIMEKEEPER_TRACK_EVENTS_MIGRATED_METRIC_ID,
                event_codes: vec![
                    TrackEvent::CorrectionByNominalRateSlew as u32,
                    CobaltTrack::Monitor as u32,
                    TEST_EXPERIMENT as u32
                ],
                payload: MetricEventPayload::Count(1),
            })
        );
        assert_eq!(
            mpsc_receiver.next().await,
            Some(MetricEvent {
                metric_id: time_metrics_registry::TIMEKEEPER_CLOCK_CORRECTION_MIGRATED_METRIC_ID,
                event_codes: vec![
                    Direction::Negative as u32,
                    CobaltTrack::Monitor as u32,
                    TEST_EXPERIMENT as u32
                ],
                payload: MetricEventPayload::IntegerValue(777),
            })
        );
    }
}
