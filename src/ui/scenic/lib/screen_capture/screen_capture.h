// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_SCREEN_CAPTURE_SCREEN_CAPTURE_H_
#define SRC_UI_SCENIC_LIB_SCREEN_CAPTURE_SCREEN_CAPTURE_H_

#include <fidl/fuchsia.ui.composition/cpp/fidl.h>
#include <fidl/fuchsia.ui.composition/cpp/wire.h>
#include <lib/async/cpp/executor.h>
#include <lib/fidl/cpp/wire/server.h>

#include <deque>
#include <unordered_map>

#include "src/ui/scenic/lib/allocation/buffer_collection_importer.h"
#include "src/ui/scenic/lib/flatland/engine/engine.h"
#include "src/ui/scenic/lib/flatland/flatland_types.h"
#include "src/ui/scenic/lib/flatland/renderer/renderer.h"

using glm::vec2;
using GetRenderables = std::function<flatland::Renderables()>;

namespace screen_capture {

class ScreenCapture : public fidl::WireServer<fuchsia_ui_composition::ScreenCapture> {
 public:
  static std::vector<flatland::ResolvedLayer> RotateRenderables(
      const std::vector<flatland::ResolvedLayer>& layers,
      fuchsia_ui_composition::wire::Rotation rotation, uint32_t image_width, uint32_t image_height);

  ScreenCapture(const std::vector<std::shared_ptr<allocation::BufferCollectionImporter>>&
                    buffer_collection_importers,
                std::shared_ptr<flatland::Renderer> renderer, GetRenderables get_renderables);

  ~ScreenCapture() override;

  // |fidl::WireServer<fuchsia_ui_composition::ScreenCapture>|
  void Configure(ConfigureRequestView request, ConfigureCompleter::Sync& completer) override;
  void Configure(
      fuchsia_ui_composition::wire::ScreenCaptureConfig args,
      fit::function<void(fit::result<fuchsia_ui_composition::wire::ScreenCaptureError>)> callback);
  void Configure(
      fuchsia_ui_composition::ScreenCaptureConfig args,
      fit::function<void(fit::result<fuchsia_ui_composition::ScreenCaptureError>)> callback);

  // |fidl::WireServer<fuchsia_ui_composition::ScreenCapture>|
  void GetNextFrame(GetNextFrameRequestView request,
                    GetNextFrameCompleter::Sync& completer) override;
  void GetNextFrame(fuchsia_ui_composition::wire::GetNextFrameArgs args,
                    fit::function<void(fit::result<fuchsia_ui_composition::wire::ScreenCaptureError,
                                                   fuchsia_ui_composition::wire::FrameInfo>)>
                        callback);
  void GetNextFrame(fuchsia_ui_composition::GetNextFrameArgs args,
                    fit::function<void(fit::result<fuchsia_ui_composition::ScreenCaptureError,
                                                   fuchsia_ui_composition::FrameInfo>)>
                        callback);

  // |fidl::WireServer<fuchsia_ui_composition::ScreenCapture>|
  void ReleaseFrame(ReleaseFrameRequestView request,
                    ReleaseFrameCompleter::Sync& completer) override;
  void ReleaseFrame(
      uint32_t buffer_id,
      fit::function<void(fit::result<fuchsia_ui_composition::ScreenCaptureError>)> callback);

 private:
  enum class ConfigureState : uint8_t {
    kUnconfigured,
    kConfiguring,
    kConfigured,
  };

  void ClearImages(ConfigureState state = ConfigureState::kUnconfigured);

  // Clients cannot use zero as an Image ID.
  static constexpr int64_t kInvalidId = 0;

  std::vector<std::shared_ptr<allocation::BufferCollectionImporter>> buffer_collection_importers_;

  // Holds all registered images.
  std::unordered_map<int64_t, allocation::ImageMetadata> image_ids_;

  // Indices of available buffers.
  std::deque<uint32_t> available_buffers_;

  fuchsia_ui_composition::wire::Rotation stream_rotation_;

  std::shared_ptr<flatland::Renderer> renderer_;
  GetRenderables get_renderables_;

  ConfigureState configure_state_ = ConfigureState::kUnconfigured;

  async::Executor executor_;
};

}  // namespace screen_capture

#endif  // SRC_UI_SCENIC_LIB_SCREEN_CAPTURE_SCREEN_CAPTURE_H_
