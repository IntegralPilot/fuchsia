// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/sensors/playback/serialization.h"

#include <fcntl.h>
#include <lib/fdio/io.h>
#include <stdio.h>

#include "src/sensors/playback/proto/dataset.pb.h"
#include "src/sensors/playback/proto_conversion.h"

namespace sensors::playback::io {
namespace {
using fuchsia_sensors_types::SensorEvent;
using fuchsia_sensors_types::SensorId;
using fuchsia_sensors_types::SensorInfo;
using fuchsia_sensors_types::SensorReportingMode;
using fuchsia_sensors_types::SensorType;
using fuchsia_sensors_types::SensorWakeUpType;

using RecordedSensorEvent = sensors::playback::io::Deserializer::RecordedSensorEvent;
}  // namespace

FileDescriptor::FileDescriptor(FileDescriptor&& other) { std::swap(value_, other.value_); }

FileDescriptor::~FileDescriptor() {
  if (value_ >= 0) {
    ::close(value_);
  }
}

FileDescriptor& FileDescriptor::operator=(FileDescriptor&& other) {
  std::swap(value_, other.value_);
  return *this;
}

zx_status_t Serializer::Open(const std::string& dataset_name,
                             const std::vector<SensorInfo>& sensor_list) {
  FileDescriptor fd = ::open(file_path_.c_str(), O_WRONLY | O_CREAT);
  if (fd < 0) {
    FX_LOGS(ERROR) << "Serializer: Could not open file: " << file_path_ << ", error: " << errno;
    return ZX_ERR_IO;
  }

  fuchsia::sensors::DatasetHeader header = CreateDatasetHeader(dataset_name, sensor_list);
  std::string header_bytes;
  if (!header.SerializeToString(&header_bytes)) {
    FX_LOGS(ERROR) << "Serializer: Failed to serialize header.";
    return ZX_ERR_BAD_STATE;
  }

  size_t header_size = header_bytes.size();
  size_t bytes_written = ::write(fd, &header_size, sizeof(header_size));
  if (bytes_written < 0) {
    FX_LOGS(ERROR) << "Serializer: Failed to write header size with error: " << errno;
    return ZX_ERR_IO;
  } else if (bytes_written < sizeof(header_size)) {
    FX_LOGS(ERROR) << "Serializer: Failed to write header size.";
    return ZX_ERR_IO;
  }

  bytes_written = ::write(fd, header_bytes.data(), header_size);
  if (bytes_written < 0) {
    FX_LOGS(ERROR) << "Serializer: Failed to write header bytes with error: " << errno;
    return ZX_ERR_IO;
  } else if (bytes_written < header_size) {
    FX_LOGS(ERROR) << "Serializer: Failed to write all header bytes.";
    return ZX_ERR_IO;
  }

  fd_ = std::move(fd);
  return ZX_OK;
}

zx_status_t Serializer::WriteEvent(const SensorEvent& event,
                                   std::optional<int64_t> receipt_timestamp) {
  if (!fd_) {
    FX_LOGS(ERROR) << "Serializer: Attempt to write an event when the file is not open.";
    return ZX_ERR_BAD_STATE;
  }

  fuchsia::sensors::RecordedSensorEvent proto_event;
  *proto_event.mutable_event() = FidlToProtoSensorEvent(event);
  if (receipt_timestamp)
    proto_event.set_receipt_timestamp(*receipt_timestamp);
  std::string event_bytes;
  if (!proto_event.SerializeToString(&event_bytes)) {
    FX_LOGS(ERROR) << "Serializer: Failed to serialize event.";
    return ZX_ERR_BAD_STATE;
  }

  size_t event_size = event_bytes.size();
  size_t bytes_written = ::write(*fd_, &event_size, sizeof(event_size));
  if (bytes_written < 0) {
    FX_LOGS(ERROR) << "Serializer: Failed to write event size with error: " << errno;
    return ZX_ERR_IO;
  } else if (bytes_written < sizeof(event_size)) {
    FX_LOGS(ERROR) << "Serializer: Failed to write event size.";
    return ZX_ERR_IO;
  }

  bytes_written = ::write(*fd_, event_bytes.data(), event_size);
  if (bytes_written < 0) {
    FX_LOGS(ERROR) << "Serializer: Failed to write event bytes with error: " << errno;
    return ZX_ERR_IO;
  } else if (bytes_written < event_size) {
    FX_LOGS(ERROR) << "Serializer: Failed to write all event bytes.";
    return ZX_ERR_IO;
  }

  return ZX_OK;
}

zx_status_t Deserializer::Open(std::string& dataset_name, std::vector<SensorInfo>& sensor_list) {
  FileDescriptor fd = ::open(file_path_.c_str(), O_RDONLY);
  if (fd < 0) {
    FX_LOGS(ERROR) << "Deserializer: Could not open file: " << file_path_ << ", error: " << errno;
    return ZX_ERR_IO;
  }

  size_t header_size;
  size_t bytes_read = ::read(fd, &header_size, sizeof(header_size));
  if (bytes_read < 0) {
    FX_LOGS(ERROR) << "Deserializer: Failed to read header size with error: " << errno;
    return ZX_ERR_IO;
  } else if (bytes_read < sizeof(header_size)) {
    FX_LOGS(ERROR) << "Deserializer: Failed to read header size.";
    return ZX_ERR_BAD_STATE;
  }

  std::string header_bytes;
  header_bytes.resize(header_size);
  bytes_read = ::read(fd, header_bytes.data(), header_size);
  if (bytes_read < 0) {
    FX_LOGS(ERROR) << "Deserializer: Failed to read header bytes with error: " << errno;
    return ZX_ERR_IO;
  } else if (bytes_read < header_size) {
    FX_LOGS(ERROR) << "Deserializer: Failed to read all header bytes.";
    return ZX_ERR_BAD_STATE;
  }

  fuchsia::sensors::DatasetHeader header;
  if (!header.ParseFromString(header_bytes)) {
    FX_LOGS(ERROR) << "Deserializer: Failed to parse data file header.";
    return ZX_ERR_BAD_STATE;
  }

  dataset_name = header.name();
  for (const auto& header_sensor : header.sensors()) {
    SensorInfo sensor;
    ProtoToFidlSensorInfo(header_sensor, sensor);
    sensor_list.push_back(sensor);
  }

  fd_ = std::move(fd);
  header_size_ = header_size;
  return ZX_OK;
}

fit::result<zx_status_t, RecordedSensorEvent> Deserializer::ReadEvent() {
  if (!fd_) {
    FX_LOGS(ERROR) << "Deserializer: Attempt to read an event when the file is not open.";
    return fit::error(ZX_ERR_BAD_STATE);
  }

  size_t event_size;
  size_t bytes_read = ::read(*fd_, &event_size, sizeof(event_size));
  if (bytes_read == 0) {
    // End of file, return "error" ZX_OK.
    return fit::error(ZX_OK);
  } else if (bytes_read < 0) {
    FX_LOGS(ERROR) << "Deserializer: Failed to read event size with error: " << errno;
    return fit::error(ZX_ERR_IO);
  } else if (bytes_read < sizeof(event_size)) {
    FX_LOGS(ERROR) << "Deserializer: Couldn't read complete event size.";
    return fit::error(ZX_ERR_BAD_STATE);
  }

  std::string event_bytes;
  event_bytes.resize(event_size);
  bytes_read = ::read(*fd_, event_bytes.data(), event_size);
  if (bytes_read == 0) {
    // There shouldn't be an EOF after an event size.
    FX_LOGS(ERROR) << "Deserializer: Encountered EOF after reading event size.";
    return fit::error(ZX_ERR_BAD_STATE);
  } else if (bytes_read < 0) {
    FX_LOGS(ERROR) << "Deserializer: Failed to read event with error: " << errno;
    return fit::error(ZX_ERR_IO);
  } else if (bytes_read < event_size) {
    FX_LOGS(ERROR) << "Deserializer: Failed to read all event bytes.";
    return fit::error(ZX_ERR_BAD_STATE);
  }

  fuchsia::sensors::RecordedSensorEvent proto_event;
  if (!proto_event.ParseFromString(event_bytes)) {
    FX_LOGS(ERROR) << "Deserializer: Failed to parse a sensor event.";
    return fit::error(ZX_ERR_BAD_STATE);
  }
  std::optional<int64_t> receipt_timestamp;
  if (proto_event.has_receipt_timestamp())
    receipt_timestamp = proto_event.receipt_timestamp();

  return fit::ok(
      RecordedSensorEvent{ProtoToFidlSensorEvent(proto_event.event()), receipt_timestamp});
}

zx_status_t Deserializer::SeekToFirstEvent() {
  if (!fd_) {
    FX_LOGS(ERROR) << "Deserializer: Attempt to seek when the file is not open.";
    return ZX_ERR_BAD_STATE;
  }

  ssize_t desired_offset = sizeof(header_size_) + header_size_;
  off_t new_offset = lseek(*fd_, desired_offset, SEEK_SET);
  if (new_offset < 0) {
    FX_LOGS(ERROR) << "Deserializer: Seek failed with error: " << errno;
    return ZX_ERR_IO;
  }
  return new_offset == desired_offset ? ZX_OK : ZX_ERR_BAD_STATE;
}

}  // namespace sensors::playback::io
