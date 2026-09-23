// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result};
use fidl::endpoints::RequestStream;
use fidl_fuchsia_hardware_sensors as fsensors;
use fidl_fuchsia_sensors_types::{
    EventPayload, Scale, SensorEvent, SensorInfo, SensorReportingMode, SensorType,
    SensorWakeUpType, Unit,
};
use fuchsia_async as fasync;
use fuchsia_component::server::ServiceFs;
use futures::future::join_all;
use futures::lock::Mutex;
use futures::{StreamExt, TryStreamExt};
use log::{info, warn};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::time::{Duration, Instant};

struct TimeSource {
    pub boot_instant: Instant,
}

impl TimeSource {
    fn new() -> TimeSource {
        Self { boot_instant: Instant::now() }
    }

    fn get_monotonic_time_from_boot(&self) -> i64 {
        self.boot_instant.elapsed().as_millis() as i64
    }
}

/// Represents the overall state of the fake Power Sensor
struct FakeSensorState {
    pub sensors: HashMap<i32, Arc<Mutex<FakeSensor>>>,
    pub sensor_frequencies: HashMap<i32, Arc<AtomicI64>>,
    pub enable_flags: HashMap<i32, Arc<AtomicBool>>,
}

impl FakeSensorState {
    pub fn new_default() -> FakeSensorState {
        let mut sensors = HashMap::new();
        let mut sensor_frequencies = HashMap::new();
        let mut enable_flags = HashMap::new();

        let time_source = Arc::new(Mutex::new(TimeSource::new()));

        // Construct and initialize default FakeSensors
        for id in 0..3 {
            let frequency_ns = Arc::new(AtomicI64::new(1000000000)); // default 1s sampling period
            sensor_frequencies.insert(id, frequency_ns.clone());

            let enable_flag = Arc::new(AtomicBool::new(false)); // disable by default
            enable_flags.insert(id, enable_flag.clone());

            let sensor = Arc::new(Mutex::new(FakeSensor::new(
                id,
                frequency_ns,
                enable_flag,
                id as f32,
                time_source.clone(),
            )));
            sensors.insert(id, sensor);
        }

        FakeSensorState { sensors, sensor_frequencies, enable_flags }
    }
}

/// Represents a fake sensor with one measurement rail
#[derive(Clone)]
struct FakeSensor {
    pub info: SensorInfo,             // Sensor config info
    pub frequency_ns: Arc<AtomicI64>, // Externally-adjustable sampling period
    pub enabled: Arc<AtomicBool>,     // Flag for enabling/disabling the sensor
    pub measurement_value: f32,       // Fixed measurement value to return

    pub sequence_number: Arc<AtomicU64>, // Internal increment-only seequence number for events
    pub time_source: Arc<Mutex<TimeSource>>, // Monotonic clock for timestamps
}

impl FakeSensor {
    // Constructs a new FakeSensor with a specified ID and fixed measurement value to return
    pub fn new(
        sensor_id: i32,
        frequency_ns: Arc<AtomicI64>,
        enabled: Arc<AtomicBool>,
        measurement_value: f32,
        time_source: Arc<Mutex<TimeSource>>,
    ) -> FakeSensor {
        FakeSensor {
            info: SensorInfo {
                sensor_id: Some(sensor_id),
                name: Some(format!("fake_sensor_{}", sensor_id)),
                vendor: Some("fake_sensor_vendor".to_string()),
                version: Some(10),
                sensor_type: Some(SensorType::Power),
                wake_up: Some(SensorWakeUpType::NonWakeUp),
                reporting_mode: Some(SensorReportingMode::Continuous),
                measurement_unit: Some(Unit::Joules),
                measurement_scale: Some(Scale::Micro),
                ..Default::default()
            },
            frequency_ns,
            enabled,
            measurement_value,
            sequence_number: Arc::new(AtomicU64::new(0)),
            time_source,
        }
    }
}

