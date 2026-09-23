// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use log::warn;
use std::collections::VecDeque;

/// Trait for time function, for use in HistoricalList functions
pub trait Timestamped {
    fn time(&self) -> fuchsia_async::MonotonicInstant;
}

/// Struct for list that stores historical data in a VecDeque, up to the some number of most
/// recent entries.
#[derive(Clone, Debug, PartialEq)]
pub struct HistoricalList<T: Timestamped, const N: usize>(pub VecDeque<T>);

impl<T, const N: usize> HistoricalList<T, N>
where
    T: Timestamped + Clone,
{
    pub fn new() -> Self {
        Self(VecDeque::with_capacity(N))
    }

    /// Add a new entry, purging the oldest if at capacity. Entry must be newer than the most recent
    /// existing entry.
    pub fn add(&mut self, historical_data: T) {
        if let Some(newest) = self.0.back()
            && historical_data.time() < newest.time()
        {
            warn!("HistoricalList entry must be newer than existing elements.");
            return;
        }
        if self.0.len() == N {
            let _ = self.0.pop_front();
        }
        self.0.push_back(historical_data);
    }

    #[allow(clippy::needless_return, reason = "mass allow for https://fxbug.dev/381896734")]
    /// Retrieve list of entries with a time more recent than earliest_time, sorted from oldest to
    /// newest. May be empty.
    pub fn get_recent(&self, earliest_time: fuchsia_async::MonotonicInstant) -> Vec<T> {
        let i = self.0.partition_point(|data| data.time() < earliest_time);
        return self.0.iter().skip(i).cloned().collect();
    }

    #[allow(clippy::needless_return, reason = "mass allow for https://fxbug.dev/381896734")]
    /// Retrieve list of entries with a time before than latest_time, sorted from oldest to
    /// newest. May be empty.
    pub fn get_before(&self, latest_time: fuchsia_async::MonotonicInstant) -> Vec<T> {
        let i = self.0.partition_point(|data| data.time() <= latest_time);
        return self.0.iter().take(i).cloned().collect();
    }

    /// Retrieve list of entries inclusively between latest_time and earliest_time, sorted from
    /// oldest to newest. May be empty.
    pub fn get_between(
        &self,
        earliest_time: fuchsia_async::MonotonicInstant,
        latest_time: fuchsia_async::MonotonicInstant,
    ) -> Vec<T> {
        let i = self.0.partition_point(|data| data.time() < earliest_time);
        let j = self.0.partition_point(|data| data.time() <= latest_time);
        match j.checked_sub(i) {
            Some(diff) => self.0.iter().skip(i).take(diff).cloned().collect(),
            _ => {
                warn!(
                    "Invalid time bounds - earliest time: {:?}, latest time: {:?}",
                    earliest_time, latest_time
                );
                vec![]
            }
        }
    }
}

impl<T, const N: usize> Default for HistoricalList<T, N>
where
    T: Timestamped + Clone,
{
    fn default() -> Self {
        Self::new()
    }
}

// Allow for storing just timestamps
impl Timestamped for zx::MonotonicInstant {
    fn time(&self) -> fuchsia_async::MonotonicInstant {
        fuchsia_async::MonotonicInstant::from_zx(*self)
    }
}
impl Timestamped for fuchsia_async::MonotonicInstant {
    fn time(&self) -> fuchsia_async::MonotonicInstant {
        *self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zx::MonotonicDuration;

    const EARLIEST_TIME: fuchsia_async::MonotonicInstant =
        fuchsia_async::MonotonicInstant::from_nanos(1_000_000_000);
    fn create_test_list(
        earlist_time: fuchsia_async::MonotonicInstant,
    ) -> HistoricalList<fuchsia_async::MonotonicInstant, 4> {
        HistoricalList(VecDeque::from_iter([
            earlist_time,
            earlist_time + MonotonicDuration::from_seconds(1),
            earlist_time + MonotonicDuration::from_seconds(3),
            earlist_time + MonotonicDuration::from_seconds(5),
        ]))
    }

    #[fuchsia::test]
    fn test_historical_list_capacity() {
        let mut historical_list = create_test_list(EARLIEST_TIME);

        // Verify that the list did not exceed capacity, and purged the oldest entry.
        historical_list.add(EARLIEST_TIME + MonotonicDuration::from_seconds(10));
        assert_eq!(historical_list.0.len(), 4);
        assert!(!historical_list.0.contains(&EARLIEST_TIME));

        // Verify that you cannot add a value older than the most recent entry.
        let t = historical_list.clone();
        historical_list.add(EARLIEST_TIME);
        assert_eq!(historical_list, t);
    }

    #[fuchsia::test]
    fn test_get_recent() {
        let historical_list = create_test_list(EARLIEST_TIME);

        assert_eq!(
            historical_list.get_recent(EARLIEST_TIME + MonotonicDuration::from_seconds(1)),
            vec![
                EARLIEST_TIME + MonotonicDuration::from_seconds(1),
                EARLIEST_TIME + MonotonicDuration::from_seconds(3),
                EARLIEST_TIME + MonotonicDuration::from_seconds(5)
            ]
        );

        assert_eq!(
            historical_list.get_recent(
                EARLIEST_TIME
                    + MonotonicDuration::from_seconds(5)
                    + MonotonicDuration::from_millis(1)
            ),
            vec![]
        );
    }

    #[fuchsia::test]
    fn test_get_before() {
        let historical_list = create_test_list(EARLIEST_TIME);

        assert_eq!(
            historical_list.get_before(EARLIEST_TIME + MonotonicDuration::from_seconds(3)),
            vec![
                EARLIEST_TIME,
                EARLIEST_TIME + MonotonicDuration::from_seconds(1),
                EARLIEST_TIME + MonotonicDuration::from_seconds(3),
            ]
        );

        assert_eq!(
            historical_list.get_before(EARLIEST_TIME - MonotonicDuration::from_millis(1)),
            vec![]
        );
    }

    #[fuchsia::test]
    fn test_get_between() {
        let historical_list = create_test_list(EARLIEST_TIME);

        assert_eq!(
            historical_list.get_between(
                EARLIEST_TIME + MonotonicDuration::from_seconds(1),
                EARLIEST_TIME + MonotonicDuration::from_seconds(5)
            ),
            vec![
                EARLIEST_TIME + MonotonicDuration::from_seconds(1),
                EARLIEST_TIME + MonotonicDuration::from_seconds(3),
                EARLIEST_TIME + MonotonicDuration::from_seconds(5),
            ]
        );

        assert_eq!(historical_list.get_between(EARLIEST_TIME, EARLIEST_TIME), vec![EARLIEST_TIME]);

        assert_eq!(
            historical_list.get_between(
                EARLIEST_TIME + MonotonicDuration::from_seconds(10),
                EARLIEST_TIME + MonotonicDuration::from_seconds(10)
            ),
            vec![]
        );

        assert_eq!(
            historical_list.get_between(
                EARLIEST_TIME + MonotonicDuration::from_seconds(10),
                EARLIEST_TIME + MonotonicDuration::from_seconds(11)
            ),
            vec![]
        );

        // Verify that an empty list is returned for equal and invalid time bounds.
        assert_eq!(
            historical_list
                .get_between(EARLIEST_TIME + MonotonicDuration::from_seconds(3), EARLIEST_TIME),
            vec![]
        );
    }
}
