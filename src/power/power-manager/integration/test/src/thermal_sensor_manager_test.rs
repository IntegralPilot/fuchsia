// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use assert_matches::assert_matches;
use power_manager_integration_test_lib::{TestEnv, TestEnvBuilder};
use {
    fidl_fuchsia_powermanager_driver_temperaturecontrol as ftemperaturecontrol,
    fidl_fuchsia_thermal as fthermal, fuchsia_async as fasync,
};

const CPU_MANAGER_CONFIG_PATH: &'static str =
    "/pkg/thermal_sensor_manager_test/cpu_manager_node_config.json5";
const POWER_MANAGER_CONFIG_PATH: &'static str =
    "/pkg/thermal_sensor_manager_test/power_manager_node_config.json5";
const TEMPERATURE_CTRL_PATH: &'static str = "/dev/sys/platform/soc_thermal/control";
const TEMPERATURE_SENSOR: &'static str = "soc_thermal";

async fn create_test_env() -> TestEnv {
    TestEnvBuilder::new()
        .cpu_manager_node_config_path(CPU_MANAGER_CONFIG_PATH)
        .power_manager_node_config_path(POWER_MANAGER_CONFIG_PATH)
        .build()
        .await
}

#[fuchsia::test]
async fn test_list_sensors() {
    let mut env = create_test_env().await;
    let sensor_manager = env.connect_to_protocol::<fthermal::SensorManagerMarker>();
    let sensors = sensor_manager.list_sensors().await.unwrap();

    assert_eq!(
        sensors,
        vec![fthermal::SensorInfo {
            name: Some(TEMPERATURE_SENSOR.to_string()),
            ..Default::default()
        }]
    );

    env.destroy().await;
}

#[fuchsia::test]
async fn test_connect_passes_through_temperature_with_no_override() {
    let mut env = create_test_env().await;
    let sensor_control =
        env.connect_to_device::<ftemperaturecontrol::DeviceMarker>(TEMPERATURE_CTRL_PATH);
    let sensor_manager = env.connect_to_protocol::<fthermal::SensorManagerMarker>();

    let (sensor, sensor_server_end) = fidl::endpoints::create_proxy();

    sensor_manager
        .connect(fthermal::SensorManagerConnectRequest {
            name: Some(TEMPERATURE_SENSOR.to_string()),
            server_end: Some(fthermal::SensorServer_::Temperature(sensor_server_end)),
            ..Default::default()
        })
        .await
        .unwrap()
        .unwrap();

    let sensor_name = sensor.get_sensor_name().await.unwrap();
    assert_eq!(TEMPERATURE_SENSOR, &sensor_name);

    {
        let expected_temperature = 10.0;
        let set_status =
            sensor_control.set_temperature_celsius(expected_temperature).await.unwrap();
        zx::ok(set_status).unwrap();

        let (status, temperature_c) = sensor.get_temperature_celsius().await.unwrap();
        zx::ok(status).unwrap();
        assert_eq!(expected_temperature, temperature_c);
    }

    // Try a different temperature.
    {
        let expected_temperature = 14.0;
        let set_status =
            sensor_control.set_temperature_celsius(expected_temperature).await.unwrap();
        zx::ok(set_status).unwrap();

        let (status, temperature_c) = sensor.get_temperature_celsius().await.unwrap();
        zx::ok(status).unwrap();
        assert_eq!(expected_temperature, temperature_c);
    }

    env.destroy().await;
}

