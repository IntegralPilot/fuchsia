// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/sensors/playback/file_reader.h"

#include <lib/fpromise/bridge.h>

namespace sensors::playback {
namespace {
using fpromise::bridge;
using fpromise::promise;
using fpromise::result;
using fpromise::scope;

using fuchsia_hardware_sensors::ConfigurePlaybackError;
using fuchsia_sensors_types::SensorEvent;
using fuchsia_sensors_types::SensorId;
using fuchsia_sensors_types::SensorInfo;

using RecordedSensorEvent = sensors::playback::io::Deserializer::RecordedSensorEvent;
}  // namespace

FileReader::FileReader(const std::string& file_path, async_dispatcher_t* dispatcher)
    : ActorBase(dispatcher, scope_),
      deserializer_(file_path),
      read_scope_(std::make_unique<scope>()) {}

promise<void, ConfigurePlaybackError> FileReader::Open() {
  bridge<void, ConfigurePlaybackError> bridge;
  Schedule([this, completer = std::move(bridge.completer)]() mutable {
    zx_status_t result = deserializer_.Open(dataset_name_, sensor_list_);
    switch (result) {
      case ZX_OK:
        completer.complete_ok();
        break;
      case ZX_ERR_IO:
        completer.complete_error(ConfigurePlaybackError::kFileOpenFailed);
        break;
      case ZX_ERR_BAD_STATE:
        completer.complete_error(ConfigurePlaybackError::kFileParseError);
        break;
      default:
        ZX_ASSERT_MSG(false, "Saw unexpected return value from Open: %d", result);
        break;
    }
  });
  return bridge.consumer.promise();
}

promise<std::vector<SensorInfo>> FileReader::GetSensorsList() {
  bridge<std::vector<SensorInfo>> bridge;
  Schedule([this, completer = std::move(bridge.completer)]() mutable {
    completer.complete_ok(sensor_list_);
  });
  return bridge.consumer.promise();
}

promise<void> FileReader::SetEventCallback(
    std::function<void(const RecordedSensorEvent&)> event_callback) {
  bridge<void> bridge;
  Schedule([this, event_callback = std::move(event_callback),
            completer = std::move(bridge.completer)]() mutable {
    event_callback_ = std::move(event_callback);
    completer.complete_ok();
  });
  return bridge.consumer.promise();
}

promise<void> FileReader::SetReadCompleteCallback(
    std::function<void(std::optional<zx_status_t>)> read_complete_callback) {
  bridge<void> bridge;
  Schedule([this, read_complete_callback = std::move(read_complete_callback),
            completer = std::move(bridge.completer)]() mutable {
    read_complete_callback_ = std::move(read_complete_callback);
    completer.complete_ok();
  });
  return bridge.consumer.promise();
}

void FileReader::ReadEvents(int max_events) {
  ZX_ASSERT(max_events >= 0);
  Schedule(fpromise::make_promise([this, max_events]() {
             if (max_events == 0) {
               read_complete_callback_(std::nullopt);
               return;
             }
             auto result = deserializer_.ReadEvent();
             if (!result.is_ok()) {
               read_complete_callback_(result.error_value());
               return;
             }
             event_callback_(*result);
             ReadEvents(max_events - 1);
           }).wrap_with(*read_scope_));
}

promise<void> FileReader::StopScheduledReads() {
  bridge<void> bridge;
  Schedule([this, completer = std::move(bridge.completer)]() mutable {
    read_scope_ = std::make_unique<scope>();
    completer.complete_ok();
  });
  return bridge.consumer.promise();
}

promise<void> FileReader::SeekToFirstEvent() {
  bridge<void> bridge;
  Schedule([this, completer = std::move(bridge.completer)]() mutable {
    ZX_ASSERT(deserializer_.SeekToFirstEvent() == ZX_OK);
    completer.complete_ok();
  });
  return bridge.consumer.promise();
}

}  // namespace sensors::playback
