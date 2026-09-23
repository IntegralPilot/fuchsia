// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

/**
This program integration-tests the triage-detect program using the OpaqueTest library
to inject a fake CrashReporter, ArchiveAccessor, and config-file directory.

triage-detect will be able to fetch Diagnostic data, evaluate it according to the .triage
files it finds, and request whatever crash reports are appropriate. Meanwhile, the fakes
will be writing TestEvent entries to a stream for later evaluation.

Each integration test is stored in a file whose name starts with "test". This supplies:

1) Some number of Diagnostic data strings. When the program tries to fetch Diagnostic data
   after these strings are exhausted, the fake ArchiveAccessor writes to a special "done" channel
   to terminate the test.

2) Some number of config files to place in the fake directory.

3) A vector of vectors of crash report signatures. The inner vector should match the crash
   report requests sent between each fetch of Diagnostic data. Order of the inner vector does
   not matter, but duplicates do matter.
*/
use anyhow::Error;
use fidl::endpoints;
// fidl_fuchsia_testing is unrelated to fidl_fuchsia_testing_harness
use fidl_fuchsia_testing::{self as fake_clock_fidl, Increment};
use fidl_fuchsia_testing_harness::{self as ffth, RealmProxy_Marker};
use fidl_test_detect_factory as ftest;
use fuchsia_component::client::connect_to_protocol;
use futures::StreamExt;
use log::*;
use realm_proxy_client::Error::OperationError;
use std::cmp::Ordering;
use std::time::Duration;
use test_case::test_case;

// Test that the "repeat" field of snapshots works correctly.
mod test_snapshot_throttle;
// Test that the "trigger" field of snapshots works correctly.
mod test_trigger_truth;
// Test that no report is filed unless "config.json" has the magic contents.
mod test_filing_enable;
// Test that all characters other than [a-z-] are converted to that set.
mod test_snapshot_sanitizing;

#[test_case(test_snapshot_throttle::test())]
#[test_case(test_trigger_truth::test())]
#[test_case(test_snapshot_sanitizing::test())]
#[test_case(test_filing_enable::test_with_enable())]
#[test_case(test_filing_enable::test_bad_enable())]
#[test_case(test_filing_enable::test_false_enable())]
#[test_case(test_filing_enable::test_no_enable())]
#[test_case(test_filing_enable::test_without_file())]
#[fuchsia::test]
async fn triage_detect_test(test_data: TestData) -> Result<(), Error> {
    info!("running test case {}", test_data.name);
    let realm_factory = connect_to_protocol::<ftest::RealmFactoryMarker>()?;
    let (realm_proxy, realm_server) = endpoints::create_proxy::<RealmProxy_Marker>();
    realm_factory
        .create_realm(test_data.realm_options, realm_server)
        .await?
        .map_err(OperationError)?;
    let (_proxy, server_end) = endpoints::create_proxy::<fidl_fuchsia_component::BinderMarker>();
    let _result = realm_proxy
        .connect_to_named_protocol("fuchsia.component.DetectBinder", server_end.into_channel())
        .await
        .expect("connection failed");

    let event_proxy = realm_factory.get_triage_detect_events().await?.into_proxy();
    let mut event_stream = event_proxy.take_event_stream();
    let fake_clock_control_proxy =
        connect_into_realm::<fake_clock_fidl::FakeClockControlMarker>(&realm_proxy).await;
    // If there's only one group of EventsThenAdvance, then this test does not depend on
    // clock control, but it might depend on a running clock, so leave the clock free-running.
    let controlled_clock_advance = if test_data.expected_events.len() == 1 {
        false
    } else {
        fake_clock_control_proxy.pause().await.expect("Clock pause");
        true
    };
    for EventsThenAdvance { events, advance } in test_data.expected_events {
        let mut expected_events = events;
        let mut actual_events = get_n_events(&mut event_stream, expected_events.len()).await;
        actual_events.sort_by(compare_crash_signatures_only);
        expected_events.sort_by(compare_crash_signatures_only);
        assert_events_eq(&expected_events, &actual_events);
        if controlled_clock_advance {
            fake_clock_control_proxy
                .advance(&Increment::Determined(advance.as_nanos() as i64))
                .await
                .expect("Clock advance")
                .expect("Clock advance args");
        }
    }
    Ok(())
}

// May return fewer than requested if OnDone or OnBail is received early, or fail an
// assert if the stream ends before N events are received.
async fn get_n_events(
    stream: &mut ftest::TriageDetectEventsEventStream,
    n: usize,
) -> Vec<TestEvent> {
    let mut events = vec![];
    for event_index in 0..n {
        if let Some(Ok(event)) = stream.next().await {
            let event = TestEvent::from(event);
            match event {
                TestEvent::Done | TestEvent::Bail => {
                    // Record the event so tests can assert whether we bailed early.
                    events.push(event);
                    return events;
                }
                _ => events.push(event),
            };
        } else {
            // This assert will fail if the stream runs out too soon
            assert_eq!(
                n, event_index,
                "Stream emptied too soon: count expected vs. index where it failed"
            );
        }
    }
    events
}

