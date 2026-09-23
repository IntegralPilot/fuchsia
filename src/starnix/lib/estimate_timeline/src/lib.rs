// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::collections::VecDeque;

const NANOS_PER_MILLISECOND: i64 = 1_000_000;
const TIMELINE_DRIFT_THRESHOLD_MS: TimeOffsetMillis = TimeOffsetMillis(200);
const TIMELINE_BOOT_TIME_THRESHOLD_MS: TimeOffsetMillis = TimeOffsetMillis(200);
const TIMELINE_MAX_SIZE: usize = 400;
const TIMELINE_TRIM_SIZE: usize = 200;

/// Represents a time offset in milliseconds.
#[derive(Debug, Copy, Clone, PartialEq, PartialOrd)]
struct TimeOffsetMillis(i64);

impl From<zx::BootInstant> for TimeOffsetMillis {
    fn from(value: zx::BootInstant) -> Self {
        Self(value.into_nanos() / NANOS_PER_MILLISECOND)
    }
}

impl From<zx::MonotonicInstant> for TimeOffsetMillis {
    fn from(value: zx::MonotonicInstant) -> Self {
        Self(value.into_nanos() / NANOS_PER_MILLISECOND)
    }
}

impl TimeOffsetMillis {
    fn to_nanos(&self) -> i64 {
        self.0 * NANOS_PER_MILLISECOND
    }
}

impl std::ops::Add for TimeOffsetMillis {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        Self(self.0 + rhs.0)
    }
}

impl std::ops::Sub for TimeOffsetMillis {
    type Output = Self;

    fn sub(self, rhs: Self) -> Self::Output {
        Self(self.0 - rhs.0)
    }
}

// Time fetcher interface allowing for zero-overhead fakes of time
// for testing purposes. In production code this is static (not dyn,
// ensuring no runtime overhead in production caused by tests).
// The fuchsia_async fake time can't be used as Starnix has unique
// runtime constraints and we can't rely on there being an active async context.
pub trait TimeFetcher {
    // Fetches the current monotonic time
    fn get_monotonic(&self) -> zx::MonotonicInstant;

    // Fetches the current boot time
    fn get_boot(&self) -> zx::BootInstant;
}

#[derive(Debug, Default)]
pub struct DefaultFetcher;

impl TimeFetcher for DefaultFetcher {
    fn get_monotonic(&self) -> zx::MonotonicInstant {
        zx::MonotonicInstant::get()
    }

    fn get_boot(&self) -> zx::BootInstant {
        zx::BootInstant::get()
    }
}

/// A single entry in the timeline, mapping a boot time to a monotonic offset.
type TimelineEntry = (zx::BootInstant, TimeOffsetMillis);

#[derive(Debug, Default)]
pub struct TimelineEstimator<T: TimeFetcher> {
    /// A timeline of (boot_time_nanos, monotonic_offset_millis) pairs.
    /// The monotonic offset is calculated as (boot_time_nanos - monotonic_time_nanos) / 1,000,000.
    timeline: VecDeque<TimelineEntry>,
    /// Max timeline size value
    max_timeline_size: u64,
    /// Number of times the timeline has overflowed
    timeline_overflows: u64,
    time_fetcher: T,
}

impl<T: TimeFetcher> TimelineEstimator<T> {
    pub fn new(time_fetcher: T) -> Self {
        Self {
            max_timeline_size: 0,
            timeline_overflows: 0,
            timeline: VecDeque::new(),
            time_fetcher,
        }
    }

    pub fn max_timeline_size(&self) -> u64 {
        self.max_timeline_size
    }

    pub fn timeline_overflows(&self) -> u64 {
        self.timeline_overflows
    }

