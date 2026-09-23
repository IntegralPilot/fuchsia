// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/media/audio/services/device_registry/device_detector.h"

#include <fidl/fuchsia.audio.device/cpp/common_types.h>
#include <fidl/fuchsia.audio.device/cpp/natural_types.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fidl/cpp/wire/internal/transport_channel.h>
#include <lib/syslog/cpp/macros.h>

#include <memory>

#include "src/lib/fsl/io/device_watcher.h"
#include "src/media/audio/services/device_registry/inspector.h"
#include "src/media/audio/services/device_registry/logging.h"

namespace media_audio {

namespace fad = fuchsia_audio_device;
namespace fha = fuchsia_hardware_audio;

zx::result<std::shared_ptr<DeviceDetector>> DeviceDetector::Create(
    DeviceDetectionHandler handler, DeviceDetectionIdleHandler idle_handler,
    async_dispatcher_t* dispatcher, bool ignore_devices) {
  // The constructor is private, forcing clients to use DeviceDetector::Create().
  class MakePublicCtor : public DeviceDetector {
   public:
    MakePublicCtor(DeviceDetectionHandler handler, DeviceDetectionIdleHandler idle_handler,
                   async_dispatcher_t* dispatcher, bool ignore_devices)
        : DeviceDetector(std::move(handler), std::move(idle_handler), dispatcher, ignore_devices) {}
  };

  auto detector = std::make_shared<MakePublicCtor>(std::move(handler), std::move(idle_handler),
                                                   dispatcher, ignore_devices);

  if (auto status = detector->StartDeviceWatchers(); status != ZX_OK) {
    return zx::error(status);
  }
  return zx::ok(detector);
}

zx_status_t DeviceDetector::StartDeviceWatchers() {
  ADR_LOG_METHOD(kLogDeviceDetection)
      << "by configuration, devices will be " << (ignore_devices_ ? "ignored" : "detected");

  // StartDeviceWatchers should never be called a second time.
  FX_CHECK(watchers_.empty());
  FX_CHECK(dispatcher_);

  for (const auto& dev_node : kAudioDevNodes) {
    auto watcher = fsl::DeviceWatcher::CreateWithIdleCallback(
        dev_node.path,
        [this, device_type = dev_node.device_type](
            const fidl::ClientEnd<fuchsia_io::Directory>& dir, const std::string& filename) {
          if (ignore_devices_) {
            ADR_LOG_OBJECT(kLogDeviceDetection || kLogDeviceAddErrorRemove)
                << device_type << " device was ignored";
            return;
          }
          if (!dispatcher_) {
            FX_LOGS(ERROR) << "DeviceWatcher fired but dispatcher is gone";
            Inspector::Singleton()->RecordDetectionFailureOther();
            return;
          }
          if (device_type == fad::DeviceType::kCodec ||
              device_type == fad::DeviceType::kComposite) {
            ADR_LOG_OBJECT(kLogDeviceDetection) << "Successful detection, proceeding to Connect";
            DriverClientFromDevFs(dir, filename, device_type);
          } else {
            ADR_WARN_OBJECT() << device_type << " device detection not supported";
            Inspector::Singleton()->RecordDetectionFailureUnsupported();
            return;
          }
        },
        [this, device_type = dev_node.device_type]() {
          DeviceTypeCompletedInitialDetection(device_type);
        },
        dispatcher_);

    // If any of our directory-monitors cannot be created, destroy them all and fail.
    if (watcher == nullptr) {
      FX_LOGS(ERROR) << "DeviceDetector failed to create DeviceWatcher for '" << dev_node.path
                     << "'; stopping all device monitoring.";
      watchers_.clear();
      handler_ = nullptr;
      Inspector::Singleton()->RecordDetectionFailureOther();
      return ZX_ERR_INTERNAL;
    }
    watchers_.emplace_back(std::move(watcher));
  }

  return ZX_OK;
}

void DeviceDetector::DriverClientFromDevFs(const fidl::ClientEnd<fuchsia_io::Directory>& dir,
                                           const std::string& name, fad::DeviceType device_type) {
  ADR_LOG_STATIC(kLogDeviceDetection);
  FX_CHECK(handler_);

  std::optional<fad::DriverClient> driver_client;

  if (device_type == fad::DeviceType::kCodec) {
    zx::result client_end = component::ConnectAt<fha::CodecConnector>(dir, name);
    if (client_end.is_error()) {
      FX_PLOGS(ERROR, client_end.error_value())
          << "DeviceDetector failed to connect to device node at '" << name << "'";
      Inspector::Singleton()->RecordDetectionFailureToConnect();
      return;
    }
    fidl::Client connector(std::move(client_end.value()), dispatcher_);
    auto [client, server] = fidl::Endpoints<fha::Codec>::Create();
    auto status = connector->Connect(std::move(server));
    if (!status.is_ok()) {
      FX_PLOGS(ERROR, status.error_value().status())
          << "Connector/Connect failed for " << device_type;
      Inspector::Singleton()->RecordDetectionFailureToConnect();
      return;
    }
    driver_client = fad::DriverClient::WithCodec(std::move(client));
  } else if (device_type == fad::DeviceType::kComposite) {
    zx::result client_end = component::ConnectAt<fha::Composite>(dir, name);
    if (client_end.is_error()) {
      FX_PLOGS(ERROR, client_end.error_value())
          << "DeviceDetector failed to connect to DFv2 Composite node at '" << name << "'";
      Inspector::Singleton()->RecordDetectionFailureToConnect();
      return;
    }
    driver_client = fad::DriverClient::WithComposite(std::move(client_end.value()));
  } else {
    ADR_WARN_METHOD() << device_type << " device detection not yet supported";
    Inspector::Singleton()->RecordDetectionFailureUnsupported();
    return;
  }

  ADR_LOG_METHOD(kLogDeviceDetection)
      << "Detected and connected to " << device_type << " '" << name << "'";
  handler_(name, device_type, std::move(*driver_client));
}

void DeviceDetector::DeviceTypeCompletedInitialDetection(
    fuchsia_audio_device::DeviceType device_type) {
  ADR_LOG_METHOD(kLogDeviceDetection) << device_type;
  FX_DCHECK(device_type != fuchsia_audio_device::DeviceType::Unknown());
  FX_DCHECK(!initial_detection_complete_);
  initial_detection_complete_by_device_type_[device_type] = true;

  if (std::ranges::all_of(initial_detection_complete_by_device_type_.begin(),
                          initial_detection_complete_by_device_type_.end(),
                          [](auto map_entry) { return map_entry.second; })) {
    idle_handler_();
    initial_detection_complete_ = true;
  }
}

}  // namespace media_audio
