// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use diagnostics_assertions::{AnyProperty, assert_data_tree};
use diagnostics_reader::ArchiveReader;
use fidl_fuchsia_component::BinderMarker;
use fidl_fuchsia_diagnostics as fdiagnostics;
use fidl_fuchsia_metrics_test::MetricEventLoggerQuerierMarker;
use fidl_fuchsia_mockrebootcontroller::MockRebootControllerMarker;
use fidl_fuchsia_samplertestcontroller::SamplerTestControllerMarker;
use fuchsia_component::client::{connect_to_protocol_at, connect_to_protocol_at_path};
use realm_client::InstalledNamespace;
use utils::{Event, EventVerifier};

mod test_topology;
mod utils;

async fn wait_for_single_counter_inspect(ns: &InstalledNamespace) {
    let accessor = connect_to_protocol_at::<fdiagnostics::ArchiveAccessorMarker>(&ns).unwrap();
    let _ = ArchiveReader::inspect()
        .with_archive(accessor)
        .add_selector(format!("{}:root", test_topology::COUNTER_NAME))
        .snapshot()
        .await
        .expect("got inspect data");
}

/// Runs the Sampler and a test component that can have its inspect properties
/// manipulated by the test via fidl, and uses cobalt mock and log querier to
/// verify that the sampler observers changes as expected, and logs them to
/// cobalt as expected.
#[fuchsia::test]
async fn event_count_sampler_test() {
    let ns = test_topology::create_realm().await.expect("initialized topology");
    let test_app_controller = connect_to_protocol_at::<SamplerTestControllerMarker>(&ns).unwrap();
    wait_for_single_counter_inspect(&ns).await;
    let reboot_controller = connect_to_protocol_at::<MockRebootControllerMarker>(&ns).unwrap();
    let logger_querier = connect_to_protocol_at::<MetricEventLoggerQuerierMarker>(&ns).unwrap();
    let _sampler_binder = connect_to_protocol_at_path::<BinderMarker>(format!(
        "{}/fuchsia.component.SamplerBinder",
        ns.prefix()
    ))
    .unwrap();

    let mut project_5_events = EventVerifier::new(&logger_querier, 5);
    let mut project_13_events = EventVerifier::new(&logger_querier, 13);

    test_app_controller.increment_int(1).await.unwrap();

    project_5_events
        .validate_with_count(
            vec![
                Event { id: 101, value: 1, codes: vec![0, 0] },
                Event { id: 102, value: 10, codes: vec![0, 0] },
                Event { id: 103, value: 20, codes: vec![0, 0] },
                Event { id: 104, value: 1, codes: vec![0, 0] },
            ],
            "initial in event_count",
        )
        .await;

    // Make sure I'm getting the expected FIRE events.
    // Three components in components.json5: integer_1, integer_2, and integer_42
    //   with codes 121, 122, and 142, respectively.
    // Two metrics in fire_1.json5: id 2 (event codes [0, 0]); id 3 (no event codes)
    // One metric in fire_2.json5: id 4 (event codes [14, 15])
    // One metric in fire_3.json5: id 5 (event codes [99])
    //   metrics in fire_1 and fire_2 are by moniker; in fire_3 is by hash
    //   metrics in fire_1 and fire_2 all use the same selector and should pick
    //     up the same published Inspect data.
    // The component instance ID file (in realm_factory/src/mocks.rs) gives an instance ID
    //   for the integer_42 component.
    // The Inspect data (from test_component/src/main.rs) publishes data
    //   for integer_1 (10) and integer_2 (20) by moniker, and integer_42 by ID (42).
    //   Also other data that shouldn't be picked up by FIRE.
    // So I'd expect id's 2, 3, and 4, for integer_1 and integer_2, and id 5 for integer_42.
    //   Seven events total.
    project_13_events
        .validate_with_count(
            vec![
                Event { id: 2, value: 10, codes: vec![121, 0, 0] },
                Event { id: 2, value: 20, codes: vec![122, 0, 0] },
                Event { id: 3, value: 10, codes: vec![121] },
                Event { id: 3, value: 20, codes: vec![122] },
                Event { id: 4, value: 10, codes: vec![121, 14, 15] },
                Event { id: 4, value: 20, codes: vec![122, 14, 15] },
                Event { id: 5, value: 42, codes: vec![142, 99] },
            ],
            "First FIRE events",
        )
        .await;

    // We want to guarantee a sample takes place before we increment the value again.
    // This is to verify that despite two samples taking place, the count type isn't uploaded with no diff
    // and the metric that is upload_once isn't sampled again.
    test_app_controller.wait_for_sample().await.unwrap().unwrap();

    project_5_events
        .validate_with_count(
            vec![Event { id: 102, value: 10, codes: vec![0, 0] }],
            "second in event_count",
        )
        .await;

    test_app_controller.increment_int(1).await.unwrap();

    // Same FIRE events as before, except metric ID 3 is upload_once.
    project_13_events
        .validate_with_count(
            vec![
                Event { id: 2, value: 10, codes: vec![121, 0, 0] },
                Event { id: 2, value: 20, codes: vec![122, 0, 0] },
                Event { id: 4, value: 10, codes: vec![121, 14, 15] },
                Event { id: 4, value: 20, codes: vec![122, 14, 15] },
                Event { id: 5, value: 42, codes: vec![142, 99] },
            ],
            "Second FIRE events",
        )
        .await;

    test_app_controller.wait_for_sample().await.unwrap().unwrap();

    project_5_events
        .validate_with_count(
            vec![
                // Even though we incremented metric-1 its value stays at 1 since it's being cached.
                Event { id: 101, value: 1, codes: vec![0, 0] },
                Event { id: 102, value: 10, codes: vec![0, 0] },
            ],
            "before reboot in event_count",
        )
        .await;

    // FIRE continues...
    project_13_events
        .validate_with_count(
            vec![
                Event { id: 2, value: 10, codes: vec![121, 0, 0] },
                Event { id: 2, value: 20, codes: vec![122, 0, 0] },
                Event { id: 4, value: 10, codes: vec![121, 14, 15] },
                Event { id: 4, value: 20, codes: vec![122, 14, 15] },
                Event { id: 5, value: 42, codes: vec![142, 99] },
            ],
            "More FIRE events",
        )
        .await;

    test_app_controller.increment_int(1).await.unwrap();

    // trigger_reboot calls the on_reboot callback that drives sampler shutdown. this
    // should await until sampler has finished its cleanup, which means we should have some events
    // present when we're done, and the sampler task should be finished.
    reboot_controller.trigger_reboot().await.unwrap().unwrap();

    project_5_events
        .validate_with_count(
            vec![
                // The metric configured to run every 3000 seconds gets polled
                Event { id: 104, value: 2, codes: vec![0, 0] },
                Event { id: 102, value: 10, codes: vec![0, 0] },
                // 101 and 104 refer to the same datum, but are occurrences that
                // have each been polled a different number of times
                Event { id: 101, value: 1, codes: vec![0, 0] },
            ],
            "after reboot in event_count",
        )
        .await;

    // Just to make sure
    project_13_events
        .validate_with_count(
            vec![
                Event { id: 2, value: 10, codes: vec![121, 0, 0] },
                Event { id: 2, value: 20, codes: vec![122, 0, 0] },
                Event { id: 4, value: 10, codes: vec![121, 14, 15] },
                Event { id: 4, value: 20, codes: vec![122, 14, 15] },
                Event { id: 5, value: 42, codes: vec![142, 99] },
            ],
            "Last FIRE events",
        )
        .await;
}

