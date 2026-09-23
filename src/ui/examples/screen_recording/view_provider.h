// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_EXAMPLES_SCREEN_RECORDING_VIEW_PROVIDER_H_
#define SRC_UI_EXAMPLES_SCREEN_RECORDING_VIEW_PROVIDER_H_

#include <fidl/fuchsia.math/cpp/fidl.h>
#include <fidl/fuchsia.sysmem2/cpp/fidl.h>
#include <fidl/fuchsia.ui.app/cpp/fidl.h>
#include <fidl/fuchsia.ui.composition.internal/cpp/fidl.h>
#include <fidl/fuchsia.ui.composition/cpp/fidl.h>
#include <fidl/fuchsia.ui.views/cpp/fidl.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>

#include "src/lib/ui/flatland-frame-scheduling/src/simple_present.h"

namespace screen_recording_example {

using fuchsia_ui_composition::ContentId;
using fuchsia_ui_composition::LayoutInfo;
using fuchsia_ui_composition::TransformId;

struct BouncingSquare {
  int32_t x;
  int32_t y;
  int32_t x_speed;
  int32_t y_speed;
  fuchsia_math::SizeU size;
};

class ViewProviderImpl final : public fidl::Server<fuchsia_ui_app::ViewProvider> {
 public:
  explicit ViewProviderImpl(component::OutgoingDirectory& outgoing, async_dispatcher_t* dispatcher);
  ~ViewProviderImpl() override = default;

  // |fidl::Server<fuchsia_ui_app::ViewProvider>|
  void CreateView2(CreateView2Request& request, CreateView2Completer::Sync& completer) override;
  void CreateViewWithViewRef(CreateViewWithViewRefRequest& request,
                             CreateViewWithViewRefCompleter::Sync& completer) override {
    ZX_PANIC("Not Implemented");
  }

 private:
  fidl::Client<fuchsia_ui_composition::Flatland>& flatland() {
    return flatland_connection_->FlatlandClient();
  }

  void DrawSquare();
  void CheckHit();
  void PresentCallback();
  void ScreenCaptureCallback();
  void SetUpFlatland();
  fuchsia_ui_composition::ColorRgba RandomColor();

  async_dispatcher_t* dispatcher_;
  fidl::ServerBindingGroup<fuchsia_ui_app::ViewProvider> bindings_;
  std::optional<fuchsia_ui_composition::LayoutInfo> layout_;
  fidl::WireClient<fuchsia_sysmem2::Allocator> sysmem_allocator_;
  std::unique_ptr<simple_present::FlatlandConnection> flatland_connection_;
  fidl::SyncClient<fuchsia_ui_composition::Allocator> flatland_allocator_;
  fidl::Client<fuchsia_ui_composition::ParentViewportWatcher> parent_watcher_;

  fidl::Client<fuchsia_ui_composition_internal::ScreenCapture> screen_capture_;

  const TransformId kRootTransformId{1};
  const TransformId kChildTransformId1{2};
  const TransformId kChildTransformId2{3};
  const TransformId kBouncingSquareTransformId{4};

  ContentId kSquareRectId{0};

  uint32_t num_buffers_ = 3;
  // Release fences passed into Present() for each buffer. Indexed by buffer index.
  std::vector<zx::event> present_release_fences_;

  BouncingSquare bs_ = {0, 0, 10, 10, fuchsia_math::SizeU(40, 40)};

  uint32_t display_width_ = 0;
  uint32_t display_height_ = 0;
  uint32_t half_display_width_ = 0;
  uint32_t num_pixels_ = 0;
};

}  // namespace screen_recording_example

#endif  // SRC_UI_EXAMPLES_SCREEN_RECORDING_VIEW_PROVIDER_H_
