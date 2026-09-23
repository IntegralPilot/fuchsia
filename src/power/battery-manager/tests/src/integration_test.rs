// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use fidl::endpoints::ServiceMarker as _;
use fidl_fuchsia_component_test as ftest;
use fidl_fuchsia_driver_test as fdt;
use fidl_fuchsia_hardware_power_battery as fbattery;
use fidl_test_hardwarepowercontrol as ftest_battery;

use fidl_fuchsia_power_battery as fpower;

use fidl_fuchsia_power_battery_test as spower;
use fidl_fuchsia_power_system as fsystem;
use fidl_fuchsia_testing as ftesting;
use fuchsia_async as fasync;
use fuchsia_component_test::{
    Capability, ChildOptions, LocalComponentHandles, RealmBuilder, RealmInstance, Ref, Route,
};
use fuchsia_driver_test::{DriverTestRealmBuilder, DriverTestRealmInstance};
use futures::channel::mpsc;
use futures::future::FutureExt as _;
use futures::{SinkExt as _, StreamExt as _};
use test_case::test_case;
use test_util::assert_gt;
use zx;

/// Dictates which battery FIDL protocols are routed from the DriverTestRealm
/// to the battery manager under test.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum FidlRouteMode {
    /// Route only the new `fuchsia.hardware.power.battery` protocols.
    NewOnly,
    /// Route only the old `fuchsia.power.battery` protocols.
    OldOnly,
    /// Route both old and new battery protocols.
    Both,
}

const BATTERY_MANAGER_URL: &str = "#meta/battery_manager_fake_time.cm";
const FAKE_CLOCK_URL: &str = "#meta/fake_clock.cm";

#[derive(Debug, PartialEq, Eq)]
enum LeaseEvent {
    Acquired(String),
    Dropped(String),
}

async fn run_fake_sag(
    handles: LocalComponentHandles,
    event_sender: mpsc::Sender<LeaseEvent>,
) -> Result<(), anyhow::Error> {
    use fuchsia_component::server as fserver;
    use futures::TryStreamExt as _;

    let mut fs = fserver::ServiceFs::new();
    let mut tasks = vec![];

    fs.dir("svc").add_fidl_service(move |mut stream: fsystem::ActivityGovernorRequestStream| {
        let mut event_sender = event_sender.clone();
        tasks.push(fasync::Task::local(async move {
            while let Some(request) =
                stream.try_next().await.expect("failed to serve ActivityGovernor")
            {
                match request {
                    fsystem::ActivityGovernorRequest::AcquireWakeLease { name, responder } => {
                        log::info!("Fake SAG: AcquireWakeLease called: {}", name);
                        let (local_token, remote_token) = zx::EventPair::create();

                        let _ = event_sender.send(LeaseEvent::Acquired(name.clone())).await;

                        let mut event_sender = event_sender.clone();
                        fasync::Task::local(async move {
                            let _ = fasync::OnSignals::new(
                                &local_token,
                                zx::Signals::OBJECT_PEER_CLOSED,
                            )
                            .await;
                            log::info!("Fake SAG: Lease token dropped for {}", name);
                            let _ = event_sender.send(LeaseEvent::Dropped(name)).await;
                        })
                        .detach();

                        responder.send(Ok(remote_token)).expect("failed to send response");
                    }
                    fsystem::ActivityGovernorRequest::AcquireUnmonitoredWakeLease {
                        name,
                        responder,
                    } => {
                        log::info!("Fake SAG: AcquireUnmonitoredWakeLease called: {}", name);
                        let (local_token, remote_token) = zx::EventPair::create();

                        let _ = event_sender.send(LeaseEvent::Acquired(name.clone())).await;

                        let mut event_sender = event_sender.clone();
                        fasync::Task::local(async move {
                            let _ = fasync::OnSignals::new(
                                &local_token,
                                zx::Signals::OBJECT_PEER_CLOSED,
                            )
                            .await;
                            log::info!("Fake SAG: Lease token dropped for {}", name);
                            let _ = event_sender.send(LeaseEvent::Dropped(name)).await;
                        })
                        .detach();

                        responder.send(Ok(remote_token)).expect("failed to send response");
                    }
                    _ => panic!("Fake SAG: Unimplemented method"),
                }
            }
        }));
    });

    fs.serve_connection(handles.outgoing_dir)?;
    fs.collect::<()>().await;
    Ok(())
}

