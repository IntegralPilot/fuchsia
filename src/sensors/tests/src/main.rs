// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::pin::pin;
use fidl_fuchsia_sensors::*;
use fidl_fuchsia_sensors_types::*;
use fuchsia_async::TestExecutor;
use fuchsia_component::client::{connect_to_protocol, connect_to_protocol_at};
use futures_util::StreamExt;
use realm_client::{InstalledNamespace, extend_namespace};
use std::collections::HashSet;
use {fidl_fuchsia_hardware_sensors as playback_fidl, fidl_fuchsia_sensors_realm as sensors_realm};

async fn setup_realm() -> anyhow::Result<InstalledNamespace> {
    let realm_factory = connect_to_protocol::<sensors_realm::RealmFactoryMarker>()?;
    let (dictionary_1, dictionary_2) = fidl::EventPair::create();

    realm_factory.create_realm(dictionary_1).await?.map_err(realm_client::Error::OperationError)?;
    let ns = extend_namespace(realm_factory, dictionary_2).await?;

    Ok(ns)
}

async fn get_events_from_stream(
    num_events: usize,
    stream: &mut ManagerEventStream,
) -> Vec<SensorEvent> {
    let mut events: Vec<SensorEvent> = Vec::new();
    for _i in 1..num_events {
        let mut event: SensorEvent =
            stream.next().await.unwrap().unwrap().into_on_sensor_event().unwrap();
        // The test cannot know these values ahead of time, so it can zero them so that it can
        // match the rest of the event.
        event.timestamp = 0;
        event.sequence_number = 0;

        events.push(event);
    }
    events
}

fn get_heart_rate_sensor() -> SensorInfo {
    SensorInfo {
        sensor_id: Some(12345678),
        name: Some(String::from("HEART_RATE")),
        vendor: Some(String::from("Fuchsia")),
        version: Some(1),
        sensor_type: Some(SensorType::HeartRate),
        wake_up: Some(SensorWakeUpType::NonWakeUp),
        reporting_mode: Some(SensorReportingMode::OnChange),
        ..Default::default()
    }
}

fn get_heart_rate_events() -> Vec<SensorEvent> {
    let mut events: Vec<SensorEvent> = Vec::new();
    for i in 1..4 {
        let event = SensorEvent {
            sensor_id: get_heart_rate_sensor().sensor_id.unwrap(),
            sensor_type: SensorType::HeartRate,
            payload: EventPayload::Float(i as f32),
            // These two values get ignored by playback
            sequence_number: 0,
            timestamp: 0,
        };
        events.push(event);
    }
    events
}

fn get_playback_config() -> playback_fidl::PlaybackSourceConfig {
    let test_sensor = get_heart_rate_sensor();
    let events = get_heart_rate_events();

    let fixed_values_config = playback_fidl::FixedValuesPlaybackConfig {
        sensor_list: Some(vec![test_sensor]),
        sensor_events: Some(events),
        ..Default::default()
    };

    playback_fidl::PlaybackSourceConfig::FixedValuesConfig(fixed_values_config)
}

async fn setup() -> anyhow::Result<(InstalledNamespace, ManagerProxy), anyhow::Error> {
    let realm = setup_realm().await?;
    let manager_proxy = connect_to_protocol_at::<ManagerMarker>(&realm)?;

    let _ = manager_proxy.configure_playback(&get_playback_config()).await?;

    Ok((realm, manager_proxy))
}

async fn clear_playback_config(proxy: &ManagerProxy) {
    let fixed_values_config = playback_fidl::FixedValuesPlaybackConfig {
        sensor_list: None,
        sensor_events: None,
        ..Default::default()
    };

    let _ = proxy
        .configure_playback(&playback_fidl::PlaybackSourceConfig::FixedValuesConfig(
            fixed_values_config,
        ))
        .await;
}