/// Runs the Sampler and a test component that can have its inspect properties
/// manipulated by the test via fidl, and verifies that Sampler publishes
/// the expected Inspect data reflecting its configuration.
#[fuchsia::test]
async fn reboot_server_crashed_test() {
    let ns = test_topology::create_realm().await.expect("initialized topology");
    let test_app_controller = connect_to_protocol_at::<SamplerTestControllerMarker>(&ns).unwrap();
    wait_for_single_counter_inspect(&ns).await;
    let reboot_controller = connect_to_protocol_at::<MockRebootControllerMarker>(&ns).unwrap();
    let logger_querier = connect_to_protocol_at::<MetricEventLoggerQuerierMarker>(&ns).unwrap();
    let _sampler_binder = connect_to_protocol_at_path::<BinderMarker>(format!(
        "{}/fuchsia.component.SamplerBinder",
        ns.prefix()
    ))
    .unwrap();
    let mut project_5_events = EventVerifier::new(&logger_querier, 5);

    // Crash the reboot server to verify that sampler continues to sample.
    reboot_controller.crash_reboot_channel().await.unwrap().unwrap();

    project_5_events
        .validate_with_count(
            vec![
                Event { id: 101, value: 0, codes: vec![0, 0] },
                Event { id: 104, value: 0, codes: vec![0, 0] },
                Event { id: 102, value: 10, codes: vec![0, 0] },
                Event { id: 103, value: 20, codes: vec![0, 0] },
            ],
            "initial in crashed-test",
        )
        .await;

    test_app_controller.increment_int(1).await.unwrap();

    project_5_events
        .validate_with_count(
            vec![
                Event { id: 101, value: 1, codes: vec![0, 0] },
                Event { id: 102, value: 10, codes: vec![0, 0] },
            ],
            "post increment, after crashing in crashed-test",
        )
        .await;

    // We want to guarantee a sample takes place before we increment the value again.
    // This is to verify that despite two samples taking place, the count type isn't uploaded with
    // no diff and the metric that is upload_once isn't sampled again.
    test_app_controller.wait_for_sample().await.unwrap().unwrap();

    project_5_events
        .validate_with_count(
            vec![Event { id: 102, value: 10, codes: vec![0, 0] }],
            "final in crashed-test",
        )
        .await;
}

