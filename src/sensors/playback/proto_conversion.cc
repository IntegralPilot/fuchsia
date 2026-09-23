// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/sensors/playback/proto_conversion.h"

#include <fcntl.h>
#include <lib/fdio/io.h>
#include <stdio.h>

#include "src/sensors/playback/proto/dataset.pb.h"

namespace sensors::playback::io {
namespace {
using fuchsia_sensors_types::EventPayload;
using fuchsia_sensors_types::Pose;
using fuchsia_sensors_types::SensorEvent;
using fuchsia_sensors_types::SensorId;
using fuchsia_sensors_types::SensorInfo;
using fuchsia_sensors_types::SensorReportingMode;
using fuchsia_sensors_types::SensorType;
using fuchsia_sensors_types::SensorWakeUpType;
using fuchsia_sensors_types::UncalibratedVec3FSample;
}  // namespace

void FidlToProtoSensorInfo(const SensorInfo& fidl_sensor,
                           fuchsia::sensors::SensorInfo& proto_sensor) {
  proto_sensor.set_sensor_id(*fidl_sensor.sensor_id());
  proto_sensor.set_name(*fidl_sensor.name());
  proto_sensor.set_vendor(*fidl_sensor.vendor());
  proto_sensor.set_version(*fidl_sensor.version());
  proto_sensor.set_sensor_type(
      ConvertEnum<fuchsia::sensors::SensorType>(*fidl_sensor.sensor_type()));
  proto_sensor.set_wake_up(ConvertEnum<fuchsia::sensors::SensorWakeUpType>(*fidl_sensor.wake_up()));
  proto_sensor.set_reporting_mode(
      ConvertEnum<fuchsia::sensors::SensorReportingMode>(*fidl_sensor.reporting_mode()));
}

void ProtoToFidlSensorInfo(const fuchsia::sensors::SensorInfo& proto_sensor,
                           fuchsia_sensors_types::SensorInfo& fidl_sensor) {
  fidl_sensor.sensor_id(proto_sensor.sensor_id());
  fidl_sensor.name(proto_sensor.name());
  fidl_sensor.vendor(proto_sensor.vendor());
  fidl_sensor.version(proto_sensor.version());
  fidl_sensor.sensor_type(ConvertEnum<SensorType>(proto_sensor.sensor_type()));
  fidl_sensor.wake_up(ConvertEnum<SensorWakeUpType>(proto_sensor.wake_up()));
  fidl_sensor.reporting_mode(ConvertEnum<SensorReportingMode>(proto_sensor.reporting_mode()));
}

fuchsia::sensors::Vec3F FidlToProtoVec3F(const fuchsia_math::Vec3F& fidl_vec3) {
  fuchsia::sensors::Vec3F proto_vec3;
  proto_vec3.set_x(fidl_vec3.x());
  proto_vec3.set_y(fidl_vec3.y());
  proto_vec3.set_z(fidl_vec3.z());

  return proto_vec3;
}

fuchsia_math::Vec3F ProtoToFidlVec3F(const fuchsia::sensors::Vec3F& proto_vec3) {
  fuchsia_math::Vec3F fidl_vec3;
  fidl_vec3.x(proto_vec3.x());
  fidl_vec3.y(proto_vec3.y());
  fidl_vec3.z(proto_vec3.z());

  return fidl_vec3;
}

fuchsia::sensors::QuaternionF FidlToProtoQuaternionF(
    const fuchsia_math::QuaternionF& fidl_quaternion) {
  fuchsia::sensors::QuaternionF proto_quaternion;
  proto_quaternion.set_x(fidl_quaternion.x());
  proto_quaternion.set_y(fidl_quaternion.y());
  proto_quaternion.set_z(fidl_quaternion.z());
  proto_quaternion.set_w(fidl_quaternion.w());

  return proto_quaternion;
}

fuchsia_math::QuaternionF ProtoToFidlQuaternionF(
    const fuchsia::sensors::QuaternionF& proto_quaternion) {
  fuchsia_math::QuaternionF fidl_quaternion;
  fidl_quaternion.x(proto_quaternion.x());
  fidl_quaternion.y(proto_quaternion.y());
  fidl_quaternion.z(proto_quaternion.z());
  fidl_quaternion.w(proto_quaternion.w());

  return fidl_quaternion;
}

fuchsia::sensors::UncalibratedVec3FSample FidlToProtoUncalibratedVec3FSample(
    const UncalibratedVec3FSample& fidl_uncalibrated_vec3_sample) {
  fuchsia::sensors::UncalibratedVec3FSample proto_sample;
  *proto_sample.mutable_sample() = FidlToProtoVec3F(fidl_uncalibrated_vec3_sample.sample());
  *proto_sample.mutable_biases() = FidlToProtoVec3F(fidl_uncalibrated_vec3_sample.biases());

  return proto_sample;
}

UncalibratedVec3FSample ProtoToFidlUncalibratedVec3FSample(
    const fuchsia::sensors::UncalibratedVec3FSample& proto_uncalibrated_vec3_sample) {
  UncalibratedVec3FSample fidl_sample;
  fidl_sample.sample(ProtoToFidlVec3F(proto_uncalibrated_vec3_sample.sample()));
  fidl_sample.biases(ProtoToFidlVec3F(proto_uncalibrated_vec3_sample.biases()));

  return fidl_sample;
}

fuchsia::sensors::Pose FidlToProtoPose(const Pose& fidl_pose) {
  fuchsia::sensors::Pose proto_sample;
  *proto_sample.mutable_rotation() = FidlToProtoQuaternionF(fidl_pose.rotation());
  *proto_sample.mutable_translation() = FidlToProtoVec3F(fidl_pose.translation());
  *proto_sample.mutable_rotation_delta() = FidlToProtoQuaternionF(fidl_pose.rotation_delta());
  *proto_sample.mutable_translation_delta() = FidlToProtoVec3F(fidl_pose.translation_delta());

  return proto_sample;
}

Pose ProtoToFidlPose(const fuchsia::sensors::Pose& proto_pose) {
  Pose fidl_sample;
  fidl_sample.rotation(ProtoToFidlQuaternionF(proto_pose.rotation()));
  fidl_sample.translation(ProtoToFidlVec3F(proto_pose.translation()));
  fidl_sample.rotation_delta(ProtoToFidlQuaternionF(proto_pose.rotation_delta()));
  fidl_sample.translation_delta(ProtoToFidlVec3F(proto_pose.translation_delta()));

  return fidl_sample;
}

fuchsia::sensors::SensorEvent FidlToProtoSensorEvent(
    const fuchsia_sensors_types::SensorEvent& fidl_event) {
  using Tag = EventPayload::Tag;
  fuchsia::sensors::SensorEvent proto_event;
  proto_event.set_timestamp(fidl_event.timestamp());
  proto_event.set_sensor_id(fidl_event.sensor_id());
  proto_event.set_sensor_type(ConvertEnum<fuchsia::sensors::SensorType>(fidl_event.sensor_type()));
  proto_event.set_sequence_number(fidl_event.sequence_number());
  switch (fidl_event.payload().Which()) {
    case Tag::kVec3:
      *proto_event.mutable_vec3() = FidlToProtoVec3F(fidl_event.payload().vec3().value());
      break;
    case Tag::kQuaternion:
      *proto_event.mutable_quaternion() =
          FidlToProtoQuaternionF(fidl_event.payload().quaternion().value());
      break;
    case Tag::kUncalibratedVec3:
      *proto_event.mutable_uncalibrated_vec3() =
          FidlToProtoUncalibratedVec3FSample(fidl_event.payload().uncalibrated_vec3().value());
      break;
    case Tag::kFloat:
      proto_event.set_float_value(fidl_event.payload().float_().value());
      break;
    case Tag::kInteger:
      proto_event.set_integer_value(fidl_event.payload().integer().value());
      break;
    case Tag::kPose:
      *proto_event.mutable_pose() = FidlToProtoPose(fidl_event.payload().pose().value());
      break;
    default:
      ZX_ASSERT_MSG(false, "SensorEvent fidl message does not have a payload.");
      break;
  }

  return proto_event;
}

SensorEvent ProtoToFidlSensorEvent(const fuchsia::sensors::SensorEvent& proto_event) {
  using PayloadCase = fuchsia::sensors::SensorEvent::PayloadCase;
  std::optional<SensorEvent> fidl_event;
  switch (proto_event.payload_case()) {
    case PayloadCase::kVec3:
      fidl_event =
          SensorEvent{{.payload = EventPayload::WithVec3(ProtoToFidlVec3F(proto_event.vec3()))}};
      break;
    case PayloadCase::kQuaternion:
      fidl_event = SensorEvent{{.payload = EventPayload::WithQuaternion(
                                    ProtoToFidlQuaternionF(proto_event.quaternion()))}};
      break;
    case PayloadCase::kUncalibratedVec3:
      fidl_event =
          SensorEvent{{.payload = EventPayload::WithUncalibratedVec3(
                           ProtoToFidlUncalibratedVec3FSample(proto_event.uncalibrated_vec3()))}};
      break;
    case PayloadCase::kFloatValue:
      fidl_event = SensorEvent{{.payload = EventPayload::WithFloat_(proto_event.float_value())}};
      break;
    case PayloadCase::kIntegerValue:
      fidl_event = SensorEvent{{.payload = EventPayload::WithInteger(proto_event.integer_value())}};
      break;
    case PayloadCase::kPose:
      fidl_event =
          SensorEvent{{.payload = EventPayload::WithPose(ProtoToFidlPose(proto_event.pose()))}};
      break;
    case PayloadCase::PAYLOAD_NOT_SET:
      ZX_ASSERT_MSG(false, "SensorEvent proto does not have a payload.");
      break;
  }
  fidl_event->timestamp(proto_event.timestamp());
  fidl_event->sensor_id(proto_event.sensor_id());
  fidl_event->sensor_type(ConvertEnum<SensorType>(proto_event.sensor_type()));
  fidl_event->sequence_number(proto_event.sequence_number());

  return *fidl_event;
}

fuchsia::sensors::DatasetHeader CreateDatasetHeader(const std::string& dataset_name,
                                                    const std::vector<SensorInfo> sensor_list) {
  fuchsia::sensors::DatasetHeader header;
  header.set_name(dataset_name);
  for (const SensorInfo& sensor : sensor_list) {
    fuchsia::sensors::SensorInfo* header_sensor = header.add_sensors();
    FidlToProtoSensorInfo(sensor, *header_sensor);
  }

  return header;
}

}  // namespace sensors::playback::io