async fn setup_second_playback_driver(
    realm: &InstalledNamespace,
    manager: &ManagerProxy,
) -> playback_fidl::PlaybackProxy {
    let playback_proxy = connect_to_protocol_at::<playback_fidl::PlaybackMarker>(&realm).unwrap();

    let test_sensor = SensorInfo {
        sensor_id: Some(87654321),
        name: Some(String::from("HEART_RATE2")),
        vendor: Some(String::from("Fuchsia")),
        version: Some(1),
        sensor_type: Some(SensorType::HeartRate),
        wake_up: Some(SensorWakeUpType::NonWakeUp),
        reporting_mode: Some(SensorReportingMode::OnChange),
        ..Default::default()
    };

    let mut playback_events: Vec<SensorEvent> = Vec::new();
    for i in 1..4 {
        let event = SensorEvent {
            sensor_id: test_sensor.sensor_id.unwrap(),
            sensor_type: SensorType::HeartRate,
            payload: EventPayload::Float(i as f32),
            // These two values get ignored by playback
            sequence_number: 0,
            timestamp: 0,
        };
        playback_events.push(event);
    }

    let fixed_values_config = playback_fidl::FixedValuesPlaybackConfig {
        sensor_list: Some(vec![test_sensor.clone()]),
        sensor_events: Some(playback_events),
        ..Default::default()
    };

    let _ = playback_proxy
        .configure_playback(&playback_fidl::PlaybackSourceConfig::FixedValuesConfig(
            fixed_values_config,
        ))
        .await
        .unwrap();

    // Wait for the sensor service to be exposed and for the instance to be managed by the
    // SensorManager. Eventually the SensorManager could notify its clients that a new sensor has
    // been added, but currently the clients must check for new sensors.
    loop {
        let fidl_sensors = manager.get_sensors_list().await.unwrap();
        if fidl_sensors.contains(&test_sensor) {
            break;
        }
    }

    playback_proxy
}

#[fuchsia::test]
async fn test_configure_playback() -> anyhow::Result<()> {
    let (_realm, proxy) = setup().await?;
    clear_playback_config(&proxy).await;

    let mut fixed_values_config = playback_fidl::FixedValuesPlaybackConfig {
        sensor_list: Some(vec![get_heart_rate_sensor()]),
        sensor_events: Some(get_heart_rate_events()),
        ..Default::default()
    };
    let res = proxy
        .configure_playback(&playback_fidl::PlaybackSourceConfig::FixedValuesConfig(
            fixed_values_config,
        ))
        .await;
    assert!(res.is_ok());
    assert!(proxy.get_sensors_list().await.unwrap().contains(&get_heart_rate_sensor()));

    fixed_values_config = playback_fidl::FixedValuesPlaybackConfig {
        sensor_list: None,
        sensor_events: None,
        ..Default::default()
    };

    let res = proxy
        .configure_playback(&playback_fidl::PlaybackSourceConfig::FixedValuesConfig(
            fixed_values_config,
        ))
        .await;

    assert_eq!(res.unwrap(), Err(ConfigurePlaybackError::ConfigMissingFields));

    // Misconfiguring playback should remove the sensors and proxy from future requests.
    assert_eq!(
        proxy.activate(get_heart_rate_sensor().sensor_id.unwrap()).await.unwrap(),
        Err(ActivateSensorError::InvalidSensorId)
    );
    assert!(!proxy.get_sensors_list().await.unwrap().contains(&get_heart_rate_sensor()));

    Ok(())
}

#[fuchsia::test]
async fn test_get_sensors_list() -> anyhow::Result<()> {
    let (realm, proxy) = setup().await?;
    let _playback = setup_second_playback_driver(&realm, &proxy).await;

    let test_sensor = SensorInfo {
        sensor_id: Some(87654321),
        name: Some(String::from("HEART_RATE2")),
        vendor: Some(String::from("Fuchsia")),
        version: Some(1),
        sensor_type: Some(SensorType::HeartRate),
        wake_up: Some(SensorWakeUpType::NonWakeUp),
        reporting_mode: Some(SensorReportingMode::OnChange),
        ..Default::default()
    };

    let fidl_sensors = proxy.get_sensors_list().await.unwrap();
    assert!(fidl_sensors.contains(&get_heart_rate_sensor()));
    assert!(fidl_sensors.contains(&test_sensor));

    clear_playback_config(&proxy).await;

    let fidl_sensors = proxy.get_sensors_list().await.unwrap();
    assert!(!fidl_sensors.contains(&get_heart_rate_sensor()));

    Ok(())
}

