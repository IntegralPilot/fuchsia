// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/display/vsync_source_manager.h"

#include <lib/async/default.h>

#include "src/ui/scenic/lib/display/display_manager.h"
#include "src/ui/scenic/lib/display/vsync_source.h"

namespace display {

VsyncSourceManager::VsyncSourceManager(DisplayManager& display_manager)
    : dispatcher_(async_get_default_dispatcher()),
      display_manager_(display_manager),
      weak_factory_(this) {}

void VsyncSourceManager::CreateBinding(
    fidl::ServerEnd<fuchsia_ui_display_singleton::VsyncSource> server_end) {
  const auto id = ++listener_id_;
  auto vsync_listener =
      std::make_unique<VsyncSource>(dispatcher_, display_manager_, std::move(server_end),
                                    [weak_ptr = weak_factory_.GetWeakPtr(), id,
                                     dispatcher = dispatcher_](fidl::UnbindInfo unbind_info) {
                                      FX_DCHECK(dispatcher == async_get_default_dispatcher());
                                      if (!weak_ptr) {
                                        return;
                                      }
                                      weak_ptr->RemoveVsyncSource(id);
                                    });
  vsync_listeners_.emplace(id, std::move(vsync_listener));
}

void VsyncSourceManager::RemoveVsyncSource(uint32_t id) {
  const bool removed = vsync_listeners_.erase(id);
  FX_DCHECK(removed) << "no listener with id=" << id;
}

}  // namespace display
