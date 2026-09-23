// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MEDIA_AUDIO_SERVICES_DEVICE_REGISTRY_TESTING_STUB_ADR_STUB_CONTROL_CREATOR_SERVER_H_
#define SRC_MEDIA_AUDIO_SERVICES_DEVICE_REGISTRY_TESTING_STUB_ADR_STUB_CONTROL_CREATOR_SERVER_H_

#include <fidl/fuchsia.audio.device/cpp/fidl.h>
#include <lib/fidl/cpp/wire/unknown_interaction_handler.h>
#include <lib/syslog/cpp/macros.h>

#include <memory>
#include <string_view>

#include "src/media/audio/services/common/base_fidl_server.h"
#include "src/media/audio/services/common/fidl_thread.h"
#include "src/media/audio/services/device_registry/logging.h"

namespace media_audio {

// FIDL server for fuchsia_audio_device/ControlCreator (a stub "do-nothing" implementation).
class StubControlCreatorServer : public BaseFidlServer<StubControlCreatorServer, fidl::Server,
                                                       fuchsia_audio_device::ControlCreator> {
  static constexpr bool kLogStubControlCreatorServer = true;

 public:
  static std::shared_ptr<StubControlCreatorServer> Create(
      std::shared_ptr<const FidlThread> thread,
      fidl::ServerEnd<fuchsia_audio_device::ControlCreator> server_end) {
    ADR_LOG_STATIC(kLogStubControlCreatorServer);
    return BaseFidlServer::Create(std::move(thread), std::move(server_end));
  }

  // fuchsia.audio.device.ControlCreator implementation
  void Create(CreateRequest& request, CreateCompleter::Sync& completer) override {
    ADR_LOG_STATIC(kLogStubControlCreatorServer);
    completer.Reply(fit::success(fuchsia_audio_device::ControlCreatorCreateResponse{}));
  }

  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_audio_device::ControlCreator> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override {
    ADR_WARN_METHOD() << "unknown method (ControlCreator) ordinal " << metadata.method_ordinal;
    completer.Close(ZX_ERR_NOT_SUPPORTED);
  }

 private:
  template <typename ServerT, template <typename T> typename FidlServerT, typename ProtocolT>
  friend class BaseFidlServer;

  static constexpr std::string_view kClassName = "StubControlCreatorServer";

  StubControlCreatorServer() = default;
};

}  // namespace media_audio

#endif  // SRC_MEDIA_AUDIO_SERVICES_DEVICE_REGISTRY_TESTING_STUB_ADR_STUB_CONTROL_CREATOR_SERVER_H_