async fn setup_realm(
    mode: FidlRouteMode,
    suspend_enabled: bool,
) -> Result<(RealmInstance, mpsc::Receiver<LeaseEvent>)> {
    let builder = RealmBuilder::new().await?;
    builder.driver_test_realm_setup().await?;

    let (event_sender, event_receiver) = mpsc::channel(10);
    let fake_sag = builder
        .add_local_child(
            "fake_sag",
            move |handles| run_fake_sag(handles, event_sender.clone()).boxed(),
            ChildOptions::new(),
        )
        .await?;

    let mut dtr_exposes = vec![];
    if mode == FidlRouteMode::NewOnly || mode == FidlRouteMode::Both {
        dtr_exposes.push(ftest::Capability::Service(ftest::Service {
            name: Some(fbattery::ServiceMarker::SERVICE_NAME.to_string()),
            ..Default::default()
        }));
        dtr_exposes.push(ftest::Capability::Service(ftest::Service {
            name: Some(ftest_battery::ServiceMarker::SERVICE_NAME.to_string()),
            ..Default::default()
        }));
    }
    if mode == FidlRouteMode::OldOnly || mode == FidlRouteMode::Both {
        dtr_exposes.push(ftest::Capability::Service(ftest::Service {
            name: Some(fpower::InfoServiceMarker::SERVICE_NAME.to_string()),
            ..Default::default()
        }));
    }
    builder.driver_test_realm_add_dtr_exposes(&dtr_exposes).await?;

    let battery_manager =
        builder.add_child("battery_manager", BATTERY_MANAGER_URL, ChildOptions::new()).await?;

    let fake_clock = builder.add_child("fake_clock", FAKE_CLOCK_URL, ChildOptions::new()).await?;

    // Route LogSink to battery_manager, fake_clock and fake_sag
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol_by_name("fuchsia.logger.LogSink"))
                .from(Ref::parent())
                .to(&battery_manager)
                .to(&fake_clock)
                .to(&fake_sag),
        )
        .await?;

    // Route FakeClock to battery_manager
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol_by_name("fuchsia.testing.FakeClock"))
                .from(&fake_clock)
                .to(&battery_manager),
        )
        .await?;

    // Expose FakeClockControl to parent (test runner)
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol_by_name("fuchsia.testing.FakeClockControl"))
                .from(&fake_clock)
                .to(Ref::parent()),
        )
        .await?;

    // Route storage and configuration
    builder
        .add_route(
            Route::new()
                .capability(Capability::storage("data"))
                .capability(Capability::storage("tmp"))
                .from(Ref::parent())
                .to(&battery_manager),
        )
        .await?;

    builder
        .add_capability(cm_rust::CapabilityDecl::Config(cm_rust::ConfigurationDecl {
            name: "fuchsia.power.SuspendEnabled".parse().unwrap(),
            value: suspend_enabled.into(),
        }))
        .await?;

    builder
        .add_route(
            Route::new()
                .capability(Capability::configuration("fuchsia.power.SuspendEnabled"))
                .from(Ref::self_())
                .to(&battery_manager),
        )
        .await?;

    // Route driver services to battery_manager
    let mut dtr_route = Route::new();
    if mode == FidlRouteMode::NewOnly || mode == FidlRouteMode::Both {
        dtr_route = dtr_route.capability(Capability::service::<fbattery::ServiceMarker>());
    }
    if mode == FidlRouteMode::OldOnly || mode == FidlRouteMode::Both {
        dtr_route = dtr_route.capability(Capability::service::<fpower::InfoServiceMarker>());
    }
    builder
        .add_route(
            dtr_route.from(Ref::child(fuchsia_driver_test::COMPONENT_NAME)).to(&battery_manager),
        )
        .await?;

    // Expose Control Service from DTR to parent
    if mode == FidlRouteMode::NewOnly || mode == FidlRouteMode::Both {
        builder
            .add_route(
                Route::new()
                    .capability(Capability::service::<ftest_battery::ServiceMarker>())
                    .from(Ref::child(fuchsia_driver_test::COMPONENT_NAME))
                    .to(Ref::parent()),
            )
            .await?;
    }

    // Expose BatteryManager and BatterySimulator to the test runner
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol::<fpower::BatteryManagerMarker>())
                .capability(Capability::protocol::<spower::BatterySimulatorMarker>())
                .from(&battery_manager)
                .to(Ref::parent()),
        )
        .await?;

    // Route ActivityGovernor protocol from fake_sag to battery_manager
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol::<fsystem::ActivityGovernorMarker>())
                .from(&fake_sag)
                .to(&battery_manager),
        )
        .await?;

    let realm = builder.build().await?;

    realm
        .driver_test_realm_start(fdt::RealmArgs {
            root_driver: Some("fuchsia-boot:///platform-bus#meta/platform-bus.cm".to_owned()),
            dtr_exposes: Some(dtr_exposes),
            software_devices: Some(vec![fdt::SoftwareDevice {
                device_name: "fake-battery".to_string(),
                device_id: bind_fuchsia_platform::BIND_PLATFORM_DEV_DID_FAKE_BATTERY,
            }]),
            ..Default::default()
        })
        .await?;

    Ok((realm, event_receiver))
}

