// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_SENSORS_PLAYBACK_FILE_READER_H_
#define SRC_SENSORS_PLAYBACK_FILE_READER_H_

#include <fidl/fuchsia.hardware.sensors/cpp/fidl.h>
#include <lib/fit/function.h>
#include <lib/fpromise/promise.h>
#include <lib/fpromise/scope.h>
#include <lib/syslog/cpp/log_settings.h>
#include <lib/syslog/cpp/macros.h>
#include <zircon/syscalls.h>
#include <zircon/syscalls/clock.h>

#include <vector>

#include "src/camera/lib/actor/actor_base.h"
#include "src/sensors/playback/serialization.h"

namespace sensors::playback {

class FileReader : public camera::actor::ActorBase {
  using ConfigurePlaybackError = fuchsia_hardware_sensors::ConfigurePlaybackError;
  using RecordedSensorEvent = io::Deserializer::RecordedSensorEvent;
  using SensorEvent = fuchsia_sensors_types::SensorEvent;
  using SensorId = fuchsia_sensors_types::SensorId;
  using SensorInfo = fuchsia_sensors_types::SensorInfo;

 public:
  FileReader(const std::string& file_path, async_dispatcher_t* dispatcher);

  fpromise::promise<void, ConfigurePlaybackError> Open();
  fpromise::promise<std::vector<SensorInfo>> GetSensorsList();

  fpromise::promise<void> SetEventCallback(
      std::function<void(const RecordedSensorEvent&)> event_callback);
  fpromise::promise<void> SetReadCompleteCallback(
      std::function<void(std::optional<zx_status_t>)> read_complete_callback);

  void ReadEvents(int max_events);
  fpromise::promise<void> StopScheduledReads();

  fpromise::promise<void> SeekToFirstEvent();

 private:
  // Callback to call with sensor events.
  std::function<void(const RecordedSensorEvent&)> event_callback_;
  // Callback which is called when the requested read is complete.
  std::function<void(std::optional<zx_status_t>)> read_complete_callback_;

  // File deserializer.
  io::Deserializer deserializer_;

  // Human readable name for the dataset being processed.
  std::string dataset_name_;

  // List of "sensors" currently provided by the playback data.
  std::vector<SensorInfo> sensor_list_;

  // A promise scope to wrap scheduled read promises.
  std::unique_ptr<fpromise::scope> read_scope_;

  // This should always be the last thing in the object. Otherwise scheduled tasks within this scope
  // which reference members of this object may be allowed to run after destruction of this object
  // has started. Keeping this at the end ensures that the scope is destroyed first, cancelling any
  // scheduled tasks before the rest of the members are destroyed.
  fpromise::scope scope_;
};

}  // namespace sensors::playback

#endif  // SRC_SENSORS_PLAYBACK_FILE_READER_H_
