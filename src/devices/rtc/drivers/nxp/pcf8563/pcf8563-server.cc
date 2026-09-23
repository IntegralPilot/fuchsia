// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "pcf8563-server.h"

#include <fidl/fuchsia.hardware.rtc/cpp/fidl.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/fdf/cpp/dispatcher.h>
#include <lib/fit/result.h>
#include <lib/zx/result.h>

#include "pcf8563.h"

namespace pcf8563 {

namespace frtc = fuchsia_hardware_rtc;

frtc::Service::InstanceHandler RtcServer::GetInstanceHandler() {
  fdf::UnownedDispatcher dispatcher = fdf::Dispatcher::GetCurrent();
  return frtc::Service::InstanceHandler({
      .device = bindings_.CreateHandler(this, dispatcher->async_dispatcher(),
                                        fidl::kIgnoreBindingClosure),
  });
}

void RtcServer::Get(GetCompleter::Sync& completer) {
  if (zx::result result{device_->Read()}; result.is_error()) {
    completer.Reply(fit::error(result.status_value()));
  } else {
    if (device_->IsInvalid(result.value())) {
      completer.Reply(fit::error(ZX_ERR_OUT_OF_RANGE));
    } else {
      completer.Reply(fit::ok(result.value()));
    }
  }
}

void RtcServer::Set2(Set2Request& req, Set2Completer::Sync& completer) {
  if (device_->IsInvalid(req.rtc())) {
    completer.Reply(fit::error(ZX_ERR_OUT_OF_RANGE));
    return;
  }

  zx::result result{device_->Write(req.rtc())};
  completer.Reply(result);
}

void RtcServer::OnUnbound(fidl::UnbindInfo info, fidl::ServerEnd<frtc::Device> server_end) {
  if (info.is_peer_closed()) {
    FDF_LOG(DEBUG, "client disconnected");
  } else if (!info.is_user_initiated()) {
    FDF_LOG(ERROR, "client unbound: %s", info.status_string());
  }
}

}  // namespace pcf8563
