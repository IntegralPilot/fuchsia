// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_DISPLAY_VSYNC_SOURCE_MANAGER_H_
#define SRC_UI_SCENIC_LIB_DISPLAY_VSYNC_SOURCE_MANAGER_H_

#include <fidl/fuchsia.ui.display.singleton/cpp/fidl.h>
#include <lib/async/dispatcher.h>

#include <memory>
#include <unordered_map>

#include "src/lib/fxl/memory/weak_ptr.h"
#include "src/ui/scenic/lib/display/vsync_source.h"

namespace display {

namespace test {
class VsyncSourceTest;
}  // namespace test

class DisplayManager;

// Manages VsyncSource connections.
//
// All calls happen on `dispatcher_` thread where this class is constructed.
class VsyncSourceManager {
 public:
  explicit VsyncSourceManager(DisplayManager& display_manager);

  // Creates a new binding for the VsyncSource service.
  void CreateBinding(fidl::ServerEnd<fuchsia_ui_display_singleton::VsyncSource> server_end);

 private:
  friend class display::test::VsyncSourceTest;

  void RemoveVsyncSource(uint32_t id);

  async_dispatcher_t* const dispatcher_;
  DisplayManager& display_manager_;

  uint32_t listener_id_ = 0;
  std::unordered_map<uint32_t, std::unique_ptr<VsyncSource>> vsync_listeners_;

  fxl::WeakPtrFactory<VsyncSourceManager> weak_factory_;
};

}  // namespace display

#endif  // SRC_UI_SCENIC_LIB_DISPLAY_VSYNC_SOURCE_MANAGER_H_
