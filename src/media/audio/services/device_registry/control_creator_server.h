// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MEDIA_AUDIO_SERVICES_DEVICE_REGISTRY_CONTROL_CREATOR_SERVER_H_
#define SRC_MEDIA_AUDIO_SERVICES_DEVICE_REGISTRY_CONTROL_CREATOR_SERVER_H_

#include <fidl/fuchsia.audio.device/cpp/fidl.h>
#include <lib/fidl/cpp/wire/unknown_interaction_handler.h>

#include <memory>

#include "src/media/audio/services/common/base_fidl_server.h"
#include "src/media/audio/services/device_registry/inspector.h"

namespace media_audio {

class AudioDeviceRegistry;

// FIDL server for fuchsia_audio_device/ControlCreator. Builds fuchsia_audio_device/ControlServers.
class ControlCreatorServer : public std::enable_shared_from_this<ControlCreatorServer>,
                             public BaseFidlServer<ControlCreatorServer, fidl::Server,
                                                   fuchsia_audio_device::ControlCreator> {
 public:
  static std::shared_ptr<ControlCreatorServer> Create(
      std::shared_ptr<const FidlThread> thread,
      fidl::ServerEnd<fuchsia_audio_device::ControlCreator> server_end,
      const std::shared_ptr<AudioDeviceRegistry>& parent);

  ~ControlCreatorServer() override;

  // fuchsia.audio.device.ControlCreator implementation
  void Create(CreateRequest& request, CreateCompleter::Sync& completer) override;
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_audio_device::ControlCreator> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override;

  const std::shared_ptr<FidlServerInspectInstance>& inspect() {
    return control_creator_inspect_instance_;
  }
  void SetInspect(std::shared_ptr<FidlServerInspectInstance> instance) {
    control_creator_inspect_instance_ = std::move(instance);
  }

  // Static object count, for debugging purposes.
  static uint64_t count() { return count_; }

 private:
  template <typename ServerT, template <typename T> typename FidlServerT, typename ProtocolT>
  friend class BaseFidlServer;

  static constexpr std::string_view kClassName = "ControlCreatorServer";
  static inline uint64_t count_ = 0;

  explicit ControlCreatorServer(std::shared_ptr<AudioDeviceRegistry> parent);

  std::shared_ptr<AudioDeviceRegistry> parent_;

  std::vector<fidl::UnknownMethodCompleter::Async> unknown_method_completers_;

  std::shared_ptr<FidlServerInspectInstance> control_creator_inspect_instance_;
};

}  // namespace media_audio

#endif  // SRC_MEDIA_AUDIO_SERVICES_DEVICE_REGISTRY_CONTROL_CREATOR_SERVER_H_
