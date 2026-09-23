// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod frequency;
mod kalman_filter;

use crate::diagnostics::{Diagnostics, Event};
use crate::enums::{FrequencyDiscardReason, Track};
use crate::time_source::Sample;
use crate::{Config, UtcTransform};
use chrono::prelude::*;
use frequency::FrequencyEstimator;
use kalman_filter::KalmanFilter;
use log::{info, warn};
use std::sync::Arc;

/// The maximum change in Kalman filter estimate that can occur before we discard any previous
/// samples being used as part of a long term frequency assessment. This is similar to the value
/// used by the ClockManager to determine when to step the externally visible clock but it need
/// not be identical. Since the steps are measured at different reference times there would always
/// be the possibility of an inconsistency.
const MAX_STEP_FOR_FREQUENCY_CONTINUITY: zx::BootDuration = zx::BootDuration::from_seconds(1);

/// Converts a floating point frequency to a rate adjustment in PPM.
fn frequency_to_adjust_ppm(frequency: f64) -> i32 {
    ((frequency - 1.0f64) * 1_000_000f64).round() as i32
}

/// Limits the supplied frequency to within +/- MAX_FREQUENCY_ERROR.
fn clamp_frequency(input: f64, max_frequency_error: f64) -> f64 {
    input.clamp(1.0f64 - max_frequency_error, 1.0f64 + max_frequency_error)
}

/// Maintains an estimate of the relationship between true UTC time and reference time on this
/// device, based on time samples received from one or more time sources.
#[derive(Debug)]
pub struct Estimator<D: Diagnostics> {
    /// A Kalman Filter to track the UTC offset and uncertainty using a supplied frequency.
    filter: KalmanFilter,
    /// An estimator to produce long term estimates of the oscillator frequency.
    frequency_estimator: FrequencyEstimator,
    /// The track of the estimate being managed.
    track: Track,
    /// A diagnostics implementation for recording events of note.
    diagnostics: Arc<D>,
    /// Timekeeper config.
    config: Arc<Config>,
}

impl<D: Diagnostics> Estimator<D> {
    /// Construct a new estimator initialized to the supplied sample.
    pub fn new(track: Track, sample: Sample, diagnostics: Arc<D>, config: Arc<Config>) -> Self {
        let frequency_estimator = FrequencyEstimator::new(&sample, Arc::clone(&config));
        let filter = KalmanFilter::new(&sample, Arc::clone(&config));
        diagnostics.record(Event::KalmanFilterUpdated {
            track,
            reference: filter.reference(),
            utc: filter.utc(),
            sqrt_covariance: filter.sqrt_covariance(),
        });
        Estimator { filter, frequency_estimator, track, diagnostics, config }
    }

