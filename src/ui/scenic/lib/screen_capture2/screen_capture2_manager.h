// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_SCREEN_CAPTURE2_SCREEN_CAPTURE2_MANAGER_H_
#define SRC_UI_SCENIC_LIB_SCREEN_CAPTURE2_SCREEN_CAPTURE2_MANAGER_H_

#include <fidl/fuchsia.ui.composition.internal/cpp/fidl.h>

#include <memory>
#include <unordered_map>

#include "screen_capture2.h"
#include "src/ui/scenic/lib/flatland/engine/engine.h"
#include "src/ui/scenic/lib/flatland/renderer/renderer.h"
#include "src/ui/scenic/lib/scheduling/frame_scheduler.h"
#include "src/ui/scenic/lib/screen_capture/screen_capture_buffer_collection_importer.h"

namespace screen_capture2 {

class ScreenCapture2Manager {
 public:
  ScreenCapture2Manager(std::shared_ptr<flatland::Renderer> renderer,
                        std::shared_ptr<screen_capture::ScreenCaptureBufferCollectionImporter>
                            screen_capture_buffer_collection_importer,
                        std::function<flatland::Renderables()> get_renderables_callback);
  ~ScreenCapture2Manager();

  void CreateClient(fidl::ServerEnd<fuchsia_ui_composition_internal::ScreenCapture> screen_capture);

  // Called at FrameScheduler OnCpuWorkDone time.
  void RenderPendingScreenCaptures();

  size_t client_count() const { return clients_.size(); }

 private:
  std::shared_ptr<flatland::Renderer> renderer_;
  std::shared_ptr<screen_capture::ScreenCaptureBufferCollectionImporter>
      screen_capture_buffer_collection_importer_;

  std::unordered_map<ScreenCapture*, std::unique_ptr<ScreenCapture>> clients_;
  fidl::ServerBindingGroup<fuchsia_ui_composition_internal::ScreenCapture> client_bindings_;

  // Callback provided to pass to clients.
  std::function<flatland::Renderables()> get_renderables_callback_;
};

}  // namespace screen_capture2
#endif  // SRC_UI_SCENIC_LIB_SCREEN_CAPTURE2_SCREEN_CAPTURE2_MANAGER_H_