fn assert_events_eq(expected: &[TestEvent], actual: &[TestEvent]) {
    if let Some(index) = find_first_different_index(expected, actual) {
        panic!(
            "\n\n\
             Wanted events: {:?}\n\
             Got events:    {:?}\n\
             Which differ at index {}:\n\
             * Want: {:?}\n\
             * Got:  {:?}\n\
            \n\n",
            expected, actual, index, expected[index], actual[index],
        );
    }
}

fn find_first_different_index(left: &[TestEvent], right: &[TestEvent]) -> Option<usize> {
    match left.iter().zip(right.iter()).position(|(l, r)| l != r) {
        Some(index) => Some(index),
        None => {
            if left.len() == right.len() {
                return None;
            }
            Some(std::cmp::min(left.len(), right.len()) - 1)
        }
    }
}

// A comparator used to sort test events by crash signature.
// Subgroups of crash reports that occur between non-crash-reports (such as diagnostics fetches)
//   are sorted internally, but the list is not sorted between crash-report subgroups if
//   the comparator is used in a stable sort.
// For example: the events
// {C, A, FETCH, D, B, FETCH, G} are sorted as:
// {A, C, FETCH, B, D, FETCH, G}.
fn compare_crash_signatures_only(prev: &TestEvent, next: &TestEvent) -> Ordering {
    if let TestEvent::CrashReport { crash_signature, .. } = prev {
        let left = crash_signature;
        if let TestEvent::CrashReport { crash_signature, .. } = next {
            let right = crash_signature;
            return left.partial_cmp(right).unwrap();
        }
    }

    Ordering::Equal
}

pub(crate) fn create_vmo(contents: impl Into<String>) -> zx::Vmo {
    let contents = contents.into();
    let vmo = zx::Vmo::create(contents.len() as u64).unwrap();
    vmo.write(contents.as_bytes(), 0).unwrap();
    vmo
}

pub(crate) struct EventsThenAdvance {
    events: Vec<TestEvent>,
    advance: Duration,
}

pub(crate) struct TestData {
    name: String,
    realm_options: ftest::RealmOptions,
    expected_events: Vec<EventsThenAdvance>,
}

impl TestData {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            realm_options: ftest::RealmOptions { ..Default::default() },
            expected_events: vec![],
        }
    }

    pub fn add_triage_config(mut self, config_js: impl Into<String>) -> Self {
        let triage_configs = self.realm_options.triage_configs.get_or_insert_with(Vec::new);
        triage_configs.push(create_vmo(config_js));
        self
    }

    pub fn add_inspect_data(mut self, inspect_data_js: impl Into<String>) -> Self {
        let inspect_data = self.realm_options.inspect_data.get_or_insert_with(Vec::new);
        inspect_data.push(create_vmo(inspect_data_js));
        self
    }

    pub fn set_program_config(mut self, program_config_js: impl Into<String>) -> Self {
        self.realm_options.program_config.replace(create_vmo(program_config_js));
        self
    }

    pub fn expect_events(mut self, events: Vec<TestEvent>) -> Self {
        self.expected_events.push(EventsThenAdvance { events, advance: Duration::from_nanos(0) });
        self
    }

    pub fn events_then_advance(mut self, events: Vec<TestEvent>, advance: Duration) -> Self {
        self.expected_events.push(EventsThenAdvance { events, advance });
        self
    }
}

// A TriageDetectEventsEvent representation that allows us to compare events.
#[derive(Debug, PartialEq)]
pub(crate) enum TestEvent {
    Bail,
    DiagnosticFetch,
    Done,
    CrashReport { crash_signature: String, crash_program_name: String },
    CrashReportingProductRegistration { product_name: String, program_name: String },
}

impl From<ftest::TriageDetectEventsEvent> for TestEvent {
    fn from(event: ftest::TriageDetectEventsEvent) -> Self {
        match event {
            ftest::TriageDetectEventsEvent::OnDone {} => TestEvent::Done,
            ftest::TriageDetectEventsEvent::OnBail {} => TestEvent::Bail,
            ftest::TriageDetectEventsEvent::OnDiagnosticFetch {} => TestEvent::DiagnosticFetch,
            ftest::TriageDetectEventsEvent::OnCrashReport {
                crash_signature,
                crash_program_name,
            } => TestEvent::CrashReport { crash_signature, crash_program_name },
            ftest::TriageDetectEventsEvent::OnCrashReportingProductRegistration {
                product_name,
                program_name,
            } => TestEvent::CrashReportingProductRegistration { product_name, program_name },
            _ => panic!("unknown event {event:?}"),
        }
    }
}

// This next function is copied from
// src/sys/time/timekeeper_integration/tests/faketime/integration.rs
// It should probably be part of the TRF affordances.

/// Connect to the FIDL protocol defined by the marker `T`, which served from
/// the realm associated with the realm proxy `p`. Since `ConnectToNamedProtocol`
/// is "stringly typed", this function allows us to use a less error prone protocol
/// marker instead of a string name.
async fn connect_into_realm<T>(
    realm_proxy: &ffth::RealmProxy_Proxy,
) -> <T as endpoints::ProtocolMarker>::Proxy
where
    T: endpoints::ProtocolMarker,
{
    let (proxy, server_end) = endpoints::create_proxy::<T>();
    let _result = realm_proxy
        .connect_to_named_protocol(
            <T as fidl::endpoints::ProtocolMarker>::DEBUG_NAME,
            server_end.into_channel(),
        )
        .await
        .expect("connection failed");
    proxy
}