/// Handler for Driver requests, served over the Service service capability
async fn handle_driver_service_request(
    mut stream: fsensors::DriverRequestStream,
    fake_sensor_state: Arc<Mutex<FakeSensorState>>,
) {
    while let Ok(Some(request)) = stream.try_next().await {
        match request {
            // Return info for available sensors
            fsensors::DriverRequest::GetSensorsList { responder } => {
                info!("Got GetSensorsList request");
                let fake_sensors = &fake_sensor_state.lock().await.sensors;
                let fake_sensor_info: Vec<SensorInfo> = join_all(
                    fake_sensors
                        .iter()
                        .map(|(_id, sensor)| async { sensor.lock().await.info.clone() }),
                )
                .await;

                let _ = responder.send(&fake_sensor_info);
                info!("Handled GetSensorsList request");
            }
            // Activate the sensor with the specified sensor_id
            fsensors::DriverRequest::ActivateSensor { sensor_id, responder } => {
                info!("Got ActivateSensor request: {}", sensor_id);

                // Retrieve reference to the sensor associated with the ID or return an error if
                // the ID is invalid
                let (Some(fake_sensor), Some(sensor_enabled)) = ({
                    let state = fake_sensor_state.lock().await;
                    (
                        state.sensors.get(&sensor_id).cloned(),
                        state.enable_flags.get(&sensor_id).cloned(),
                    )
                }) else {
                    warn!("Invalid sensor ID {}", sensor_id);
                    let _ = responder.send(Err(fsensors::ActivateSensorError::InvalidSensorId));
                    continue;
                };

                // Sensors should only have one async Task for sampling. If the sensor is already
                // active, don't spawn a duplicate Task.
                if sensor_enabled.load(Ordering::Acquire) == true {
                    info!("Sensor {} already active", sensor_id);
                    let _ = responder.send(Ok(()));
                    continue;
                }

                info!("Activating sensor {}", sensor_id);

                // Set the sensor as enabled and spawn an async Task to send measurements.
                // Activating a sensor that is already active is treated as a NO-OP.
                sensor_enabled.store(true, Ordering::Release);
                fasync::Task::local({
                    let fake_sensor: Arc<Mutex<FakeSensor>> = fake_sensor;
                    let control_handle = stream.control_handle();

                    async move {
                        loop {
                            let sensor = fake_sensor.lock().await.clone();

                            // End the Task if sensor is disabled (i.e. client called
                            // Driver.DeactivateSensor())
                            if sensor.enabled.load(Ordering::Acquire) == false {
                                info!("Stopping sensor");
                                break;
                            }

                            // Simulate periodic measurements by delaying for the configured
                            // sampling frequency before sending the measurement
                            fasync::Timer::new(Duration::from_nanos(
                                sensor.frequency_ns.load(Ordering::Acquire) as u64,
                            ))
                            .await;

                            let sequence_num =
                                sensor.sequence_number.fetch_add(1, Ordering::SeqCst);
                            let _ = control_handle.send_on_sensor_event(&SensorEvent {
                                timestamp: sensor
                                    .time_source
                                    .lock()
                                    .await
                                    .get_monotonic_time_from_boot(),
                                sensor_id: sensor.info.sensor_id.unwrap(),
                                sensor_type: sensor.info.sensor_type.unwrap(),
                                sequence_number: sequence_num,
                                payload: EventPayload::Float(sensor.measurement_value),
                            });
                        }
                    }
                })
                .detach();

                info!("Activated sensor {}", sensor_id);

                let _ = responder.send(Ok(()));

                info!("Handled ActivateSensor request");
            }
            // Deactivate the specified sensor and stop sending measurements. Calling
            // DeactivateSensor() on a sensor that is already deactivated is treated as a NO-OP.
            fsensors::DriverRequest::DeactivateSensor { sensor_id, responder } => {
                info!("Got DeactivateSensor request: {}", sensor_id);

                let enabled_hash = &fake_sensor_state.lock().await.enable_flags;
                let Some(sensor_enable_flag) = enabled_hash.get(&sensor_id) else {
                    warn!("Invalid sensor ID {}", sensor_id);
                    let _ = responder.send(Err(fsensors::DeactivateSensorError::InvalidSensorId));
                    continue;
                };

                sensor_enable_flag.store(false, Ordering::Release);

                let _ = responder.send(Ok(()));

                info!("Handled DeactivateSensor request");
            }
            // Set a new measurement reporting rate for the specified sensor
            fsensors::DriverRequest::ConfigureSensorRate {
                sensor_id,
                sensor_rate_config,
                responder,
            } => {
                info!("Got ConfigureSensorRate request: {}", sensor_id);

                // Validate new requested sampling rate
                if sensor_rate_config.sampling_period_ns.is_none() {
                    warn!("Invalid requested sampling period (unspecified)");
                    let _ = responder.send(Err(fsensors::ConfigureSensorRateError::InvalidConfig));
                    continue;
                } else if sensor_rate_config.sampling_period_ns.is_some_and(|rate| rate < 0) {
                    warn!("Invalid requested sampling period (negative value)");
                    let _ = responder.send(Err(fsensors::ConfigureSensorRateError::InvalidConfig));
                    continue;
                }

                // Validate/extract the frequency setting for the sensor
                let frequencies_hash = &fake_sensor_state.lock().await.sensor_frequencies;
                let Some(sensor_frequency) = frequencies_hash.get(&sensor_id) else {
                    warn!("Invalid sensor ID {}", sensor_id);
                    let _ =
                        responder.send(Err(fsensors::ConfigureSensorRateError::InvalidSensorId));
                    continue;
                };

                sensor_frequency
                    .store(sensor_rate_config.sampling_period_ns.unwrap(), Ordering::Release);

                let _ = responder.send(Ok(()));

                info!("Handled ConfigureSensorRate request");
            }
            fsensors::DriverRequest::_UnknownMethod { ordinal: _, .. } => {
                info!("Got unknown Driver request!");
            }
        }
    }
}

