// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/flatland/engine/engine.h"

#include <fidl/fuchsia.hardware.display.types/cpp/fidl.h>
#include <lib/async/cpp/time.h>
#include <lib/syslog/cpp/macros.h>

#include <sstream>
#include <string>

#include "src/ui/scenic/lib/flatland/global_matrix_data.h"
#include "src/ui/scenic/lib/flatland/global_resolved_layers.h"
#include "src/ui/scenic/lib/flatland/global_topology_data.h"
#include "src/ui/scenic/lib/flatland/scene_dumper.h"
#include "src/ui/scenic/lib/scheduling/frame_scheduler.h"
#include "src/ui/scenic/lib/utils/check_is_on_thread.h"
#include "src/ui/scenic/lib/utils/helpers.h"

// Hardcoded double buffering.
// TODO(https://fxbug.dev/42156567): make this configurable.  Even fancier: is it worth considering
// sharing a pool of framebuffers between multiple displays?  (assuming that their dimensions are
// similar, etc.)
static constexpr uint32_t kNumDisplayFramebuffers = 2;

namespace flatland {

Engine::Engine(std::shared_ptr<DisplayCompositor> flatland_compositor,
               std::shared_ptr<FlatlandPresenterImpl> flatland_presenter,
               std::shared_ptr<UberStructSystem> uber_struct_system,
               std::shared_ptr<LinkSystem> link_system, inspect::Node inspect_node,
               GetRootTransformFunc get_root_transform)
    : flatland_compositor_(std::move(flatland_compositor)),
      flatland_presenter_(std::move(flatland_presenter)),
      uber_struct_system_(std::move(uber_struct_system)),
      link_system_(std::move(link_system)),
      cleared_scene_state_(std::make_unique<SceneState>()),
      inspect_node_(std::move(inspect_node)),
      get_root_transform_(std::move(get_root_transform)),
      executor_(async_get_default_dispatcher()) {
  utils::CheckIsOnMainThread();
  FX_DCHECK(flatland_compositor_);
  FX_DCHECK(flatland_presenter_);
  FX_DCHECK(uber_struct_system_);
  FX_DCHECK(link_system_);
  InitializeInspectObjects();
}

constexpr char kSceneDump[] = "scene_dump";

void Engine::InitializeInspectObjects() {
  inspect_scene_dump_ = inspect_node_.CreateLazyValues(kSceneDump, [this] {
    inspect::Inspector inspector;
    const auto root_transform = get_root_transform_();
    if (!root_transform) {
      inspector.GetRoot().CreateString(kSceneDump, "(No Root Transform)", &inspector);
      return fpromise::make_ok_promise(std::move(inspector));
    }

    SceneState scene_state;
    scene_state.Initialize(*this, *root_transform);
    std::ostringstream output;
    DumpScene(scene_state.snapshot.map, scene_state.topology_data, scene_state.resolved_layers,
              output);
    inspector.GetRoot().CreateString(kSceneDump, output.str(), &inspector);
    return fpromise::make_ok_promise(std::move(inspector));
  });

  inspect_frame_results_ = inspect_node_.CreateChild("Frame result counts");
  inspect_direct_display_frame_count_ = inspect_frame_results_.CreateUint("Direct to display", 0);
  inspect_gpu_composition_frame_count_ = inspect_frame_results_.CreateUint("GPU composition", 0);
  inspect_failed_frame_count_ = inspect_frame_results_.CreateUint("Failed", 0);
}

void Engine::RenderScheduledFrame(uint64_t frame_number, zx::time presentation_time,
                                  const FlatlandDisplay& display,
                                  scheduling::FramePresentedCallback callback) {
  utils::CheckIsOnMainThread();

  // Emit a counter called "ScenicRender" for visualization in the Trace Viewer.
  //
  // This counter is flipped between 0 and 1 and back on each frame, and is
  // used to visually delineate successive frames in the sometimes busy trace
  // view.
  static bool render_edge_flag = false;
  TRACE_COUNTER("gfx", "ScenicRender", 0, "", TA_UINT32(render_edge_flag = !render_edge_flag));
  // NOTE: this name is important for benchmarking.  Do not remove or modify it
  // without also updating the "process_gfx_trace.go" script.
  TRACE_DURATION("gfx", "RenderFrame", "frame_number", frame_number, "time",
                 presentation_time.get());
  TRACE_FLOW_STEP("gfx", "scenic_frame", frame_number);

  // Initialize scene state which will be cached and reused for the rest of the frame, including
  // for non-rendering actions such as updating the view tree.
  FX_DCHECK(!current_scene_state_);
  FX_DCHECK(cleared_scene_state_);
  current_scene_state_ = std::move(cleared_scene_state_);
  SceneState& scene_state = *current_scene_state_;
  scene_state.Initialize(*this, display.root_transform());

  display::Display* const hw_display = display.display();

#if defined(USE_FLATLAND_VERBOSE_LOGGING)
  std::ostringstream str;
  str << "Engine::RenderScheduledFrame() frame_number=" << frame_number
      << "\nRoot transform of global topology: " << scene_state.topology_data.topology_vector[0]
      << "\nTopologically-sorted transforms and their corresponding parent transforms:";
  for (size_t i = 1; i < scene_state.topology_data.topology_vector.size(); ++i) {
    str << "\n        " << scene_state.topology_data.topology_vector[i] << " -> "
        << scene_state.topology_data.topology_vector[scene_state.topology_data.parent_indices[i]];
  }
  str << "\nFrame display-list contains " << scene_state.resolved_layers.size()
      << " resolved layers (in increasing Z-order):";
  for (const auto& layer : scene_state.resolved_layers) {
    str << "\n        layer: " << layer;
  }
  FLATLAND_VERBOSE_LOG << str.str();
#endif

  if (auto it = seen_display_ids_.find(hw_display->display_id());
      it == seen_display_ids_.end() || !it->second) {
    // We already "rotated the scene state" above;
    // doing it again would fail a CHECK.
    SkipRender(std::move(callback), /*rotate_scene_state=*/false);
    return;
  }

  CullLayersInPlace(&scene_state.resolved_layers, hw_display->width_in_px(),
                    hw_display->height_in_px());

  // Don't render any initial frames if there is no image that could actually be rendered. We do
  // this to avoid triggering any changes in the display until we have content ready to render. We
  // invoke |callback| to continue the render loop.
  if (!first_frame_with_image_is_rendered_) {
    if (scene_state.resolved_layers.empty()) {
      // We already "rotated the scene state" above; doing it again would fail a CHECK.
      SkipRender(std::move(callback), /*rotate_scene_state=*/false);
      return;
    }
    first_frame_with_image_is_rendered_ = true;
  }

  RenderData render_data = {
      .display_id = hw_display->display_id(),
      .layers = scene_state.resolved_layers,
  };

  auto fences = flatland_presenter_->TakeFences();
  auto frame_result = flatland_compositor_->RenderFrame(
      frame_number, presentation_time, std::span<const RenderData>(&render_data, 1),
      std::move(fences.release_fences), std::move(fences.release_counters),
      std::move(fences.present_fences), std::move(callback));
  RecordFrameResult(frame_result);
}

void Engine::RecordFrameResult(DisplayCompositor::RenderFrameResult result) {
  switch (result) {
    case DisplayCompositor::RenderFrameResult::kDirectToDisplay:
      inspect_direct_display_frame_count_.Add(1);
      break;
    case DisplayCompositor::RenderFrameResult::kGpuComposition:
      inspect_gpu_composition_frame_count_.Add(1);
      break;
    case DisplayCompositor::RenderFrameResult::kFailure:
      inspect_failed_frame_count_.Add(1);
      break;
  }
}

void Engine::UpdateLinkWatchersAfterViewTreePublished() {
  TRACE_DURATION("gfx", "flatland::Engine::UpdateLinkWatchersAfterViewTreePublished");
  utils::CheckIsOnMainThread();
  FX_DCHECK(current_scene_state_);

  const auto& scene_state = *current_scene_state_;
  link_system_->UpdateLinkWatchers(scene_state.topology_data.topology_vector,
                                   scene_state.global_matrices, scene_state.snapshot.map);
}

void Engine::CleanUpFrame() {
  TRACE_DURATION("gfx", "flatland::Engine::CleanUpFrame");
  utils::CheckIsOnMainThread();

  FX_DCHECK(current_scene_state_);
  FX_DCHECK(!cleared_scene_state_);

  // Only happens the first frame.
  if (!previous_scene_state_) {
    previous_scene_state_ = std::make_unique<SceneState>();
  }

  // Previous becomes cleared, current becomes previous.
  cleared_scene_state_ = std::move(previous_scene_state_);
  cleared_scene_state_->Clear();
  previous_scene_state_ = std::move(current_scene_state_);
}

view_tree::GeneratedSubtreeSnapshot Engine::GenerateViewTreeSnapshot(
    const TransformHandle& root_transform) {
  TRACE_DURATION("gfx", "flatland::Engine::GenerateViewTreeSnapshot");
  utils::CheckIsOnMainThread();

  const auto [link_child_to_parent_transform_map, link_topology_changed] =
      link_system_->GetLinkChildToParentTransformMap();

  FX_DCHECK(current_scene_state_);

  if (!uber_struct_system_->MustRecomputeViewTree() && !link_topology_changed) {
    return view_tree::SubtreeSnapshotNoDiff();
  }

  const auto& uber_struct_snapshot = current_scene_state_->snapshot;
  const auto& topology_data = current_scene_state_->topology_data;
  const auto& global_matrices = current_scene_state_->global_matrices;
  const auto& global_clip_regions = current_scene_state_->clip_regions;

  auto hit_regions =
      ComputeGlobalHitRegions(topology_data.topology_vector, topology_data.parent_indices,
                              global_matrices, uber_struct_snapshot.map);

  return flatland::GlobalTopologyData::GenerateViewTreeSnapshot(
      topology_data, uber_struct_snapshot.map, std::move(hit_regions), global_clip_regions,
      global_matrices, link_child_to_parent_transform_map);
}

// TODO(https://fxbug.dev/42162342) If we put Screenshot on its own thread, we should make this
// call thread safe.
Renderables Engine::GetRenderables(const FlatlandDisplay& display) {
  utils::CheckIsOnMainThread();

  TransformHandle root = display.root_transform();

  SceneState scene_state;
  scene_state.Initialize(*this, root);
  const auto hw_display = display.display();

  auto resolved_layers = std::move(scene_state.resolved_layers);

  CullLayersInPlace(&resolved_layers, hw_display->width_in_px(), hw_display->height_in_px());

  return resolved_layers;
}

void Engine::SceneState::Initialize(Engine& engine, TransformHandle root_transform) {
  TRACE_DURATION("gfx", "flatland::Engine::SceneState::Initialize");
  snapshot = engine.uber_struct_system_->Snapshot();

  const auto links = engine.link_system_->GetResolvedTopologyLinks();
  const auto link_system_id = engine.link_system_->GetInstanceId();

  GlobalTopologyData::ComputeGlobalTopologyData(/*output=*/topology_data, snapshot.map, links,
                                                link_system_id, root_transform);

  ComputeGlobalMatrices(/*output=*/global_matrices, topology_data.topology_vector,
                        topology_data.parent_indices, snapshot.map);

  ComputeGlobalTransformClipRegions(/*output=*/clip_regions, topology_data.topology_vector,
                                    topology_data.parent_indices, global_matrices, snapshot.map);

  ComputeGlobalOpacityValues(/*output=*/opacities, topology_data.topology_vector,
                             topology_data.parent_indices, snapshot.map);

  ComputeGlobalResolvedLayers(/*output=*/resolved_layers, topology_data, snapshot.map,
                              global_matrices, clip_regions, opacities);
}

void Engine::SceneState::Clear() {
  TRACE_DURATION("gfx", "flatland::Engine::SceneState::Clear");
  {
    TRACE_DURATION("gfx", "flatland::Engine::SceneState::Clear[snapshot]");
    snapshot.map.clear();
  }
  {
    TRACE_DURATION("gfx", "flatland::Engine::SceneState::Clear[topology_data]");
    topology_data.Clear();
  }
  {
    TRACE_DURATION("gfx", "flatland::Engine::SceneState::Clear[global_matrices]");
    global_matrices.clear();
  }
  {
    TRACE_DURATION("gfx", "flatland::Engine::SceneState::Clear[clip_regions]");
    clip_regions.clear();
  }
  {
    TRACE_DURATION("gfx", "flatland::Engine::SceneState::Clear[opacities]");
    opacities.clear();
  }
  {
    TRACE_DURATION("gfx", "flatland::Engine::SceneState::Clear[resolved_layers]");
    resolved_layers.clear();
  }
}

void Engine::SkipRender(scheduling::FramePresentedCallback callback, bool rotate_scene_state) {
  TRACE_DURATION("gfx", "flatland::Engine::SkipRender");
  utils::CheckIsOnMainThread();

  if (rotate_scene_state) {
    // We don't populate the SceneState, but we still need to move it from "cleared" -> "current" in
    // order to satisfy the checks when `CleanupFrame()` is called.
    FX_DCHECK(!current_scene_state_);
    FX_DCHECK(cleared_scene_state_);
    current_scene_state_ = std::move(cleared_scene_state_);
  }

  const zx::time now = async::Now(async_get_default_dispatcher());
  auto fences = flatland_presenter_->TakeFences();
  utils::SignalReleaseFences(fences.release_fences);
  utils::SignalCounterFences(fences.release_counters, now);
  utils::SignalCounterFences(fences.present_fences, now);
  callback({.render_done_time = now, .actual_presentation_time = now});
}

void Engine::AddDisplay(display::Display& display) {
  utils::CheckIsOnMainThread();

  auto [it, inserted] = seen_display_ids_.emplace(display.display_id(), false);
  if (!inserted) {
    return;
  }

  // This display has _not_ been added to the DisplayCompositor yet.
  DisplayInfo display_info{
      .dimensions = glm::uvec2{display.width_in_px(), display.height_in_px()},
      .formats = display.pixel_formats(),
      .max_layer_count = display.max_layer_count(),
  };
  fpromise::promise<> promise =
      flatland_compositor_->AddDisplay(&display, display_info, kNumDisplayFramebuffers)
          .and_then([it] { it->second = true; });
  executor_.schedule_task(std::move(promise));
}

}  // namespace flatland