fn assert_default_battery_info(info: &fpower::BatteryInfo) {
    assert_eq!(info.level_percent, Some(ftest_battery::DEFAULT_ROUNDED_LEVEL_PERCENT as f32));
    assert_eq!(info.charge_status, Some(fpower::ChargeStatus::Charging));
    assert_eq!(info.charge_source, Some(fpower::ChargeSource::AcAdapter));
    assert_eq!(info.present_voltage_mv, Some(ftest_battery::DEFAULT_PRESENT_VOLTAGE_MV));
    assert_eq!(info.remaining_charge_uah, Some(ftest_battery::DEFAULT_REMAINING_CHARGE_UAH));
    assert!(info.timestamp.is_some());
}

async fn wait_for_battery_info(
    mut watcher_stream: fpower::BatteryInfoWatcherRequestStream,
) -> Result<(fpower::BatteryInfo, fpower::BatteryInfoWatcherRequestStream)> {
    while let Some(Ok(fpower::BatteryInfoWatcherRequest::OnChangeBatteryInfo {
        info,
        responder,
        ..
    })) = watcher_stream.next().await
    {
        responder.send()?;
        if info.level_percent.is_some() && info.status != Some(fpower::BatteryStatus::NotAvailable)
        {
            return Ok((info, watcher_stream));
        }
    }
    Err(anyhow::anyhow!("Watcher stream ended without receiving valid battery info"))
}

async fn update_and_check(
    control: &ftest_battery::ControlProxy,
    fake_clock_control: &ftesting::FakeClockControlProxy,
    watcher_stream: &mut fpower::BatteryInfoWatcherRequestStream,
    advance_time: zx::MonotonicDuration,
    raw_level: f32,
    expected_scaled: f32,
) -> Result<()> {
    fake_clock_control
        .advance(&ftesting::Increment::Determined(advance_time.into_nanos()))
        .await?
        .map_err(|e| anyhow::anyhow!("failed to advance fake clock: {:?}", e))?;

    control
        .set_battery_status(&fbattery::Status {
            level_percent: Some(raw_level),
            charge_status: Some(fbattery::ChargeStatus::Charging),
            ..Default::default()
        })
        .await?;

    let Some(Ok(fpower::BatteryInfoWatcherRequest::OnChangeBatteryInfo {
        info, responder, ..
    })) = watcher_stream.next().await
    else {
        return Err(anyhow::anyhow!("Watcher stream ended prematurely"));
    };
    responder.send()?;
    let level = info.level_percent.expect("level_percent missing");
    assert!(
        (level - expected_scaled).abs() < f32::EPSILON,
        "expected level_percent {expected_scaled}, got {level}"
    );
    Ok(())
}