enum IncomingService {
    DriverService(fsensors::ServiceRequest),
}

#[fuchsia::main]
async fn main() -> Result<()> {
    info!("Starting fake power sensor...");
    // Initialize the internal state
    let fake_sensor_state = Arc::new(Mutex::new(FakeSensorState::new_default()));

    // Initialize and serve the outgoing namespace with the Driver Service
    let mut service_fs = ServiceFs::new_local();
    service_fs
        .dir("svc")
        .add_fidl_service_instance("fake_power_sensor_driver", IncomingService::DriverService);
    service_fs.take_and_serve_directory_handle().context("failed to serve outgoing namespace")?;

    // Start the async handler for incoming Service requests
    service_fs
        .for_each_concurrent(None, move |request: IncomingService| {
            let fake_sensor_state = fake_sensor_state.clone();
            async move {
                match request {
                    IncomingService::DriverService(fsensors::ServiceRequest::Driver(stream)) => {
                        handle_driver_service_request(stream, fake_sensor_state).await
                    }
                }
            }
        })
        .await;

    info!("Fake power sensor ended unexpectedly");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use fidl::endpoints::*;
    use fidl_fuchsia_sensors_types as fsensors_types;

    /// Constructs a default FakeSensorState for unittesting
    fn construct_test_state() -> (fsensors::DriverProxy, Arc<Mutex<FakeSensorState>>) {
        let test_state = Arc::new(Mutex::new(FakeSensorState::new_default()));
        let (driver_proxy, driver_stream) = create_proxy_and_stream::<fsensors::DriverMarker>();

        fasync::Task::spawn({
            let state = test_state.clone();
            async move {
                handle_driver_service_request(driver_stream, state).await;
            }
        })
        .detach();

        (driver_proxy, test_state.clone())
    }

    /// Constructs a SensorRateConfig with a given sampling period
    fn construct_sensor_rate_config(rate: Option<i64>) -> fsensors_types::SensorRateConfig {
        fsensors_types::SensorRateConfig {
            sampling_period_ns: rate,
            max_reporting_latency_ns: Some(100000), // Value not used by fake sensor
            ..Default::default()
        }
    }

    /// Verifies the result of a Driver.ActivateSensor() call
    fn validate_sensor_activation_result(
        result: &Result<fsensors::DriverActivateSensorResult, fidl::Error>,
        expected: Result<(), fsensors::ActivateSensorError>,
    ) {
        assert!(result.is_ok());

        let activate_result = result.clone().unwrap();
        assert_eq!(activate_result, expected);
    }

    /// Verifies the result of a Driver.DeactivateSensor() call
    fn validate_sensor_deactivation_result(
        result: &Result<fsensors::DriverDeactivateSensorResult, fidl::Error>,
        expected: Result<(), fsensors::DeactivateSensorError>,
    ) {
        assert!(result.is_ok());

        let deactivate_result = result.clone().unwrap();
        assert_eq!(deactivate_result, expected);
    }

    /// Verifies the result of a Driver.ConfigureSensorRate() call
    fn validate_sensor_config_result(
        result: &Result<fsensors::DriverConfigureSensorRateResult, fidl::Error>,
        expected: Result<(), fsensors::ConfigureSensorRateError>,
    ) {
        assert!(result.is_ok());

        let activate_result = result.clone().unwrap();
        assert_eq!(activate_result, expected);
    }

    /// Verifies a sensor's state given the sensor ID and expected state (true = active)
    async fn validate_sensor_activation_state(
        sensor_id: i32,
        expected_state: bool,
        sensor_state: &Arc<Mutex<FakeSensorState>>,
    ) {
        let state = sensor_state.lock().await;

        let fake_sensor_enabled = state.enable_flags.get(&sensor_id).cloned();
        assert!(
            fake_sensor_enabled.is_some_and(|flag| flag.load(Ordering::Acquire) == expected_state)
        );
    }

    /// Verifies a sensor's frequency given the sensor ID and expected frequency
    async fn validate_sensor_frequency(
        sensor_id: i32,
        expected_freq: i64,
        sensor_state: &Arc<Mutex<FakeSensorState>>,
    ) {
        let state = sensor_state.lock().await;

        let fake_sensor_freq = state.sensor_frequencies.get(&sensor_id).cloned();
        assert!(fake_sensor_freq.is_some_and(|freq| freq.load(Ordering::Acquire) == expected_freq));
    }

    // Validate implementation of fsensors.Driver.GetSensorInfo() FIDL method
    #[fuchsia::test]
    async fn test_get_sensors_info() {
        let (driver_proxy, _state) = construct_test_state();

        let sensors_result = driver_proxy.get_sensors_list().await;
        assert!(sensors_result.is_ok());

        let sensors = sensors_result.unwrap();
        assert_eq!(sensors.len(), 3);

        // Verify SensorInfo for each reported fake sensor
        for id in 0..3 {
            let target_sensor_info =
                sensors.iter().find(|fake_sensor_info| fake_sensor_info.sensor_id == Some(id));
            assert!(target_sensor_info.is_some());

            let info = target_sensor_info.unwrap();

            assert_eq!(info.name, Some(format!("fake_sensor_{}", id)));
            assert_eq!(info.vendor, Some("fake_sensor_vendor".to_string()));
            assert_eq!(info.version, Some(10));
            assert_eq!(info.sensor_type, Some(SensorType::Power));
            assert_eq!(info.wake_up, Some(SensorWakeUpType::NonWakeUp));
            assert_eq!(info.reporting_mode, Some(SensorReportingMode::Continuous));
            assert_eq!(info.measurement_unit, Some(Unit::Joules));
            assert_eq!(info.measurement_scale, Some(Scale::Micro));
        }
    }

    // Validate implementations of fsensors.Driver.ActivateSensor() and
    // fsensors.Driver.DeactivateSensor() FIDL methods
    #[fuchsia::test]
    async fn test_activate_deactivate_sensor() {
        let (driver_proxy, state) = construct_test_state();

        // Activate with valid sensor ID
        let mut activate_result = driver_proxy.activate_sensor(0).await;
        validate_sensor_activation_result(&activate_result, Ok(()));
        validate_sensor_activation_state(0, true, &state).await;

        // Activate with invalid sensor IDs
        activate_result = driver_proxy.activate_sensor(3).await;
        validate_sensor_activation_result(
            &activate_result,
            Err(fsensors::ActivateSensorError::InvalidSensorId),
        );

        // Activating a sensor that is already active should return a success
        activate_result = driver_proxy.activate_sensor(0).await;
        validate_sensor_activation_result(&activate_result, Ok(()));
        validate_sensor_activation_state(0, true, &state).await;

        // Deactivate with valid sensor ID
        let mut deactivate_result = driver_proxy.deactivate_sensor(0).await;
        validate_sensor_deactivation_result(&deactivate_result, Ok(()));
        validate_sensor_activation_state(0, false, &state).await;

        // Deactivate with invalid sensor ID
        deactivate_result = driver_proxy.deactivate_sensor(3).await;
        validate_sensor_deactivation_result(
            &deactivate_result,
            Err(fsensors::DeactivateSensorError::InvalidSensorId),
        );

        // Deactivating a sensor that is already deactivated should return a success
        deactivate_result = driver_proxy.deactivate_sensor(0).await;
        validate_sensor_deactivation_result(&deactivate_result, Ok(()));
        validate_sensor_activation_state(0, false, &state).await;
    }

    // Validate implementation of fsensors.Driver.ConfigureSensorRate() FIDL method
    #[fuchsia::test]
    async fn test_configure_sensor_rate() {
        let (driver_proxy, state) = construct_test_state();

        // Valid ID and config value
        let mut configure_result = driver_proxy
            .configure_sensor_rate(0, &construct_sensor_rate_config(Some(2000000000)))
            .await;
        validate_sensor_config_result(&configure_result, Ok(()));
        validate_sensor_frequency(0, 2000000000, &state).await;

        // Valid ID, invalid config values
        configure_result =
            driver_proxy.configure_sensor_rate(0, &construct_sensor_rate_config(Some(-10))).await;
        validate_sensor_config_result(
            &configure_result,
            Err(fsensors::ConfigureSensorRateError::InvalidConfig),
        );
        validate_sensor_frequency(0, 2000000000, &state).await;

        configure_result =
            driver_proxy.configure_sensor_rate(0, &construct_sensor_rate_config(None)).await;
        validate_sensor_config_result(
            &configure_result,
            Err(fsensors::ConfigureSensorRateError::InvalidConfig),
        );
        validate_sensor_frequency(0, 2000000000, &state).await;

        // Invalid ID, valid config
        configure_result = driver_proxy
            .configure_sensor_rate(3, &construct_sensor_rate_config(Some(3000000000)))
            .await;
        validate_sensor_config_result(
            &configure_result,
            Err(fsensors::ConfigureSensorRateError::InvalidSensorId),
        );

        // Invalid ID and config value
        configure_result =
            driver_proxy.configure_sensor_rate(3, &construct_sensor_rate_config(Some(-10))).await;
        validate_sensor_config_result(
            &configure_result,
            Err(fsensors::ConfigureSensorRateError::InvalidConfig),
        );

        configure_result =
            driver_proxy.configure_sensor_rate(3, &construct_sensor_rate_config(None)).await;
        validate_sensor_config_result(
            &configure_result,
            Err(fsensors::ConfigureSensorRateError::InvalidConfig),
        );
    }
}
