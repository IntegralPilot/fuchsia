// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// This module provides a thin abstraction over chronological types using type definitions. This
// can be further abstracted through dedicated types and more sophisticated APIs that support
// different clock implementations. At time of writing, it is tightly integrated with
// `fuchsia_async` and quantizes time to 1s.

//! Monotonic clock and chronological APIs.

use num::Integer;
use std::fmt::Display;
use std::mem;
use thiserror::Error;

/// Monotonic time error.
///
/// Describes temporal errors that occur when points in time are unexpectedly less (earlier) than
/// some reference time.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
#[error("non-monotonic tick or timestamp")]
pub struct MonotonicityError;

/// Unit of time.
///
/// Time is expressed and quantized as integer multiples of this basis unit.
pub type Quanta = i64;

pub trait QuantaExt {
    /// Converts the quanta into a `Display` that describes a duration in the nearest (largest)
    /// units.
    fn into_nearest_unit_display(self) -> impl Display;
}

impl QuantaExt for Quanta {
    fn into_nearest_unit_display(self) -> impl Display {
        const SECONDS_PER_MINUTE: i64 = 60;
        const SECONDS_PER_HOUR: i64 = SECONDS_PER_MINUTE * 60;
        const SECONDS_PER_DAY: i64 = SECONDS_PER_HOUR * 24;

        match (
            self.div_rem(&SECONDS_PER_DAY),
            self.div_rem(&SECONDS_PER_HOUR),
            self.div_rem(&SECONDS_PER_MINUTE),
        ) {
            ((days, 0), _, _) => format!("{}d", days),
            (_, (hours, 0), _) => format!("{}h", hours),
            (_, _, (minutes, 0)) => format!("{}m", minutes),
            _ => format!("{}s", self),
        }
    }
}

/// A point in time.
pub type Timestamp = fuchsia_async::BootInstant;

pub trait TimestampExt {
    /// Calculates the number of quanta between zero and the current timestamp.
    fn quantize(self) -> Quanta;

    /// Creates a timestamp from elapsed Quanta from the 0 timestamp.
    fn from_quanta(quanta: Quanta) -> Self;
}

impl TimestampExt for Timestamp {
    fn quantize(self) -> Quanta {
        (self - Timestamp::from_nanos(0)).into_quanta()
    }

    fn from_quanta(quanta: Quanta) -> Self {
        Timestamp::from_nanos(0) + Duration::from_quanta(quanta)
    }
}

/// A vector in time.
pub type Duration = zx::BootDuration;

pub trait DurationExt {
    /// The unit duration.
    ///
    /// Durations are expressed in terms of this unit and so cannot express periods beyond this
    /// resolution.
    const QUANTUM: Self;

    /// Constructs a `Duration` from quanta.
    fn from_quanta(quanta: Quanta) -> Self;

    /// Converts the duration into quanta.
    fn into_quanta(self) -> Quanta;
}

impl DurationExt for Duration {
    const QUANTUM: Self = Duration::from_seconds(1);

    fn from_quanta(quanta: Quanta) -> Self {
        Duration::from_seconds(quanta)
    }

    fn into_quanta(self) -> Quanta {
        self.into_seconds()
    }
}

/// An arrow in time.
///
/// A `Tick` represents a directed point or relative displacement in time with respect to some
/// reference time.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Tick {
    start: Timestamp,
    end: Timestamp,
    last_sample_timestamp: Option<Timestamp>,
}

impl Tick {
    /// Quantizes the tick into start and end points in time, in that order.
    pub fn quantize(self) -> (Quanta, Quanta) {
        let start = self.start.quantize();
        let end = self.end.quantize();
        (start, end)
    }

    /// Return true if the sampling period at the start time of the tick has a sample.
    /// Otherwise, return false.
    pub fn start_has_sample(self, max_sampling_period: Quanta) -> bool {
        let start = self.start.quantize();
        match self.last_sample_timestamp {
            Some(last_sample_timestamp) => {
                let sample_time = last_sample_timestamp.quantize();
                (start / max_sampling_period) == (sample_time / max_sampling_period)
            }
            None => false,
        }
    }
}