#[test_case(FidlRouteMode::NewOnly; "new_only")]
#[test_case(FidlRouteMode::OldOnly; "old_only")]
#[test_case(FidlRouteMode::Both; "both")]
#[fuchsia::test]
async fn test_get_battery_info(mode: FidlRouteMode) -> Result<()> {
    let (realm, _lease_events) = setup_realm(mode, false).await?;
    let battery_mgr: fpower::BatteryManagerProxy =
        realm.root.connect_to_protocol_at_exposed_dir()?;

    let (watcher_client, watcher_stream) =
        fidl::endpoints::create_request_stream::<fpower::BatteryInfoWatcherMarker>();
    battery_mgr.watch(watcher_client)?;

    let (info, _stream) = wait_for_battery_info(watcher_stream).await?;
    assert_default_battery_info(&info);

    let get_info = battery_mgr.get_battery_info().await?;
    assert_default_battery_info(&get_info);
    Ok(())
}

#[fuchsia::test]
async fn test_watcher() -> Result<()> {
    let (realm, _lease_events) = setup_realm(FidlRouteMode::Both, false).await?;
    let battery_mgr: fpower::BatteryManagerProxy =
        realm.root.connect_to_protocol_at_exposed_dir()?;

    let (watcher_client, watcher_stream) =
        fidl::endpoints::create_request_stream::<fpower::BatteryInfoWatcherMarker>();
    battery_mgr.watch(watcher_client)?;

    let (info, _stream) = wait_for_battery_info(watcher_stream).await?;
    assert_default_battery_info(&info);
    Ok(())
}

#[fuchsia::test]
async fn test_simulator() -> Result<()> {
    let (realm, _lease_events) = setup_realm(FidlRouteMode::Both, false).await?;
    let battery_mgr: fpower::BatteryManagerProxy =
        realm.root.connect_to_protocol_at_exposed_dir()?;
    let simulator: spower::BatterySimulatorProxy =
        realm.root.connect_to_protocol_at_exposed_dir()?;

    let (watcher_client, watcher_stream) =
        fidl::endpoints::create_request_stream::<fpower::BatteryInfoWatcherMarker>();
    battery_mgr.watch(watcher_client)?;

    // Wait for the initial update from DTR driver first
    let (_info, mut watcher_stream) = wait_for_battery_info(watcher_stream).await?;

    // Now disconnect real battery to trigger simulation mode
    simulator.disconnect_real_battery()?;

    // Wait for the simulation mode update to be propagated
    if let Some(Ok(fpower::BatteryInfoWatcherRequest::OnChangeBatteryInfo { responder, .. })) =
        watcher_stream.next().await
    {
        responder.send()?;
    }

    // Now push simulated updates
    simulator.set_battery_percentage(50.0)?;
    simulator.set_charge_status(fpower::ChargeStatus::Discharging)?;

    // We should receive the updated battery info callback
    loop {
        if let Some(Ok(fpower::BatteryInfoWatcherRequest::OnChangeBatteryInfo {
            info,
            responder,
            ..
        })) = watcher_stream.next().await
        {
            responder.send()?;
            if info.level_percent == Some(50.0)
                && info.charge_status == Some(fpower::ChargeStatus::Discharging)
            {
                break;
            }
        } else {
            panic!("Watcher stream ended before receiving expected simulated updates");
        }
    }
    Ok(())
}

