// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MEDIA_AUDIO_SERVICES_DEVICE_REGISTRY_DEVICE_DETECTOR_H_
#define SRC_MEDIA_AUDIO_SERVICES_DEVICE_REGISTRY_DEVICE_DETECTOR_H_

#include <fidl/fuchsia.audio.device/cpp/common_types.h>
#include <fidl/fuchsia.audio.device/cpp/natural_types.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/result.h>

#include <functional>
#include <memory>
#include <string>
#include <string_view>
#include <unordered_map>

#include "src/lib/fsl/io/device_watcher.h"

namespace media_audio {

using DeviceDetectionHandler = std::function<void(
    std::string_view, fuchsia_audio_device::DeviceType, fuchsia_audio_device::DriverClient)>;
using DeviceDetectionIdleHandler = std::function<void(void)>;

// This class detects devices and invokes the provided handler for those devices. It uses multiple
// file-system watchers that focus on the device file system (devfs), specifically the locations
// where registered audio devices are exposed (dev/class/audio-composite, etc).
class DeviceDetector {
  struct DeviceNodeSpecifier {
    const char* path;
    fuchsia_audio_device::DeviceType device_type;
  };

  static constexpr DeviceNodeSpecifier kAudioDevNodes[] = {
      {
          .path = "/dev/class/audio-composite",
          .device_type = fuchsia_audio_device::DeviceType::kComposite,
      },
      {
          .path = "/dev/class/codec",
          .device_type = fuchsia_audio_device::DeviceType::kCodec,
      },
  };

 public:
  // Immediately kick off watchers in 'devfs' directories where audio devices are found.
  // Upon detection, our DeviceDetectionHandler is run on the dispatcher's thread.
  static zx::result<std::shared_ptr<DeviceDetector>> Create(DeviceDetectionHandler handler,
                                                            DeviceDetectionIdleHandler idle_handler,
                                                            async_dispatcher_t* dispatcher,
                                                            bool ignore_devices);

  virtual ~DeviceDetector() = default;

 private:
  static constexpr std::string_view kClassName = "DeviceDetector";

  DeviceDetector(DeviceDetectionHandler handler, DeviceDetectionIdleHandler idle_handler,
                 async_dispatcher_t* dispatcher, bool ignore_devices)
      : handler_(std::move(handler)),
        idle_handler_(std::move(idle_handler)),
        ignore_devices_(ignore_devices),
        dispatcher_(dispatcher) {
    initial_detection_complete_by_device_type_.emplace(fuchsia_audio_device::DeviceType::kCodec,
                                                       false);
    initial_detection_complete_by_device_type_.emplace(fuchsia_audio_device::DeviceType::kComposite,
                                                       false);
  }
  DeviceDetector() = delete;

  zx_status_t StartDeviceWatchers();

  // Open a devnode at the given path; use its FDIO device channel to connect (retrieve) the
  // device's primary protocol (Composite, etc).
  void DriverClientFromDevFs(const fidl::ClientEnd<fuchsia_io::Directory>& dir,
                             const std::string& name, fuchsia_audio_device::DeviceType device_type);

  void DeviceTypeCompletedInitialDetection(fuchsia_audio_device::DeviceType device_type);

  DeviceDetectionHandler handler_;
  DeviceDetectionIdleHandler idle_handler_;
  std::vector<std::unique_ptr<fsl::DeviceWatcher>> watchers_;

  // If set, don't initialize/control detected devices: allow tests to directly connect to drivers.
  bool ignore_devices_;

  struct DeviceTypeHash {
    std::size_t operator()(fuchsia_audio_device::DeviceType type) const noexcept {
      return std::hash<uint32_t>{}(static_cast<uint32_t>(type));
    }
  };

  std::unordered_map<fuchsia_audio_device::DeviceType, bool, DeviceTypeHash>
      initial_detection_complete_by_device_type_;
  bool initial_detection_complete_ = false;  // only included for consistency check

  async_dispatcher_t* dispatcher_;
};

}  // namespace media_audio

#endif  // SRC_MEDIA_AUDIO_SERVICES_DEVICE_REGISTRY_DEVICE_DETECTOR_H_
