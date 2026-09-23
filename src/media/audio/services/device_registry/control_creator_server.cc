// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/media/audio/services/device_registry/control_creator_server.h"

#include <lib/fidl/cpp/wire/unknown_interaction_handler.h>
#include <lib/fit/internal/result.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/clock.h>

#include "src/media/audio/services/device_registry/audio_device_registry.h"
#include "src/media/audio/services/device_registry/inspector.h"
#include "src/media/audio/services/device_registry/logging.h"

namespace media_audio {

namespace fad = fuchsia_audio_device;

// static
std::shared_ptr<ControlCreatorServer> ControlCreatorServer::Create(
    std::shared_ptr<const FidlThread> thread, fidl::ServerEnd<fad::ControlCreator> server_end,
    const std::shared_ptr<AudioDeviceRegistry>& parent) {
  ADR_LOG_STATIC(kLogControlCreatorServerMethods) << " parent " << parent;

  return BaseFidlServer::Create(std::move(thread), std::move(server_end), parent);
}

ControlCreatorServer::ControlCreatorServer(std::shared_ptr<AudioDeviceRegistry> parent)
    : parent_(std::move(parent)) {
  ADR_LOG_METHOD(kLogObjectLifetimes);
  SetInspect(Inspector::Singleton()->RecordControlCreatorInstance(zx::clock::get_monotonic()));

  ++count_;
  LogObjectCounts();
}

ControlCreatorServer::~ControlCreatorServer() {
  ADR_LOG_METHOD(kLogObjectLifetimes);
  inspect()->RecordDestructionTime(zx::clock::get_monotonic());

  --count_;
  LogObjectCounts();
}

void ControlCreatorServer::Create(CreateRequest& request, CreateCompleter::Sync& completer) {
  ADR_LOG_METHOD(kLogControlCreatorServerMethods);

  if (!request.token_id()) {
    ADR_WARN_METHOD() << "required 'token_id' is absent";
    completer.Reply(fit::error(fad::ControlCreatorError::kInvalidTokenId));
    return;
  }

  if (!request.control_server()) {
    ADR_WARN_METHOD() << "required 'control_server' is absent";
    completer.Reply(fit::error(fad::ControlCreatorError::kInvalidControl));
    return;
  }

  auto [status, device] = parent_->FindDeviceByTokenId(*request.token_id());
  if (status == AudioDeviceRegistry::DevicePresence::Unknown) {
    completer.Reply(fit::error(fad::ControlCreatorError::kDeviceNotFound));
    return;
  }
  if (status == AudioDeviceRegistry::DevicePresence::Error) {
    completer.Reply(fit::error(fad::ControlCreatorError::kDeviceError));
    return;
  }

  FX_CHECK(device);
  // TODO(https://fxbug.dev/42068381): Decide when we proactively call GetHealthState, if at all.

  auto control = parent_->CreateControlServer(std::move(*request.control_server()), device);

  if (!control) {
    completer.Reply(fit::error(fad::ControlCreatorError::kAlreadyAllocated));
    return;
  }

  completer.Reply(fit::success(fad::ControlCreatorCreateResponse{}));
}

// We complain but don't close the connection, to accommodate older and newer clients.
void ControlCreatorServer::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_audio_device::ControlCreator> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {
  ADR_WARN_METHOD() << "unknown method (ControlCreator) ordinal " << metadata.method_ordinal;
  if (metadata.unknown_method_type == fidl::UnknownMethodType::kTwoWay) {
    // Pend the completer indefinitely.
    unknown_method_completers_.emplace_back(completer.ToAsync());
  }
}

}  // namespace media_audio