#[fuchsia::test]
async fn test_activate_sensor() -> anyhow::Result<()> {
    let (_realm, proxy) = setup().await?;
    let id = get_heart_rate_sensor().sensor_id.expect("sensor_id");

    assert!(proxy.activate(id).await.unwrap().is_ok());

    // Activate an already activated sensor
    assert!(proxy.activate(id).await.unwrap().is_ok());

    assert_eq!(proxy.activate(-1).await.unwrap(), Err(ActivateSensorError::InvalidSensorId));

    clear_playback_config(&proxy).await;
    // Playback config was cleared, so this sensor should not exist anymore;
    assert_eq!(proxy.activate(id).await.unwrap(), Err(ActivateSensorError::InvalidSensorId));

    Ok(())
}

#[fuchsia::test]
async fn test_deactivate_sensor() -> anyhow::Result<()> {
    let (_realm, proxy) = setup().await?;
    let id = get_heart_rate_sensor().sensor_id.unwrap();

    assert!(proxy.deactivate(id).await?.is_ok());

    // Deactivate an already deactivated sensor.
    assert!(proxy.deactivate(id).await?.is_ok());

    // Deactivate an invalid sensor id.
    assert_eq!(proxy.deactivate(-1).await?, Err(DeactivateSensorError::InvalidSensorId));

    Ok(())
}

#[fuchsia::test]
async fn test_configure_sensor_rates() -> anyhow::Result<()> {
    let (_realm, proxy) = setup().await?;
    let id = get_heart_rate_sensor().sensor_id.unwrap();

    let mut config = SensorRateConfig {
        sampling_period_ns: Some(1),
        max_reporting_latency_ns: Some(1),
        ..Default::default()
    };

    assert!(proxy.activate(id).await?.is_ok());
    assert!(proxy.configure_sensor_rates(id, &config.clone()).await?.is_ok());

    assert_eq!(
        proxy.configure_sensor_rates(-1, &config.clone()).await?,
        Err(ConfigureSensorRateError::InvalidSensorId)
    );

    config.max_reporting_latency_ns = None;
    config.sampling_period_ns = None;
    assert_eq!(
        proxy.configure_sensor_rates(id, &config.clone()).await?,
        Err(ConfigureSensorRateError::InvalidConfig)
    );

    Ok(())
}

#[fuchsia::test]
async fn test_sensor_event_stream() -> anyhow::Result<()> {
    let (_realm, proxy) = setup().await?;
    let id = get_heart_rate_sensor().sensor_id.unwrap();

    let config = SensorRateConfig {
        sampling_period_ns: Some(0),
        max_reporting_latency_ns: Some(0),
        ..Default::default()
    };

    assert!(proxy.activate(id).await?.is_ok());
    assert!(proxy.configure_sensor_rates(id, &config.clone()).await?.is_ok());

    let mut event_stream = proxy.take_event_stream();
    let events = get_events_from_stream(4, &mut event_stream).await;

    assert_eq!(events.len(), 3);

    let test_events = get_heart_rate_events();
    for event in events {
        assert!(test_events.contains(&event));
    }

    Ok(())
}

