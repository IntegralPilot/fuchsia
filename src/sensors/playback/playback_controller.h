// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_SENSORS_PLAYBACK_PLAYBACK_CONTROLLER_H_
#define SRC_SENSORS_PLAYBACK_PLAYBACK_CONTROLLER_H_

#include <fidl/fuchsia.hardware.sensors/cpp/fidl.h>
#include <lib/fit/function.h>
#include <lib/fpromise/promise.h>
#include <lib/fpromise/scope.h>
#include <lib/syslog/cpp/log_settings.h>
#include <lib/syslog/cpp/macros.h>
#include <zircon/syscalls.h>
#include <zircon/syscalls/clock.h>

#include <future>
#include <queue>
#include <unordered_map>
#include <vector>

#include "src/camera/lib/actor/actor_base.h"
#include "src/sensors/playback/file_reader.h"
#include "src/sensors/playback/playback_config_validation.h"

namespace sensors::playback {

class PlaybackController : public camera::actor::ActorBase {
  using ActivateSensorError = fuchsia_hardware_sensors::ActivateSensorError;
  using DeactivateSensorError = fuchsia_hardware_sensors::DeactivateSensorError;
  using ConfigurePlaybackError = fuchsia_hardware_sensors::ConfigurePlaybackError;
  using ConfigureSensorRateError = fuchsia_hardware_sensors::ConfigureSensorRateError;
  using FilePlaybackConfig = fuchsia_hardware_sensors::FilePlaybackConfig;
  using FixedValuesPlaybackConfig = fuchsia_hardware_sensors::FixedValuesPlaybackConfig;
  using PlaybackSourceConfig = fuchsia_hardware_sensors::PlaybackSourceConfig;
  using SensorEvent = fuchsia_sensors_types::SensorEvent;
  using SensorId = fuchsia_sensors_types::SensorId;
  using SensorInfo = fuchsia_sensors_types::SensorInfo;
  using SensorRateConfig = fuchsia_sensors_types::SensorRateConfig;

 public:
  PlaybackController(async_dispatcher_t* dispatcher, async_dispatcher_t* file_read_dispatcher);

  fpromise::promise<void, ConfigurePlaybackError> ConfigurePlayback(
      const PlaybackSourceConfig& config);

  fpromise::promise<std::vector<SensorInfo>> GetSensorsList();

  fpromise::promise<void, ActivateSensorError> ActivateSensor(SensorId sensor_id);

  fpromise::promise<void, DeactivateSensorError> DeactivateSensor(SensorId sensor_id);

  fpromise::promise<void, ConfigureSensorRateError> ConfigureSensorRate(
      SensorId sensor_id, SensorRateConfig rate_config);

  fpromise::promise<void> SetDisconnectDriverClientCallback(
      std::function<fpromise::promise<void>(zx_status_t)> disconnect_callback);

  fpromise::promise<void> SetEventCallback(std::function<void(const SensorEvent&)> event_callback);

  void DriverClientDisconnected(fit::callback<void()> unbind_callback);

  void PlaybackClientDisconnected(fit::callback<void()> unbind_callback);

 private:
  constexpr static int64_t kDefaultSamplingPeriodNs = 1e9;     // 1 second.
  constexpr static int64_t kDefaultMaxReportingLatencyNs = 0;  // No buffering.

  enum class PlaybackMode : std::uint8_t {
    kNone,
    kFilePlaybackMode,
    kFixedValuesMode,
  };

  enum class PlaybackState : std::uint8_t { kStopped, kRunning };

  struct SensorPlaybackState {
    bool enabled = false;
    zx::duration sampling_period = zx::duration(kDefaultSamplingPeriodNs);
    zx::duration max_reporting_latency = zx::duration(kDefaultMaxReportingLatencyNs);

    // Output buffer used when max reporting latency is non-zero.
    std::queue<SensorEvent> event_buffer;

    // For fixed value playback
    std::vector<SensorEvent> fixed_sensor_events;
    int next_fixed_event = 0;
    std::optional<zx::time> last_scheduled_event_time;
  };