#[fuchsia::test]
async fn test_connect_passes_through_temperature_with_no_override_when_sensor_manager_proxy_dropped()
 {
    let mut env = create_test_env().await;
    let sensor_control =
        env.connect_to_device::<ftemperaturecontrol::DeviceMarker>(TEMPERATURE_CTRL_PATH);
    let sensor_manager = env.connect_to_protocol::<fthermal::SensorManagerMarker>();

    let (sensor, sensor_server_end) = fidl::endpoints::create_proxy();

    sensor_manager
        .connect(fthermal::SensorManagerConnectRequest {
            name: Some(TEMPERATURE_SENSOR.to_string()),
            server_end: Some(fthermal::SensorServer_::Temperature(sensor_server_end)),
            ..Default::default()
        })
        .await
        .unwrap()
        .unwrap();

    // Explicitly drop the sensor_manager proxy and wait a bit.
    drop(sensor_manager);
    fasync::Timer::new(std::time::Duration::from_secs(5)).await;

    let sensor_name = sensor.get_sensor_name().await.unwrap();
    assert_eq!(TEMPERATURE_SENSOR, &sensor_name);

    {
        let expected_temperature = 10.0;
        let set_status =
            sensor_control.set_temperature_celsius(expected_temperature).await.unwrap();
        zx::ok(set_status).unwrap();

        let (status, temperature_c) = sensor.get_temperature_celsius().await.unwrap();
        zx::ok(status).unwrap();
        assert_eq!(expected_temperature, temperature_c);
    }

    // Try a different temperature.
    {
        let expected_temperature = 14.0;
        let set_status =
            sensor_control.set_temperature_celsius(expected_temperature).await.unwrap();
        zx::ok(set_status).unwrap();

        let (status, temperature_c) = sensor.get_temperature_celsius().await.unwrap();
        zx::ok(status).unwrap();
        assert_eq!(expected_temperature, temperature_c);
    }

    env.destroy().await;
}

#[fuchsia::test]
async fn test_connect_passes_through_temperature_with_temperature_override() {
    let mut env = create_test_env().await;
    let sensor_control =
        env.connect_to_device::<ftemperaturecontrol::DeviceMarker>(TEMPERATURE_CTRL_PATH);
    let sensor_manager = env.connect_to_protocol::<fthermal::SensorManagerMarker>();

    let (sensor, sensor_server_end) = fidl::endpoints::create_proxy();

    sensor_manager
        .connect(fthermal::SensorManagerConnectRequest {
            name: Some(TEMPERATURE_SENSOR.to_string()),
            server_end: Some(fthermal::SensorServer_::Temperature(sensor_server_end)),
            ..Default::default()
        })
        .await
        .unwrap()
        .unwrap();

    let sensor_name = sensor.get_sensor_name().await.unwrap();
    assert_eq!(TEMPERATURE_SENSOR, &sensor_name);

    {
        let sensor_temperature = 10.0;
        let set_status = sensor_control.set_temperature_celsius(sensor_temperature).await.unwrap();
        zx::ok(set_status).unwrap();

        // Server should return this temperature rather than the 'real' sensor temperature.
        let expected_temperature: f32 = 23.0;
        sensor_manager
            .set_temperature_override(TEMPERATURE_SENSOR, expected_temperature.into())
            .await
            .unwrap()
            .unwrap();

        let (status, temperature_c) = sensor.get_temperature_celsius().await.unwrap();
        zx::ok(status).unwrap();
        assert_eq!(expected_temperature, temperature_c);
    }

    // Try a different temperature.
    {
        let sensor_temperature = 14.0;
        let set_status = sensor_control.set_temperature_celsius(sensor_temperature).await.unwrap();
        zx::ok(set_status).unwrap();

        // Server should return this temperature rather than the 'real' sensor temperature.
        let expected_temperature: f32 = 35.0;
        sensor_manager
            .set_temperature_override(TEMPERATURE_SENSOR, expected_temperature.into())
            .await
            .unwrap()
            .unwrap();

        let (status, temperature_c) = sensor.get_temperature_celsius().await.unwrap();
        zx::ok(status).unwrap();
        assert_eq!(expected_temperature, temperature_c);
    }

    env.destroy().await;
}