/// A monotonic reference point in time that advances when a sample is observed or
/// an interpolation occurs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ObservationTime {
    pub(crate) last_update_timestamp: Timestamp,
    pub(crate) last_sample_timestamp: Option<Timestamp>,
}

impl ObservationTime {
    /// Advances the observation time to the given point in time.
    ///
    /// Returns a [`Tick`] that describes the displacement in time.
    ///
    /// # Errors
    ///
    /// Returns an error if the given point in time is not monotonic with respect to the current
    /// observation time.
    ///
    /// [`Tick`]: crate::experimental::clock::Tick
    pub fn tick(
        &mut self,
        timestamp: Timestamp,
        is_sample: bool,
    ) -> Result<Tick, MonotonicityError> {
        if timestamp < self.last_update_timestamp {
            Err(MonotonicityError)
        } else {
            let new_observation_time = ObservationTime {
                last_update_timestamp: timestamp,
                last_sample_timestamp: if is_sample {
                    Some(timestamp)
                } else {
                    self.last_sample_timestamp
                },
            };
            let prev = mem::replace(self, new_observation_time);
            Ok(Tick {
                start: prev.last_update_timestamp,
                end: timestamp,
                last_sample_timestamp: prev.last_sample_timestamp,
            })
        }
    }

    pub(crate) fn at(last_update_timestamp: Timestamp) -> Self {
        Self { last_update_timestamp, last_sample_timestamp: None }
    }
}

impl Default for ObservationTime {
    fn default() -> Self {
        ObservationTime { last_update_timestamp: Timestamp::now(), last_sample_timestamp: None }
    }
}

/// Data associated with a [point in time][`Timestamp`], such as a sample or event.
///
/// [`Timestamp`]: crate::experimental::clock::Timestamp
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Timed<T> {
    timestamp: Timestamp,
    inner: T,
}

impl<T> Timed<T> {
    /// Constructs a `Timed` from data at the [current time][`Timestamp::now`].
    ///
    /// [`Timestamp::now`]: crate::experimental::clock::Timestamp::now
    pub fn now(inner: T) -> Self {
        Timed { timestamp: Timestamp::now(), inner }
    }

    /// Constructs a `Timed` from data at the given point in time.
    pub fn at(inner: T, timestamp: impl Into<Timestamp>) -> Self {
        Timed { timestamp: timestamp.into(), inner }
    }

    /// Maps a `Timed<T>` to a `Timed<T>` by applying the given function to the inner data.
    pub fn map<U, F>(self, f: F) -> Timed<U>
    where
        F: FnOnce(T) -> U,
    {
        let Timed { timestamp, inner } = self;
        Timed { timestamp, inner: f(inner) }
    }

    /// Gets the point in time associated with the data.
    pub fn timestamp(&self) -> &Timestamp {
        &self.timestamp
    }

    /// Gets the inner data.
    pub fn inner(&self) -> &T {
        &self.inner
    }
}

impl<T> Timed<Option<T>> {
    pub fn transpose(self) -> Option<Timed<T>> {
        let Timed { timestamp, inner } = self;
        inner.map(|inner| Timed { timestamp, inner })
    }
}

impl<T, E> Timed<Result<T, E>> {
    pub fn transpose(self) -> Result<Timed<T>, E> {
        let Timed { timestamp, inner } = self;
        inner.map(|inner| Timed { timestamp, inner })
    }
}

impl<T> From<Timed<T>> for (Timestamp, T) {
    fn from(timed: Timed<T>) -> Self {
        let Timed { timestamp, inner: sample } = timed;
        (timestamp, sample)
    }
}

#[cfg(test)]
mod tests {
    use crate::experimental::clock::{
        Duration, DurationExt as _, MonotonicityError, ObservationTime, QuantaExt as _, Tick,
        Timed, Timestamp, TimestampExt as _,
    };
    use fuchsia_async as fasync;

    #[test]
    fn quantize() {
        let timestamp = Timestamp::from_nanos(0) + Duration::QUANTUM;
        assert_eq!(timestamp.quantize(), 1);

        let tick = Tick {
            start: timestamp,
            end: timestamp + Duration::from_quanta(3),
            last_sample_timestamp: Some(Timestamp::from_nanos(-999)),
        };
        assert_eq!(tick.quantize(), (1, 4));
    }