/// Runs the Sampler and a test component that can have its inspect properties
/// manipulated by the test via fidl. Verifies that Sampler publishes its own
/// status correctly in its own Inspect data.
///
/// It is technically possible for this to flake, because project metrics are
/// written to Inspect as they are loaded from config files. This means that
/// if there are more than one configs for a given project, the ArchiveReader
/// access could happen in between config loads. However, in practice the
/// Archivist access should be much slower than starting Sampler and loading
/// configs.
#[fuchsia::test]
async fn sampler_inspect_test() {
    let ns = test_topology::create_realm().await.expect("initialized topology");
    let _sampler_binder = connect_to_protocol_at_path::<BinderMarker>(format!(
        "{}/fuchsia.component.SamplerBinder",
        ns.prefix()
    ))
    .unwrap();

    let accessor = connect_to_protocol_at::<fdiagnostics::ArchiveAccessorMarker>(&ns).unwrap();

    // Observe verification shows up in inspect.
    let hierarchy = ArchiveReader::inspect()
        .with_archive(accessor)
        // this selector is left in place to catch anything "extra" that is written
        .add_selector(format!("{}:root", test_topology::SAMPLER_NAME))
        .add_selector(format!(
            "{}:root/sampler_executor_stats/project_5",
            test_topology::SAMPLER_NAME
        ))
        .add_selector(format!(
            "{}:root/sampler_executor_stats/project_13",
            test_topology::SAMPLER_NAME
        ))
        .add_selector(format!("{}:root/fuchsia.inspect.Health", test_topology::SAMPLER_NAME))
        .snapshot()
        .await
        .expect("got inspect data")
        .pop()
        .expect("retry until there is a payload")
        .payload
        .expect("there is data");

    assert_data_tree!(
        hierarchy,
        root: {
            sampler_executor_stats: {
                project_5: {
                    metrics_configured: 4u64,
                    cobalt_logs_sent: AnyProperty,
                    events: contains {},
                },
                project_13: {
                    metrics_configured: 10u64,
                    cobalt_logs_sent: AnyProperty,
                    events: contains {},
                },
            },
            "fuchsia.inspect.Health": {
                start_timestamp_nanos: AnyProperty,
                status: AnyProperty
            }
        }
    );
}

