// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl::endpoints::DiscoverableProtocolMarker;
use fidl_fuchsia_thermal as fthermal;
use fuchsia_component_test::{Capability, ChildOptions, RealmBuilder, RealmInstance, Ref, Route};
use log::*;

const SENSOR_NAME: &'static str = "fake-trippoint";

struct TestEnv {
    realm_instance: RealmInstance,
}

impl TestEnv {
    /// Connects to a protocol exposed by a component within the RealmInstance.
    pub fn connect_to_protocol<P: DiscoverableProtocolMarker>(&self) -> P::Proxy {
        self.realm_instance.root.connect_to_protocol_at_exposed_dir().unwrap()
    }
}

async fn create_test_env() -> TestEnv {
    info!("building the test env");

    let builder = RealmBuilder::new().await.unwrap();

    let component_ref = builder
        .add_child(
            "fake-thermal-sensor-manager",
            "#meta/fake-thermal-sensor-manager.cm",
            ChildOptions::new(),
        )
        .await
        .expect("Failed to add child: fake-thermal-sensor-manager");

    // Expose capabilities from fake-thermal-sensor-manager.
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol_by_name("fuchsia.thermal.SensorManager"))
                .from(&component_ref)
                .to(Ref::parent()),
        )
        .await
        .unwrap();

    let realm_instance = builder.build().await.expect("Failed to build RealmInstance");
    TestEnv { realm_instance }
}

#[fuchsia::test]
async fn list_sensors_returns_fake_sensor() {
    let env = create_test_env().await;
    let sensor_manager = env.connect_to_protocol::<fthermal::SensorManagerMarker>();
    let sensors = sensor_manager.list_sensors().await.unwrap();
    assert_eq!(1, sensors.len());
    assert_eq!(
        fthermal::SensorInfo { name: Some(SENSOR_NAME.to_string()), ..Default::default() },
        sensors[0]
    )
}

#[fuchsia::test]
async fn connect_returns_fake_data() {
    let env = create_test_env().await;
    let sensor_manager = env.connect_to_protocol::<fthermal::SensorManagerMarker>();
    let (sensor, server_end) = fidl::endpoints::create_proxy();

    sensor_manager
        .connect(fthermal::SensorManagerConnectRequest {
            name: Some(SENSOR_NAME.to_string()),
            server_end: Some(fthermal::SensorServer_::Temperature(server_end)),
            ..Default::default()
        })
        .await
        .unwrap()
        .unwrap();

    let (status, temp_c) = sensor.get_temperature_celsius().await.unwrap();
    assert_eq!(zx::sys::ZX_OK, status);
    assert_eq!(25.0, temp_c);

    let sensor_name = sensor.get_sensor_name().await.unwrap();
    assert_eq!(SENSOR_NAME, sensor_name);
}

#[fuchsia::test]
async fn setting_and_clearing_temperature_override_works() {
    let env = create_test_env().await;
    let sensor_manager = env.connect_to_protocol::<fthermal::SensorManagerMarker>();
    let (sensor, server_end) = fidl::endpoints::create_proxy();
    let override_temperature: f32 = 21.0;

    sensor_manager
        .set_temperature_override(SENSOR_NAME, override_temperature.into())
        .await
        .unwrap()
        .unwrap();

    sensor_manager
        .connect(fthermal::SensorManagerConnectRequest {
            name: Some(SENSOR_NAME.to_string()),
            server_end: Some(fthermal::SensorServer_::Temperature(server_end)),
            ..Default::default()
        })
        .await
        .unwrap()
        .unwrap();

    let (status, temp_c) = sensor.get_temperature_celsius().await.unwrap();
    assert_eq!(zx::sys::ZX_OK, status);
    assert_eq!(override_temperature, temp_c);

    sensor_manager.clear_temperature_override(SENSOR_NAME).await.unwrap().unwrap();

    let (status, temp_c) = sensor.get_temperature_celsius().await.unwrap();
    assert_eq!(zx::sys::ZX_OK, status);
    assert_eq!(25.0, temp_c);
}
