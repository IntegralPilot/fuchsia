// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "screen_capture2_manager.h"

#include <lib/async/default.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/trace/event.h>

#include <memory>

#include "src/lib/files/file.h"
#include "src/lib/fsl/handles/object_info.h"
#include "src/lib/fxl/memory/weak_ptr.h"
#include "src/ui/scenic/lib/flatland/engine/engine.h"
#include "src/ui/scenic/lib/flatland/renderer/renderer.h"
#include "src/ui/scenic/lib/screen_capture2/screen_capture2.h"

namespace screen_capture2 {

ScreenCapture2Manager::ScreenCapture2Manager(
    std::shared_ptr<flatland::Renderer> renderer,
    std::shared_ptr<screen_capture::ScreenCaptureBufferCollectionImporter>
        screen_capture_buffer_collection_importer,
    std::function<flatland::Renderables()> get_renderables_callback)
    : renderer_(renderer),
      screen_capture_buffer_collection_importer_(screen_capture_buffer_collection_importer),
      get_renderables_callback_(get_renderables_callback) {
  FX_DCHECK(renderer_);
  FX_DCHECK(screen_capture_buffer_collection_importer_);
  FX_DCHECK(get_renderables_callback_);
}

ScreenCapture2Manager::~ScreenCapture2Manager() = default;

void ScreenCapture2Manager::CreateClient(
    fidl::ServerEnd<fuchsia_ui_composition_internal::ScreenCapture> request) {
  auto instance = std::make_unique<ScreenCapture>(
      screen_capture_buffer_collection_importer_, renderer_,
      /*get_renderables=*/[this]() { return get_renderables_callback_(); });
  ScreenCapture* ptr = instance.get();
  clients_[ptr] = std::move(instance);
  client_bindings_.AddBinding(async_get_default_dispatcher(), std::move(request), ptr,
                              [this, ptr](fidl::UnbindInfo info) { clients_.erase(ptr); });
}

void ScreenCapture2Manager::RenderPendingScreenCaptures() {
  TRACE_DURATION("gfx", "ScreenCapture2Manager::RenderPendingScreenCaptures");
  // After the newest batch of renderables has been produced, loop through all of the bindings and
  // render into the client's buffer if they have requested one.
  for (const auto& [ptr, _] : clients_) {
    ptr->MaybeRenderFrame();
  }
}

}  // namespace screen_capture2