#[fuchsia::test]
async fn test_connect_passes_through_temperature_uses_real_temperature_after_override_cleared() {
    let mut env = create_test_env().await;
    let sensor_control =
        env.connect_to_device::<ftemperaturecontrol::DeviceMarker>(TEMPERATURE_CTRL_PATH);
    let sensor_manager = env.connect_to_protocol::<fthermal::SensorManagerMarker>();

    let (sensor, sensor_server_end) = fidl::endpoints::create_proxy();

    sensor_manager
        .connect(fthermal::SensorManagerConnectRequest {
            name: Some(TEMPERATURE_SENSOR.to_string()),
            server_end: Some(fthermal::SensorServer_::Temperature(sensor_server_end)),
            ..Default::default()
        })
        .await
        .unwrap()
        .unwrap();

    let sensor_name = sensor.get_sensor_name().await.unwrap();
    assert_eq!(TEMPERATURE_SENSOR, &sensor_name);

    {
        let sensor_temperature = 10.0;
        let set_status = sensor_control.set_temperature_celsius(sensor_temperature).await.unwrap();
        zx::ok(set_status).unwrap();

        // Server should return this temperature rather than the 'real' sensor temperature.
        let expected_temperature: f32 = 23.0;
        sensor_manager
            .set_temperature_override(TEMPERATURE_SENSOR, expected_temperature.into())
            .await
            .unwrap()
            .unwrap();

        let (status, temperature_c) = sensor.get_temperature_celsius().await.unwrap();
        zx::ok(status).unwrap();
        assert_eq!(expected_temperature, temperature_c);
    }

    // Try a different temperature and clear override.
    {
        let expected_temperature = 24.0;
        let set_status =
            sensor_control.set_temperature_celsius(expected_temperature).await.unwrap();
        zx::ok(set_status).unwrap();

        sensor_manager.clear_temperature_override(TEMPERATURE_SENSOR).await.unwrap().unwrap();

        let (status, temperature_c) = sensor.get_temperature_celsius().await.unwrap();
        zx::ok(status).unwrap();
        assert_eq!(expected_temperature, temperature_c);
    }

    env.destroy().await;
}

#[fuchsia::test]
async fn test_set_temperature_override_returns_error_on_unknown_sensor() {
    let mut env = create_test_env().await;
    let sensor_manager = env.connect_to_protocol::<fthermal::SensorManagerMarker>();
    assert_matches!(
        sensor_manager.set_temperature_override("unknown-sensor", 0.0).await,
        Ok(Err(fthermal::SetTemperatureOverrideError::SensorNotFound))
    );
    env.destroy().await;
}

#[fuchsia::test]
async fn test_clear_temperature_override_returns_error_on_unknown_sensor() {
    let mut env = create_test_env().await;
    let sensor_manager = env.connect_to_protocol::<fthermal::SensorManagerMarker>();
    assert_matches!(
        sensor_manager.clear_temperature_override("unknown-sensor").await,
        Ok(Err(fthermal::ClearTemperatureOverrideError::SensorNotFound))
    );
    env.destroy().await;
}

#[fuchsia::test]
async fn test_connect_returns_error_on_unknown_sensor() {
    let mut env = create_test_env().await;
    let sensor_manager = env.connect_to_protocol::<fthermal::SensorManagerMarker>();

    let (_sensor, sensor_server_end) = fidl::endpoints::create_proxy();
    assert_matches!(
        sensor_manager
            .connect(fthermal::SensorManagerConnectRequest {
                name: Some("unknown-sensor".to_string()),
                server_end: Some(fthermal::SensorServer_::Temperature(sensor_server_end)),
                ..Default::default()
            })
            .await,
        Ok(Err(fthermal::ConnectError::SensorNotFound))
    );
    env.destroy().await;
}

#[fuchsia::test]
async fn test_connect_returns_error_with_missing_args() {
    let mut env = create_test_env().await;
    let sensor_manager = env.connect_to_protocol::<fthermal::SensorManagerMarker>();

    let (_sensor, sensor_server_end) = fidl::endpoints::create_proxy();
    assert_matches!(
        sensor_manager.connect(Default::default()).await,
        Ok(Err(fthermal::ConnectError::InvalidArguments))
    );
    assert_matches!(
        sensor_manager
            .connect(fthermal::SensorManagerConnectRequest {
                server_end: Some(fthermal::SensorServer_::Temperature(sensor_server_end)),
                ..Default::default()
            })
            .await,
        Ok(Err(fthermal::ConnectError::InvalidArguments))
    );
    assert_matches!(
        sensor_manager
            .connect(fthermal::SensorManagerConnectRequest {
                name: Some("unknown-sensor".to_string()),
                ..Default::default()
            })
            .await,
        Ok(Err(fthermal::ConnectError::InvalidArguments))
    );
    env.destroy().await;
}
