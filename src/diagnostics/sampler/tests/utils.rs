// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_metrics::{MetricEvent, MetricEventPayload};
use fidl_fuchsia_metrics_test::{LogMethod, MetricEventLoggerQuerierProxy};

/// An expected event, to be validated against a MetricEvent logged by fake-Cobalt
#[derive(Debug)]
pub struct Event {
    /// Metric ID
    pub id: u32,
    /// Value read from Inspect
    pub value: i64,
    /// Event code list
    pub codes: Vec<u32>,
}

impl PartialEq<MetricEvent> for Event {
    fn eq(&self, other: &MetricEvent) -> bool {
        let payload_value = match other.payload {
            MetricEventPayload::Count(event_count) => event_count as i64,
            MetricEventPayload::IntegerValue(value) => value,
            _ => panic!("Only should be observing Occurrence or Integer; got {other:?}"),
        };
        other.metric_id == self.id && payload_value == self.value && other.event_codes == self.codes
    }
}

impl PartialEq<Event> for MetricEvent {
    fn eq(&self, other: &Event) -> bool {
        other == self
    }
}

#[derive(Debug)]
pub struct EventVerifier<'a> {
    project_id: u32,
    events_received: Vec<MetricEvent>,
    logger_querier: &'a MetricEventLoggerQuerierProxy,
}

impl<'a> EventVerifier<'a> {
    pub fn new(logger_querier: &'a MetricEventLoggerQuerierProxy, project_id: u32) -> Self {
        Self { project_id, events_received: vec![], logger_querier }
    }

    /// Make sure all expected events were received. Remember any extra events for future validation.
    pub async fn validate(&mut self, events: Vec<Event>, message: &str) {
        while self.events_received.len() < events.len() {
            let (mut new_events, _) = self
                .logger_querier
                .watch_logs(self.project_id, LogMethod::LogMetricEvents)
                .await
                .unwrap();
            self.events_received.append(&mut new_events);
        }
        let actual_events = self.events_received[..events.len()].to_vec();
        self.events_received = self.events_received[events.len()..].to_vec();
        for event in events {
            let event_occurrences = Self::event_occurrences(&actual_events, &event);
            assert_eq!(
                event_occurrences, 1,
                "In {message}, event {event:?} {0} {actual_events:?}",
                "should have been present once in",
            );
        }
    }

    /// Validate, and make sure no extra events were received.
    pub async fn validate_with_count(&mut self, events: Vec<Event>, message: &str) {
        self.validate(events, message).await;
        assert!(
            self.events_received.is_empty(),
            "In {message}, extra events were received: {:?}",
            self.events_received
        );
    }

    fn event_occurrences(events: &Vec<MetricEvent>, expected_event: &Event) -> usize {
        let mut observed_count = 0;
        for event in events {
            if event == expected_event {
                observed_count += 1;
            }
        }
        observed_count
    }
}
