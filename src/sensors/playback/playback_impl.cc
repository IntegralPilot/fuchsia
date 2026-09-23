// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/sensors/playback/playback_impl.h"

namespace sensors::playback {
namespace {
using fpromise::promise;
using fpromise::result;

using fuchsia_hardware_sensors::ConfigurePlaybackError;
using fuchsia_hardware_sensors::FixedValuesPlaybackConfig;
using fuchsia_hardware_sensors::Playback;
using fuchsia_hardware_sensors::PlaybackSourceConfig;
}  // namespace

bool PlaybackImpl::client_connected_ = false;

PlaybackImpl::PlaybackImpl(async_dispatcher_t* dispatcher, PlaybackController& controller)
    : ActorBase(dispatcher, scope_), controller_(controller) {}

void PlaybackImpl::ConfigurePlayback(ConfigurePlaybackRequest& request,
                                     ConfigurePlaybackCompleter::Sync& completer) {
  ZX_ASSERT(binding_ref_.has_value());

  Schedule(controller_.ConfigurePlayback(request.source_config())
               .then([completer = completer.ToAsync()](
                         result<void, ConfigurePlaybackError>& result) mutable -> promise<void> {
                 if (result.is_ok()) {
                   completer.Reply(fit::success());
                 } else {
                   completer.Reply(fit::error(result.error()));
                 }
                 return fpromise::make_ok_promise();
               }));
}

void PlaybackImpl::handle_unknown_method(fidl::UnknownMethodMetadata<Playback> metadata,
                                         fidl::UnknownMethodCompleter::Sync& completer) {
  ZX_ASSERT(binding_ref_.has_value());
  completer.Close(ZX_ERR_NOT_SUPPORTED);
}

void PlaybackImpl::OnUnbound(fidl::UnbindInfo info, fidl::ServerEnd<Playback> server_end,
                             fit::callback<void()> unbind_callback) {
  // |is_user_initiated| returns true if the server code called |Close| on a
  // completer, or |Unbind| / |Close| on the |binding_ref_|, to proactively
  // teardown the connection. These cases are usually part of normal server
  // shutdown, so logging is unnecessary.
  if (info.is_user_initiated()) {
    unbind_callback();
    return;
  }
  if (info.is_peer_closed()) {
    // If the peer (the client) closed their endpoint, log that as INFO.
    FX_LOGS(INFO) << "Client disconnected";
  } else {
    // Treat other unbind causes as errors.
    FX_LOGS(ERROR) << "Server error: " << info;
  }
  controller_.PlaybackClientDisconnected(std::move(unbind_callback));
}

}  // namespace sensors::playback
