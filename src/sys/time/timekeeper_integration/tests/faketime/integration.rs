// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result};
use fidl::endpoints;
use fidl_fuchsia_metrics_test::{LogMethod, MetricEventLoggerQuerierProxy};
use fidl_fuchsia_testing::Increment;
use fidl_fuchsia_time_external::{Status, TimeSample};
use fuchsia_component::client;

use fidl_fuchsia_testing as ffte;
use fidl_fuchsia_testing_harness as ffth;
use fidl_fuchsia_time as fft;
use fidl_test_time_realm as fttr;
use fuchsia_async as fasync;
use futures::{Future, StreamExt};
use std::sync::Arc;
use test_util::assert_geq;
use time_metrics_registry::{
    TIMEKEEPER_TIME_SOURCE_EVENTS_MIGRATED_METRIC_ID,
    TimekeeperTimeSourceEventsMigratedMetricDimensionEventType as TimeSourceEvent,
};
use timekeeper_integration_lib::{
    FakeClockController, RemotePushSourcePuppet, STD_DEV, VALID_TIME, create_cobalt_event_stream,
    new_nonshareable_clock, poll_until_async, poll_until_async_2,
};

use fidl_fuchsia_testing as _; // TODO: fmil - Figure out why this is needed.

fn koid_of(c: &zx::Clock) -> u64 {
    c.as_handle_ref().koid().expect("infallible").raw_koid()
}

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

/// Run a test against an instance of timekeeper with fake time. Timekeeper will maintain the
/// provided clock.
///
/// Note that while timekeeper is run with fake time, calls to directly read time
/// on the test component retrieve the real time. When the test component needs to read fake
/// time, it must do so using the `FakeClockController` handle. Basically, tests should access
/// fake time through fake_clock_controller.get_monotonic() instead of the common methods such as
/// zx::BootInstant::get().
///
/// The provided `test_fn` is provided with handles to manipulate the time source, observe events
/// passed to cobalt, and manipulate the fake time.
async fn faketime_test<F, Fut>(utc_clock: zx::Clock, test_fn: F) -> Result<()>
where
    F: FnOnce(
        Arc<zx::Clock>,
        Arc<RemotePushSourcePuppet>,
        MetricEventLoggerQuerierProxy,
        FakeClockController,
    ) -> Fut,
    Fut: Future<Output = ()>,
{
    let utc_clock_copy = utc_clock.duplicate_handle(zx::Rights::SAME_RIGHTS).expect("duplicated");
    log::debug!("using utc_clock_copy with koid: {}", koid_of(&utc_clock_copy));
    let utc_clock = Arc::new(utc_clock);
    log::debug!("using utc_clock with koid     : {}", koid_of(&*utc_clock));
    let test_realm_proxy =
        client::connect_to_protocol::<fttr::RealmFactoryMarker>().with_context(|| {
            format!(
                "while connecting to: {}",
                <fttr::RealmFactoryMarker as fidl::endpoints::ProtocolMarker>::DEBUG_NAME
            )
        })?;

    // realm_proxy must live as long as you need your test realm to live.
    let (realm_proxy, realm_server_end) = endpoints::create_proxy::<ffth::RealmProxy_Marker>();

    let (push_source_puppet, _opts, cobalt_metric_client) = test_realm_proxy
        .create_realm(fttr::RealmOptions { ..Default::default() }, utc_clock_copy, realm_server_end)
        .await
        .expect("FIDL protocol error")
        .expect("Error value returned from the call");
    let cobalt = cobalt_metric_client.into_proxy();

    // Fake clock controller for the tests that need that.
    let fake_clock_proxy = connect_into_realm::<ffte::FakeClockMarker>(&realm_proxy).await;
    let fake_clock_control_proxy =
        connect_into_realm::<ffte::FakeClockControlMarker>(&realm_proxy).await;
    let fake_clock_controller =
        FakeClockController::new(fake_clock_control_proxy, fake_clock_proxy);

    let push_source_puppet = push_source_puppet.into_proxy();
    let push_source_controller = RemotePushSourcePuppet::new(push_source_puppet);
    log::debug!("faketime_test: about to run test_fn");
    let result = test_fn(utc_clock, push_source_controller, cobalt, fake_clock_controller).await;
    log::debug!("faketime_test: done with run test_fn");
    Ok(result)
}

/// Start freely running the fake time at 60,000x the real time rate.
async fn freerun_time_fast(fake_clock: &FakeClockController) {
    fake_clock.pause().await.expect("Failed to pause time");
    fake_clock
        .resume_with_increments(
            zx::BootDuration::from_millis(1).into_nanos(),
            &Increment::Determined(zx::BootDuration::from_minutes(1).into_nanos()),
        )
        .await
        .expect("Failed to resume time")
        .expect("Resume returned error");
}

/// The duration after which timekeeper restarts an inactive time source.
const INACTIVE_SOURCE_RESTART_DURATION: zx::BootDuration = zx::BootDuration::from_hours(1);

