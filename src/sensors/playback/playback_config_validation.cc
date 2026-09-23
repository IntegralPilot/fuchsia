// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/sensors/playback/playback_config_validation.h"

#include <unordered_map>
#include <unordered_set>
#include <vector>

namespace sensors::playback {
namespace {

using ConfigurePlaybackError = fuchsia_hardware_sensors::ConfigurePlaybackError;
using EventPayload = fuchsia_sensors_types::EventPayload;
using SensorEvent = fuchsia_sensors_types::SensorEvent;
using SensorId = fuchsia_sensors_types::SensorId;
using SensorInfo = fuchsia_sensors_types::SensorInfo;
using SensorType = fuchsia_sensors_types::SensorType;

}  // namespace

// Check that the payload union is the correct data type for the sensor type.
bool CheckPayloadForType(SensorType type, const EventPayload& payload) {
  switch (type) {
    case SensorType::kAccelerometer:
    case SensorType::kMagneticField:
    case SensorType::kOrientation:
    case SensorType::kGyroscope:
    case SensorType::kGravity:
    case SensorType::kLinearAcceleration:
      return payload.Which() == EventPayload::Tag::kVec3;
      break;

    case SensorType::kRotationVector:
    case SensorType::kGeomagneticRotationVector:
    case SensorType::kGameRotationVector:
      return payload.Which() == EventPayload::Tag::kQuaternion;
      break;

    case SensorType::kMagneticFieldUncalibrated:
    case SensorType::kGyroscopeUncalibrated:
    case SensorType::kAccelerometerUncalibrated:
      return payload.Which() == EventPayload::Tag::kUncalibratedVec3;
      break;

    case SensorType::kDeviceOrientation:
    case SensorType::kLight:
    case SensorType::kPressure:
    case SensorType::kProximity:
    case SensorType::kRelativeHumidity:
    case SensorType::kAmbientTemperature:
    case SensorType::kSignificantMotion:
    case SensorType::kStepDetector:
    case SensorType::kTiltDetector:
    case SensorType::kWakeGesture:
    case SensorType::kGlanceGesture:
    case SensorType::kPickUpGesture:
    case SensorType::kWristTiltGesture:
    case SensorType::kStationaryDetect:
    case SensorType::kMotionDetect:
    case SensorType::kHeartBeat:
    case SensorType::kLowLatencyOffbodyDetect:
    case SensorType::kHeartRate:
      return payload.Which() == EventPayload::Tag::kFloat;
      break;

    case SensorType::kStepCounter:
      return payload.Which() == EventPayload::Tag::kInteger;
      break;

    case SensorType::kPose6Dof:
      return payload.Which() == EventPayload::Tag::kPose;
      break;

    default:
      return false;
  };
}

std::optional<ConfigurePlaybackError> ValidateSensorList(
    const std::optional<std::vector<SensorInfo>>& sensor_list) {
  if (!sensor_list)
    return ConfigurePlaybackError::kConfigMissingFields;
  for (const SensorInfo& info : *sensor_list) {
    if (!info.sensor_id() || !info.name() || !info.vendor() || !info.version() ||
        !info.sensor_type() || !info.wake_up() || !info.reporting_mode()) {
      return ConfigurePlaybackError::kConfigMissingFields;
    }
  }

  std::unordered_map<SensorId, SensorInfo> sensors_by_id;
  for (const SensorInfo& info : *sensor_list) {
    if (sensors_by_id.find(*info.sensor_id()) != sensors_by_id.end()) {
      return ConfigurePlaybackError::kDuplicateSensorInfo;
    }
    sensors_by_id[*info.sensor_id()] = info;
  }

  return std::nullopt;
}

// Check for a non-emtpy sensor event list, and that the events are valid.
std::optional<ConfigurePlaybackError> ValidateSensorEventList(
    const std::optional<std::vector<SensorEvent>>& sensor_events,
    const std::vector<SensorInfo>& sensor_list) {
  if (!sensor_events)
    return ConfigurePlaybackError::kConfigMissingFields;
  if (sensor_events->empty())
    return ConfigurePlaybackError::kNoEventsForSensor;

  std::unordered_map<SensorId, SensorInfo> sensors_by_id;
  for (const SensorInfo& info : sensor_list) {
    sensors_by_id[*info.sensor_id()] = info;
  }

  std::unordered_set<SensorId> event_ids;
  for (const SensorEvent& event : *sensor_events) {
    // Check that the sensor ID corresponds to a sensor.
    auto it = sensors_by_id.find(event.sensor_id());
    if (it == sensors_by_id.end()) {
      return ConfigurePlaybackError::kEventFromUnknownSensor;
    }
    // Check that the sensor type matches the specified sensor.
    if (it->second.sensor_type() != event.sensor_type()) {
      return ConfigurePlaybackError::kEventSensorTypeMismatch;
    }
    // Check that the payload union is the correct data type for the sensor type.
    if (!CheckPayloadForType(event.sensor_type(), event.payload())) {
      return ConfigurePlaybackError::kEventPayloadTypeMismatch;
    }
    event_ids.insert(event.sensor_id());
  }
  // Check that there is at least one event for every sensor.
  for (const SensorInfo& info : sensor_list) {
    if (event_ids.count(*info.sensor_id()) == 0) {
      return ConfigurePlaybackError::kNoEventsForSensor;
    }
  }

  return std::nullopt;
}

}  // namespace sensors::playback