#[fuchsia::test]
async fn test_two_clients() -> anyhow::Result<()> {
    let realm = setup_realm().await?;
    let manager1 = connect_to_protocol_at::<ManagerMarker>(&realm)?;
    let manager2 = connect_to_protocol_at::<ManagerMarker>(&realm)?;

    let _ = manager1.configure_playback(&get_playback_config()).await?;
    // Populate the internal sensors list.
    let _ = manager1.get_sensors_list().await;

    let fixed_values_config = playback_fidl::FixedValuesPlaybackConfig {
        sensor_list: Some(vec![get_heart_rate_sensor()]),
        sensor_events: Some(get_heart_rate_events()),
        ..Default::default()
    };

    let _ = manager1
        .configure_playback(&playback_fidl::PlaybackSourceConfig::FixedValuesConfig(
            fixed_values_config,
        ))
        .await?;

    let id = get_heart_rate_sensor().sensor_id.unwrap();
    assert!(manager1.activate(id).await?.is_ok());
    assert!(manager2.activate(id).await?.is_ok());

    let mut stream1 = manager1.take_event_stream();
    let mut stream2 = manager2.take_event_stream();

    let events1: Vec<SensorEvent> = get_events_from_stream(4, &mut stream1).await;
    let events2: Vec<SensorEvent> = get_events_from_stream(4, &mut stream2).await;

    let test_events = get_heart_rate_events();
    for event in &events1 {
        assert!(test_events.contains(&event));
    }

    assert_eq!(events1, events2);

    Ok(())
}

#[fuchsia::test]
async fn test_two_driver_providers() -> anyhow::Result<()> {
    let (realm, proxy) = setup().await?;
    let _playback = setup_second_playback_driver(&realm, &proxy).await;

    let config = SensorRateConfig {
        sampling_period_ns: Some(500000000), // 0.5s.
        max_reporting_latency_ns: Some(0),
        ..Default::default()
    };

    let fidl_sensors = proxy.get_sensors_list().await.unwrap();
    assert_eq!(fidl_sensors.len(), 2);

    for sensor in fidl_sensors {
        let id = sensor.sensor_id.expect("sensor_id");
        assert!(proxy.activate(id).await?.is_ok());
        assert!(proxy.configure_sensor_rates(id, &config.clone()).await?.is_ok());
    }

    let mut event_stream = proxy.take_event_stream();
    let events: Vec<SensorEvent> = get_events_from_stream(8, &mut event_stream).await;

    let mut unique_sensor_ids = HashSet::new();
    for event in events {
        unique_sensor_ids.insert(event.sensor_id);
    }
    assert_eq!(unique_sensor_ids.len(), 2);

    Ok(())
}

#[fuchsia::test]
fn test_subscribe_unsubscribe() -> anyhow::Result<()> {
    let mut exec = TestExecutor::new();
    let (realm, manager1) = exec.run_singlethreaded(setup()).unwrap();
    let manager2 = connect_to_protocol_at::<ManagerMarker>(&realm)?;

    let mut stream1 = manager1.take_event_stream();
    let mut stream2 = manager2.take_event_stream();

    // Ensure there are no events before activating the sensors.
    assert!(exec.run_until_stalled(&mut pin!(async { stream1.next().await })).is_pending());
    assert!(exec.run_until_stalled(&mut pin!(async { stream2.next().await })).is_pending());

    let id = get_heart_rate_sensor().sensor_id.unwrap();
    assert!(exec.run_singlethreaded(manager1.activate(id)).unwrap().is_ok());
    assert!(exec.run_singlethreaded(manager2.activate(id)).unwrap().is_ok());

    // Ensure both clients receive events after activating the sensor.
    let events1 = exec.run_singlethreaded(get_events_from_stream(4, &mut stream1));
    let events2 = exec.run_singlethreaded(get_events_from_stream(4, &mut stream2));

    let test_events = get_heart_rate_events();
    for event in &events1 {
        assert!(test_events.contains(&event));
    }
    assert_eq!(events1, events2);

    // Unsubscribe the first client from the sensor and ensure it gets no more events.
    assert!(exec.run_singlethreaded(manager1.deactivate(id)).unwrap().is_ok());
    assert!(exec.run_until_stalled(&mut pin!(async { stream1.next().await })).is_pending());

    // Ensure the second client continues to receive events.
    let events2 = exec.run_singlethreaded(get_events_from_stream(4, &mut stream2));
    let test_events = get_heart_rate_events();
    for event in &events2 {
        assert!(test_events.contains(&event));
    }

    Ok(())
}