    /// Update the estimate to include the supplied sample.
    pub fn update(&mut self, sample: Sample) {
        // Begin by letting the Kalman filter consume the sample at its existing frequency...
        let utc = sample.utc;
        let change = match self.filter.update(&sample) {
            Ok(change) => change,
            Err(err) => {
                warn!("Rejected update: {}", err);
                return;
            }
        };
        let sqrt_covariance = self.filter.sqrt_covariance();
        self.diagnostics.record(Event::KalmanFilterUpdated {
            track: self.track,
            reference: self.filter.reference(),
            utc: self.filter.utc(),
            sqrt_covariance,
        });
        info!(
            "Received {:?} update to {}. estimate_change={}ns, sqrt_covariance={}ns",
            self.track,
            Utc.timestamp_nanos(utc.into_nanos()),
            change.into_nanos(),
            sqrt_covariance.into_nanos()
        );

        // If this was a big change just flush any long term frequency estimate ...
        if change.into_nanos().abs() > MAX_STEP_FOR_FREQUENCY_CONTINUITY.into_nanos() {
            self.frequency_estimator.update_disjoint(&sample);
            self.diagnostics.record(Event::FrequencyWindowDiscarded {
                track: self.track,
                reason: FrequencyDiscardReason::TimeStep,
            });
            info!("Discarding {:?} frequency window due to time step", self.track);
            return;
        }

        // ... otherwise see if the sample lets us calculate a new frequency we can give to
        // the Kalman filter for next time.
        match self.frequency_estimator.update(&sample) {
            Ok(Some((raw_frequency, window_count))) => {
                let clamped_frequency =
                    clamp_frequency(raw_frequency, self.config.get_max_frequency_error());
                self.filter.update_frequency(clamped_frequency);
                self.diagnostics.record(Event::FrequencyUpdated {
                    track: self.track,
                    reference: sample.reference,
                    rate_adjust_ppm: frequency_to_adjust_ppm(clamped_frequency),
                    window_count,
                });
                info!(
                    "Received {:?} frequency update: raw={:.9}, clamped={:.9}",
                    self.track, raw_frequency, clamped_frequency
                );
            }
            Ok(None) => {}
            Err(reason) => {
                self.diagnostics
                    .record(Event::FrequencyWindowDiscarded { track: self.track, reason });
                warn!("Discarding {:?} frequency window: {:?}", self.track, reason);
            }
        }
    }

