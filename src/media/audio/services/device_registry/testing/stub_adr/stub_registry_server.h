// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MEDIA_AUDIO_SERVICES_DEVICE_REGISTRY_TESTING_STUB_ADR_STUB_REGISTRY_SERVER_H_
#define SRC_MEDIA_AUDIO_SERVICES_DEVICE_REGISTRY_TESTING_STUB_ADR_STUB_REGISTRY_SERVER_H_

#include <fidl/fuchsia.audio.device/cpp/fidl.h>
#include <fidl/fuchsia.audio.device/cpp/natural_types.h>
#include <lib/fidl/cpp/wire/internal/transport_channel.h>
#include <lib/fidl/cpp/wire/unknown_interaction_handler.h>
#include <lib/syslog/cpp/macros.h>

#include <memory>
#include <string_view>

#include "src/media/audio/services/common/base_fidl_server.h"
#include "src/media/audio/services/common/fidl_thread.h"
#include "src/media/audio/services/device_registry/logging.h"

namespace media_audio {

// FIDL server for fuchsia_audio_device/Registry (a stub "do-nothing" implementation).
class StubRegistryServer
    : public BaseFidlServer<StubRegistryServer, fidl::Server, fuchsia_audio_device::Registry> {
  static constexpr bool kLogStubRegistryServer = true;

 public:
  static std::shared_ptr<StubRegistryServer> Create(
      std::shared_ptr<const FidlThread> thread,
      fidl::ServerEnd<fuchsia_audio_device::Registry> server_end) {
    ADR_LOG_STATIC(kLogStubRegistryServer);
    return BaseFidlServer::Create(std::move(thread), std::move(server_end));
  }

  // fuchsia.audio.device.Registry implementation
  void WatchDevicesAdded(WatchDevicesAddedCompleter::Sync& completer) final {
    // Here we assume the parent has already sent us the InitialDeviceDiscoveryIsComplete().
    if (!responded_to_initial_watch_devices_added_) {
      ADR_LOG_STATIC(kLogStubRegistryServer) << " initial call; returning empty list";
      completer.Reply(fit::success(fuchsia_audio_device::RegistryWatchDevicesAddedResponse{{
          .devices = std::vector<fuchsia_audio_device::Info>{},
      }}));
      responded_to_initial_watch_devices_added_ = true;
      return;
    }

    if (watch_devices_added_completer_.has_value()) {
      ADR_WARN_METHOD() << "previous request has not yet completed";
      completer.Reply(fit::error<fuchsia_audio_device::RegistryWatchDevicesAddedError>(
          fuchsia_audio_device::RegistryWatchDevicesAddedError::kAlreadyPending));
      return;
    }

    ADR_LOG_STATIC(kLogStubRegistryServer) << " pending indefinitely";
    watch_devices_added_completer_ = completer.ToAsync();
  }

  void WatchDeviceRemoved(WatchDeviceRemovedCompleter::Sync& completer) final {
    if (watch_device_removed_completer_.has_value()) {
      ADR_WARN_METHOD() << "previous request has not yet completed";
      completer.Reply(fit::error<fuchsia_audio_device::RegistryWatchDeviceRemovedError>(
          fuchsia_audio_device::RegistryWatchDeviceRemovedError::kAlreadyPending));
      return;
    }
    ADR_LOG_STATIC(kLogStubRegistryServer);
    watch_device_removed_completer_ = completer.ToAsync();
  }

  void CreateObserver(CreateObserverRequest& request,
                      CreateObserverCompleter::Sync& completer) final {
    ADR_LOG_STATIC(kLogStubRegistryServer);
    completer.Reply(fit::success(fuchsia_audio_device::RegistryCreateObserverResponse{}));
  }

  void handle_unknown_method(fidl::UnknownMethodMetadata<fuchsia_audio_device::Registry> metadata,
                             fidl::UnknownMethodCompleter::Sync& completer) override {
    ADR_WARN_METHOD() << "unknown method (Registry) ordinal " << metadata.method_ordinal;
    completer.Close(ZX_ERR_NOT_SUPPORTED);
  }

 private:
  template <typename ServerT, template <typename T> typename FidlServerT, typename ProtocolT>
  friend class BaseFidlServer;

  static constexpr std::string_view kClassName = "StubRegistryServer";

  StubRegistryServer() = default;

  bool responded_to_initial_watch_devices_added_ = false;
  std::optional<WatchDevicesAddedCompleter::Async> watch_devices_added_completer_;
  std::optional<WatchDeviceRemovedCompleter::Async> watch_device_removed_completer_;
};

}  // namespace media_audio

#endif  // SRC_MEDIA_AUDIO_SERVICES_DEVICE_REGISTRY_TESTING_STUB_ADR_STUB_REGISTRY_SERVER_H_
