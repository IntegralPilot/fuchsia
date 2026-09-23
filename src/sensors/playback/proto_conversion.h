// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_SENSORS_PLAYBACK_PROTO_CONVERSION_H_
#define SRC_SENSORS_PLAYBACK_PROTO_CONVERSION_H_

#include <fidl/fuchsia.hardware.sensors/cpp/fidl.h>
#include <lib/syslog/cpp/log_settings.h>
#include <lib/syslog/cpp/macros.h>

#include <vector>

#include "src/sensors/playback/proto/dataset.pb.h"

namespace sensors::playback::io {

template <typename OutEnumType, typename InEnumType>
OutEnumType ConvertEnum(InEnumType in_value) {
  uint32_t value = static_cast<uint32_t>(in_value);
  return static_cast<OutEnumType>(value);
}

void FidlToProtoSensorInfo(const fuchsia_sensors_types::SensorInfo& fidl_sensor,
                           fuchsia::sensors::SensorInfo& proto_sensor);
void ProtoToFidlSensorInfo(const fuchsia::sensors::SensorInfo& proto_sensor,
                           fuchsia_sensors_types::SensorInfo& fidl_sensor);

fuchsia::sensors::Vec3F FidlToProtoVec3F(const fuchsia_math::Vec3F& fidl_vec3);
fuchsia_math::Vec3F ProtoToFidlVec3F(const fuchsia::sensors::Vec3F& proto_vec3);

fuchsia::sensors::QuaternionF FidlToProtoQuaternionF(
    const fuchsia_math::QuaternionF& fidl_quaternion);
fuchsia_math::QuaternionF ProtoToFidlQuaternionF(
    const fuchsia::sensors::QuaternionF& proto_quaternion);

fuchsia::sensors::UncalibratedVec3FSample FidlToProtoUncalibratedVec3FSample(
    const fuchsia_sensors_types::UncalibratedVec3FSample& fidl_uncalibrated_vec3_sample);
fuchsia_sensors_types::UncalibratedVec3FSample ProtoToFidlUncalibratedVec3FSample(
    const fuchsia::sensors::UncalibratedVec3FSample& proto_quaternion_vec3_sample);

fuchsia::sensors::Pose FidlToProtoPose(const fuchsia_sensors_types::Pose& fidl_pose);
fuchsia_sensors_types::Pose ProtoToFidlPose(const fuchsia::sensors::Pose& proto_pose);

fuchsia::sensors::SensorEvent FidlToProtoSensorEvent(
    const fuchsia_sensors_types::SensorEvent& fidl_pose);
fuchsia_sensors_types::SensorEvent ProtoToFidlSensorEvent(
    const fuchsia::sensors::SensorEvent& proto_pose);

fuchsia::sensors::DatasetHeader CreateDatasetHeader(
    const std::string& dataset_name,
    const std::vector<fuchsia_sensors_types::SensorInfo> sensor_list);

}  // namespace sensors::playback::io

#endif  // SRC_SENSORS_PLAYBACK_PROTO_CONVERSION_H_