  struct FileTimestampData {
    // The time at which the first event from a file was scheduled to be sent (in the
    // playback time domain).
    zx::time first_presentation_time;
    // The timestamp of the first event from a file (in the playback time domain).
    zx::time first_event_timestamp;
    // The presentation timestamp of the last event read from a file (in the playback time domain).
    zx::time last_read_presentation_time;
    // The event timestamp of the last event read from a file (in the playback time domain).
    zx::time last_read_event_timestamp;

    // The first event presentation time in a file (in the file's time domain).
    zx::time file_first_presentation_time;
    // The first event timestamp time in a file (in the file's time domain).
    zx::time file_first_event_timestamp;
  };

  // Tells any attached Driver protocol implementation to disconnect their client with the given
  // epitaph.
  fpromise::promise<void> DisconnectDriverClient(zx_status_t epitaph);

  // Populate the sensor lists for this playback component with the sensors from the given list.
  // This should only be called from a promise scheduled to run on this actor's executor.
  void AdoptSensorList(const std::vector<SensorInfo>& sensor_list);

  fpromise::promise<void> ClearPlaybackConfig();

  fpromise::promise<void, ConfigurePlaybackError> ConfigureFixedValues(
      const FixedValuesPlaybackConfig& config);

  fpromise::promise<void, ConfigurePlaybackError> ConfigureFilePlayback(
      const FilePlaybackConfig& config);

  fpromise::promise<void> UpdatePlaybackState(SensorId sensor_id, bool enabled);

  fpromise::promise<void> ScheduleSensorEvents(SensorId sensor_id);

  fpromise::promise<void> StartFileReads();
  fpromise::promise<void> ScheduleFileReads(bool first_after_seek);

  // Generates the next event for a sensor given the current internal state of the fixed playback
  // sequence.
  // This should only be called from a promise scheduled to run on this actor's executor.
  SensorEvent GenerateNextFixedEventForSensor(SensorId sensor_id, zx::time timestamp);

  fpromise::promise<void> ScheduleFixedEvent(SensorId sensor_id);

  fpromise::promise<void> SendOrBufferEvent(const SensorEvent& event);

  fpromise::promise<void> SendBufferedEvents(SensorId sensor_id);

  fpromise::promise<void> StopScheduledPlayback();

  // Callback to initiate a driver client disconnect.
  std::function<fpromise::promise<void>(zx_status_t)> disconnect_driver_client_callback_;

  // Callback to call with sensor events.
  std::function<void(const SensorEvent&)> event_callback_;

  // List of "sensors" currently provided by the playback data.
  std::vector<SensorInfo> sensor_list_;

  // Overall playback state.
  PlaybackState playback_state_ = PlaybackState::kStopped;
  // Playback states for each sensor.
  std::unordered_map<SensorId, SensorPlaybackState> sensor_playback_state_;

  // Number of currently enabled sensors.
  int enabled_sensor_count_ = 0;

  // Which playback mode is currently active.
  PlaybackMode playback_mode_;

  // Dispatcher to use when creating a FileReader.
  async_dispatcher_t* file_read_dispatcher_;

  // A FileReader actor which will be created if playback from a file is configured.
  std::optional<FileReader> file_reader_;
  // Flag to indicate if the FileReader playback is currently running.
  bool file_reader_running_ = false;
  // A set of initial timestamp conditions. Used for calculating event and presentation timestamps
  // in the playback time domain from the ones in the data file.
  std::optional<FileTimestampData> file_timestamp_data_;

  // A promise scope to wrap scheduled SendEvent and scheduling promises.
  std::unique_ptr<fpromise::scope> running_playback_scope_;

  // This should always be the last thing in the object. Otherwise scheduled tasks within this scope
  // which reference members of this object may be allowed to run after destruction of this object
  // has started. Keeping this at the end ensures that the scope is destroyed first, cancelling any
  // scheduled tasks before the rest of the members are destroyed.
  fpromise::scope scope_;
};

}  // namespace sensors::playback

#endif  // SRC_SENSORS_PLAYBACK_PLAYBACK_CONTROLLER_H_
