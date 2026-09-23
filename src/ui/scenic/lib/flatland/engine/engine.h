// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_FLATLAND_ENGINE_ENGINE_H_
#define SRC_UI_SCENIC_LIB_FLATLAND_ENGINE_ENGINE_H_

#include <fidl/fuchsia.ui.display.color/cpp/fidl.h>
#include <lib/fit/function.h>
#include <lib/inspect/component/cpp/component.h>
#include <lib/zx/eventpair.h>

#include <map>
#include <optional>
#include <utility>

#include "src/ui/scenic/lib/display/fidl_id_types.h"
#include "src/ui/scenic/lib/flatland/engine/display_compositor.h"
#include "src/ui/scenic/lib/flatland/flatland_display.h"
#include "src/ui/scenic/lib/flatland/flatland_presenter_impl.h"
#include "src/ui/scenic/lib/flatland/flatland_types.h"
#include "src/ui/scenic/lib/flatland/global_matrix_data.h"
#include "src/ui/scenic/lib/flatland/global_resolved_layers.h"
#include "src/ui/scenic/lib/flatland/link_system.h"
#include "src/ui/scenic/lib/flatland/uber_struct_system.h"
#include "src/ui/scenic/lib/scheduling/frame_scheduler.h"
#include "src/ui/scenic/lib/view_tree/snapshot_types.h"

namespace flatland {

using GetRootTransformFunc = fit::function<std::optional<TransformHandle>()>;
using Renderables = std::vector<ResolvedLayer>;

// Engine is responsible for building a display list for DisplayCompositor, to insulate it from
// needing to know anything about the Flatland scene graph.
class Engine {
 public:
  Engine(std::shared_ptr<flatland::DisplayCompositor> flatland_compositor,
         std::shared_ptr<flatland::FlatlandPresenterImpl> flatland_presenter,
         std::shared_ptr<flatland::UberStructSystem> uber_struct_system,
         std::shared_ptr<flatland::LinkSystem> link_system, inspect::Node inspect_node,
         GetRootTransformFunc get_root_transform);
  ~Engine() = default;

  // Orchestrates the generation and submission of a frame to the `DisplayCompositor`.
  //
  // This updates scene topology and link watchers, culls invisible content, and
  // handles first-frame startup logic to avoid driving the display before content
  // is ready.
  void RenderScheduledFrame(uint64_t frame_number, zx::time presentation_time,
                            const FlatlandDisplay& display,
                            scheduling::FramePresentedCallback callback);

  // Dispatches updated layout information (coordinate transforms, view dimensions,
  // device pixel ratio, etc.) to layout observers and link watchers based on the
  // current frame's scene state.
  //
  // CRITICAL: This must be called *after* the new ViewTree snapshot has been fully
  // updated and published (e.g. in `UpdateSnapshot()`), but *before* the frame's scene
  // state is cleared by `CleanUpFrame()`. This ensures layout observers do not query
  // or receive layout updates against a stale ViewTree snapshot.
  void UpdateLinkWatchersAfterViewTreePublished();

  // Resets internal state to prepare for the next frame.
  //
  // This completes the frame cycle; it must be called after every invocation of
  // `RenderScheduledFrame()` or `SkipRender()`. Attempting to render a new
  // frame without cleaning up the previous one will trigger a DCHECK.
  void CleanUpFrame();

  // Snapshots the current Flatland content tree rooted at |root_transform|. |root_transform| is set
  // from the root transform of the display returned from
  // |FlatlandManager::GetPrimaryFlatlandDisplayForRendering|.
  view_tree::GeneratedSubtreeSnapshot GenerateViewTreeSnapshot(
      const TransformHandle& root_transform);

  // Returns all renderables reachable from the display's root transform.
  Renderables GetRenderables(const FlatlandDisplay& display);

  // Signal all release fences and skip rendering.
  // `rotate_scene_state == true` is probably what you want, unless you know you don't.
  void SkipRender(scheduling::FramePresentedCallback callback, bool rotate_scene_state = true);

  void AddDisplay(display::Display& display);

 private:
  // Holds the per-frame scene state that is generated from the latest UberStructs from each
  // Flatland session, linked together by the LinkSystem.
  struct SceneState {
    void Initialize(Engine& engine, TransformHandle root_transform);

    // Clear all fields without deallocating memory.
    void Clear();

    UberStructSnapshot snapshot;
    flatland::GlobalTopologyData topology_data;
    flatland::GlobalMatrixVector global_matrices;
    flatland::GlobalTransformClipRegionVector clip_regions;
    flatland::GlobalOpacityVector opacities;
    std::vector<ResolvedLayer> resolved_layers;
  };

  // Initialize all inspect::Nodes, so that the Engine state can be observed.
  void InitializeInspectObjects();

  // Tally the frame result so that it can be displayed via Inspect.
  void RecordFrameResult(DisplayCompositor::RenderFrameResult result);

  std::shared_ptr<flatland::DisplayCompositor> flatland_compositor_;
  std::shared_ptr<flatland::FlatlandPresenterImpl> flatland_presenter_;
  std::shared_ptr<flatland::UberStructSystem> uber_struct_system_;
  std::shared_ptr<flatland::LinkSystem> link_system_;

  // Updated every frame, and cached for purposes like ViewTree generation.  These states are
  // double-buffered; even though there are 3 variables, only 2 of them are non-null at any given
  // moment.  Using 3 vars instead of 2 allows us to assert that usage invariants hold.
  std::unique_ptr<SceneState> current_scene_state_;
  std::unique_ptr<SceneState> previous_scene_state_;
  std::unique_ptr<SceneState> cleared_scene_state_;

  bool first_frame_with_image_is_rendered_ = false;

  // Used to skip rendering until the display is added.
  std::map<display::DisplayId, bool> seen_display_ids_;

  inspect::Node inspect_node_;
  inspect::LazyNode inspect_scene_dump_;
  inspect::Node inspect_frame_results_;
  inspect::UintProperty inspect_direct_display_frame_count_;
  inspect::UintProperty inspect_gpu_composition_frame_count_;
  inspect::UintProperty inspect_failed_frame_count_;
  GetRootTransformFunc get_root_transform_;

  async::Executor executor_;
};

}  // namespace flatland

#endif  // SRC_UI_SCENIC_LIB_FLATLAND_ENGINE_ENGINE_H_