#[fuchsia::test]
async fn test_watch_dynamic_updates() -> Result<()> {
    let (realm, _lease_events) = setup_realm(FidlRouteMode::NewOnly, false).await?;

    let battery_mgr: fpower::BatteryManagerProxy =
        realm.root.connect_to_protocol_at_exposed_dir()?;
    let service = fuchsia_component::client::Service::open_from_dir(
        realm.root.get_exposed_dir(),
        ftest_battery::ServiceMarker,
    )?;
    let service_instance = service.watch_for_any().await?;
    let control = service_instance.connect_to_control()?;

    let (watcher_client, watcher_stream) =
        fidl::endpoints::create_request_stream::<fpower::BatteryInfoWatcherMarker>();
    battery_mgr.watch(watcher_client)?;

    // Wait for the initial update from driver
    let (info, mut watcher_stream) = wait_for_battery_info(watcher_stream).await?;
    assert_default_battery_info(&info);
    let t0 = info.timestamp.expect("timestamp missing from default info");

    let fake_clock_control =
        realm.root.connect_to_protocol_at_exposed_dir::<ftesting::FakeClockControlProxy>()?;

    fake_clock_control.pause().await?;

    // Advance fake time by 10 seconds.
    fake_clock_control
        .advance(&ftesting::Increment::Determined(
            zx::MonotonicDuration::from_seconds(10).into_nanos(),
        ))
        .await?
        .map_err(|e| anyhow::anyhow!("failed to advance fake clock: {:?}", e))?;

    // Now update fake battery using driver Control.
    // Raw level 99.1% maps to 100.0% scaled level after processing.
    control
        .set_battery_status(&fbattery::Status {
            level_percent: Some(99.1),
            charge_status: Some(fbattery::ChargeStatus::Charging),
            ..Default::default()
        })
        .await?;

    // Wait for the update on watcher stream
    let Some(Ok(fpower::BatteryInfoWatcherRequest::OnChangeBatteryInfo {
        info, responder, ..
    })) = watcher_stream.next().await
    else {
        panic!("Watcher stream ended before receiving expected driver updates");
    };
    responder.send()?;
    let level = info.level_percent.expect("level_percent missing");
    assert!((level - 100.0).abs() < f32::EPSILON, "expected 100.0, got {level}");
    assert_eq!(info.charge_status, Some(fpower::ChargeStatus::Charging));
    let t1 = info.timestamp.expect("timestamp missing from first update");
    assert_gt!(t1, t0);

    Ok(())
}

#[fuchsia::test]
async fn test_shutdown_offset_scaling() -> Result<()> {
    let (realm, _lease_events) = setup_realm(FidlRouteMode::NewOnly, false).await?;

    let battery_mgr: fpower::BatteryManagerProxy =
        realm.root.connect_to_protocol_at_exposed_dir()?;
    let service = fuchsia_component::client::Service::open_from_dir(
        realm.root.get_exposed_dir(),
        ftest_battery::ServiceMarker,
    )?;
    let service_instance = service.watch_for_any().await?;
    let control = service_instance.connect_to_control()?;

    let (watcher_client, watcher_stream) =
        fidl::endpoints::create_request_stream::<fpower::BatteryInfoWatcherMarker>();
    battery_mgr.watch(watcher_client)?;

    // Wait for the initial update from driver
    let (info, mut watcher_stream) = wait_for_battery_info(watcher_stream).await?;
    assert_default_battery_info(&info);

    let fake_clock_control =
        realm.root.connect_to_protocol_at_exposed_dir::<ftesting::FakeClockControlProxy>()?;
    fake_clock_control.pause().await?;

    let advance_1000s = zx::MonotonicDuration::from_seconds(1000);

    // Test cases:
    // 1. Raw level at or below offset (3.0%) -> Scaled level 0.0%
    update_and_check(&control, &fake_clock_control, &mut watcher_stream, advance_1000s, 4.0, 2.0)
        .await?;
    update_and_check(&control, &fake_clock_control, &mut watcher_stream, advance_1000s, 3.1, 1.0)
        .await?;
    update_and_check(&control, &fake_clock_control, &mut watcher_stream, advance_1000s, 3.0, 0.0)
        .await?;
    update_and_check(&control, &fake_clock_control, &mut watcher_stream, advance_1000s, 2.0, 0.0)
        .await?;

    // 2. Raw level in middle (51.5%) -> Scaled level 50.0%
    update_and_check(&control, &fake_clock_control, &mut watcher_stream, advance_1000s, 51.5, 50.0)
        .await?;

    // 3. Raw level 99.0% -> Scaled level 99.0%
    update_and_check(&control, &fake_clock_control, &mut watcher_stream, advance_1000s, 99.0, 99.0)
        .await?;

    // 4. Raw level 100% -> Scaled level 100.0%
    update_and_check(
        &control,
        &fake_clock_control,
        &mut watcher_stream,
        advance_1000s,
        100.0,
        100.0,
    )
    .await?;

    // 5. Raw level above 100% (101.0%) -> Scaled level capped at 100.0%
    update_and_check(
        &control,
        &fake_clock_control,
        &mut watcher_stream,
        advance_1000s,
        101.0,
        100.0,
    )
    .await?;

    Ok(())
}

