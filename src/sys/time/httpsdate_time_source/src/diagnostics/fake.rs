// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::datatypes::{HttpsSample, Phase};
use crate::diagnostics::{Diagnostics, Event};
use fuchsia_runtime::{UtcDuration, UtcInstant};
use fuchsia_sync::Mutex;
use httpdate_hyper::HttpsDateErrorType;

/// A fake `Diagnostics` implementation useful for verifying unittests.
pub struct FakeDiagnostics {
    /// An ordered list of the events received since the last reset.
    events: Mutex<Vec<OwnedEvent>>,
}

/// A copy of `Event` where all contents are owned.
#[derive(PartialEq, Debug)]
enum OwnedEvent {
    NetworkCheckSuccessful,
    Success(HttpsSample),
    Failure(HttpsDateErrorType),
    Phase(Phase),
}

impl<'a> From<Event<'a>> for OwnedEvent {
    fn from(event: Event<'a>) -> Self {
        match event {
            Event::NetworkCheckSuccessful => Self::NetworkCheckSuccessful,
            Event::Success(sample) => Self::Success(sample.clone()),
            Event::Failure(error) => Self::Failure(error),
            Event::Phase(phase) => Self::Phase(phase),
        }
    }
}

impl FakeDiagnostics {
    /// Constructs a new `FakeDiagnostics`.
    pub fn new() -> Self {
        FakeDiagnostics { events: Mutex::new(Vec::new()) }
    }

    /// Assert that the recorded events equals the provided events.
    pub fn assert_events<'a, I: IntoIterator<Item = Event<'a>>>(&self, events: I) {
        let owned_events =
            events.into_iter().map(|event| OwnedEvent::from(event)).collect::<Vec<_>>();
        assert_eq!(owned_events, *self.events.lock());
    }

    /// Assert that the recorded events starts with the provided events.
    pub fn assert_events_starts_with<'a, I: IntoIterator<Item = Event<'a>>>(&self, events: I) {
        let expected_events =
            events.into_iter().map(|event| OwnedEvent::from(event)).collect::<Vec<_>>();
        let actual_events = self.events.lock();
        assert!(actual_events.len() >= expected_events.len());
        assert_eq!(expected_events.as_slice(), &actual_events[..expected_events.len()]);
    }

    /// Clears all recorded interactions.
    pub fn reset(&self) {
        self.events.lock().clear();
    }
}

impl Diagnostics for FakeDiagnostics {
    fn record<'a>(&self, event: Event<'a>) {
        self.events.lock().push(event.into());
    }
}

impl<T: AsRef<FakeDiagnostics> + Send + Sync> Diagnostics for T {
    fn record<'a>(&self, event: Event<'a>) {
        self.as_ref().record(event);
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::datatypes::Poll;
    use std::sync::LazyLock;

    static TEST_SAMPLE: LazyLock<HttpsSample> = LazyLock::new(|| HttpsSample {
        utc: UtcInstant::from_nanos(111_111_111),
        reference: zx::BootInstant::from_nanos(222_222_222),
        standard_deviation: UtcDuration::from_millis(235),
        final_bound_size: UtcDuration::from_millis(100),
        polls: vec![Poll { round_trip_time: zx::BootDuration::from_nanos(23) }],
    });
    static TEST_SUCCESS: LazyLock<Event<'static>> = LazyLock::new(|| Event::Success(&*TEST_SAMPLE));
    static TEST_SAMPLE_2: LazyLock<HttpsSample> = LazyLock::new(|| {
        let mut new = TEST_SAMPLE.clone();
        new.polls = vec![];
        new
    });
    static TEST_SUCCESS_2: LazyLock<Event<'static>> =
        LazyLock::new(|| Event::Success(&*TEST_SAMPLE_2));
    const TEST_FAILURE: Event<'static> = Event::Failure(HttpsDateErrorType::NetworkError);
    const TEST_PHASE: Event<'static> = Event::Phase(Phase::Converge);

    #[fuchsia::test]
    fn log_and_reset_events() {
        let diagnostics = FakeDiagnostics::new();
        diagnostics.assert_events(vec![]);

        diagnostics.record(*TEST_SUCCESS);
        diagnostics.assert_events(vec![*TEST_SUCCESS]);

        diagnostics.record(TEST_FAILURE);
        diagnostics.record(TEST_PHASE);
        diagnostics.assert_events(vec![*TEST_SUCCESS, TEST_FAILURE, TEST_PHASE]);

        diagnostics.reset();
        diagnostics.assert_events(vec![]);
    }

    #[test]
    #[should_panic]
    fn log_events_wrong_event_type() {
        let diagnostics = FakeDiagnostics::new();
        diagnostics.assert_events(vec![]);

        diagnostics.record(*TEST_SUCCESS);
        diagnostics.assert_events(vec![TEST_FAILURE]);
    }

    #[test]
    #[should_panic]
    fn log_events_wrong_sample() {
        let diagnostics = FakeDiagnostics::new();
        diagnostics.assert_events(vec![]);

        diagnostics.record(*TEST_SUCCESS);
        diagnostics.assert_events(vec![*TEST_SUCCESS_2]);
    }

    #[test]
    #[should_panic]
    fn log_events_wrong_event_count() {
        let diagnostics = FakeDiagnostics::new();
        diagnostics.assert_events(vec![]);

        diagnostics.record(*TEST_SUCCESS);
        diagnostics.assert_events(vec![*TEST_SUCCESS, *TEST_SUCCESS]);
    }
}
