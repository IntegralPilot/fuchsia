// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/sensors/playback/serialization.h"

#include <fcntl.h>
#include <fidl/fuchsia.hardware.sensors/cpp/fidl.h>
#include <lib/fdio/io.h>
#include <lib/syslog/cpp/macros.h>
#include <stdio.h>

#include <gtest/gtest.h>

#include "src/sensors/playback/proto_conversion.h"

using fuchsia_hardware_sensors::Driver;
using fuchsia_hardware_sensors::Playback;
using fuchsia_math::QuaternionF;
using fuchsia_math::Vec3F;

using fuchsia_sensors_types::EventPayload;
using fuchsia_sensors_types::Pose;
using fuchsia_sensors_types::SensorEvent;
using fuchsia_sensors_types::SensorId;
using fuchsia_sensors_types::SensorInfo;
using fuchsia_sensors_types::SensorReportingMode;
using fuchsia_sensors_types::SensorType;
using fuchsia_sensors_types::SensorWakeUpType;
using fuchsia_sensors_types::UncalibratedVec3FSample;

using RecordedSensorEvent = sensors::playback::io::Deserializer::RecordedSensorEvent;

namespace sensors::playback::io {
namespace test {
constexpr SensorId kAccelSensorId = 1;
constexpr SensorId kGyroSensorId = 2;

SensorInfo CreateAccelerometer(SensorId accel_sensor_id) {
  SensorInfo accelerometer_info;
  accelerometer_info.sensor_id(accel_sensor_id);
  accelerometer_info.name("accelerometer");
  accelerometer_info.vendor("Accelomax");
  accelerometer_info.version(1);
  accelerometer_info.sensor_type(SensorType::kAccelerometer);
  accelerometer_info.wake_up(SensorWakeUpType::kWakeUp);
  accelerometer_info.reporting_mode(SensorReportingMode::kContinuous);
  return accelerometer_info;
}

SensorInfo CreateGyroscope(SensorId gyro_sensor_id) {
  SensorInfo gyroscope_info;
  gyroscope_info.sensor_id(gyro_sensor_id);
  gyroscope_info.name("gyroscope");
  gyroscope_info.vendor("Gyrocorp");
  gyroscope_info.version(1);
  gyroscope_info.sensor_type(SensorType::kGyroscope);
  gyroscope_info.wake_up(SensorWakeUpType::kWakeUp);
  gyroscope_info.reporting_mode(SensorReportingMode::kContinuous);
  return gyroscope_info;
}

void CreateAccelerometerEvents(SensorId accel_sensor_id, std::vector<SensorEvent>& event_list) {
  const int64_t kOneMillisecondInNanoseconds = 1e6;
  for (int i = 0; i < 99; i++) {
    Vec3F sample{{
        .x = i % 3 == 0 ? 1.0f : 0.0f,
        .y = i % 3 == 1 ? 1.0f : 0.0f,
        .z = i % 3 == 2 ? 1.0f : 0.0f,
    }};

    SensorEvent event{{.payload = EventPayload::WithVec3(sample)}};
    event.timestamp() = i * kOneMillisecondInNanoseconds;
    event.sensor_id() = accel_sensor_id;
    event.sensor_type() = SensorType::kAccelerometer;
    event.sequence_number() = i;

    event_list.push_back(event);
  }
}

void CreateGyroscopeEvents(SensorId gyro_sensor_id, std::vector<SensorEvent>& event_list) {
  const int64_t kOneMillisecondInNanoseconds = 1e6;
  for (int i = 0; i < 9; i++) {
    Vec3F sample{{
        .x = i % 3 == 1 ? 1.0f : 0.0f,
        .y = i % 3 == 2 ? 1.0f : 0.0f,
        .z = i % 3 == 0 ? 1.0f : 0.0f,
    }};

    SensorEvent event{{.payload = EventPayload::WithVec3(sample)}};
    event.timestamp() = i * 10 * kOneMillisecondInNanoseconds;
    event.sensor_id() = gyro_sensor_id;
    event.sensor_type() = SensorType::kGyroscope;
    event.sequence_number() = i;

    event_list.push_back(event);
  }
}

TEST(SensorsPlaybackProtoConversionTest, EnumConversionTest) {
  SensorType fidl_type = SensorType::kGyroscope;
  fuchsia::sensors::SensorType proto_type = ConvertEnum<fuchsia::sensors::SensorType>(fidl_type);
  ASSERT_EQ(ConvertEnum<SensorType>(proto_type), fidl_type);
}

TEST(SensorsPlaybackProtoConversionTest, SensorInfoConversionTest) {
  SensorInfo fidl_sensor_info = CreateAccelerometer(kAccelSensorId);

  fuchsia::sensors::SensorInfo proto_sensor_info;
  FidlToProtoSensorInfo(fidl_sensor_info, proto_sensor_info);

  SensorInfo fidl_sensor_info2;
  ProtoToFidlSensorInfo(proto_sensor_info, fidl_sensor_info2);

  ASSERT_EQ(fidl_sensor_info, fidl_sensor_info2);
}

TEST(SensorsPlaybackProtoConversionTest, Vec3FConversionTest) {
  Vec3F fidl_vec{{
      .x = 1.0f,
      .y = 2.0f,
      .z = 3.0f,
  }};

  fuchsia::sensors::Vec3F proto_vec = FidlToProtoVec3F(fidl_vec);
  Vec3F fidl_vec2 = ProtoToFidlVec3F(proto_vec);

  ASSERT_EQ(fidl_vec, fidl_vec2);
}

TEST(SensorsPlaybackProtoConversionTest, Quaternion3FConversionTest) {
  QuaternionF fidl_quaternion{{
      .x = 1.0f,
      .y = 2.0f,
      .z = 3.0f,
      .w = 4.0f,
  }};

  fuchsia::sensors::QuaternionF proto_quaternion = FidlToProtoQuaternionF(fidl_quaternion);
  QuaternionF fidl_quaternion2 = ProtoToFidlQuaternionF(proto_quaternion);

  ASSERT_EQ(fidl_quaternion, fidl_quaternion2);
}

TEST(SensorsPlaybackProtoConversionTest, UncalibratedVec3FSampleConversionTest) {
  Vec3F sample{{
      .x = 1.0f,
      .y = 2.0f,
      .z = 3.0f,
  }};
  Vec3F biases{{
      .x = 4.0f,
      .y = 5.0f,
      .z = 6.0f,
  }};
  UncalibratedVec3FSample fidl_uncalibrated_vec;
  fidl_uncalibrated_vec.sample(sample);
  fidl_uncalibrated_vec.biases(biases);

  fuchsia::sensors::UncalibratedVec3FSample proto_uncalibrated_vec =
      FidlToProtoUncalibratedVec3FSample(fidl_uncalibrated_vec);
  UncalibratedVec3FSample fidl_uncalibrated_vec2 =
      ProtoToFidlUncalibratedVec3FSample(proto_uncalibrated_vec);

  ASSERT_EQ(fidl_uncalibrated_vec, fidl_uncalibrated_vec2);
}

TEST(SensorsPlaybackProtoConversionTest, PoseConversionTest) {
  Vec3F translation{{
      .x = 1.0f,
      .y = 2.0f,
      .z = 3.0f,
  }};
  QuaternionF rotation{{
      .x = 4.0f,
      .y = 5.0f,
      .z = 6.0f,
      .w = 7.0f,
  }};
  Vec3F translation_delta{{
      .x = 8.0f,
      .y = 9.0f,
      .z = 10.0f,
  }};
  QuaternionF rotation_delta{{
      .x = 11.0f,
      .y = 12.0f,
      .z = 13.0f,
      .w = 14.0f,
  }};
  Pose fidl_pose;
  fidl_pose.translation(translation);
  fidl_pose.rotation(rotation);
  fidl_pose.translation_delta(translation_delta);
  fidl_pose.rotation_delta(rotation_delta);

  fuchsia::sensors::Pose proto_pose = FidlToProtoPose(fidl_pose);
  Pose fidl_pose2 = ProtoToFidlPose(proto_pose);

  ASSERT_EQ(fidl_pose, fidl_pose2);
}

TEST(SensorsPlaybackProtoConversionTest, Vec3SensorEventConversionTest) {
  Vec3F sample{{
      .x = 1.0f,
      .y = 2.0f,
      .z = 3.0f,
  }};

  SensorEvent fidl_event{{.payload = EventPayload::WithVec3(sample)}};
  fidl_event.timestamp() = 54321;
  fidl_event.sensor_id() = kAccelSensorId;
  fidl_event.sensor_type() = SensorType::kAccelerometer;
  fidl_event.sequence_number() = 1.0;

  fuchsia::sensors::SensorEvent proto_event = FidlToProtoSensorEvent(fidl_event);
  SensorEvent fidl_event2 = ProtoToFidlSensorEvent(proto_event);

  ASSERT_EQ(fidl_event, fidl_event2);
}

TEST(SensorsPlaybackProtoConversionTest, QuaternionSensorEventConversionTest) {
  QuaternionF sample{{
      .x = 1.0f,
      .y = 2.0f,
      .z = 3.0f,
      .w = 4.0f,
  }};

  SensorEvent fidl_event{{.payload = EventPayload::WithQuaternion(sample)}};
  fidl_event.timestamp() = 54321;
  fidl_event.sensor_id() = kAccelSensorId;
  fidl_event.sensor_type() = SensorType::kAccelerometer;
  fidl_event.sequence_number() = 1.0;

  fuchsia::sensors::SensorEvent proto_event = FidlToProtoSensorEvent(fidl_event);
  SensorEvent fidl_event2 = ProtoToFidlSensorEvent(proto_event);

  ASSERT_EQ(fidl_event, fidl_event2);
}

TEST(SensorsPlaybackProtoConversionTest, UncalibratedVec3SensorEventConversionTest) {
  Vec3F sample{{
      .x = 1.0f,
      .y = 2.0f,
      .z = 3.0f,
  }};
  Vec3F biases{{
      .x = 4.0f,
      .y = 5.0f,
      .z = 6.0f,
  }};
  UncalibratedVec3FSample uncalibrated_vec;
  uncalibrated_vec.sample(sample);
  uncalibrated_vec.biases(biases);

  SensorEvent fidl_event{{.payload = EventPayload::WithUncalibratedVec3(uncalibrated_vec)}};
  fidl_event.timestamp() = 54321;
  fidl_event.sensor_id() = kAccelSensorId;
  fidl_event.sensor_type() = SensorType::kAccelerometer;
  fidl_event.sequence_number() = 1.0;

  fuchsia::sensors::SensorEvent proto_event = FidlToProtoSensorEvent(fidl_event);
  SensorEvent fidl_event2 = ProtoToFidlSensorEvent(proto_event);

  ASSERT_EQ(fidl_event, fidl_event2);
}

TEST(SensorsPlaybackProtoConversionTest, FloatSensorEventConversionTest) {
  SensorEvent fidl_event{{.payload = EventPayload::WithFloat_(5.0f)}};
  fidl_event.timestamp() = 54321;
  fidl_event.sensor_id() = kAccelSensorId;
  fidl_event.sensor_type() = SensorType::kAccelerometer;
  fidl_event.sequence_number() = 1.0;

  fuchsia::sensors::SensorEvent proto_event = FidlToProtoSensorEvent(fidl_event);
  SensorEvent fidl_event2 = ProtoToFidlSensorEvent(proto_event);

  ASSERT_EQ(fidl_event, fidl_event2);
}

TEST(SensorsPlaybackProtoConversionTest, IntegerSensorEventConversionTest) {
  SensorEvent fidl_event{{.payload = EventPayload::WithInteger(5)}};
  fidl_event.timestamp() = 54321;
  fidl_event.sensor_id() = kAccelSensorId;
  fidl_event.sensor_type() = SensorType::kAccelerometer;
  fidl_event.sequence_number() = 1.0;

  fuchsia::sensors::SensorEvent proto_event = FidlToProtoSensorEvent(fidl_event);
  SensorEvent fidl_event2 = ProtoToFidlSensorEvent(proto_event);

  ASSERT_EQ(fidl_event, fidl_event2);
}

TEST(SensorsPlaybackProtoConversionTest, PoseSensorEventConversionTest) {
  Vec3F translation{{
      .x = 1.0f,
      .y = 2.0f,
      .z = 3.0f,
  }};
  QuaternionF rotation{{
      .x = 4.0f,
      .y = 5.0f,
      .z = 6.0f,
      .w = 7.0f,
  }};
  Vec3F translation_delta{{
      .x = 8.0f,
      .y = 9.0f,
      .z = 10.0f,
  }};
  QuaternionF rotation_delta{{
      .x = 11.0f,
      .y = 12.0f,
      .z = 13.0f,
      .w = 14.0f,
  }};
  Pose pose;
  pose.translation(translation);
  pose.rotation(rotation);
  pose.translation_delta(translation_delta);
  pose.rotation_delta(rotation_delta);

  SensorEvent fidl_event{{.payload = EventPayload::WithPose(pose)}};
  fidl_event.timestamp() = 54321;
  fidl_event.sensor_id() = kAccelSensorId;
  fidl_event.sensor_type() = SensorType::kAccelerometer;
  fidl_event.sequence_number() = 1.0;

  fuchsia::sensors::SensorEvent proto_event = FidlToProtoSensorEvent(fidl_event);
  SensorEvent fidl_event2 = ProtoToFidlSensorEvent(proto_event);

  ASSERT_EQ(fidl_event, fidl_event2);
}

const char kTestDatasetPath[] = "/tmp/dataset";
class SensorsPlaybackSerializationTest : public testing::Test {
  void TearDown() override { unlink(kTestDatasetPath); }
};

TEST_F(SensorsPlaybackSerializationTest, SerializeDeserializeTest) {
  const std::string dataset_name = "Test Dataset";
  std::vector<SensorInfo> sensor_list;
  std::vector<SensorEvent> event_list;

  sensor_list.push_back(CreateAccelerometer(kAccelSensorId));
  CreateAccelerometerEvents(kAccelSensorId, event_list);

  sensor_list.push_back(CreateGyroscope(kGyroSensorId));
  CreateGyroscopeEvents(kGyroSensorId, event_list);

  std::sort(event_list.begin(), event_list.end(), [](const SensorEvent& a, const SensorEvent& b) {
    return a.timestamp() < b.timestamp();
  });
  {
    Serializer serializer(kTestDatasetPath);

    ASSERT_EQ(serializer.Open(dataset_name, sensor_list), ZX_OK);

    for (const SensorEvent& event : event_list) {
      ASSERT_EQ(serializer.WriteEvent(event, event.timestamp()), ZX_OK);
    }
  }

  std::string read_dataset_name;
  std::vector<SensorInfo> read_sensor_list;
  std::vector<SensorEvent> read_event_list;

  Deserializer deserializer(kTestDatasetPath);
  ASSERT_EQ(deserializer.Open(read_dataset_name, read_sensor_list), ZX_OK);
  ASSERT_EQ(dataset_name, read_dataset_name);
  ASSERT_EQ(sensor_list, read_sensor_list);

  fit::result<zx_status_t, RecordedSensorEvent> result = fit::error(ZX_ERR_BAD_STATE);
  for (;;) {
    result = deserializer.ReadEvent();
    if (result.is_ok()) {
      ASSERT_EQ(result->receipt_timestamp, result->event.timestamp());
      read_event_list.push_back(result->event);
    } else {
      break;
    }
  }
  ASSERT_EQ(result.error_value(), ZX_OK);
  ASSERT_EQ(event_list, read_event_list);
}

TEST_F(SensorsPlaybackSerializationTest, DeserializeFailureNoSuchFile) {
  std::string read_dataset_name;
  std::vector<SensorInfo> read_sensor_list;
  Deserializer deserializer("FileThatDoesNotExist");

  ASSERT_EQ(deserializer.Open(read_dataset_name, read_sensor_list), ZX_ERR_IO);
}

TEST_F(SensorsPlaybackSerializationTest, DeserializeFailureReadWhenFileNotOpen) {
  Deserializer deserializer("FileThatDoesNotExist");
  auto result = deserializer.ReadEvent();
  ASSERT_FALSE(result.is_ok());
  ASSERT_EQ(result.error_value(), ZX_ERR_BAD_STATE);
}

TEST_F(SensorsPlaybackSerializationTest, DeserializeFailureSeekWhenFileNotOpen) {
  Deserializer deserializer("FileThatDoesNotExist");
  zx_status_t result = deserializer.SeekToFirstEvent();
  ASSERT_EQ(result, ZX_ERR_BAD_STATE);
}

TEST_F(SensorsPlaybackSerializationTest, DeserializeFailureIncompleteHeaderSize) {
  std::string read_dataset_name;
  std::vector<SensorInfo> read_sensor_list;

  {
    // Create the file but write nothing to it.
    FileDescriptor fd = ::open(kTestDatasetPath, O_WRONLY | O_CREAT);
    ASSERT_GE(fd.value(), 0);
  }

  Deserializer deserializer(kTestDatasetPath);

  ASSERT_EQ(deserializer.Open(read_dataset_name, read_sensor_list), ZX_ERR_BAD_STATE);
}

TEST_F(SensorsPlaybackSerializationTest, DeserializeFailureIncompleteHeader) {
  std::string read_dataset_name;
  std::vector<SensorInfo> read_sensor_list;

  {
    // Create the file and write a header size but no header.
    size_t header_size = 90;
    FileDescriptor fd = ::open(kTestDatasetPath, O_WRONLY | O_CREAT);
    ASSERT_GE(fd.value(), 0);
    ASSERT_EQ(::write(fd, &header_size, sizeof(header_size)),
              static_cast<long>(sizeof(header_size)));
  }

  Deserializer deserializer(kTestDatasetPath);

  ASSERT_EQ(deserializer.Open(read_dataset_name, read_sensor_list), ZX_ERR_BAD_STATE);
}

TEST_F(SensorsPlaybackSerializationTest, DeserializeFailureGarbageHeader) {
  std::string read_dataset_name;
  std::vector<SensorInfo> read_sensor_list;

  {
    // Create the file and write a valid header size, but a garbage header that won't parse.
    const char kGarbageString[] = "This is not a valid header.";
    size_t header_size = sizeof(kGarbageString);
    FileDescriptor fd = ::open(kTestDatasetPath, O_WRONLY | O_CREAT);
    ASSERT_GE(fd.value(), 0);
    ASSERT_EQ(::write(fd, &header_size, sizeof(header_size)),
              static_cast<long>(sizeof(header_size)));
    ASSERT_EQ(::write(fd, kGarbageString, header_size), static_cast<long>(header_size));
  }

  Deserializer deserializer(kTestDatasetPath);

  ASSERT_EQ(deserializer.Open(read_dataset_name, read_sensor_list), ZX_ERR_BAD_STATE);
}

TEST_F(SensorsPlaybackSerializationTest, DeserializeFailureIncompleteEventSize) {
  const std::string dataset_name = "Test Dataset";
  std::vector<SensorInfo> sensor_list;

  sensor_list.push_back(CreateAccelerometer(kAccelSensorId));
  sensor_list.push_back(CreateGyroscope(kGyroSensorId));
  fuchsia::sensors::DatasetHeader header = CreateDatasetHeader(dataset_name, sensor_list);

  {
    // Create the file and write a valid header size/header, but then an incomplete event size.
    FileDescriptor fd = ::open(kTestDatasetPath, O_WRONLY | O_CREAT);
    ASSERT_GE(fd.value(), 0);
    std::string header_bytes;
    ASSERT_TRUE(header.SerializeToString(&header_bytes));
    size_t header_size = header_bytes.size();
    ASSERT_EQ(::write(fd, &header_size, sizeof(header_size)),
              static_cast<long>(sizeof(header_size)));
    ASSERT_EQ(::write(fd, header_bytes.data(), header_size), static_cast<long>(header_size));
    size_t event_size = 10;
    ASSERT_EQ(::write(fd, &event_size, 1), 1);
  }

  std::string read_dataset_name;
  std::vector<SensorInfo> read_sensor_list;
  Deserializer deserializer(kTestDatasetPath);

  ASSERT_EQ(deserializer.Open(read_dataset_name, read_sensor_list), ZX_OK);
  ASSERT_EQ(dataset_name, read_dataset_name);
  ASSERT_EQ(sensor_list, read_sensor_list);

  auto result = deserializer.ReadEvent();
  ASSERT_FALSE(result.is_ok());
  ASSERT_EQ(result.error_value(), ZX_ERR_BAD_STATE);
}

TEST_F(SensorsPlaybackSerializationTest, DeserializeFailureEOFAfterEventSize) {
  const std::string dataset_name = "Test Dataset";
  std::vector<SensorInfo> sensor_list;

  sensor_list.push_back(CreateAccelerometer(kAccelSensorId));
  sensor_list.push_back(CreateGyroscope(kGyroSensorId));
  fuchsia::sensors::DatasetHeader header = CreateDatasetHeader(dataset_name, sensor_list);

  {
    // Create the file and write a valid header size/header, then an event size but no event.
    FileDescriptor fd = ::open(kTestDatasetPath, O_WRONLY | O_CREAT);
    ASSERT_GE(fd.value(), 0);
    std::string header_bytes;
    ASSERT_TRUE(header.SerializeToString(&header_bytes));
    size_t header_size = header_bytes.size();
    ASSERT_EQ(::write(fd, &header_size, sizeof(header_size)),
              static_cast<long>(sizeof(header_size)));
    ASSERT_EQ(::write(fd, header_bytes.data(), header_size), static_cast<long>(header_size));
    size_t event_size = 10;
    ASSERT_EQ(::write(fd, &event_size, sizeof(event_size)), static_cast<long>(sizeof(event_size)));
  }

  std::string read_dataset_name;
  std::vector<SensorInfo> read_sensor_list;
  Deserializer deserializer(kTestDatasetPath);

  ASSERT_EQ(deserializer.Open(read_dataset_name, read_sensor_list), ZX_OK);
  ASSERT_EQ(dataset_name, read_dataset_name);
  ASSERT_EQ(sensor_list, read_sensor_list);

  auto result = deserializer.ReadEvent();
  ASSERT_FALSE(result.is_ok());
  ASSERT_EQ(result.error_value(), ZX_ERR_BAD_STATE);
}

TEST_F(SensorsPlaybackSerializationTest, DeserializeFailureIncompleteEvent) {
  const std::string dataset_name = "Test Dataset";
  std::vector<SensorInfo> sensor_list;

  sensor_list.push_back(CreateAccelerometer(kAccelSensorId));
  sensor_list.push_back(CreateGyroscope(kGyroSensorId));
  fuchsia::sensors::DatasetHeader header = CreateDatasetHeader(dataset_name, sensor_list);

  {
    // Create the file and write a valid header size/header, then an event size greater than 1, then
    // a random byte.
    FileDescriptor fd = ::open(kTestDatasetPath, O_WRONLY | O_CREAT);
    ASSERT_GE(fd.value(), 0);
    std::string header_bytes;
    ASSERT_TRUE(header.SerializeToString(&header_bytes));
    size_t header_size = header_bytes.size();
    ASSERT_EQ(::write(fd, &header_size, sizeof(header_size)),
              static_cast<long>(sizeof(header_size)));
    ASSERT_EQ(::write(fd, header_bytes.data(), header_size), static_cast<long>(header_size));
    size_t event_size = 10;
    ASSERT_EQ(::write(fd, &event_size, sizeof(event_size)), static_cast<long>(sizeof(event_size)));
    ASSERT_EQ(::write(fd, "a", 1), 1);
  }

  std::string read_dataset_name;
  std::vector<SensorInfo> read_sensor_list;
  Deserializer deserializer(kTestDatasetPath);

  ASSERT_EQ(deserializer.Open(read_dataset_name, read_sensor_list), ZX_OK);
  ASSERT_EQ(dataset_name, read_dataset_name);
  ASSERT_EQ(sensor_list, read_sensor_list);

  auto result = deserializer.ReadEvent();
  ASSERT_FALSE(result.is_ok());
  ASSERT_EQ(result.error_value(), ZX_ERR_BAD_STATE);
}

TEST_F(SensorsPlaybackSerializationTest, DeserializeFailureGarbageEvent) {
  const std::string dataset_name = "Test Dataset";
  std::vector<SensorInfo> sensor_list;

  sensor_list.push_back(CreateAccelerometer(kAccelSensorId));
  sensor_list.push_back(CreateGyroscope(kGyroSensorId));
  fuchsia::sensors::DatasetHeader header = CreateDatasetHeader(dataset_name, sensor_list);

  {
    // Create the file and write a valid header size/header, then a valid event size, then a garbage
    // "event" that won't parse.
    FileDescriptor fd = ::open(kTestDatasetPath, O_WRONLY | O_CREAT);
    ASSERT_GE(fd.value(), 0);
    std::string header_bytes;
    ASSERT_TRUE(header.SerializeToString(&header_bytes));
    size_t header_size = header_bytes.size();
    ASSERT_EQ(::write(fd, &header_size, sizeof(header_size)),
              static_cast<long>(sizeof(header_size)));
    ASSERT_EQ(::write(fd, header_bytes.data(), header_size), static_cast<long>(header_size));

    char kGarbageString[] = "This is not a valid event.";
    size_t event_size = sizeof(kGarbageString);
    ASSERT_EQ(::write(fd, &event_size, sizeof(event_size)), static_cast<long>(sizeof(event_size)));
    ASSERT_EQ(::write(fd, kGarbageString, event_size), static_cast<long>(event_size));
  }

  std::string read_dataset_name;
  std::vector<SensorInfo> read_sensor_list;
  Deserializer deserializer(kTestDatasetPath);

  ASSERT_EQ(deserializer.Open(read_dataset_name, read_sensor_list), ZX_OK);
  ASSERT_EQ(dataset_name, read_dataset_name);
  ASSERT_EQ(sensor_list, read_sensor_list);

  auto result = deserializer.ReadEvent();
  ASSERT_FALSE(result.is_ok());
  ASSERT_EQ(result.error_value(), ZX_ERR_BAD_STATE);
}

}  // namespace test
}  // namespace sensors::playback::io