#[fuchsia::test]
async fn test_charging_wake_lease() -> Result<()> {
    // 1. Setup realm with suspend_enabled = true
    let (realm, mut lease_events) = setup_realm(FidlRouteMode::NewOnly, true).await?;

    let battery_mgr: fpower::BatteryManagerProxy =
        realm.root.connect_to_protocol_at_exposed_dir()?;
    let service = fuchsia_component::client::Service::open_from_dir(
        realm.root.get_exposed_dir(),
        ftest_battery::ServiceMarker,
    )?;
    let service_instance = service.watch_for_any().await?;
    let control = service_instance.connect_to_control()?;

    // 2. The battery-manager starts watching driver updates.
    // It should immediately connect to fake_sag and acquire the startup lease "battery_manager".
    // Wait for the first LeaseEvent::Acquired("battery_manager")
    let event1 = lease_events.next().await.ok_or_else(|| anyhow::anyhow!("lease_events ended"))?;
    assert_eq!(event1, LeaseEvent::Acquired("battery_manager".to_string()));

    // 3. Connect a watcher client to battery_manager
    let (watcher_client, watcher_stream) =
        fidl::endpoints::create_request_stream::<fpower::BatteryInfoWatcherMarker>();
    battery_mgr.watch(watcher_client)?;

    // Wait for the initial update from driver first
    let (_info, mut watcher_stream) = wait_for_battery_info(watcher_stream).await?;

    // The startup lease should be dropped now
    let event_drop =
        lease_events.next().await.ok_or_else(|| anyhow::anyhow!("lease_events ended"))?;
    assert_eq!(event_drop, LeaseEvent::Dropped("battery_manager".to_string()));

    // 4. Inject a charging status update via Driver Control (AcAdapter plugged in)
    control
        .set_battery_status(&fbattery::Status {
            level_percent: Some(50.0),
            charge_status: Some(fbattery::ChargeStatus::Charging),
            ..Default::default()
        })
        .await?;

    // Wait for the watcher stream to propagate the Charging status
    loop {
        if let Some(Ok(fpower::BatteryInfoWatcherRequest::OnChangeBatteryInfo {
            info,
            responder,
            ..
        })) = watcher_stream.next().await
        {
            responder.send()?;
            if info.charge_status == Some(fpower::ChargeStatus::Charging)
                && info.charge_source == Some(fpower::ChargeSource::AcAdapter)
            {
                break;
            }
        } else {
            return Err(anyhow::anyhow!("Watcher stream ended prematurely"));
        }
    }

    // 5. Verify that fake_sag receives AcquireUnmonitoredWakeLease("charging_block_suspension")
    let event2 = lease_events.next().await.ok_or_else(|| anyhow::anyhow!("lease_events ended"))?;
    assert_eq!(event2, LeaseEvent::Acquired("charging_block_suspension".to_string()));

    // 6. Unplug the charger (Discharging, no source)
    control
        .set_battery_status(&fbattery::Status {
            level_percent: Some(50.0),
            charge_status: Some(fbattery::ChargeStatus::Discharging),
            ..Default::default()
        })
        .await?;

    // Wait for the watcher stream to propagate the Discharging status
    loop {
        if let Some(Ok(fpower::BatteryInfoWatcherRequest::OnChangeBatteryInfo {
            info,
            responder,
            ..
        })) = watcher_stream.next().await
        {
            responder.send()?;
            if info.charge_status == Some(fpower::ChargeStatus::Discharging) {
                break;
            }
        } else {
            return Err(anyhow::anyhow!("Watcher stream ended prematurely"));
        }
    }

    // 7. Verify that the wake lease was dropped (Fake SAG detects PEER_CLOSED)
    let event3 = lease_events.next().await.ok_or_else(|| anyhow::anyhow!("lease_events ended"))?;
    assert_eq!(event3, LeaseEvent::Dropped("charging_block_suspension".to_string()));

    Ok(())
}

