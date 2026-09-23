// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

/// Struct used to count remaining time BSS has not been detected. Used to determine
/// when trigger auto deauth.
#[derive(Debug)]
pub struct LostBssCounter {
    /// beacon_period in zx::MonotonicDuration as obtained from the AP, used to convert beacon_count to time.
    beacon_period: zx::MonotonicDuration,

    /// The number of beacon periods where client doesn't receive a single beacon frame
    /// before it declares BSS as lost.
    full_timeout: zx::MonotonicDuration,

    /// Number of intervals since we last saw a beacon. Reset to 0 as soon as we see a beacon.
    time_since_last_beacon: zx::MonotonicDuration,
}

/// In a typical use case, a full association status check interval is added every time the timeout
/// fires. This could lead to slight over-counting since the client may have received a beacon
/// during this period. To counter this effect, call should_deauthenticate() before calling
/// add_beacon_interval().
impl LostBssCounter {
    pub fn start(beacon_period: zx::MonotonicDuration, full_timeout_beacon_count: u32) -> Self {
        Self {
            beacon_period: beacon_period.clone(),
            full_timeout: beacon_period * full_timeout_beacon_count as i64,
            time_since_last_beacon: zx::MonotonicDuration::from_nanos(0),
        }
    }

    pub fn reset(&mut self) {
        self.time_since_last_beacon = zx::MonotonicDuration::from_nanos(0);
    }

    /// In the most typical use case, a full association status check interval is added when
    /// the timeout fires. So to prevent auto-deauth from triggering prematurely, it is important to
    /// call `should_deauthenticate()` first and only call `add_beacon_interval()`
    /// if `should_deauthenticate()` is false.
    pub fn should_deauthenticate(&self) -> bool {
        self.time_since_last_beacon >= self.full_timeout
    }

    pub fn add_beacon_interval(&mut self, beacon_intervals_since_last_timeout: u32) {
        self.time_since_last_beacon += self.beacon_period * beacon_intervals_since_last_timeout;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wlan_common::time::TimeUnit;

    const TEST_BEACON_PERIOD: zx::MonotonicDuration =
        zx::MonotonicDuration::from_micros(TimeUnit(42).into_micros());
    const TEST_TIMEOUT_BCN_COUNT: u32 = 1000;

    #[test]
    fn test_single_uninterrupted_period() {
        let mut counter = LostBssCounter::start(TEST_BEACON_PERIOD, TEST_TIMEOUT_BCN_COUNT);
        // about to timeout but not yet.
        counter.add_beacon_interval(TEST_TIMEOUT_BCN_COUNT - 1);
        assert!(!counter.should_deauthenticate());
        // any more time will trigger auto deauth
        counter.add_beacon_interval(1);
        assert!(counter.should_deauthenticate());
    }

    #[test]
    fn test_beacon_received_midway() {
        let mut counter = LostBssCounter::start(TEST_BEACON_PERIOD, TEST_TIMEOUT_BCN_COUNT);
        counter.add_beacon_interval(TEST_TIMEOUT_BCN_COUNT - 1);
        assert!(!counter.should_deauthenticate());

        // Beacon received some time later, resetting the timeout.
        counter.reset();

        // Verify that calling `handle_timeout` at originally scheduled time would not
        // return false, indicating no auto-deauth yet
        counter.add_beacon_interval(1);
        assert!(!counter.should_deauthenticate());
        // But if no beacon is received in timeout + 1 intervals, auto-deauth will trigger
        counter.add_beacon_interval(TEST_TIMEOUT_BCN_COUNT - 1);
        assert!(counter.should_deauthenticate());
    }
}
