// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::fmt;

use fidl_fuchsia_time_external::TimeSample;

use fuchsia_runtime::{UtcDuration, UtcInstant};
use push_source::Update;
use time_metrics_registry::HttpsdateBoundSizeMigratedMetricDimensionPhase as CobaltPhase;

/// An internal representation of a `fuchsia.time.external.TimeSample` that contains
/// additional metrics.
#[derive(Clone, Debug, PartialEq)]
pub struct HttpsSample {
    /// The utc time sample.
    pub utc: UtcInstant,
    /// A reference time at which the `utc` sample was most valid.
    pub reference: zx::BootInstant,
    /// Standard deviation of the error distribution of `utc`.
    pub standard_deviation: UtcDuration,
    /// The size of the final bound on utc time for the sample.
    pub final_bound_size: UtcDuration,
    /// Metrics for individual polls used to produce this sample.
    pub polls: Vec<Poll>,
}

impl fmt::Display for HttpsSample {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "UTC {} (bound: {:.1}ms, polls: [",
            self.utc.into_nanos(),
            self.final_bound_size.into_micros() as f64 / 1000.0,
        )?;
        for (i, poll) in self.polls.iter().enumerate() {
            if i > 0 {
                write!(f, ", ")?;
            }
            write!(f, "{}", poll)?;
        }
        write!(f, "])")
    }
}

impl Into<TimeSample> for HttpsSample {
    fn into(self) -> TimeSample {
        TimeSample {
            reference: Some(self.reference),
            utc: Some(self.utc.into_nanos()),
            standard_deviation: Some(self.standard_deviation.into_nanos()),
            ..Default::default()
        }
    }
}

impl Into<Update> for HttpsSample {
    fn into(self) -> Update {
        Into::<TimeSample>::into(self).into()
    }
}

/// Metrics for an individual poll.
#[derive(Debug, Clone, PartialEq)]
pub struct Poll {
    /// The round trip latency observed during this poll.
    pub round_trip_time: zx::BootDuration,
}

impl fmt::Display for Poll {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:.1}ms", self.round_trip_time.into_micros() as f64 / 1000.0)
    }
}

#[cfg(test)]
impl Poll {
    /// Construct a `Poll` with the given round trip time and no offset.
    pub fn with_round_trip_time(round_trip_time: zx::BootDuration) -> Self {
        Self { round_trip_time }
    }
}

/// A phase in the HTTPS algorithm.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Phase {
    /// A phase comprised of the first sample only.
    Initial,
    /// A phase during which samples are produced relatively frequently to converge on an accurate
    /// time.
    Converge,
    /// A phase during which samples are produced relatively infrequently to maintain an accurate
    /// time.
    Maintain,
}

impl Into<CobaltPhase> for Phase {
    fn into(self) -> CobaltPhase {
        match self {
            Phase::Initial => CobaltPhase::Initial,
            Phase::Converge => CobaltPhase::Converge,
            Phase::Maintain => CobaltPhase::Maintain,
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use std::sync::Arc;

    #[fuchsia::test]
    fn test_https_sample_into_update() {
        let utc_time = UtcInstant::from_nanos(111_111_111_111);
        let boot_time = zx::BootInstant::from_nanos(222_222_222_222);
        let standard_deviation = UtcDuration::from_nanos(333_333);

        let sample = HttpsSample {
            utc: utc_time,
            reference: boot_time,
            standard_deviation,
            final_bound_size: UtcDuration::from_nanos(9001),
            polls: vec![],
        };

        assert_eq!(
            <HttpsSample as Into<Update>>::into(sample),
            Update::Sample(Arc::new(TimeSample {
                reference: Some(boot_time),
                utc: Some(utc_time.into_nanos()),
                standard_deviation: Some(standard_deviation.into_nanos()),
                ..Default::default()
            }))
        );
    }

    #[fuchsia::test]
    fn test_https_sample_display() {
        let sample = HttpsSample {
            utc: UtcInstant::from_nanos(1_788_306_697_968_401_192),
            reference: zx::BootInstant::from_nanos(100),
            standard_deviation: UtcDuration::from_nanos(200),
            final_bound_size: UtcDuration::from_nanos(64_500_000),
            polls: vec![
                Poll { round_trip_time: zx::BootDuration::from_nanos(10_000_000) },
                Poll { round_trip_time: zx::BootDuration::from_nanos(1_600_000) },
            ],
        };
        assert_eq!(
            sample.to_string(),
            "UTC 1788306697968401192 (bound: 64.5ms, polls: [10.0ms, 1.6ms])"
        );

        let empty_polls = HttpsSample { polls: vec![], ..sample };
        assert_eq!(empty_polls.to_string(), "UTC 1788306697968401192 (bound: 64.5ms, polls: [])");
    }
}