    /// Returns a `UtcTransform` describing the estimated synthetic time and error as a function
    /// of the reference time.
    pub fn transform(&self) -> UtcTransform {
        self.filter.transform()
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::diagnostics::FakeDiagnostics;
    use crate::make_test_config;
    use fuchsia_runtime::{UtcDuration, UtcInstant};
    use test_util::assert_near;

    // Note: we need to ensure the absolute times are not near the January 1st leap second.
    const TIME_1: zx::BootInstant = zx::BootInstant::from_nanos(100_010_000_000_000);
    const TIME_2: zx::BootInstant = zx::BootInstant::from_nanos(100_020_000_000_000);
    const TIME_3: zx::BootInstant = zx::BootInstant::from_nanos(100_030_000_000_000);
    const OFFSET_1: zx::BootDuration = zx::BootDuration::from_seconds(777);
    const OFFSET_2: zx::BootDuration = zx::BootDuration::from_seconds(999);
    const STD_DEV_1: zx::BootDuration = zx::BootDuration::from_millis(22);
    const TEST_TRACK: Track = Track::Primary;
    const SQRT_COV_1: u64 = STD_DEV_1.into_nanos() as u64;

    fn create_filter_event(
        reference: zx::BootInstant,
        offset: zx::BootDuration,
        sqrt_covariance: u64,
    ) -> Event {
        Event::KalmanFilterUpdated {
            track: TEST_TRACK,
            reference,
            utc: UtcInstant::from_nanos((reference + offset).into_nanos()),
            sqrt_covariance: zx::BootDuration::from_nanos(sqrt_covariance as i64),
        }
    }

    fn create_window_discard_event(reason: FrequencyDiscardReason) -> Event {
        Event::FrequencyWindowDiscarded { track: TEST_TRACK, reason }
    }

    #[fuchsia::test]
    fn frequency_to_adjust_ppm_test() {
        assert_eq!(frequency_to_adjust_ppm(0.999), -1000);
        assert_eq!(frequency_to_adjust_ppm(0.999999), -1);
        assert_eq!(frequency_to_adjust_ppm(1.0), 0);
        assert_eq!(frequency_to_adjust_ppm(1.000001), 1);
        assert_eq!(frequency_to_adjust_ppm(1.001), 1000);
    }

    #[fuchsia::test]
    fn clamp_frequency_test() {
        let max_frequency_error = 10. / 1_000_000.;
        assert_near!(clamp_frequency(-452.0, max_frequency_error), 0.99999, 1e-9);
        assert_near!(clamp_frequency(0.99, max_frequency_error), 0.99999, 1e-9);
        assert_near!(clamp_frequency(1.0000001, max_frequency_error), 1.0000001, 1e-9);
        assert_near!(clamp_frequency(1.01, max_frequency_error), 1.00001, 1e-9);
    }

    #[fuchsia::test]
    fn initialize() {
        let config = make_test_config();
        let diagnostics = Arc::new(FakeDiagnostics::new());
        let estimator = Estimator::new(
            TEST_TRACK,
            Sample::new(
                UtcInstant::from_nanos((TIME_1 + OFFSET_1).into_nanos()),
                TIME_1,
                STD_DEV_1,
            ),
            Arc::clone(&diagnostics),
            Arc::clone(&config),
        );
        diagnostics.assert_events(&[create_filter_event(TIME_1, OFFSET_1, SQRT_COV_1)]);
        let transform = estimator.transform();
        assert_eq!(transform.synthetic(TIME_1).into_nanos(), (TIME_1 + OFFSET_1).into_nanos());
        assert_eq!(transform.synthetic(TIME_2).into_nanos(), (TIME_2 + OFFSET_1).into_nanos());
        assert_eq!(transform.error_bound(TIME_1), 2 * SQRT_COV_1);
        // Earlier time should return same error bound.
        assert_eq!(
            transform.error_bound(TIME_1 - zx::BootDuration::from_seconds(1)),
            2 * SQRT_COV_1
        );
        // Later time should have a higher bound.
        assert_eq!(
            transform.error_bound(TIME_1 + zx::BootDuration::from_seconds(1)),
            2 * SQRT_COV_1 + 2000 * config.get_oscillator_error_std_dev_ppm() as u64
        );
    }

    #[fuchsia::test]
    fn apply_inconsistent_update() {
        let diagnostics = Arc::new(FakeDiagnostics::new());
        let config = make_test_config();
        let mut estimator = Estimator::new(
            TEST_TRACK,
            Sample::new(
                UtcInstant::from_nanos((TIME_1 + OFFSET_1).into_nanos()),
                TIME_1,
                STD_DEV_1,
            ),
            Arc::clone(&diagnostics),
            config,
        );
        estimator.update(Sample::new(
            UtcInstant::from_nanos((TIME_2 + OFFSET_2).into_nanos()),
            TIME_2,
            STD_DEV_1,
        ));

        // Expected offset is biased slightly towards the second estimate.
        let expected_offset = zx::BootDuration::from_nanos(88_8002_580_002);
        let expected_sqrt_cov = 15_556_529u64;
        assert_eq!(
            estimator.transform().synthetic(TIME_3).into_nanos(),
            (TIME_3 + expected_offset).into_nanos(),
        );

        // The frequency estimator should have discarded the first update.
        assert_eq!(estimator.frequency_estimator.window_count(), 0);
        assert_eq!(estimator.frequency_estimator.current_sample_count(), 1);

        diagnostics.assert_events(&[
            create_filter_event(TIME_1, OFFSET_1, SQRT_COV_1),
            create_filter_event(TIME_2, expected_offset, expected_sqrt_cov),
            create_window_discard_event(FrequencyDiscardReason::TimeStep),
        ]);
    }

    #[fuchsia::test]
    fn apply_consistent_update() {
        let diagnostics = Arc::new(FakeDiagnostics::new());
        let config = make_test_config();
        let mut estimator = Estimator::new(
            TEST_TRACK,
            Sample::new(
                UtcInstant::from_nanos((TIME_1 + OFFSET_1).into_nanos()),
                TIME_1,
                STD_DEV_1,
            ),
            Arc::clone(&diagnostics),
            config,
        );
        estimator.update(Sample::new(
            UtcInstant::from_nanos((TIME_2 + OFFSET_1).into_nanos()),
            TIME_2,
            STD_DEV_1,
        ));

        let expected_sqrt_cov = 15_556_529u64;
        assert_eq!(
            estimator.transform().synthetic(TIME_3).into_nanos(),
            (TIME_3 + OFFSET_1).into_nanos(),
        );

        // The frequency estimator should have retained both samples.
        assert_eq!(estimator.frequency_estimator.window_count(), 0);
        assert_eq!(estimator.frequency_estimator.current_sample_count(), 2);

        diagnostics.assert_events(&[
            create_filter_event(TIME_1, OFFSET_1, SQRT_COV_1),
            create_filter_event(TIME_2, OFFSET_1, expected_sqrt_cov),
        ]);
    }

    #[fuchsia::test]
    fn earlier_reference_ignored() {
        let diagnostics = Arc::new(FakeDiagnostics::new());
        let config = make_test_config();
        let mut estimator = Estimator::new(
            TEST_TRACK,
            Sample::new(
                UtcInstant::from_nanos((TIME_2 + OFFSET_1).into_nanos()),
                TIME_2,
                STD_DEV_1,
            ),
            Arc::clone(&diagnostics),
            config,
        );
        assert_eq!(
            estimator.transform().synthetic(TIME_3).into_nanos(),
            (TIME_3 + OFFSET_1).into_nanos(),
        );
        estimator.update(Sample::new(
            UtcInstant::from_nanos((TIME_1 + OFFSET_2).into_nanos()),
            TIME_1,
            STD_DEV_1,
        ));
        assert_eq!(
            estimator.transform().synthetic(TIME_3).into_nanos(),
            (TIME_3 + OFFSET_1).into_nanos(),
        );
        // Ignored event should not be logged.
        diagnostics.assert_events(&[create_filter_event(TIME_2, OFFSET_1, SQRT_COV_1)]);
        // Nor included in the frequency estimator.
        assert_eq!(estimator.frequency_estimator.current_sample_count(), 1);
    }

    #[fuchsia::test]
    fn frequency_convergence() {
        // Generate two days of samples at a fixed, slightly erroneous, frequency.
        let reference_sample = Sample::new(
            UtcInstant::from_nanos((TIME_1 + OFFSET_1).into_nanos()),
            TIME_2,
            STD_DEV_1,
        );
        let mut samples = Vec::<Sample>::new();
        {
            let test_frequency = 1.000003;
            let utc_spacing = UtcDuration::from_hours(1) + UtcDuration::from_millis(1);
            let reference_spacing = zx::BootDuration::from_nanos(
                (utc_spacing.into_nanos() as f64 / test_frequency) as i64,
            );
            for i in 1..48 {
                samples.push(Sample::new(
                    reference_sample.utc + utc_spacing * i,
                    reference_sample.reference + reference_spacing * i,
                    reference_sample.std_dev,
                ));
            }
        }

        let diagnostics = Arc::new(FakeDiagnostics::new());
        let config = make_test_config();
        let mut estimator =
            Estimator::new(TEST_TRACK, reference_sample, Arc::clone(&diagnostics), config);

        // Run through these samples, asking the estimator to predict the utc of each sample based
        // on the sample's reference value, before feeding that sample into the estimator.
        for (i, sample) in samples.into_iter().enumerate() {
            let estimate = estimator.transform().synthetic(sample.reference);
            let error = zx::BootDuration::from_nanos((sample.utc - estimate).into_nanos().abs());

            // For the first day of samples the estimator will perform imperfectly until its been
            // able to estimate the long term frequency. After this it should get much better.
            // We calculated the new frequency after consuming the first window outside the first
            // day (i=23) and it begins to help on the next sample (i=24).
            let expected_error = match i {
                0 => zx::BootDuration::from_millis(11),
                _ if i <= 23 => zx::BootDuration::from_millis(12),
                24 => zx::BootDuration::from_millis(2),
                _ => zx::BootDuration::from_millis(0),
            };

            assert_near!(error, expected_error, zx::BootDuration::from_millis(1));
            estimator.update(sample);
        }
    }
}
