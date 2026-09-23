// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::collections::VecDeque;

/// The intended sampling rate. In practice, variations due to scheduling jitter are expected.
pub const SAMPLING_RATE: zx::MonotonicDuration = zx::Duration::from_millis(500);

const MAX_RETENTION: zx::MonotonicDuration = zx::Duration::from_seconds(300);

struct Sample {
    some_value: zx::MonotonicDuration,
    full_value: zx::MonotonicDuration,
    timestamp: zx::MonotonicInstant,
}

/// Stores the latest samples in monotonically-incresing timestamp order, covering up to
/// `MAX_RETENTION`.
pub struct History {
    data: VecDeque<Sample>,
}

impl History {
    pub fn new(
        initial_some_value: zx::MonotonicDuration,
        initial_full_value: zx::MonotonicDuration,
        initial_timestamp: zx::MonotonicInstant,
    ) -> History {
        let mut data = VecDeque::from([Sample {
            some_value: initial_some_value,
            full_value: initial_full_value,
            timestamp: initial_timestamp,
        }]);

        // Hint about how big the queue might become.
        data.reserve(
            (MAX_RETENTION.into_seconds_f64() / SAMPLING_RATE.into_seconds_f64()).ceil() as usize
        );

        History { data }
    }

    /// Inserts a new sample at the end of the queue, then drops elements older than it by more than
    /// the maximum retention time.
    pub fn add_new_sample(
        &mut self,
        some_value: zx::MonotonicDuration,
        full_value: zx::MonotonicDuration,
        timestamp: zx::MonotonicInstant,
    ) {
        // Elements must be inserted monotonically.
        assert!(timestamp > self.data.back().unwrap().timestamp);

        self.data.push_back(Sample { some_value, full_value, timestamp });

        // Cleanup old samples.
        while self.data.front().unwrap().timestamp + MAX_RETENTION < timestamp {
            self.data.pop_front();
        }
    }

    /// Gets the `some` and `full` values at a given point in time, interpolating if necessary.
    pub fn query_at(
        &self,
        query_timestamp: zx::MonotonicInstant,
    ) -> (zx::MonotonicDuration, zx::MonotonicDuration) {
        match self.data.binary_search_by_key(&query_timestamp, |s| s.timestamp) {
            // Exact match.
            Ok(index) => {
                let sample = &self.data[index];
                (sample.some_value, sample.full_value)
            }

            // The queried timestamp lies before the beginning of the history. Let's return the
            // first element.
            Err(0) => {
                let sample = &self.data[0];
                (sample.some_value, sample.full_value)
            }

            // The queried timestamp lies after the end of the history. Let's return the last
            // element.
            Err(index) if index == self.data.len() => {
                let sample = &self.data[self.data.len() - 1];
                (sample.some_value, sample.full_value)
            }

            // The queried timestamp lies between two elements. Let's interpolate.
            Err(index_after) => {
                let before = &self.data[index_after - 1];
                let after = &self.data[index_after];
                let k = (query_timestamp - before.timestamp).into_seconds_f64()
                    / (after.timestamp - before.timestamp).into_seconds_f64();

                let some = before.some_value
                    + zx::MonotonicDuration::from_nanos(
                        ((after.some_value - before.some_value).into_nanos() as f64 * k) as i64,
                    );
                let full = before.full_value
                    + zx::MonotonicDuration::from_nanos(
                        ((after.full_value - before.full_value).into_nanos() as f64 * k) as i64,
                    );

                (
                    some.clamp(before.some_value, after.some_value),
                    full.clamp(before.full_value, after.full_value),
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_query() {
        let mut history = History::new(
            zx::Duration::from_nanos(1000),
            zx::Duration::from_nanos(500),
            zx::Instant::from_nanos(1000000000),
        );
        history.add_new_sample(
            zx::Duration::from_nanos(2000),
            zx::Duration::from_nanos(1000),
            zx::Instant::from_nanos(1500000000),
        );
        history.add_new_sample(
            zx::Duration::from_nanos(2000),
            zx::Duration::from_nanos(1000),
            zx::Instant::from_nanos(2000000000),
        );
        history.add_new_sample(
            zx::Duration::from_nanos(3000),
            zx::Duration::from_nanos(1500),
            zx::Instant::from_nanos(2500000000),
        );

        // Verify that querying before the beginning of the history returns the first values.
        assert_eq!(
            history.query_at(zx::Instant::from_nanos(0)),
            (zx::Duration::from_nanos(1000), zx::Duration::from_nanos(500))
        );

        // Verify that querying after the end of the history returns the last values.
        assert_eq!(
            history.query_at(zx::Instant::from_nanos(3000000000)),
            (zx::Duration::from_nanos(3000), zx::Duration::from_nanos(1500))
        );

        // Verify querying timestamps that correspond exactly to elements in the queue.
        assert_eq!(
            history.query_at(zx::Instant::from_nanos(1000000000)),
            (zx::Duration::from_nanos(1000), zx::Duration::from_nanos(500))
        );
        assert_eq!(
            history.query_at(zx::Instant::from_nanos(1500000000)),
            (zx::Duration::from_nanos(2000), zx::Duration::from_nanos(1000))
        );
        assert_eq!(
            history.query_at(zx::Instant::from_nanos(2000000000)),
            (zx::Duration::from_nanos(2000), zx::Duration::from_nanos(1000))
        );
        assert_eq!(
            history.query_at(zx::Instant::from_nanos(2500000000)),
            (zx::Duration::from_nanos(3000), zx::Duration::from_nanos(1500))
        );

        // Verify that queries whose timestamps lie between elements return interpolated values.
        assert_eq!(
            history.query_at(zx::Instant::from_nanos(1100000000)),
            (zx::Duration::from_nanos(1200), zx::Duration::from_nanos(600))
        );
    }

    #[test]
    fn test_cleanup() {
        // Verify that, as long as we stay below the `MAX_RETENTION`, no elements are dropped.
        const INITIAL_TIME: zx::MonotonicInstant = zx::Instant::from_nanos(1000000000);
        let mut history = History::new(
            zx::Duration::from_nanos(1000),
            zx::Duration::from_nanos(500),
            INITIAL_TIME,
        );
        let mut now = INITIAL_TIME;
        let mut total_inserted_elements = 1; // The ctor already inserted one.
        loop {
            now += zx::Duration::from_nanos(1000000000);
            if now <= INITIAL_TIME + MAX_RETENTION {
                history.add_new_sample(
                    zx::Duration::from_nanos(1000),
                    zx::Duration::from_nanos(500),
                    now,
                );
                total_inserted_elements += 1;
            } else {
                break;
            }
        }
        assert_eq!(history.data.len(), total_inserted_elements);

        // Verify that, if we insert one more element, an old element is dropped (or, in other
        // words, the length of the queue does not change).
        history.add_new_sample(zx::Duration::from_nanos(1000), zx::Duration::from_nanos(500), now);
        assert_eq!(history.data.len(), total_inserted_elements);
    }
}
