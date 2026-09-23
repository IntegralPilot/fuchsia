// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/sensors/playback/playback_controller.h"

#include <lib/fpromise/bridge.h>

namespace sensors::playback {
namespace {
using fpromise::bridge;
using fpromise::promise;
using fpromise::result;
using fpromise::scope;

using fuchsia_hardware_sensors::ActivateSensorError;
using fuchsia_hardware_sensors::ConfigurePlaybackError;
using fuchsia_hardware_sensors::ConfigureSensorRateError;
using fuchsia_hardware_sensors::DeactivateSensorError;
using fuchsia_hardware_sensors::FixedValuesPlaybackConfig;
using fuchsia_hardware_sensors::PlaybackSourceConfig;
using fuchsia_sensors_types::SensorEvent;
using fuchsia_sensors_types::SensorId;
using fuchsia_sensors_types::SensorInfo;
using fuchsia_sensors_types::SensorRateConfig;

using RecordedSensorEvent = io::Deserializer::RecordedSensorEvent;

// Pushing events to a channel too quickly can cause crashes due to the channel filling up. Right
// now the limit seems to be in the neighbourhood of 1 kHz. Most real sensors will not support rates
// that high, and if they do it will be via a shared memory interface rather than a channel. In
// Android, the highest rates that most sensors support without using shared memory is ~200 Hz.
// Eventually we'll add minimum sampling period information to SensorInfo which can be used to do
// more realistic clamping, but for now we'll clamp to 200 Hz to avoid crashes.
const int64_t kMinSamplingPeriod = 5e6;
void TemporarilyClampSensorRateConfig(SensorRateConfig& rate_config) {
  rate_config.sampling_period_ns(std::max(*rate_config.sampling_period_ns(), kMinSamplingPeriod));
}

}  // namespace

PlaybackController::PlaybackController(async_dispatcher_t* dispatcher,
                                       async_dispatcher_t* file_read_dispatcher)
    : ActorBase(dispatcher, scope_),
      file_read_dispatcher_(file_read_dispatcher),
      running_playback_scope_(std::make_unique<scope>()) {}

promise<void, ConfigurePlaybackError> PlaybackController::ConfigurePlayback(
    const PlaybackSourceConfig& config) {
  bridge<void, ConfigurePlaybackError> bridge;
  Schedule([this, config, completer = std::move(bridge.completer)]() mutable -> promise<void> {
    switch (config.Which()) {
      case PlaybackSourceConfig::Tag::kFixedValuesConfig:
        // Stop any playback and clear the playback configuration.
        return StopScheduledPlayback()
            .and_then(ClearPlaybackConfig())
            .then([this, config](result<void>& result) -> promise<void, ConfigurePlaybackError> {
              std::optional<FixedValuesPlaybackConfig> fixed_config = config.fixed_values_config();
              return ConfigureFixedValues(*fixed_config);
            })
            .then([completer = std::move(completer)](
                      result<void, ConfigurePlaybackError>& result) mutable -> promise<void> {
              if (result.is_ok()) {
                completer.complete_ok();
              } else {
                completer.complete_error(result.error());
              }
              return fpromise::make_ok_promise();
            });
        break;
      case PlaybackSourceConfig::Tag::kFilePlaybackConfig:
        return StopScheduledPlayback()
            .and_then(ClearPlaybackConfig())
            .then([this, config](result<void>& result) -> promise<void, ConfigurePlaybackError> {
              std::optional<FilePlaybackConfig> file_config = config.file_playback_config();
              return ConfigureFilePlayback(*file_config);
            })
            .then([completer = std::move(completer)](
                      result<void, ConfigurePlaybackError>& result) mutable -> promise<void> {
              if (result.is_ok()) {
                completer.complete_ok();
              } else {
                completer.complete_error(result.error());
              }
              return fpromise::make_ok_promise();
            });
        break;
      default:
        // Complete the bridged promise with an error and do nothing further.
        completer.complete_error(ConfigurePlaybackError::kInvalidConfigType);
        return fpromise::make_ok_promise();
    };
  });
  return bridge.consumer.promise();
}

promise<std::vector<SensorInfo>> PlaybackController::GetSensorsList() {
  bridge<std::vector<SensorInfo>> bridge;
  Schedule([this, completer = std::move(bridge.completer)]() mutable -> promise<void> {
    // TODO(b/343048375): Disconnect the client if no playback configuration is present.
    if (playback_mode_ == PlaybackMode::kNone) {
      FX_LOGS(WARNING)
          << "GetSensorsList: Driver API call made while playback configuration is unset."
          << " In the future this will result in the Driver client being disconnected "
          << "with epitaph ZX_ERR_BAD_STATE.";
    }

    completer.complete_ok(sensor_list_);
    return fpromise::make_ok_promise();
  });
  return bridge.consumer.promise();
}

promise<void, ActivateSensorError> PlaybackController::ActivateSensor(SensorId sensor_id) {
  bridge<void, ActivateSensorError> bridge;
  Schedule(fpromise::make_promise(
      [this, sensor_id, completer = std::move(bridge.completer)]() mutable -> promise<void> {
        if (playback_mode_ == PlaybackMode::kNone) {
          FX_LOGS(ERROR) << "ActivateSensor: Driver API call made while playback configuration is "
                         << "unset, disconnecting Driver client with epitaph ZX_ERR_BAD_STATE.";
          return DisconnectDriverClient(ZX_ERR_BAD_STATE);
        }

        if (sensor_playback_state_.count(sensor_id) == 0) {
          completer.complete_error(ActivateSensorError::kInvalidSensorId);
          return fpromise::make_ok_promise();
        }

        return UpdatePlaybackState(sensor_id, true)
            .and_then([completer = std::move(completer)]() mutable { completer.complete_ok(); });
      }));

  return bridge.consumer.promise();
}

promise<void, DeactivateSensorError> PlaybackController::DeactivateSensor(SensorId sensor_id) {
  bridge<void, DeactivateSensorError> bridge;
  Schedule(fpromise::make_promise(
      [this, sensor_id, completer = std::move(bridge.completer)]() mutable -> promise<void> {
        if (playback_mode_ == PlaybackMode::kNone) {
          FX_LOGS(ERROR) << "DeactivateSensor: Driver API call made while playback configuration is"
                         << " unset, disconnecting Driver client with epitaph ZX_ERR_BAD_STATE.";
          return DisconnectDriverClient(ZX_ERR_BAD_STATE);
        }

        if (sensor_playback_state_.count(sensor_id) == 0) {
          FX_LOGS(ERROR) << "DeactivateSensor error, invalid sensor ID.";
          completer.complete_error(DeactivateSensorError::kInvalidSensorId);
          return fpromise::make_ok_promise();
        }

        return UpdatePlaybackState(sensor_id, false)
            .and_then([completer = std::move(completer)]() mutable { completer.complete_ok(); });
      }));

  return bridge.consumer.promise();
}

promise<void, ConfigureSensorRateError> PlaybackController::ConfigureSensorRate(
    SensorId sensor_id, SensorRateConfig rate_config) {
  bridge<void, ConfigureSensorRateError> bridge;
  Schedule([this, sensor_id, rate_config,
            completer = std::move(bridge.completer)]() mutable -> promise<void> {
    if (playback_mode_ == PlaybackMode::kNone) {
      FX_LOGS(ERROR) << "ConfigureSensorRate: Driver API call made while playback configuration is "
                     << "unset, disconnecting Driver client with epitaph ZX_ERR_BAD_STATE.";
      return DisconnectDriverClient(ZX_ERR_BAD_STATE);
    }

    if (!rate_config.sampling_period_ns() || !rate_config.max_reporting_latency_ns()) {
      FX_LOGS(ERROR) << "ConfigureSensorRate: Fields missing from rate config.";
      completer.complete_error(ConfigureSensorRateError::kInvalidConfig);
      return fpromise::make_ok_promise();
    }
    if (sensor_playback_state_.count(sensor_id) == 0) {
      FX_LOGS(ERROR) << "ConfigureSensorRate: Invalid sensor ID.";
      completer.complete_error(ConfigureSensorRateError::kInvalidSensorId);
      return fpromise::make_ok_promise();
    }

    TemporarilyClampSensorRateConfig(rate_config);

    FX_LOGS(INFO) << "ConfigureSensorRate: Setting sampling period to "
                  << *rate_config.sampling_period_ns() << " ns and max reporting lagency to "
                  << *rate_config.max_reporting_latency_ns() << " ns for sensor " << sensor_id;

    sensor_playback_state_[sensor_id].sampling_period =
        zx::duration(*rate_config.sampling_period_ns());
    sensor_playback_state_[sensor_id].max_reporting_latency =
        zx::duration(*rate_config.max_reporting_latency_ns());
    completer.complete_ok();
    return fpromise::make_ok_promise();
  });
  return bridge.consumer.promise();
}

promise<void> PlaybackController::SetDisconnectDriverClientCallback(
    std::function<promise<void>(zx_status_t)> disconnect_callback) {
  bridge<void> bridge;
  Schedule([this, disconnect_callback = std::move(disconnect_callback),
            completer = std::move(bridge.completer)]() mutable {
    disconnect_driver_client_callback_ = std::move(disconnect_callback);
    completer.complete_ok();
  });
  return bridge.consumer.promise();
}

promise<void> PlaybackController::SetEventCallback(
    std::function<void(const SensorEvent&)> event_callback) {
  bridge<void> bridge;
  Schedule([this, event_callback = std::move(event_callback),
            completer = std::move(bridge.completer)]() mutable {
    event_callback_ = std::move(event_callback);
    completer.complete_ok();
  });
  return bridge.consumer.promise();
}

void PlaybackController::DriverClientDisconnected(fit::callback<void()> unbind_callback) {
  Schedule([this, unbind_callback = std::move(unbind_callback)]() mutable -> promise<void> {
    disconnect_driver_client_callback_ = nullptr;
    event_callback_ = nullptr;
    if (playback_state_ == PlaybackState::kRunning) {
      FX_LOGS(INFO) << "Stopping playback due to driver client disconnect.";
      playback_state_ = PlaybackState::kStopped;
      return StopScheduledPlayback().and_then(
          [unbind_callback = std::move(unbind_callback)]() mutable { unbind_callback(); });
    }

    unbind_callback();
    return fpromise::make_ok_promise();
  });
}

void PlaybackController::PlaybackClientDisconnected(fit::callback<void()> unbind_callback) {
  Schedule([this,
            unbind_callback = std::move(unbind_callback)]() mutable -> fpromise::promise<void> {
    if (playback_state_ == PlaybackState::kRunning) {
      FX_LOGS(INFO) << "Stopping playback, disconnecting driver client, and clearing playback "
                    << "configuration due to playback client disconnection.";
      playback_state_ = PlaybackState::kStopped;
      return StopScheduledPlayback()
          .and_then(DisconnectDriverClient(ZX_ERR_BAD_STATE))
          .and_then(ClearPlaybackConfig())
          .and_then(
              [unbind_callback = std::move(unbind_callback)]() mutable { unbind_callback(); });
    }
    if (disconnect_driver_client_callback_) {
      FX_LOGS(INFO) << "Disconnecting driver client and clearing playback configuration due to "
                    << "playback client disconnect. ";
    }
    return DisconnectDriverClient(ZX_ERR_BAD_STATE)
        .and_then(ClearPlaybackConfig())
        .and_then([unbind_callback = std::move(unbind_callback)]() mutable { unbind_callback(); });
  });
}

promise<void> PlaybackController::DisconnectDriverClient(zx_status_t epitaph) {
  return fpromise::make_promise([this, epitaph]() -> promise<void> {
    event_callback_ = nullptr;
    if (disconnect_driver_client_callback_) {
      std::function<promise<void>(zx_status_t)> callback = disconnect_driver_client_callback_;
      disconnect_driver_client_callback_ = nullptr;
      return callback(epitaph);
    }
    return fpromise::make_ok_promise();
  });
}

void PlaybackController::AdoptSensorList(const std::vector<SensorInfo>& sensor_list) {
  sensor_list_ = sensor_list;
  for (const SensorInfo& info : sensor_list_) {
    sensor_playback_state_[*info.sensor_id()] = SensorPlaybackState();
  }
}

promise<void> PlaybackController::ClearPlaybackConfig() {
  return fpromise::make_promise([this]() {
    FX_LOGS(INFO) << "Clearing playback configuration.";
    sensor_list_.clear();

    playback_state_ = PlaybackState::kStopped;
    sensor_playback_state_.clear();
    enabled_sensor_count_ = 0;
    playback_mode_ = PlaybackMode::kNone;
    file_reader_ = std::nullopt;
    file_timestamp_data_ = std::nullopt;
  });
}

promise<void, ConfigurePlaybackError> PlaybackController::ConfigureFixedValues(
    const FixedValuesPlaybackConfig& config) {
  return fpromise::make_promise([this, config]() mutable -> promise<void, ConfigurePlaybackError> {
    FX_LOGS(INFO) << "ConfigurePlayback: Configuring to emit a repeating fixed list.";
    // Check for required fields.
    std::optional<ConfigurePlaybackError> result = ValidateSensorList(config.sensor_list());
    if (result) {
      FX_LOGS(ERROR) << "ConfigurePlayback: Received invalid or incomplete sensor list.";
      return fpromise::make_error_promise(*result);
    }
    result = ValidateSensorEventList(config.sensor_events(), *config.sensor_list());
    if (result) {
      FX_LOGS(ERROR) << "ConfigurePlayback: Received sensor event list with invalid or "
                     << "incomplete entries.";
      return fpromise::make_error_promise(*result);
    }

    playback_mode_ = PlaybackMode::kFixedValuesMode;
    AdoptSensorList(*config.sensor_list());

    // Store the events on a per-sensor basis.
    for (const SensorEvent& event : *config.sensor_events()) {
      sensor_playback_state_[event.sensor_id()].fixed_sensor_events.push_back(event);
    }

    return fpromise::make_result_promise<void, ConfigurePlaybackError>(fpromise::ok());
  });
}

promise<void, ConfigurePlaybackError> PlaybackController::ConfigureFilePlayback(
    const FilePlaybackConfig& config) {
  return fpromise::make_promise([this, config]() -> promise<void, ConfigurePlaybackError> {
    FX_LOGS(INFO) << "ConfigurePlayback: Configuring to read from a file.";
    // Check for required fields.
    if (!config.file_path()) {
      FX_LOGS(ERROR) << "ConfigurePlayback: File playback config missing required fields.";
      return fpromise::make_error_promise(ConfigurePlaybackError::kConfigMissingFields);
    }

    playback_mode_ = PlaybackMode::kFilePlaybackMode;
    file_reader_.emplace(*config.file_path(), file_read_dispatcher_);

    return file_reader_->Open()
        .and_then([this, config]() {
          promise<std::vector<SensorInfo>> sensor_list_promise = file_reader_->GetSensorsList();

          return sensor_list_promise.then([this](result<std::vector<SensorInfo>>& list_result)
                                              -> promise<void, ConfigurePlaybackError> {
            std::optional<ConfigurePlaybackError> validation_error =
                ValidateSensorList(std::make_optional(list_result.value()));

            if (validation_error) {
              FX_LOGS(ERROR) << "ConfigurePlayback: Received invalid or incomplete sensor list.";
              return ClearPlaybackConfig().then([validation_error](result<void, void>& /*result*/) {
                return fpromise::make_error_promise(*validation_error);
              });
            }

            AdoptSensorList(list_result.value());
            return fpromise::make_result_promise<void, ConfigurePlaybackError>(fpromise::ok());
          });
        })
        .or_else([this](ConfigurePlaybackError& error) {
          return ClearPlaybackConfig().then([error](result<void, void>& result) {
            return fpromise::make_error_promise<ConfigurePlaybackError>(error);
          });
        });
  });
}

promise<void> PlaybackController::UpdatePlaybackState(SensorId sensor_id, bool enabled) {
  return fpromise::make_promise([this, sensor_id, enabled]() -> promise<void> {
    SensorPlaybackState& state = sensor_playback_state_[sensor_id];
    bool already_enabled = state.enabled;
    state.enabled = enabled;

    if (!already_enabled && enabled) {
      FX_LOGS(INFO) << "ActivateSensor: Enabling sensor " << sensor_id;
      enabled_sensor_count_ += 1;
    } else if (already_enabled && !enabled) {
      FX_LOGS(INFO) << "DeactivateSensor: Disabling sensor " << sensor_id;
      enabled_sensor_count_ -= 1;
    } else if (!already_enabled && !enabled) {
      FX_LOGS(WARNING) << "DeactivateSensor: Sensor " << sensor_id << " already disabled.";
      return fpromise::make_ok_promise();
    } else if (already_enabled && enabled) {
      FX_LOGS(WARNING) << "ActivateSensor: Sensor " << sensor_id << " already enabled.";
      return fpromise::make_ok_promise();
    }
    ZX_ASSERT(enabled_sensor_count_ >= 0);

    if (playback_state_ == PlaybackState::kStopped) {
      // If the overall state is stopped but there are enabled sensors, start playback.
      if (enabled_sensor_count_ > 0) {
        FX_LOGS(INFO) << "Starting sensor playback.";

        playback_state_ = PlaybackState::kRunning;
      }
    } else if (playback_state_ == PlaybackState::kRunning) {
      // If the overall state is running but there are no sensors enabled, stop playback.
      if (enabled_sensor_count_ == 0) {
        FX_LOGS(INFO) << "Stopping sensor playback.";

        playback_state_ = PlaybackState::kStopped;
        return StopScheduledPlayback();
      }
    }

    if (enabled) {
      return ScheduleSensorEvents(sensor_id);
    } else {
      state.last_scheduled_event_time = std::nullopt;
      return fpromise::make_ok_promise();
    }
  });
}

promise<void> PlaybackController::ScheduleSensorEvents(SensorId sensor_id) {
  return fpromise::make_promise([this, sensor_id]() -> promise<void> {
    switch (playback_mode_) {
      case PlaybackMode::kFixedValuesMode:
        return ScheduleFixedEvent(sensor_id);
      case PlaybackMode::kFilePlaybackMode:
        if (!file_reader_running_) {
          file_reader_running_ = true;
          return StartFileReads();
        }
        return fpromise::make_ok_promise();
        break;
      case PlaybackMode::kNone:
        ZX_ASSERT_MSG(false, "Events scheduled when no playback mode was set.");
    }
  });
}

promise<void> PlaybackController::StartFileReads() {
  return file_reader_
      ->SetEventCallback([this](const RecordedSensorEvent& recorded_event) {
        Schedule([this, recorded_event]() {
          // If playback shouldn't be running any more, do nothing further.
          if (playback_state_ != PlaybackState::kRunning) {
            return;
          }

          SensorEvent event = recorded_event.event;
          zx::time receipt_timestamp =
              zx::time(recorded_event.receipt_timestamp.value_or(recorded_event.event.timestamp()));

          // Initialize the file timestamp initial conditions if they haven't been.
          zx::time schedule_time, timestamp;
          if (!file_timestamp_data_) {
            // The first scheduled event time will be now.
            schedule_time = zx::time(zx_clock_get_monotonic());
            file_timestamp_data_ = FileTimestampData();
            file_timestamp_data_->first_presentation_time = schedule_time;

            // Record the first timestamp and presentation time in file time domain.
            file_timestamp_data_->file_first_presentation_time = receipt_timestamp;
            file_timestamp_data_->file_first_event_timestamp = zx::time(event.timestamp());

            // Use the offset between the file domain timestamp and presentation time to calculate
            // the initial event timestamp in the playback domain.
            zx::duration offset = file_timestamp_data_->file_first_presentation_time -
                                  file_timestamp_data_->file_first_event_timestamp;
            timestamp = file_timestamp_data_->first_presentation_time - offset;
            file_timestamp_data_->first_event_timestamp = timestamp;
          } else {
            // Use the file domain presentation timestamps to calculate the offset since the
            // beginning of playback.
            zx::duration schedule_offset =
                zx::time(receipt_timestamp) - file_timestamp_data_->file_first_presentation_time;
            // Add the offset to the starting time in the playback clock domain to get a scheduling
            // time.
            schedule_time = file_timestamp_data_->first_presentation_time + schedule_offset;

            // Use the file domain event timestamps to calculate the offset from the first
            // timestamp.
            zx::duration timestamp_offset =
                zx::time(event.timestamp()) - file_timestamp_data_->file_first_event_timestamp;
            // Add the offset to the initial timestamp in the playback domain to get this event's
            // timestamp.
            timestamp = file_timestamp_data_->first_event_timestamp + timestamp_offset;
          }
          file_timestamp_data_->last_read_presentation_time = schedule_time;
          file_timestamp_data_->last_read_event_timestamp = timestamp;

          event.timestamp() = timestamp.get();
          ScheduleAtTime(schedule_time,
                         SendOrBufferEvent(event).wrap_with(*running_playback_scope_));
        });
      })
      .and_then(
          file_reader_->SetReadCompleteCallback([this](std::optional<zx_status_t> finished_early) {
            Schedule([this, finished_early]() -> promise<void> {
              if (finished_early) {
                if (*finished_early == ZX_OK) {
                  // We reached the end of the file. Seek back to the beginning.
                  return file_reader_->SeekToFirstEvent().and_then(
                      ScheduleFileReads(/*first_after_seek=*/true));
                } else {
                  FX_LOGS(ERROR) << "Stopping playback and clearing config. "
                                 << "Saw error reading from data file: " << *finished_early;
                  return StopScheduledPlayback().and_then(ClearPlaybackConfig());
                }
              }

              return ScheduleFileReads(false);
            });
          }))
      .and_then(ScheduleFileReads(false));
}

constexpr int kNumEventReads = 1;
promise<void> PlaybackController::ScheduleFileReads(bool first_after_seek) {
  static const zx::duration kResetTimeGap = zx::msec(10);
  return fpromise::make_promise([this, first_after_seek]() {
    if (playback_state_ != PlaybackState::kRunning) {
      return;
    }

    // If we've started reading events, schedule the next read for when the last read event is
    // scheduled to be sent. Otherwise schedule a read immediately.
    if (file_timestamp_data_) {
      // After a seek to the beginning of the file's events, reset the first presentation time and
      // event timestamp to continue the sequence after a configured gap.
      if (first_after_seek) {
        zx::duration offset = file_timestamp_data_->file_first_presentation_time -
                              file_timestamp_data_->file_first_event_timestamp;
        file_timestamp_data_->first_presentation_time =
            file_timestamp_data_->last_read_presentation_time + kResetTimeGap;
        file_timestamp_data_->first_event_timestamp =
            file_timestamp_data_->first_presentation_time - offset;
      }
      ScheduleAtTime(file_timestamp_data_->last_read_presentation_time,
                     fpromise::make_promise([this]() {
                       file_reader_->ReadEvents(kNumEventReads);
                     }).wrap_with(*running_playback_scope_));
    } else {
      file_reader_->ReadEvents(kNumEventReads);
    }
  });
}

// Generates the next event for a sensor given the current internal state of the fixed playback
// sequence.
// This should only be called from a promise scheduled to run on this actor's executor.
SensorEvent PlaybackController::GenerateNextFixedEventForSensor(SensorId sensor_id,
                                                                zx::time timestamp) {
  SensorPlaybackState& state = sensor_playback_state_[sensor_id];

  SensorEvent event = state.fixed_sensor_events[state.next_fixed_event];
  state.next_fixed_event = (state.next_fixed_event + 1) % state.fixed_sensor_events.size();
  event.timestamp(timestamp.get());

  return event;
}

promise<void> PlaybackController::ScheduleFixedEvent(SensorId sensor_id) {
  return fpromise::make_promise([this, sensor_id]() {
    // If playback shouldn't be running any more, schedule nothing further.
    if (playback_state_ != PlaybackState::kRunning) {
      return;
    }
    SensorPlaybackState& state = sensor_playback_state_[sensor_id];
    // If the sensor has become disabled, schedule nothing further.
    if (!state.enabled) {
      return;
    }

    // Schedules a promise chain which will emit an event, and then schedule the sending of the
    // following event for this sensor. If it's the first event after activation schedule it
    // immediately, otherwise schedule it for the previous event time plus the sampling period.
    //
    // Wraps the promise chain in a special scope so we can cancel it separately to the rest of the
    // controller actor promises.
    zx::time timestamp_base;
    if (!state.last_scheduled_event_time) {
      timestamp_base = zx::time(zx_clock_get_monotonic());
    } else {
      timestamp_base = *state.last_scheduled_event_time;
    }
    zx::time event_timestamp = timestamp_base + state.sampling_period;
    state.last_scheduled_event_time = event_timestamp;
    SensorEvent next_event = GenerateNextFixedEventForSensor(sensor_id, event_timestamp);
    ScheduleAtTime(event_timestamp, SendOrBufferEvent(next_event)
                                        .and_then(ScheduleFixedEvent(sensor_id))
                                        .wrap_with(*running_playback_scope_));
  });
}

promise<void> PlaybackController::SendOrBufferEvent(const SensorEvent& event) {
  return fpromise::make_promise([this, event]() {
    SensorId sensor_id = event.sensor_id();
    SensorPlaybackState& state = sensor_playback_state_[sensor_id];

    // If playback shouldn't be running any more, do nothing.
    if (playback_state_ != PlaybackState::kRunning)
      return;
    // If the sensor has become disabled, do nothing.
    if (!state.enabled)
      return;

    // If the maximum reporting latency for the sensor is 0, send the event immediately and do
    // nothing further.
    if (state.max_reporting_latency == zx::duration(0) && event_callback_) {
      event_callback_(event);
      return;
    }

    // Add the event to the sensor's output buffer.
    state.event_buffer.push(event);
    // If that was the first event in the buffer, schedule the buffer contents to be sent after the
    // max reporting latency has elapsed.
    if (state.event_buffer.size() == 1) {
      zx::time send_buffer_time = zx::time(event.timestamp()) + state.max_reporting_latency;
      ScheduleAtTime(send_buffer_time,
                     SendBufferedEvents(sensor_id).wrap_with(*running_playback_scope_));
    }
  });
}

promise<void> PlaybackController::SendBufferedEvents(SensorId sensor_id) {
  return fpromise::make_promise([this, sensor_id]() {
    SensorPlaybackState& state = sensor_playback_state_[sensor_id];

    // If playback shouldn't be running any more, do nothing.
    if (playback_state_ != PlaybackState::kRunning)
      return;
    // If the sensor has become disabled, do nothing.
    if (!state.enabled)
      return;
    // Do nothing if the event callback isn't set.
    if (!event_callback_)
      return;

    while (!state.event_buffer.empty()) {
      event_callback_(state.event_buffer.front());
      state.event_buffer.pop();
    }
  });
}

promise<void> PlaybackController::StopScheduledPlayback() {
  auto stop_scheduled_sends = fpromise::make_promise([this]() {
    running_playback_scope_ = std::make_unique<scope>();
    file_reader_running_ = false;
    file_timestamp_data_ = std::nullopt;
  });

  if (file_reader_running_) {
    return file_reader_->StopScheduledReads().and_then(std::move(stop_scheduled_sends));
  }
  return stop_scheduled_sends;
}

}  // namespace sensors::playback
