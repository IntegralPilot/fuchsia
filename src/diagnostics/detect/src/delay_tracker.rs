// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

/// DelayTracker keeps track of how recently we sent a crash report of each type, and whether we
/// should mute too-frequent requests.
use {
    crate::{MINIMUM_SIGNATURE_INTERVAL_NANOS, Mode},
    fuchsia_triage::SnapshotTrigger,
    injectable_time::TimeSource,
    log::warn,
    std::collections::HashMap,
    std::sync::Arc,
};

pub struct DelayTracker {
    last_sent: HashMap<String, i64>,
    time_source: Arc<dyn TimeSource + Send + Sync>,
    program_mode: Mode,
}

impl DelayTracker {
    pub(crate) fn new(
        time_source: Arc<dyn TimeSource + Send + Sync>,
        program_mode: Mode,
    ) -> DelayTracker {
        DelayTracker { last_sent: HashMap::new(), time_source, program_mode }
    }

    fn appropriate_report_interval(&self, desired_interval: i64) -> i64 {
        if self.program_mode == Mode::IntegrationTest
            || desired_interval >= MINIMUM_SIGNATURE_INTERVAL_NANOS
        {
            desired_interval
        } else {
            MINIMUM_SIGNATURE_INTERVAL_NANOS
        }
    }

    // If it's OK to send, remember the time and return true.
    pub(crate) fn ok_to_send(&mut self, snapshot: &SnapshotTrigger) -> bool {
        let now = self.time_source.now();
        let interval = self.appropriate_report_interval(snapshot.interval);
        let should_send = match self.last_sent.get(&snapshot.signature) {
            None => true,
            Some(time) => time <= &(now - interval),
        };
        if should_send {
            self.last_sent.insert(snapshot.signature.to_string(), now);
            if snapshot.interval < MINIMUM_SIGNATURE_INTERVAL_NANOS {
                // To reduce logspam, put this warning here rather than above where the
                // calculation is. The calculation may happen every time we check diagnostics; this
                // will happen at most every MINIMUM_SIGNATURE_INTERVAL (except in tests).
                warn!(
                    "Signature {} has interval {} nanos, less than minimum {}",
                    snapshot.signature, snapshot.interval, MINIMUM_SIGNATURE_INTERVAL_NANOS
                );
            }
        }
        should_send
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use injectable_time::FakeTime;
    use static_assertions::const_assert;

    #[fuchsia::test]
    fn verify_test_mode() {
        let time = Arc::new(FakeTime::new());
        let mut tracker = DelayTracker::new(time.clone(), Mode::IntegrationTest);
        time.set_ticks(1);
        let trigger_slow = SnapshotTrigger { signature: "slow".to_string(), interval: 10 };
        let trigger_fast = SnapshotTrigger { signature: "fast".to_string(), interval: 1 };
        let ok_slow_1 = tracker.ok_to_send(&trigger_slow);
        let ok_fast_1 = tracker.ok_to_send(&trigger_fast);
        time.set_ticks(3);
        let ok_slow_2 = tracker.ok_to_send(&trigger_slow);
        let ok_fast_2 = tracker.ok_to_send(&trigger_fast);
        // This one should obviously succeed.
        assert!(ok_slow_1);
        // It should allow a different snapshot signature too.
        assert!(ok_fast_1);
        // It should reject the first (slow) signature the second time.
        assert!(!ok_slow_2);
        // The second (fast) signature should be accepted repeatedly.
        assert!(ok_fast_2);
    }

    #[fuchsia::test]
    fn verify_appropriate_report_interval() {
        const_assert!(MINIMUM_SIGNATURE_INTERVAL_NANOS > 1);
        let time = Arc::new(FakeTime::new());
        let test_tracker = DelayTracker::new(time.clone(), Mode::IntegrationTest);
        let production_tracker = DelayTracker::new(time, Mode::Production);

        assert_eq!(test_tracker.appropriate_report_interval(1), 1);
        assert_eq!(
            production_tracker.appropriate_report_interval(1),
            MINIMUM_SIGNATURE_INTERVAL_NANOS
        );
    }
}
