// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MEDIA_AUDIO_SERVICES_DEVICE_REGISTRY_TESTING_FAKES_FAKE_DEVICE_PRESENCE_WATCHER_H_
#define SRC_MEDIA_AUDIO_SERVICES_DEVICE_REGISTRY_TESTING_FAKES_FAKE_DEVICE_PRESENCE_WATCHER_H_

#include <unordered_set>

#include <gtest/gtest.h>

#include "src/media/audio/services/device_registry/device_presence_watcher.h"
#include "src/media/audio/services/device_registry/logging.h"

namespace media_audio {

class Device;

// This fakes the DevicePresenceWatcher interface, for use in Device testing.
class FakeDevicePresenceWatcher : public DevicePresenceWatcher {
  static constexpr bool kLogFakeDevicePresenceWatcher = false;

 public:
  FakeDevicePresenceWatcher() {
    ADR_LOG_METHOD(kLogFakeDevicePresenceWatcher || kLogObjectLifetimes);
  }
  ~FakeDevicePresenceWatcher() override {
    ADR_LOG_METHOD(kLogFakeDevicePresenceWatcher || kLogObjectLifetimes);
  }

  void DeviceIsReady(std::shared_ptr<Device> device) final {
    ADR_LOG_METHOD(kLogFakeDevicePresenceWatcher);
    ++on_ready_count_;
    if (!ready_devices_.insert(device).second) {
      ADD_FAILURE() << "DeviceIsReady: device was already in ready_devices_";
    }

    EXPECT_EQ(error_devices_.count(device), 0u) << "DeviceIsReady: device found in error_devices_";
    LogObjectCounts();
  }

  void DeviceHasError(std::shared_ptr<Device> device) final {
    ADR_LOG_METHOD(kLogFakeDevicePresenceWatcher);
    ++on_error_count_;
    if (!error_devices_.insert(device).second) {
      ++on_error_from_error_count_;
      ADR_LOG_METHOD(kLogFakeDevicePresenceWatcher) << "device was already in error_devices_";
    }

    if (ready_devices_.erase(device)) {
      ++on_error_from_ready_count_;
      ADR_LOG_METHOD(kLogFakeDevicePresenceWatcher) << "device removed from ready_devices_";
    }
    LogObjectCounts();
  }

  void DeviceIsRemoved(std::shared_ptr<Device> device) final {
    ADR_LOG_METHOD(kLogFakeDevicePresenceWatcher);
    ++on_removal_count_;
    if (error_devices_.erase(device)) {
      ++on_removal_from_error_count_;
      ADR_LOG_METHOD(kLogFakeDevicePresenceWatcher) << "device removed from error_devices_";
    }
    if (ready_devices_.erase(device)) {
      ++on_removal_from_ready_count_;
      ADR_LOG_METHOD(kLogFakeDevicePresenceWatcher) << "device removed from ready_devices_";
    }
    LogObjectCounts();
  }

  const std::unordered_set<std::shared_ptr<Device>>& ready_devices() { return ready_devices_; }
  const std::unordered_set<std::shared_ptr<Device>>& error_devices() { return error_devices_; }

  // These are monotonically increasing counts. If a device was successfully added, then encountered
  // an error, then was removed, the resulting [ready, error, removal] counts should be [1, 1, 1].
  uint16_t on_ready_count() const { return on_ready_count_; }
  uint16_t on_error_count() const { return on_error_count_; }
  uint16_t on_removal_count() const { return on_removal_count_; }
  // These provide additional counts about whether a newly err'ed or removed device was already
  // found in the ready or error lists.
  uint16_t on_error_from_error_count() const { return on_error_from_error_count_; }
  uint16_t on_error_from_ready_count() const { return on_error_from_ready_count_; }
  uint16_t on_removal_from_error_count() const { return on_removal_from_error_count_; }
  uint16_t on_removal_from_ready_count() const { return on_removal_from_ready_count_; }

 private:
  static constexpr std::string_view kClassName = "FakeDevicePresenceWatcher";

  std::unordered_set<std::shared_ptr<Device>> ready_devices_;
  std::unordered_set<std::shared_ptr<Device>> error_devices_;

  uint16_t on_ready_count_ = 0;
  uint16_t on_error_count_ = 0;
  uint16_t on_error_from_error_count_ = 0;
  uint16_t on_error_from_ready_count_ = 0;
  uint16_t on_removal_count_ = 0;
  uint16_t on_removal_from_error_count_ = 0;
  uint16_t on_removal_from_ready_count_ = 0;
};

#undef ADR_LOG_METHOD

}  // namespace media_audio

#endif  // SRC_MEDIA_AUDIO_SERVICES_DEVICE_REGISTRY_TESTING_FAKES_FAKE_DEVICE_PRESENCE_WATCHER_H_
