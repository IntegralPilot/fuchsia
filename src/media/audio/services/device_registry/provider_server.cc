// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/media/audio/services/device_registry/provider_server.h"

#include <fidl/fuchsia.audio.device/cpp/natural_types.h>
#include <lib/fidl/cpp/wire/unknown_interaction_handler.h>
#include <lib/fit/internal/result.h>
#include <lib/zx/clock.h>

#include <utility>

#include "src/media/audio/services/device_registry/audio_device_registry.h"
#include "src/media/audio/services/device_registry/device.h"
#include "src/media/audio/services/device_registry/inspector.h"
#include "src/media/audio/services/device_registry/logging.h"
#include "src/media/audio/services/device_registry/validate.h"

namespace media_audio {

namespace fad = fuchsia_audio_device;

// static
std::shared_ptr<ProviderServer> ProviderServer::Create(
    std::shared_ptr<const FidlThread> thread, fidl::ServerEnd<fad::Provider> server_end,
    std::shared_ptr<AudioDeviceRegistry> parent) {
  ADR_LOG_STATIC(kLogProviderServerMethods);

  return BaseFidlServer::Create(std::move(thread), std::move(server_end), std::move(parent));
}

ProviderServer::ProviderServer(std::shared_ptr<AudioDeviceRegistry> parent)
    : parent_(std::move(parent)) {
  ADR_LOG_METHOD(kLogObjectLifetimes);
  SetInspect(Inspector::Singleton()->RecordProviderInspectInstance(zx::clock::get_monotonic()));

  ++count_;
  LogObjectCounts();
}

ProviderServer::~ProviderServer() {
  ADR_LOG_METHOD(kLogObjectLifetimes);
  inspect()->RecordDestructionTime(zx::clock::get_monotonic());

  --count_;
  LogObjectCounts();
}

void ProviderServer::AddDevice(AddDeviceRequest& request, AddDeviceCompleter::Sync& completer) {
  ADR_LOG_METHOD(kLogProviderServerMethods);

  if (!request.device_name() || request.device_name()->empty()) {
    ADR_WARN_METHOD() << "device_name was absent/empty";
    completer.Reply(fit::error(fad::ProviderAddDeviceError::kInvalidName));
    return;
  }

  if (!request.device_type()) {
    ADR_WARN_METHOD() << "device_type was absent";
    completer.Reply(fit::error(fad::ProviderAddDeviceError::kInvalidType));
    return;
  }

  if (!request.driver_client()) {
    ADR_WARN_METHOD() << "driver_client was absent";
    completer.Reply(fit::error(fad::ProviderAddDeviceError::kInvalidDriverClient));
    return;
  }

  if (*request.device_type() == fad::DeviceType::Unknown()) {
    ADR_WARN_METHOD() << "unknown device_type";
    completer.Reply(fit::error(fad::ProviderAddDeviceError::kInvalidType));
    return;
  }

  if (!ClientIsValidForDeviceType(*request.device_type(), *request.driver_client())) {
    ADR_WARN_METHOD() << "driver_client did not match the specified device_type";
    completer.Reply(fit::error(fad::ProviderAddDeviceError::kWrongClientType));
    return;
  }

  if (*request.device_type() != fad::DeviceType::kCodec &&
      *request.device_type() != fad::DeviceType::kComposite) {
    ADR_WARN_METHOD() << "AudioDeviceRegistry does not support this client type";
    completer.Reply(fit::error(fad::ProviderAddDeviceError::kWrongClientType));
    return;
  }

  ADR_LOG_METHOD(kLogDeviceDetection)
      << "request to add " << request.device_type() << " '" << *request.device_name() << "'";

  // This kicks off device initialization, which notifies the parent when it completes.
  parent_->AddDevice(Device::Create(parent_, thread().dispatcher(), *request.device_name(),
                                    *request.device_type(), std::move(*request.driver_client()),
                                    "Provider"));

  inspect()->RecordAddedDevice(*request.device_name(), *request.device_type(),
                               zx::clock::get_monotonic());

  completer.Reply(fit::success(fad::ProviderAddDeviceResponse{}));
}

// We complain but don't close the connection, to accommodate older and newer clients.
void ProviderServer::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_audio_device::Provider> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {
  ADR_WARN_METHOD() << "unknown method (Provider) ordinal " << metadata.method_ordinal;
  if (metadata.unknown_method_type == fidl::UnknownMethodType::kTwoWay) {
    // Pend the completer indefinitely.
    unknown_method_completers_.emplace_back(completer.ToAsync());
  }
}

}  // namespace media_audio
