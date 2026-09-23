// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/screen_capture/screen_capture_manager.h"

#include <lib/syslog/cpp/macros.h>

#include "screen_capture.h"
#include "src/ui/scenic/lib/flatland/engine/engine.h"
#include "src/ui/scenic/lib/flatland/renderer/renderer.h"

namespace screen_capture {
ScreenCaptureManager::ScreenCaptureManager(
    std::shared_ptr<flatland::Engine> engine, std::shared_ptr<flatland::Renderer> renderer,
    std::shared_ptr<flatland::FlatlandManager> flatland_manager,
    std::vector<std::shared_ptr<allocation::BufferCollectionImporter>> buffer_collection_importers)
    : engine_(engine),
      renderer_(renderer),
      flatland_manager_(flatland_manager),
      buffer_collection_importers_(std::move(buffer_collection_importers)) {
  FX_DCHECK(engine_);
  FX_DCHECK(renderer_);
  FX_DCHECK(flatland_manager_);
}

void ScreenCaptureManager::CreateClient(
    fidl::ServerEnd<fuchsia_ui_composition::ScreenCapture> request) {
  auto impl = std::make_unique<screen_capture::ScreenCapture>(
      buffer_collection_importers_, renderer_, [this]() {
        FX_DCHECK(flatland_manager_);
        FX_DCHECK(engine_);

        auto display = flatland_manager_->GetPrimaryFlatlandDisplayForRendering();
        if (!display) {
          FX_LOGS(WARNING) << "No FlatlandDisplay attached at root. Returning an empty screenshot.";
          return flatland::Renderables();
        }

        return engine_->GetRenderables(*display);
      });
  screen_capture::ScreenCapture* impl_ptr = impl.get();
  auto close_handler = [impl = std::move(impl)](fidl::UnbindInfo info) {
    // Let |impl| fall out of scope.
  };
  bindings_.AddBinding(async_get_default_dispatcher(), std::move(request), impl_ptr,
                       std::move(close_handler));
}

}  // namespace screen_capture
