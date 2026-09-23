// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MEDIA_AUDIO_SERVICES_DEVICE_REGISTRY_DEVICE_PRESENCE_WATCHER_H_
#define SRC_MEDIA_AUDIO_SERVICES_DEVICE_REGISTRY_DEVICE_PRESENCE_WATCHER_H_

#include <memory>

namespace media_audio {

class Device;

// This subset of the larger parent-class interface (AudioDeviceRegistry) is the minimal API set
// necessary to test the Device class.
class DevicePresenceWatcher {
 public:
  virtual ~DevicePresenceWatcher() = default;

  // Called when a device successfully completes initialization and is ready to be configured.
  virtual void DeviceIsReady(std::shared_ptr<media_audio::Device> device) = 0;

  // Called if a device encountered an error at any time (including self-reporting as unhealthy).
  virtual void DeviceHasError(std::shared_ptr<media_audio::Device> device) = 0;

  // Called when a device is entirely removed (such as a USB surprise-remove).
  virtual void DeviceIsRemoved(std::shared_ptr<media_audio::Device> device) = 0;
};

}  // namespace media_audio

#endif  // SRC_MEDIA_AUDIO_SERVICES_DEVICE_REGISTRY_DEVICE_PRESENCE_WATCHER_H_
