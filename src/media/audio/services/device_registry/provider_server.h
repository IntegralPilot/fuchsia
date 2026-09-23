// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MEDIA_AUDIO_SERVICES_DEVICE_REGISTRY_PROVIDER_SERVER_H_
#define SRC_MEDIA_AUDIO_SERVICES_DEVICE_REGISTRY_PROVIDER_SERVER_H_

#include <fidl/fuchsia.audio.device/cpp/markers.h>
#include <fidl/fuchsia.audio.device/cpp/type_conversions.h>
#include <lib/fidl/cpp/wire/unknown_interaction_handler.h>

#include <cstdint>
#include <memory>

#include "src/media/audio/services/common/base_fidl_server.h"
#include "src/media/audio/services/device_registry/inspector.h"

namespace media_audio {

class AudioDeviceRegistry;

// FIDL server for fuchsia_audio_device/Provider. This interface programmatically adds audio devices
// (instead of their being automatically detected in `devfs`).
class ProviderServer
    : public std::enable_shared_from_this<ProviderServer>,
      public BaseFidlServer<ProviderServer, fidl::Server, fuchsia_audio_device::Provider> {
 public:
  static std::shared_ptr<ProviderServer> Create(
      std::shared_ptr<const FidlThread> thread,
      fidl::ServerEnd<fuchsia_audio_device::Provider> server_end,
      std::shared_ptr<AudioDeviceRegistry> parent);

  ProviderServer(const ProviderServer&) = delete;
  ProviderServer(ProviderServer&&) = delete;
  ProviderServer& operator=(const ProviderServer&) = delete;
  ProviderServer& operator=(ProviderServer&&) = delete;
  ~ProviderServer() override;

  // fuchsia.audio.device.Provider implementation
  void AddDevice(AddDeviceRequest& request, AddDeviceCompleter::Sync& completer) override;
  void handle_unknown_method(fidl::UnknownMethodMetadata<fuchsia_audio_device::Provider> metadata,
                             fidl::UnknownMethodCompleter::Sync& completer) override;

  const std::shared_ptr<ProviderInspectInstance>& inspect() { return provider_inspect_instance_; }
  void SetInspect(std::shared_ptr<ProviderInspectInstance> instance) {
    provider_inspect_instance_ = std::move(instance);
  }

  // Static object count, for debugging purposes.
  static uint64_t count() { return count_; }

 private:
  template <typename ServerT, template <typename T> typename FidlServerT, typename ProtocolT>
  friend class BaseFidlServer;

  static constexpr std::string_view kClassName = "ProviderServer";
  static inline uint64_t count_ = 0;

  explicit ProviderServer(std::shared_ptr<AudioDeviceRegistry> parent);

  std::shared_ptr<AudioDeviceRegistry> parent_;

  std::vector<fidl::UnknownMethodCompleter::Async> unknown_method_completers_;

  std::shared_ptr<ProviderInspectInstance> provider_inspect_instance_;
};

}  // namespace media_audio

#endif  // SRC_MEDIA_AUDIO_SERVICES_DEVICE_REGISTRY_PROVIDER_SERVER_H_
