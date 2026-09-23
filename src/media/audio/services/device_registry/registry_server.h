// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MEDIA_AUDIO_SERVICES_DEVICE_REGISTRY_REGISTRY_SERVER_H_
#define SRC_MEDIA_AUDIO_SERVICES_DEVICE_REGISTRY_REGISTRY_SERVER_H_

#include <fidl/fuchsia.audio.device/cpp/fidl.h>
#include <lib/fidl/cpp/wire/unknown_interaction_handler.h>

#include <cstdint>
#include <memory>
#include <optional>
#include <queue>
#include <vector>

#include "src/media/audio/services/common/base_fidl_server.h"
#include "src/media/audio/services/device_registry/device.h"
#include "src/media/audio/services/device_registry/inspector.h"

namespace media_audio {

class AudioDeviceRegistry;

// FIDL server for fuchsia_audio_device/Registry. This interface watches as devices arrive/depart,
// and exposes summary information about devices that are present (most notably, the device's
// TokenId which can be used to create an associated observer or Control).
class RegistryServer
    : public std::enable_shared_from_this<RegistryServer>,
      public BaseFidlServer<RegistryServer, fidl::Server, fuchsia_audio_device::Registry> {
 public:
  static std::shared_ptr<RegistryServer> Create(
      std::shared_ptr<const FidlThread> thread,
      fidl::ServerEnd<fuchsia_audio_device::Registry> server_end,
      std::shared_ptr<AudioDeviceRegistry> parent);

  RegistryServer(const RegistryServer&) = delete;
  RegistryServer(RegistryServer&&) = delete;
  RegistryServer& operator=(const RegistryServer&) = delete;
  RegistryServer& operator=(RegistryServer&&) = delete;
  ~RegistryServer() override;

  // fuchsia.audio.device.Registry implementation
  void WatchDevicesAdded(WatchDevicesAddedCompleter::Sync& completer) final;
  void WatchDeviceRemoved(WatchDeviceRemovedCompleter::Sync& completer) final;
  void CreateObserver(CreateObserverRequest& request,
                      CreateObserverCompleter::Sync& completer) final;
  void handle_unknown_method(fidl::UnknownMethodMetadata<fuchsia_audio_device::Registry> metadata,
                             fidl::UnknownMethodCompleter::Sync& completer) override;

  void InitialDeviceDiscoveryIsComplete();
  void DeviceWasAdded(const std::shared_ptr<const Device>& new_device);
  void DeviceWasRemoved(TokenId removed_id);

  const std::shared_ptr<FidlServerInspectInstance>& inspect() { return registry_inspect_instance_; }
  void SetInspect(std::shared_ptr<FidlServerInspectInstance> instance) {
    registry_inspect_instance_ = std::move(instance);
  }

  // Static object count, for debugging purposes.
  static uint64_t count() { return count_; }

 private:
  template <typename ServerT, template <typename T> typename FidlServerT, typename ProtocolT>
  friend class BaseFidlServer;

  static constexpr std::string_view kClassName = "RegistryServer";
  static inline uint64_t count_ = 0;

  explicit RegistryServer(std::shared_ptr<AudioDeviceRegistry> parent);
  void MaybeReplyWatchDevicesAdded();
  void MaybeReplyWatchDeviceRemoved();

  std::shared_ptr<AudioDeviceRegistry> parent_;
  bool initial_device_discovery_complete_ = false;
  bool responded_to_initial_watch_devices_added_ = false;

  std::vector<fuchsia_audio_device::Info> devices_added_since_notify_;
  std::optional<WatchDevicesAddedCompleter::Async> watch_devices_added_completer_;

  std::queue<TokenId> devices_removed_since_notify_;
  std::optional<WatchDeviceRemovedCompleter::Async> watch_device_removed_completer_;

  std::vector<fidl::UnknownMethodCompleter::Async> unknown_method_completers_;

  std::shared_ptr<FidlServerInspectInstance> registry_inspect_instance_;
};

}  // namespace media_audio

#endif  // SRC_MEDIA_AUDIO_SERVICES_DEVICE_REGISTRY_REGISTRY_SERVER_H_
