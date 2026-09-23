// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_SENSORS_PLAYBACK_SERIALIZATION_H_
#define SRC_SENSORS_PLAYBACK_SERIALIZATION_H_

#include <fidl/fuchsia.hardware.sensors/cpp/fidl.h>
#include <lib/fit/function.h>
#include <lib/syslog/cpp/log_settings.h>
#include <lib/syslog/cpp/macros.h>
#include <zircon/syscalls.h>
#include <zircon/syscalls/clock.h>

#include <vector>

namespace sensors::playback::io {

class FileDescriptor {
 public:
  FileDescriptor(int value) : value_(value) {}
  FileDescriptor(const FileDescriptor&) = delete;
  FileDescriptor(FileDescriptor&& other);
  ~FileDescriptor();
  FileDescriptor& operator=(const FileDescriptor&) = delete;
  FileDescriptor& operator=(FileDescriptor&& other);

  int value() { return value_; }
  operator int() { return value_; }

 private:
  int value_ = -1;
};

class Serializer {
  using SensorEvent = fuchsia_sensors_types::SensorEvent;
  using SensorId = fuchsia_sensors_types::SensorId;
  using SensorInfo = fuchsia_sensors_types::SensorInfo;

 public:
  Serializer(const std::string& file_path) : file_path_(file_path) {}

  zx_status_t Open(const std::string& dataset_name, const std::vector<SensorInfo>& sensor_list);

  zx_status_t WriteEvent(const SensorEvent& event,
                         std::optional<int64_t> receipt_timestamp = std::nullopt);

 private:
  // Path of the file that will be written.
  const std::string file_path_;

  // File descriptor of the file (will only have a value if the file is open).
  std::optional<FileDescriptor> fd_ = std::nullopt;
};

class Deserializer {
  using SensorEvent = fuchsia_sensors_types::SensorEvent;
  using SensorId = fuchsia_sensors_types::SensorId;
  using SensorInfo = fuchsia_sensors_types::SensorInfo;

 public:
  struct RecordedSensorEvent {
    SensorEvent event;
    std::optional<int64_t> receipt_timestamp;
  };

  Deserializer(const std::string& file_path) : file_path_(file_path) {}

  zx_status_t Open(std::string& dataset_name, std::vector<SensorInfo>& sensor_list);

  fit::result<zx_status_t, RecordedSensorEvent> ReadEvent();

  zx_status_t SeekToFirstEvent();

 private:
  // Path of the file that will be read.
  const std::string file_path_;

  // File descriptor of the file (will only have a value if the file is open).
  std::optional<FileDescriptor> fd_ = std::nullopt;

  // Size of the dataset header message.
  size_t header_size_;
};

}  // namespace sensors::playback::io

#endif  // SRC_SENSORS_PLAYBACK_SERIALIZATION_H_
