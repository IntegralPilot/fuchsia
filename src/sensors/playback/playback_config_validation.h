// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_SENSORS_PLAYBACK_PLAYBACK_CONFIG_VALIDATION_H_
#define SRC_SENSORS_PLAYBACK_PLAYBACK_CONFIG_VALIDATION_H_

#include <fidl/fuchsia.hardware.sensors/cpp/fidl.h>

namespace sensors::playback {

// Check that the payload union is the correct data type for the sensor type.
bool CheckPayloadForType(fuchsia_sensors_types::SensorType type,
                         const fuchsia_sensors_types::EventPayload& payload);

// Determine if a list of SensorInfo has all the required fields and no duplicate sensor IDs.
std::optional<fuchsia_hardware_sensors::ConfigurePlaybackError> ValidateSensorList(
    const std::optional<std::vector<fuchsia_sensors_types::SensorInfo>>& sensor_list);

// Check for a non-emtpy sensor event list, and that the events are valid.
std::optional<fuchsia_hardware_sensors::ConfigurePlaybackError> ValidateSensorEventList(
    const std::optional<std::vector<fuchsia_sensors_types::SensorEvent>>& sensor_events,
    const std::vector<fuchsia_sensors_types::SensorInfo>& sensor_list);

}  // namespace sensors::playback

#endif  // SRC_SENSORS_PLAYBACK_PLAYBACK_CONFIG_VALIDATION_H_
