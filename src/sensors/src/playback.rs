// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use crate::sensor_manager::{Sensor, SensorId};
use crate::utils::is_sensor_valid;
use fidl::AsHandleRef;
use fidl::endpoints::Proxy;
use fidl_fuchsia_hardware_sensors::{self as driver_fidl, PlaybackSourceConfig};
use fidl_fuchsia_sensors::ConfigurePlaybackError;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone)]
pub struct Playback {
    pub(crate) driver_proxy: driver_fidl::DriverProxy,
    pub(crate) playback_proxy: driver_fidl::PlaybackProxy,
    pub(crate) playback_sensor_ids: Vec<SensorId>,
    pub(crate) configured: bool,
}

impl Playback {
    pub fn new(
        driver_proxy: driver_fidl::DriverProxy,
        playback_proxy: driver_fidl::PlaybackProxy,
    ) -> Self {
        Self { driver_proxy, playback_proxy, playback_sensor_ids: Vec::new(), configured: false }
    }

    pub(crate) async fn get_sensors_from_config(
        &mut self,
        source_config: PlaybackSourceConfig,
    ) -> HashMap<SensorId, Sensor> {
        self.playback_sensor_ids.clear();
        let mut sensors = HashMap::<SensorId, Sensor>::new();

        // In a FixedValuesConfig, the list of sensors is known, so they can be
        // added directly to the map of sensors.
        //
        // In a FilePlaybackConfig, the playback_controller needs to read the list
        // of sensors from a file first, so the list needs to come from the proxy.
        let sensor_list = if let PlaybackSourceConfig::FixedValuesConfig(val) = source_config {
            val.sensor_list.unwrap_or(Vec::new())
        } else {
            self.driver_proxy.get_sensors_list().await.unwrap_or(Vec::new())
        };

        for sensor in sensor_list {
            if is_sensor_valid(&sensor) {
                let id = sensor.sensor_id.expect("sensor_id");

                sensors.insert(
                    id,
                    Sensor {
                        driver: self.driver_proxy.clone(),
                        info: sensor,
                        clients: HashSet::new(),
                    },
                );
                self.playback_sensor_ids.push(id);
            }
        }

        sensors
    }

    pub(crate) fn is_playback_driver_proxy(&self, driver_proxy: &driver_fidl::DriverProxy) -> bool {
        driver_proxy.as_channel().as_handle_ref().raw_handle()
            == self.driver_proxy.as_channel().as_handle_ref().raw_handle()
    }
}

pub fn from_driver_playback_error(
    val: driver_fidl::ConfigurePlaybackError,
) -> ConfigurePlaybackError {
    match val {
        driver_fidl::ConfigurePlaybackError::InvalidConfigType => {
            ConfigurePlaybackError::InvalidConfigType
        }
        driver_fidl::ConfigurePlaybackError::ConfigMissingFields => {
            ConfigurePlaybackError::ConfigMissingFields
        }
        driver_fidl::ConfigurePlaybackError::DuplicateSensorInfo => {
            ConfigurePlaybackError::DuplicateSensorInfo
        }
        driver_fidl::ConfigurePlaybackError::NoEventsForSensor => {
            ConfigurePlaybackError::NoEventsForSensor
        }
        driver_fidl::ConfigurePlaybackError::EventFromUnknownSensor => {
            ConfigurePlaybackError::EventFromUnknownSensor
        }
        driver_fidl::ConfigurePlaybackError::EventSensorTypeMismatch => {
            ConfigurePlaybackError::EventSensorTypeMismatch
        }
        driver_fidl::ConfigurePlaybackError::EventPayloadTypeMismatch => {
            ConfigurePlaybackError::EventPayloadTypeMismatch
        }
        driver_fidl::ConfigurePlaybackError::FileOpenFailed => {
            ConfigurePlaybackError::FileOpenFailed
        }
        driver_fidl::ConfigurePlaybackError::FileParseError => {
            ConfigurePlaybackError::FileParseError
        }
        driver_fidl::ConfigurePlaybackError::__SourceBreaking { unknown_ordinal } => {
            // This should be unreachable because playback is subpackaged with the sensor manager.
            log::error!(
                "Received unknown error from Sensor Playback with ordinal: {:#?}",
                unknown_ordinal
            );
            ConfigurePlaybackError::PlaybackUnavailable
        }
    }
}