#[fuchsia::test]
async fn test_rate_limiter() -> Result<()> {
    // 1. Setup realm with suspend_enabled = false
    let (realm, _lease_events) = setup_realm(FidlRouteMode::NewOnly, false).await?;

    let battery_mgr: fpower::BatteryManagerProxy =
        realm.root.connect_to_protocol_at_exposed_dir()?;
    let service = fuchsia_component::client::Service::open_from_dir(
        realm.root.get_exposed_dir(),
        ftest_battery::ServiceMarker,
    )?;
    let service_instance = service.watch_for_any().await?;
    let control = service_instance.connect_to_control()?;

    let (watcher_client, watcher_stream) =
        fidl::endpoints::create_request_stream::<fpower::BatteryInfoWatcherMarker>();
    battery_mgr.watch(watcher_client)?;

    let fake_clock_control =
        realm.root.connect_to_protocol_at_exposed_dir::<ftesting::FakeClockControlProxy>()?;
    fake_clock_control.pause().await?;

    // Wait for the initial update from driver (which is scaled 98.7% -> 99.0%)
    let (info, mut watcher_stream) = wait_for_battery_info(watcher_stream).await?;
    assert_eq!(info.level_percent, Some(99.0));

    // 2. Set raw level to 51.5% -> maps to 50.0% scaled level.
    // Advance time by 1000s to let it settle immediately (bypass the rate limiter for the setup).
    update_and_check(
        &control,
        &fake_clock_control,
        &mut watcher_stream,
        zx::MonotonicDuration::from_seconds(1000),
        51.5,
        50.0,
    )
    .await?;

    // 3. Suddenly update the raw level to 61.2% -> maps to 60.0% scaled level.
    // Step 1: Advance clock by 15 seconds. Max delta is 2.0%.
    // So the level should increase to 52.0%.
    update_and_check(
        &control,
        &fake_clock_control,
        &mut watcher_stream,
        zx::MonotonicDuration::from_seconds(15),
        61.2,
        52.0,
    )
    .await?;

    // Step 2: Advance clock by another 15 seconds.
    // The level should increase to 54.0%.
    update_and_check(
        &control,
        &fake_clock_control,
        &mut watcher_stream,
        zx::MonotonicDuration::from_seconds(15),
        61.2,
        54.0,
    )
    .await?;

    // Step 3: Advance clock by 100 seconds to let it fully reach 60.0%.
    update_and_check(
        &control,
        &fake_clock_control,
        &mut watcher_stream,
        zx::MonotonicDuration::from_seconds(100),
        61.2,
        60.0,
    )
    .await?;

    Ok(())
}