    /// Converts a boot time in nanoseconds to a monotonic time in nanoseconds.
    pub fn boot_time_to_monotonic_time(
        &mut self,
        boot_time: zx::BootInstant,
    ) -> zx::MonotonicInstant {
        let timeline = &mut self.timeline;
        let current_boot_time = self.time_fetcher.get_boot();
        let mut current_offset = TimeOffsetMillis::from(self.time_fetcher.get_boot())
            - TimeOffsetMillis::from(self.time_fetcher.get_monotonic());
        // Initialize time if needed
        if timeline.is_empty() {
            timeline.push_back((current_boot_time, current_offset));
        }
        let (last_recorded_boot_time, prev_offset) =
            timeline.back().expect("timeline must have at least one entry after initialization");
        if current_offset - *prev_offset > TIMELINE_DRIFT_THRESHOLD_MS {
            // Monotonic drift has changed, insert new record.
            // Note that this measurement may be erroneous on CQ bots
            // which can be preempted for multiple seconds at a time.
            // We need to check for that before doing an update
            let new_boot_time = TimeOffsetMillis::from(self.time_fetcher.get_boot());
            let prev_boot_time = TimeOffsetMillis::from(current_boot_time);
            if new_boot_time - prev_boot_time < TIMELINE_BOOT_TIME_THRESHOLD_MS {
                if !((current_boot_time < boot_time) || boot_time < *last_recorded_boot_time) {
                    // We tend to be one of the last components to wakeup and many other components
                    // tend to log first. If we get a log message with an earlier timestamp, assume
                    // that the timestamp on that log is in our current time offset.
                    if boot_time != current_boot_time {
                        timeline.push_back((boot_time, current_offset));
                    }
                }
                timeline.push_back((current_boot_time, current_offset));
            }
            if timeline.len() > TIMELINE_MAX_SIZE {
                // Keep only the first 200 elements.
                while timeline.len() > TIMELINE_TRIM_SIZE {
                    timeline.pop_front();
                }
                self.timeline_overflows += 1;
            }
            if timeline.len() > self.max_timeline_size as usize {
                self.max_timeline_size = timeline.len() as u64;
            }
        }

        // Find the offset that was active at or before the given boot_time.
        for (boot, offset) in timeline.iter() {
            if *boot > boot_time {
                break;
            }
            current_offset = *offset;
        }
        // Monotonic time = Boot time - Offset
        zx::MonotonicInstant::from_nanos(
            (TimeOffsetMillis::from(boot_time) - current_offset).to_nanos(),
        )
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    #[derive(Clone)]
    struct FakeFetcher {
        inner: Arc<std::sync::Mutex<FakeFetcherInner>>,
    }

    struct FakeFetcherInner {
        monotonic: zx::MonotonicInstant,
        boot: zx::BootInstant,
    }

    impl FakeFetcher {
        fn new() -> Self {
            Self {
                inner: Arc::new(std::sync::Mutex::new(FakeFetcherInner {
                    monotonic: zx::MonotonicInstant::from_nanos(0),
                    boot: zx::BootInstant::from_nanos(0),
                })),
            }
        }

        fn set_times(&self, boot: zx::BootInstant, monotonic: zx::MonotonicInstant) {
            let mut inner = self.inner.lock().unwrap();
            inner.boot = boot;
            inner.monotonic = monotonic;
        }
    }

    impl TimeFetcher for FakeFetcher {
        fn get_monotonic(&self) -> zx::MonotonicInstant {
            self.inner.lock().unwrap().monotonic
        }

        fn get_boot(&self) -> zx::BootInstant {
            self.inner.lock().unwrap().boot
        }
    }

    #[test]
    fn test_boot_time_to_monotonic_time() {
        let fetcher = FakeFetcher::new();
        // Initial state: boot=0, mono=0.
        // We want to start with something non-zero to be interesting.
        fetcher.set_times(
            zx::BootInstant::from_nanos(1000 * NANOS_PER_MILLISECOND),
            zx::MonotonicInstant::from_nanos(100 * NANOS_PER_MILLISECOND),
        ); // boot=1000ms, mono=100ms

        let mut state = TimelineEstimator::new(fetcher.clone());

        // First call initializes timeline.
        // boot=1000, mono=100. Offset=900.
        // Record: (1000, 900).
        let mono = state
            .boot_time_to_monotonic_time(zx::BootInstant::from_nanos(1000 * NANOS_PER_MILLISECOND));
        assert_eq!(mono.into_nanos(), 100 * NANOS_PER_MILLISECOND);

        // Advance 100ms (no drift change).
        // boot=1100, mono=200. Offset=900.
        fetcher.set_times(
            zx::BootInstant::from_nanos(1100 * NANOS_PER_MILLISECOND),
            zx::MonotonicInstant::from_nanos(200 * NANOS_PER_MILLISECOND),
        );

        // Query for current time.
        let mono = state
            .boot_time_to_monotonic_time(zx::BootInstant::from_nanos(1100 * NANOS_PER_MILLISECOND));
        assert_eq!(mono.into_nanos(), 200 * NANOS_PER_MILLISECOND);

        // Query for past time (1050ms).
        // Should use offset 900. 1050 - 900 = 150.
        let mono = state
            .boot_time_to_monotonic_time(zx::BootInstant::from_nanos(1050 * NANOS_PER_MILLISECOND));
        assert_eq!(mono.into_nanos(), 150 * NANOS_PER_MILLISECOND);
    }

    #[test]
    fn test_boot_time_to_monotonic_time_suspend() {
        let fetcher = FakeFetcher::new();

        fetcher.set_times(
            zx::BootInstant::from_nanos(1000 * NANOS_PER_MILLISECOND),
            zx::MonotonicInstant::from_nanos(100 * NANOS_PER_MILLISECOND),
        );

        let mut state = TimelineEstimator::new(fetcher.clone());

        // Initialize
        state
            .boot_time_to_monotonic_time(zx::BootInstant::from_nanos(1000 * NANOS_PER_MILLISECOND));

        // Suspend: boot + 1000ms, mono + 0ms.
        // boot=2000, mono=100. Offset=1900.
        fetcher.set_times(
            zx::BootInstant::from_nanos(2000 * NANOS_PER_MILLISECOND),
            zx::MonotonicInstant::from_nanos(100 * NANOS_PER_MILLISECOND),
        );

        // Call to update timeline.
        // This should detect drift (1900 - 900 = 1000 > 200).
        // Pushes (2000, 1900).
        let mono = state
            .boot_time_to_monotonic_time(zx::BootInstant::from_nanos(2000 * NANOS_PER_MILLISECOND));
        assert_eq!(mono.into_nanos(), 100 * NANOS_PER_MILLISECOND);

        assert_eq!(state.max_timeline_size, 2);

        // Check past time lookups.
        // At 1500 (during suspend). uses offset 900 -> 600.
        let mono = state
            .boot_time_to_monotonic_time(zx::BootInstant::from_nanos(1500 * NANOS_PER_MILLISECOND));
        assert_eq!(mono.into_nanos(), 600 * NANOS_PER_MILLISECOND);

        // At 2000 (just woke up). uses offset 1900 -> 100.
        let mono = state
            .boot_time_to_monotonic_time(zx::BootInstant::from_nanos(2000 * NANOS_PER_MILLISECOND));
        assert_eq!(mono.into_nanos(), 100 * NANOS_PER_MILLISECOND);
    }

    #[test]
    fn test_boot_time_to_monotonic_time_overflow() {
        let fetcher = FakeFetcher::new();
        fetcher.set_times(zx::BootInstant::from_nanos(0), zx::MonotonicInstant::from_nanos(0));

        let mut state = TimelineEstimator::new(fetcher.clone());
        state.boot_time_to_monotonic_time(zx::BootInstant::from_nanos(0));

        // We want to trigger overflow. Max size is 400.

        let mut current_boot = 0;
        let current_mono = 0;

        for _ in 0..500 {
            // Drift needs to change.
            // Increase boot by 300ms, mono by 0.
            current_boot += 300 * NANOS_PER_MILLISECOND;
            fetcher.set_times(
                zx::BootInstant::from_nanos(current_boot),
                zx::MonotonicInstant::from_nanos(current_mono),
            );

            state.boot_time_to_monotonic_time(zx::BootInstant::from_nanos(current_boot));
        }

        // Should have trimmed.
        assert!(state.timeline.len() <= TIMELINE_MAX_SIZE);
        assert!(state.timeline.len() >= TIMELINE_TRIM_SIZE);
        assert_eq!(state.max_timeline_size, (TIMELINE_MAX_SIZE) as u64);

        // 300 items expected (200 retained + 100 added).
        assert_eq!(state.timeline.len(), 300);

        for _ in 0..200 {
            current_boot += 300 * NANOS_PER_MILLISECOND;
            fetcher.set_times(
                zx::BootInstant::from_nanos(current_boot),
                zx::MonotonicInstant::from_nanos(current_mono),
            );
            state.boot_time_to_monotonic_time(zx::BootInstant::from_nanos(current_boot));
        }
        // 300 + 200 = 500 total in loop.
        // At 401 trimmed to 200. Added 99 -> 299.
        assert_eq!(state.timeline.len(), 299);
    }

    #[test]
    fn test_boot_time_to_monotonic_time_intermediate_update() {
        let fetcher = FakeFetcher::new();
        // Initial: boot=2000, mono=1000. Offset=1000.
        fetcher.set_times(
            zx::BootInstant::from_nanos(2000 * NANOS_PER_MILLISECOND),
            zx::MonotonicInstant::from_nanos(1000 * NANOS_PER_MILLISECOND),
        );

        let mut state = TimelineEstimator::new(fetcher.clone());
        // This has the side effect of performing a timeline transformation
        // (when the clock is set to a specific value). This has the side-effect of updating
        // the estimation state to a value that we want it in for subsequent tests.
        // For intermediate updates, we look at not just clock values,
        // but also timestamps from logs. This simulates a log coming in at 2 seconds after
        // the system boots.
        state
            .boot_time_to_monotonic_time(zx::BootInstant::from_nanos(2000 * NANOS_PER_MILLISECOND));

        // Update with small boot delta (<200ms) but large drift (>200ms).
        // boot=2150 (+150ms). mono=600 (-400ms).
        // offset=1550. Drift=550.
        fetcher.set_times(
            zx::BootInstant::from_nanos(2150 * NANOS_PER_MILLISECOND),
            zx::MonotonicInstant::from_nanos(600 * NANOS_PER_MILLISECOND),
        );

        // Query with intermediate time 2100.
        // Should insert (2100, 1550) and (2150, 1550).
        let mono = state
            .boot_time_to_monotonic_time(zx::BootInstant::from_nanos(2100 * NANOS_PER_MILLISECOND));

        // Expected mono = 2100 - 1550 = 550.
        assert_eq!(mono.into_nanos(), 550 * NANOS_PER_MILLISECOND);

        // Check timeline behavior.
        // Query 2050 (before 2100). Should use old offset 1000.
        // 2050 - 1000 = 1050.
        let mono_prev = state
            .boot_time_to_monotonic_time(zx::BootInstant::from_nanos(2050 * NANOS_PER_MILLISECOND));
        assert_eq!(mono_prev.into_nanos(), 1050 * NANOS_PER_MILLISECOND);

        // Query 2125 (after 2100). Should use new offset 1550.
        // 2125 - 1550 = 575.
        let mono_next = state
            .boot_time_to_monotonic_time(zx::BootInstant::from_nanos(2125 * NANOS_PER_MILLISECOND));
        assert_eq!(mono_next.into_nanos(), 575 * NANOS_PER_MILLISECOND);

        // Query 2150 (current). Offset 1550.
        // 2150 - 1550 = 600.
        let mono_curr = state
            .boot_time_to_monotonic_time(zx::BootInstant::from_nanos(2150 * NANOS_PER_MILLISECOND));
        assert_eq!(mono_curr.into_nanos(), 600 * NANOS_PER_MILLISECOND);

        // Ensure size is correct (should be 3: 2000, 2100, 2150).
        assert_eq!(state.max_timeline_size, 3);
    }

    #[test]
    fn test_boot_time_to_monotonic_time_retroactive_update() {
        let fetcher = FakeFetcher::new();
        // Initial: boot=1000, mono=100. Offset=900.
        fetcher.set_times(
            zx::BootInstant::from_nanos(1000 * NANOS_PER_MILLISECOND),
            zx::MonotonicInstant::from_nanos(100 * NANOS_PER_MILLISECOND),
        );

        let mut state = TimelineEstimator::new(fetcher.clone());
        state
            .boot_time_to_monotonic_time(zx::BootInstant::from_nanos(1000 * NANOS_PER_MILLISECOND));

        // Advance to boot=5000, mono=2000. Offset=3000.
        // Drift = 3000 - 900 = 2100 > 200.
        fetcher.set_times(
            zx::BootInstant::from_nanos(5000 * NANOS_PER_MILLISECOND),
            zx::MonotonicInstant::from_nanos(2000 * NANOS_PER_MILLISECOND),
        );

        // Query with boot=4500.
        // 1000 < 4500 < 5000.
        // Should trigger retroactive update. Timeline should have (4500, 3000).
        // Result should use offset 3000.
        // 4500 - 3000 = 1500.
        let mono = state
            .boot_time_to_monotonic_time(zx::BootInstant::from_nanos(4500 * NANOS_PER_MILLISECOND));
        assert_eq!(mono.into_nanos(), 1500 * NANOS_PER_MILLISECOND);

        // Verify the timeline explicitly if possible, or via another query.
        // If we query 4600, it should also use offset 3000.
        let mono_next = state
            .boot_time_to_monotonic_time(zx::BootInstant::from_nanos(4600 * NANOS_PER_MILLISECOND));
        assert_eq!(mono_next.into_nanos(), 1600 * NANOS_PER_MILLISECOND);

        // If we query 4400, it should use offset 900 (from first record).
        // 4400 - 900 = 3500.
        let mono_prev = state
            .boot_time_to_monotonic_time(zx::BootInstant::from_nanos(4400 * NANOS_PER_MILLISECOND));
        assert_eq!(mono_prev.into_nanos(), 3500 * NANOS_PER_MILLISECOND);
    }
}