/// Stress test that repeatedly increments Inspect metrics, triggers sampling cycles,
/// and validates events across multiple iterations under continuous load.
#[fuchsia::test]
async fn sampler_stress_test() {
    log::info!("Sampler stress test: initializing test realm topology...");
    let ns = test_topology::create_realm().await.expect("initialized topology");
    let test_app_controller = connect_to_protocol_at::<SamplerTestControllerMarker>(&ns).unwrap();
    wait_for_single_counter_inspect(&ns).await;
    let logger_querier = connect_to_protocol_at::<MetricEventLoggerQuerierMarker>(&ns).unwrap();
    let _sampler_binder = connect_to_protocol_at_path::<BinderMarker>(format!(
        "{}/fuchsia.component.SamplerBinder",
        ns.prefix()
    ))
    .unwrap();

    let mut project_5_events = EventVerifier::new(&logger_querier, 5);

    test_app_controller.increment_int(1).await.unwrap();

    // Initial state validation
    project_5_events
        .validate_with_count(
            vec![
                Event { id: 101, value: 1, codes: vec![0, 0] },
                Event { id: 102, value: 10, codes: vec![0, 0] },
                Event { id: 103, value: 20, codes: vec![0, 0] },
                Event { id: 104, value: 1, codes: vec![0, 0] },
            ],
            "initial in stress_test",
        )
        .await;
    log::info!("Sampler stress test: initial event state verified. Starting 10 stress cycles...");

    // Run 10 consecutive sample cycles under load
    for i in 1..=10 {
        log::info!(
            "Sampler stress test [cycle {}/10]: incrementing counter & waiting for sample...",
            i
        );
        test_app_controller.increment_int(1).await.unwrap();
        test_app_controller.wait_for_sample().await.unwrap().unwrap();
        project_5_events
            .validate_with_count(
                vec![
                    // Metric 101 (`samples:counter`) is configured as an Occurrence metric in
                    // `test_config.json`. Sampler logs the diff between consecutive samples rather
                    // than the cumulative count. Since we increment by 1 each cycle, the diff is 1.
                    Event { id: 101, value: 1, codes: vec![0, 0] },
                    // Metric 102 (`samples:integer_1`) is an Integer metric without `upload_once`.
                    // It samples `integer_1` (initialized to 10 in test state and never mutated).
                    // Because Integer metrics without `upload_once` are sampled every cycle,
                    // it is uploaded every time with its current value of 10.
                    // (Metric 103 has `upload_once: true` so it is only uploaded initially, and
                    // Metric 104 has a 3000s poll rate so it is not polled here).
                    Event { id: 102, value: 10, codes: vec![0, 0] },
                ],
                &format!("stress cycle {} in stress_test", i),
            )
            .await;
        log::info!("Sampler stress test [cycle {}/10]: events validated successfully", i);
    }

    log::info!("Sampler stress test: all 10 cycles finished. Inspecting daemon health...");
    // Verify Sampler is still healthy and responsive via Inspect
    let accessor = connect_to_protocol_at::<fdiagnostics::ArchiveAccessorMarker>(&ns).unwrap();
    let hierarchy = ArchiveReader::inspect()
        .with_archive(accessor)
        .add_selector(format!("{}:root/fuchsia.inspect.Health", test_topology::SAMPLER_NAME))
        .snapshot()
        .await
        .expect("got inspect data")
        .pop()
        .expect("payload present")
        .payload
        .expect("valid payload");

    assert_data_tree!(
        hierarchy,
        root: {
            "fuchsia.inspect.Health": {
                start_timestamp_nanos: AnyProperty,
                status: AnyProperty,
            }
        }
    );
    log::info!("Sampler stress test: daemon health verified. Test complete.");
}