    #[test]
    fn start_has_sample() {
        let tick = Tick {
            start: Timestamp::from_nanos(9_000_000_000),
            end: Timestamp::from_nanos(12_000_000_000),
            last_sample_timestamp: Some(Timestamp::from_nanos(8_000_000_000)),
        };
        assert!(tick.start_has_sample(10));

        let tick = Tick {
            start: Timestamp::from_nanos(10_000_000_000),
            end: Timestamp::from_nanos(13_000_000_000),
            last_sample_timestamp: Some(Timestamp::from_nanos(8_000_000_000)),
        };
        assert!(!tick.start_has_sample(10));
    }

    #[test]
    fn tick() {
        const ZERO: Timestamp = Timestamp::from_nanos(0);
        const MINUTE_ONE: Timestamp = Timestamp::from_nanos(60_000_000_000);
        const MINUTE_THREE: Timestamp = Timestamp::from_nanos(180_000_000_000);
        let mut last = ObservationTime { last_update_timestamp: ZERO, last_sample_timestamp: None };

        let tick = last.tick(MINUTE_ONE, true).unwrap();
        let expected_tick = Tick { start: ZERO, end: MINUTE_ONE, last_sample_timestamp: None };
        let expected_last = ObservationTime {
            last_update_timestamp: MINUTE_ONE,
            last_sample_timestamp: Some(MINUTE_ONE),
        };
        assert_eq!(tick, expected_tick);
        assert_eq!(last, expected_last);

        let tick = last.tick(MINUTE_THREE, false).unwrap();
        let expected_tick =
            Tick { start: MINUTE_ONE, end: MINUTE_THREE, last_sample_timestamp: Some(MINUTE_ONE) };
        let expected_last = ObservationTime {
            last_update_timestamp: MINUTE_THREE,
            last_sample_timestamp: Some(MINUTE_ONE),
        };
        assert_eq!(tick, expected_tick);
        assert_eq!(last, expected_last);

        let result = last.tick(MINUTE_ONE, false);
        assert_eq!(result, Err(MonotonicityError));
    }

    #[test]
    fn fmt_display_quanta() {
        assert_eq!(0i64.into_nearest_unit_display().to_string(), "0d");
        assert_eq!(1i64.into_nearest_unit_display().to_string(), "1s");
        assert_eq!(5i64.into_nearest_unit_display().to_string(), "5s");
        assert_eq!(60i64.into_nearest_unit_display().to_string(), "1m");
        assert_eq!(65i64.into_nearest_unit_display().to_string(), "65s");
        assert_eq!(120i64.into_nearest_unit_display().to_string(), "2m");
        assert_eq!(3600i64.into_nearest_unit_display().to_string(), "1h");
        assert_eq!(3605i64.into_nearest_unit_display().to_string(), "3605s");
        assert_eq!(86400i64.into_nearest_unit_display().to_string(), "1d");
    }

    #[test]
    fn timed_now_then_timestamp_eq_executor_now() {
        const NOW: i64 = 3_000_000_000;

        let executor = fasync::TestExecutor::new_with_fake_time();
        executor.set_fake_time(fasync::MonotonicInstant::from_nanos(NOW));

        let timed = Timed::now(());
        let (timestamp, _) = timed.into();
        assert_eq!(timestamp, Timestamp::from_nanos(NOW));
    }

    #[test]
    fn timed_at_point_then_timestamp_eq_point() {
        const AT: Timestamp = Timestamp::from_nanos(1);

        let timed = Timed::at((), AT);
        let (timestamp, _) = timed.into();
        assert_eq!(timestamp, AT);
    }

    #[test]
    fn map_timed_then_data_eq_output() {
        let timed = Timed::at(9i64, Timestamp::from_nanos(1)).map(|n| n * 7);
        let (_, n) = timed.into();
        assert_eq!(n, 63);
    }

    #[test]
    fn transpose_timed_then_output_is_congruent() {
        const AT: Timestamp = Timestamp::from_nanos(1);

        assert!(Timed::at(None::<()>, AT).transpose().is_none());
        assert!(Timed::at(Some(()), AT).transpose().is_some());
    }
}