#[fuchsia::test]
async fn test_restart_inactive_time_source_that_claims_healthy() -> Result<()> {
    let clock = new_nonshareable_clock();
    faketime_test(clock, |clock, push_source_controller, cobalt, fake_time| async move {
        let cobalt_event_stream =
            create_cobalt_event_stream(Arc::new(cobalt), LogMethod::LogMetricEvents);

        let mono_before = zx::BootInstant::from_nanos(
            fake_time.get_monotonic().await.expect("Failed to get time"),
        );
        let reference_time = fake_time.get_reference().await.expect("Failed to get time");
        push_source_controller
            .set_sample(TimeSample {
                utc: Some(VALID_TIME.into_nanos()),
                // Compatibility for CTF tests.
                monotonic: Some(reference_time.into_nanos()),
                reference: Some(reference_time),
                standard_deviation: Some(STD_DEV.into_nanos()),
                ..Default::default()
            })
            .await;
        log::debug!("before CLOCK_STARTED");
        fasync::OnSignals::new(&*clock, zx::Signals::CLOCK_STARTED)
            .await
            .expect("Failed to wait for CLOCK_STARTED");
        fasync::OnSignals::new(
            &*clock,
            zx::Signals::from_bits(fft::SIGNAL_UTC_CLOCK_SYNCHRONIZED).unwrap(),
        )
        .await
        .expect("Failed to wait for SIGNAL_UTC_CLOCK_SYNCHRONIZED");
        log::debug!("after SIGNAL_UTC_CLOCK_SYNCHRONIZED");

        assert_eq!(push_source_controller.lifetime_served_connections().await, 1);

        // Timekeeper should restart the time source after approximately an hour of inactivity.
        // Here, we run time quickly rather than leaping forward in one step. This is done as
        // Timekeeper reads the time, then calculates an hour from there. If time is jumped
        // forward in a single step, the timeout will not occur in the case Timekeeper reads the
        // time after we jump time forward.
        freerun_time_fast(&fake_time).await;

        // Wait for Timekeeper to restart the time source. This is visible to the test as a second
        // connection to the fake time source.
        poll_until_async_2!(async {
            push_source_controller.lifetime_served_connections().await > 1
        });

        // At least an hour should've passed.
        let mono_after = zx::BootInstant::from_nanos(
            fake_time.get_monotonic().await.expect("Failed to get time"),
        );
        assert_geq!(mono_after, mono_before + INACTIVE_SOURCE_RESTART_DURATION);

        // Timekeeper should report the restarted event to Cobalt.
        let restart_event = cobalt_event_stream
            .skip_while(|event| {
                let is_restart_event = event.metric_id
                    == TIMEKEEPER_TIME_SOURCE_EVENTS_MIGRATED_METRIC_ID
                    && event
                        .event_codes
                        .contains(&(TimeSourceEvent::RestartedSampleTimeOut as u32));
                futures::future::ready(!is_restart_event)
            })
            .next()
            .await
            .expect("Failed to get restart event");
        assert_eq!(restart_event.metric_id, TIMEKEEPER_TIME_SOURCE_EVENTS_MIGRATED_METRIC_ID);
        assert!(
            restart_event.event_codes.contains(&(TimeSourceEvent::RestartedSampleTimeOut as u32))
        );
    })
    .await
}

#[fuchsia::test]
async fn test_dont_restart_inactive_time_source_with_unhealthy_dependency() -> Result<()> {
    let clock = new_nonshareable_clock();
    faketime_test(clock, |clock, push_source_controller, _, fake_time| async move {
        let reference_time = fake_time.get_reference().await.expect("Failed to get time");
        push_source_controller
            .set_sample(TimeSample {
                utc: Some(VALID_TIME.into_nanos()),
                // Backwards compatibility for CTS tests.
                monotonic: Some(reference_time.into_nanos()),
                reference: Some(reference_time),
                standard_deviation: Some(STD_DEV.into_nanos()),
                ..Default::default()
            })
            .await;
        log::debug!("before CLOCK_STARTED");
        fasync::OnSignals::new(&*clock, zx::Signals::CLOCK_STARTED)
            .await
            .expect("Failed to wait for CLOCK_STARTED");
        fasync::OnSignals::new(
            &*clock,
            zx::Signals::from_bits(fft::SIGNAL_UTC_CLOCK_SYNCHRONIZED).unwrap(),
        )
        .await
        .unwrap();
        log::debug!("after SIGNAL_UTC_CLOCK_SYNCHRONIZED");
        // Report unhealthy after first sample accepted.
        push_source_controller.set_status(Status::Network).await;

        freerun_time_fast(&fake_time).await;

        // Wait longer than the usual restart duration.
        let mono_before = zx::BootInstant::from_nanos(
            fake_time.get_monotonic().await.expect("Failed to get time"),
        );
        poll_until_async!(|| async {
            let mono_now = zx::BootInstant::from_nanos(
                fake_time.get_monotonic().await.expect("Failed to get time"),
            );
            mono_now - mono_before > INACTIVE_SOURCE_RESTART_DURATION * 4
        })
        .await;

        // Since there should be no restart attempts, only one connection was made to our fake.
        assert_eq!(push_source_controller.lifetime_served_connections().await, 1);
    })
    .await
}
