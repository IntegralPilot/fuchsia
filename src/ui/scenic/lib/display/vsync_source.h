// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_DISPLAY_VSYNC_SOURCE_H_
#define SRC_UI_SCENIC_LIB_DISPLAY_VSYNC_SOURCE_H_

#include <fidl/fuchsia.ui.display.singleton/cpp/fidl.h>
#include <lib/async/dispatcher.h>
#include <lib/fit/function.h>
#include <lib/zx/time.h>

#include <functional>

#include "src/ui/scenic/lib/display/display_manager.h"

namespace display {

// A FIDL server that registers a vsync callback with the default/singleton `Display`,
// and notifies the client with each received Vsync (or not, if the client hasn't enabled
// notifications).
class VsyncSource : public fidl::Server<fuchsia_ui_display_singleton::VsyncSource> {
 public:
  VsyncSource(async_dispatcher_t* dispatcher, DisplayManager& display_manager,
              fidl::ServerEnd<fuchsia_ui_display_singleton::VsyncSource> server_end,
              std::function<void(fidl::UnbindInfo unbind_info)> close_handler);
  ~VsyncSource() override;

 private:
  // |fuchsia_ui_display_singleton::VsyncSource|
  void SetVsyncEnabled(SetVsyncEnabledRequest& request,
                       SetVsyncEnabledCompleter::Sync& completer) override;

  // Registered as a callback with the default display.
  void OnVsync(zx::time_monotonic timestamp, display::WireConfigStamp displayed_config_stamp);

  void UpdateVsyncCallbackRegistration(bool enabled);
  bool vsync_enabled() const;

  fidl::ServerBinding<fuchsia_ui_display_singleton::VsyncSource> binding_;
  DisplayManager& display_manager_;
  std::optional<Display::VsyncCallbackId> callback_id_;
};

}  // namespace display

#endif  // SRC_UI_SCENIC_LIB_DISPLAY_VSYNC_SOURCE_H_
