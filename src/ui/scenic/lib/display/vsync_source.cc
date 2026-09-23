// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/display/vsync_source.h"

#include <lib/component/incoming/cpp/protocol.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/trace/event.h>
#include <lib/zx/channel.h>
#include <zircon/types.h>

namespace display {

VsyncSource::VsyncSource(async_dispatcher_t* dispatcher, DisplayManager& display_manager,
                         fidl::ServerEnd<fuchsia_ui_display_singleton::VsyncSource> server_end,
                         std::function<void(fidl::UnbindInfo unbind_info)> close_handler)
    : binding_(dispatcher, std::move(server_end), this, std::move(close_handler)),
      display_manager_(display_manager) {}

VsyncSource::~VsyncSource() { UpdateVsyncCallbackRegistration(false); }

void VsyncSource::SetVsyncEnabled(SetVsyncEnabledRequest& request,
                                  SetVsyncEnabledCompleter::Sync& completer) {
  UpdateVsyncCallbackRegistration(request.enabled());
}

void VsyncSource::OnVsync(zx::time_monotonic timestamp,
                          display::WireConfigStamp displayed_config_stamp) {
  TRACE_DURATION("gfx", "VsyncSource::OnVsync");
  if (!vsync_enabled())
    return;

  fuchsia_ui_display_singleton::VsyncSourceOnVsyncRequest values;
  values.timestamp(timestamp.get());
  auto result = fidl::SendEvent(binding_)->OnVsync(values);
  if (result.is_error()) {
    auto& error = result.error_value().error();
    FX_LOGS(WARNING) << "OnVsync(): error while sending FIDL event: " << error.status() << " "
                     << error.status_string();
  }
}

void VsyncSource::UpdateVsyncCallbackRegistration(bool enabled) {
  // No state change.
  if (vsync_enabled() == enabled) {
    return;
  }

  // This would be an issue for multiple display case. Revisit when that functionality is available.
  auto* display = display_manager_.default_display();
  if (!display) {
    FX_LOGS(ERROR) << "No default display found. No vsyncs will be received.";
    return;
  }

  if (enabled) {
    callback_id_ = display->AddVsyncCallback(fit::bind_member<&VsyncSource::OnVsync>(this));
  } else {
    display->RemoveVsyncCallback(callback_id_.value());
    callback_id_.reset();
  }
}

bool VsyncSource::vsync_enabled() const { return callback_id_.has_value(); }

}  // namespace display