// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/screenshot/screenshot_manager.h"

#include <lib/component/incoming/cpp/protocol.h>
#include <lib/syslog/cpp/macros.h>

#include <memory>

#include "src/ui/scenic/lib/screen_capture/screen_capture.h"

namespace screenshot {

ScreenshotManager::ScreenshotManager(
    sys::ComponentContext* app_context, std::shared_ptr<allocation::Allocator> allocator,
    std::shared_ptr<flatland::Renderer> renderer, GetRenderables get_renderables,
    std::vector<std::shared_ptr<allocation::BufferCollectionImporter>> buffer_collection_importers,
    fuchsia_math::SizeU display_size, int display_rotation)
    : app_context_(app_context),
      allocator_(std::move(allocator)),
      renderer_(std::move(renderer)),
      get_renderables_(std::move(get_renderables)),
      buffer_collection_importers_(std::move(buffer_collection_importers)),
      display_size_(display_size),
      display_rotation_(display_rotation) {
  FX_DCHECK(renderer_);
}

void ScreenshotManager::CreateBinding(fidl::ServerEnd<fuchsia_ui_composition::Screenshot> request) {
  std::unique_ptr<ScreenCapture> screen_capture = std::make_unique<ScreenCapture>(
      buffer_collection_importers_, renderer_, [this]() { return get_renderables_(); });

  async_dispatcher_t* dispatcher = async_get_default_dispatcher();
  auto impl = std::make_unique<screenshot::FlatlandScreenshot>(
      app_context_, dispatcher, std::move(screen_capture), allocator_, display_size_,
      display_rotation_, [this](screenshot::FlatlandScreenshot* sc) {
        bindings_.CloseBindings(sc, ZX_ERR_SHOULD_WAIT);
      });
  screenshot::FlatlandScreenshot* impl_ptr = impl.get();
  auto close_handler = [impl = std::move(impl)](fidl::UnbindInfo info) {
    // Let |impl| fall out of scope.
  };
  bindings_.AddBinding(dispatcher, std::move(request), impl_ptr, std::move(close_handler));
}

}  // namespace screenshot
