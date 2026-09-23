// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/view_tree/geometry_provider.h"

#include <fidl/fuchsia.math/cpp/type_conversions.h>
#include <fidl/fuchsia.ui.observation.geometry/cpp/type_conversions.h>
#include <lib/async/default.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/clock.h>

#include <cmath>
#include <stack>

#include "src/ui/scenic/lib/utils/check_is_on_thread.h"
#include "src/ui/scenic/lib/utils/helpers.h"
#include "src/ui/scenic/lib/utils/math.h"
#include "src/ui/scenic/lib/utils/time.h"

namespace view_tree {

namespace {

constexpr size_t AlignTo8(size_t size) { return (size + 7) & ~7; }

size_t MeasureViewDescriptor(const fuchsia_ui_observation_geometry::ViewDescriptor& vd) {
  size_t size = 16;  // Table vector header
  size_t max_ordinal = 0;
  if (vd.view_ref_koid().has_value()) {
    max_ordinal = std::max<size_t>(max_ordinal, 1);
    size += 8;  // zx.Koid (uint64)
  }
  if (vd.layout().has_value()) {
    max_ordinal = std::max<size_t>(max_ordinal, 2);
    size += 40;  // Layout struct (extent: 16, pixel_scale: 8, inset: 16 = 40 bytes)
  }
  if (vd.extent_in_context().has_value()) {
    max_ordinal = std::max<size_t>(max_ordinal, 3);
    size += 24;  // RotatableExtent struct (origin: 8, width: 4, height: 4, angle_degrees: 4 = 20 ->
                 // 24 padded)
  }
  if (vd.extent_in_parent().has_value()) {
    max_ordinal = std::max<size_t>(max_ordinal, 4);
    size += 24;  // RotatableExtent struct (20 -> 24 padded)
  }
  if (vd.children().has_value()) {
    max_ordinal = std::max<size_t>(max_ordinal, 5);
    size += 16 + AlignTo8(vd.children()->size() * sizeof(uint32_t));
  }
  size += max_ordinal * 8;  // Envelopes
  return size;
}

size_t MeasureViewTreeSnapshot(const fuchsia_ui_observation_geometry::ViewTreeSnapshot& snapshot) {
  size_t size = 16;  // Table vector header
  size_t max_ordinal = 0;
  if (snapshot.time().has_value()) {
    max_ordinal = std::max<size_t>(max_ordinal, 1);
    size += 8;  // zx.Time (int64)
  }
  if (snapshot.views().has_value()) {
    max_ordinal = std::max<size_t>(max_ordinal, 2);
    size += 16;  // views vector header
    for (const auto& vd : *snapshot.views()) {
      size += MeasureViewDescriptor(vd);
    }
  }
  size += max_ordinal * 8;  // Envelopes
  return size;
}

}  // namespace

GeometryProvider::GeometryProvider(std::shared_ptr<view_tree::SnapshotHolder> snapshot_holder)
    : snapshot_holder_(std::move(snapshot_holder)) {}

void GeometryProvider::Register(
    fidl::ServerEnd<fuchsia_ui_observation_geometry::ViewTreeWatcher> endpoint,
    zx_koid_t context_view) {
  utils::CheckIsOnInputThread();
  RegisterViewTreeWatcherImpl(std::move(endpoint), context_view);
}

void GeometryProvider::RegisterGlobalViewTreeWatcher(
    fidl::ServerEnd<fuchsia_ui_observation_geometry::ViewTreeWatcher> endpoint) {
  utils::CheckIsOnInputThread();
  RegisterViewTreeWatcherImpl(std::move(endpoint), std::nullopt);
}

void GeometryProvider::RegisterViewTreeWatcherImpl(
    fidl::ServerEnd<fuchsia_ui_observation_geometry::ViewTreeWatcher> endpoint,
    std::optional<zx_koid_t> context_view) {
  FX_DCHECK(endpoint.is_valid()) << "precondition";
  // If the optional `context` view is provided, it must not be `ZX_KOID_INVALID`.
  FX_DCHECK(!context_view.has_value() || context_view.value() != ZX_KOID_INVALID) << "precondition";

  auto endpoint_id = endpoint_counter_++;
  auto [it, _] = endpoints_.insert(
      {endpoint_id, std::make_unique<ProviderEndpoint>(
                        std::move(endpoint), context_view,
                        [weak_provider = weak_factory_.GetWeakPtr(), endpoint_id] {
                          if (weak_provider) {
                            auto count = weak_provider->endpoints_.erase(endpoint_id);
                            FX_DCHECK(count > 0);
                          }
                        })});
  // Notify new endpoint of latest snapshot.
  FX_DCHECK(snapshot_holder_);
  bool needs_full_update = false;
  {
    auto snapshot = snapshot_holder_->GetSnapshot();
    if (snapshot->sequence_number > 0) {
      if (latest_sequence_number_ == snapshot->sequence_number) {
        // Update only the new endpoint; the others are up to date.
        it->second->AddViewTreeSnapshot(
            ExtractObservationSnapshot(it->second->context_view(), *snapshot));
      } else {
        needs_full_update = true;
      }
    }
  }
  if (needs_full_update) {
    OnNewViewTreeSnapshot();
  }
}

void GeometryProvider::OnNewViewTreeSnapshot() {
  utils::CheckIsOnInputThread();

  FX_DCHECK(snapshot_holder_);
  auto snapshot = snapshot_holder_->GetSnapshot();

  if (snapshot->sequence_number <= latest_sequence_number_) {
    return;
  }
  latest_sequence_number_ = snapshot->sequence_number;

  // Remove any dead endpoints.
  for (auto it = endpoints_.begin(); it != endpoints_.end();) {
    if (!it->second->IsAlive()) {
      it = endpoints_.erase(it);
    } else {
      ++it;
    }
  }

  // Add snapshot to each endpoint's buffer.
  for (auto& [_, endpoint] : endpoints_) {
    endpoint->AddViewTreeSnapshot(ExtractObservationSnapshot(endpoint->context_view(), *snapshot));
  }
}

std::optional<fuchsia_ui_observation_geometry::ViewTreeSnapshot>
GeometryProvider::ExtractObservationSnapshot(std::optional<zx_koid_t> endpoint_context_view,
                                             const view_tree::Snapshot& snapshot) {
  fuchsia_ui_observation_geometry::ViewTreeSnapshot view_tree_snapshot;
  view_tree_snapshot.time(utils::dispatcher_clock_now());
  std::vector<fuchsia_ui_observation_geometry::ViewDescriptor> views;
  bool views_exceeded = false;

  // |ProviderEndpoint| not having a |context_view_| get global access to the view tree as they get
  // registered through f.u.o.t.Registry.RegisterGlobalViewTreeWatcher.
  zx_koid_t context_view = 0;
  if (endpoint_context_view.has_value()) {
    context_view = endpoint_context_view.value();
  } else {
    context_view = snapshot.root;
  }

  // Empty snapshot case.
  if (context_view == ZX_KOID_INVALID && snapshot.view_tree.empty()) {
    view_tree_snapshot.views(std::vector<fuchsia_ui_observation_geometry::ViewDescriptor>{});
    return view_tree_snapshot;
  }

  // It is possible that |context_view| has not yet connected to the view tree or it has
  // disconnected. In either case, send an empty snapshot.
  if (!snapshot.view_tree.contains(context_view)) {
    view_tree_snapshot.views(std::vector<fuchsia_ui_observation_geometry::ViewDescriptor>{});
    return view_tree_snapshot;
  }

  // Perform a depth-first search on the view tree to populate |views| with
  // ViewDescriptors.
  std::stack<zx_koid_t> stack;
  std::unordered_set<zx_koid_t> visited;
  stack.push(context_view);
  while (!stack.empty()) {
    auto view_ref_koid = stack.top();
    stack.pop();
    FX_DCHECK(!visited.contains(view_ref_koid)) << "Cycle detected in the view tree";
    visited.insert(view_ref_koid);
    const auto& view = snapshot.view_tree.at(view_ref_koid);

    // Do not set a view vector in the |ViewTreeSnapshot| as the size of |views| will exceed
    // kMaxViewCount, since the number of |children| of the |view_ref_koid| exceeds
    // kMaxViewCount.
    if (view.children.size() > fuchsia_ui_observation_geometry::kMaxViewCount) {
      views_exceeded = true;
      break;
    }

    for (auto child : view.children) {
      stack.push(child);
    }

    // Closed views may have 0x0 size temporarily; do not report them as part
    // of the view tree.
    constexpr BoundingBox kZeroBoundingBox{};
    const bool is_sized_view = view.bounding_box != kZeroBoundingBox;
    if (is_sized_view) {
      views.push_back(ExtractViewDescriptor(view_ref_koid, context_view, snapshot));
    } else {
      // TODO(https://fxbug.dev/42072167): Not obvious what the correct action is for 0x0 views.
      // For now, we skip them, fingers crossed.
      FX_DLOGS(WARNING) << "found a 0x0 view in the view tree, skipping: " << view_ref_koid;
    }

    // Do not set a view vector in the |ViewTreeSnapshot| as the size of |views| will exceed
    // kMaxViewCount, since the stack is not empty.
    if (views.size() == fuchsia_ui_observation_geometry::kMaxViewCount && !stack.empty()) {
      views_exceeded = true;
      break;
    }
  }

  if (!views_exceeded) {
    view_tree_snapshot.views(std::move(views));
  }
  return view_tree_snapshot;
}

fuchsia_ui_observation_geometry::ViewDescriptor GeometryProvider::ExtractViewDescriptor(
    zx_koid_t view_ref_koid, zx_koid_t context_view, const view_tree::Snapshot& snapshot) {
  auto& view_node = snapshot.view_tree.at(view_ref_koid);

  std::array<float, 2> pixel_scale = utils::kDefaultPixelScale;

  fuchsia_math::PointF min_extent(view_node.bounding_box.min[0], view_node.bounding_box.min[1]);
  fuchsia_math::PointF max_extent(view_node.bounding_box.max[0], view_node.bounding_box.max[1]);
  fuchsia_ui_observation_geometry::AlignedExtent aligned_extent(min_extent, max_extent);
  fuchsia_math::InsetF inset(0.f, 0.f, 0.f, 0.f);

  fuchsia_ui_observation_geometry::Layout layout(aligned_extent, pixel_scale, inset);

  auto world_from_local_transform = glm::inverse(view_node.local_from_world_transform);
  auto extent_in_context_transform =
      snapshot.view_tree.at(context_view).local_from_world_transform * world_from_local_transform;

  // The coordinates of a view_node's bounding box in context_view's coordinate system.
  auto extent_in_context_top_left = utils::TransformPointerCoords(
      {view_node.bounding_box.min[0], view_node.bounding_box.min[1]}, extent_in_context_transform);
  auto extent_in_context_top_right = utils::TransformPointerCoords(
      {view_node.bounding_box.max[0], view_node.bounding_box.min[1]}, extent_in_context_transform);
  auto extent_in_context_bottom_left = utils::TransformPointerCoords(
      {view_node.bounding_box.min[0], view_node.bounding_box.max[1]}, extent_in_context_transform);

  auto extent_in_context_dx = extent_in_context_top_right[0] - extent_in_context_top_left[0];
  auto extent_in_context_dy = extent_in_context_top_right[1] - extent_in_context_top_left[1];

  // TODO(https://fxbug.dev/42174590) : Handle floating point precision errors in calculating the
  // angle. Angle of a line segment with coordinates (x1,y1) and (x2,y2) is defined as tan inverse
  // (y2-y1/x2-x1). As the return value is in radians multiply it by 180/PI.
  FX_DCHECK(extent_in_context_dx != 0 || extent_in_context_dy != 0)
      << "top left and top right coordinates cannot be the same. inputs: "
      << extent_in_context_top_right[0] << ", " << extent_in_context_top_left[0] << ", "
      << extent_in_context_top_right[1] << ", " << extent_in_context_top_left[1] << ", "
      << view_node.bounding_box.min[0] << ", " << view_node.bounding_box.min[1] << ", "
      << view_node.bounding_box.max[0] << ", " << view_node.bounding_box.max[1];
  auto angle_context = atan2(extent_in_context_dy, extent_in_context_dx) * (180. / M_PI);

  // Change the range of |angle_context| from [-pi,pi] to [0,2*pi).
  angle_context = std::fmod(angle_context + 360, 360);

  fuchsia_math::PointF origin_context(extent_in_context_top_left[0], extent_in_context_top_left[1]);
  float width_context = static_cast<float>(
      std::hypot(extent_in_context_top_right[0] - extent_in_context_top_left[0],
                 extent_in_context_top_right[1] - extent_in_context_top_left[1]));
  float height_context = static_cast<float>(
      std::hypot(extent_in_context_bottom_left[0] - extent_in_context_top_left[0],
                 extent_in_context_bottom_left[1] - extent_in_context_top_left[1]));

  fuchsia_ui_observation_geometry::RotatableExtent extent_in_context(
      origin_context, width_context, height_context, static_cast<float>(angle_context));

  glm::mat4 extent_in_parent_transform;

  // If the context view is the root node, it will not have a parent so the
  // extent_in_parent_transform will be an identity matrix.
  if (view_node.parent != ZX_KOID_INVALID) {
    extent_in_parent_transform =
        snapshot.view_tree.at(view_node.parent).local_from_world_transform *
        world_from_local_transform;
  }

  // The coordinates of a view_node's bounding box in its parent's coordinate system.
  auto extent_in_parent_top_left = utils::TransformPointerCoords(
      {view_node.bounding_box.min[0], view_node.bounding_box.min[1]}, extent_in_parent_transform);
  auto extent_in_parent_top_right = utils::TransformPointerCoords(
      {view_node.bounding_box.max[0], view_node.bounding_box.min[1]}, extent_in_parent_transform);
  auto extent_in_parent_bottom_left = utils::TransformPointerCoords(
      {view_node.bounding_box.min[0], view_node.bounding_box.max[1]}, extent_in_parent_transform);

  auto extent_in_parent_dx = extent_in_parent_top_right[0] - extent_in_parent_top_left[0];
  auto extent_in_parent_dy = extent_in_parent_top_right[1] - extent_in_parent_top_left[1];
  FX_DCHECK(extent_in_parent_dx != 0 || extent_in_parent_dy != 0)
      << "top left and top right coordinates cannot be the same";

  // TODO(https://fxbug.dev/42174590) : Handle floating point precision errors in calculating the
  // angle.
  auto angle_parent = atan2(extent_in_parent_dy, extent_in_parent_dx) * (180. / M_PI);

  // Change the range of |angle_parent| from [-pi,pi] to [0,2*pi).
  angle_parent = std::fmod(angle_parent + 360, 360);

  fuchsia_math::PointF origin_parent(extent_in_parent_top_left[0], extent_in_parent_top_left[1]);
  float width_parent =
      static_cast<float>(std::hypot(extent_in_parent_top_right[0] - extent_in_parent_top_left[0],
                                    extent_in_parent_top_right[1] - extent_in_parent_top_left[1]));
  float height_parent = static_cast<float>(
      std::hypot(extent_in_parent_bottom_left[0] - extent_in_parent_top_left[0],
                 extent_in_parent_bottom_left[1] - extent_in_parent_top_left[1]));

  fuchsia_ui_observation_geometry::RotatableExtent extent_in_parent(
      origin_parent, width_parent, height_parent, static_cast<float>(angle_parent));

  fuchsia_ui_observation_geometry::ViewDescriptor view_descriptor;
  view_descriptor.view_ref_koid(view_ref_koid);
  view_descriptor.layout(std::move(layout));
  view_descriptor.extent_in_context(std::move(extent_in_context));
  // The context view is not allowed to see into the parent (which is outside the observation
  // scope). Enforce these semantics by omitting |extent_in_parent| for the context view.
  if (view_ref_koid != context_view) {
    view_descriptor.extent_in_parent(std::move(extent_in_parent));
  }

  FX_DCHECK(view_node.children.size() <= fuchsia_ui_observation_geometry::kMaxViewCount)
      << "invariant.";
  std::vector<uint32_t> children(view_node.children.begin(), view_node.children.end());
  view_descriptor.children(std::move(children));

  return view_descriptor;
}

GeometryProvider::ProviderEndpoint::ProviderEndpoint(
    fidl::ServerEnd<fuchsia_ui_observation_geometry::ViewTreeWatcher> endpoint,
    std::optional<zx_koid_t> context_view, fit::function<void()> destroy_instance_function)
    : context_view_(std::move(context_view)) {
  binding_.emplace(async_get_default_dispatcher(), std::move(endpoint), this,
                   [destroy_instance = std::move(destroy_instance_function)](fidl::UnbindInfo) {
                     if (destroy_instance) {
                       destroy_instance();
                     }
                   });
}

GeometryProvider::ProviderEndpoint::~ProviderEndpoint() = default;

void GeometryProvider::ProviderEndpoint::AddViewTreeSnapshot(
    std::optional<fuchsia_ui_observation_geometry::ViewTreeSnapshot> view_tree_snapshot) {
  utils::CheckIsOnInputThread();

  if (view_tree_snapshot.has_value()) {
    view_tree_snapshots_.push_back(std::move(view_tree_snapshot.value()));
  }

  if (view_tree_snapshots_.size() > fuchsia_ui_observation_geometry::kBufferSize) {
    view_tree_snapshots_.pop_front();
    error_ |= fuchsia_ui_observation_geometry::Error::kBufferOverflow;
  }
  FX_DCHECK(view_tree_snapshots_.size() <= fuchsia_ui_observation_geometry::kBufferSize)
      << "invariant";

  SendResponseMaybe();
}

void GeometryProvider::ProviderEndpoint::Watch(WatchCompleter::Sync& completer) {
  utils::CheckIsOnInputThread();

  // Check if there is an ongoing Watch call. If there is an in-flight Watch call, close the channel
  // and remove itself from |endpoints_|.
  if (pending_completer_.has_value()) {
    CloseChannel();
    return;
  }

  // If there are snapshots lined up in the queue, send the response otherwise store the callback.
  pending_completer_ = completer.ToAsync();

  SendResponseMaybe();
}

void GeometryProvider::ProviderEndpoint::SendResponseMaybe() {
  // Check if we have a client waiting for a response and if we have snapshots queued up to be sent
  // to the client before sending the response.
  if (pending_completer_.has_value() && !view_tree_snapshots_.empty()) {
    SendResponse();
  }
}

void GeometryProvider::ProviderEndpoint::SendResponse() {
  FX_DCHECK(!view_tree_snapshots_.empty());
  FX_DCHECK(pending_completer_.has_value());

  fuchsia_ui_observation_geometry::WatchResponse watch_response;
  watch_response.epoch_end(utils::dispatcher_clock_now());

  size_t response_size = 16 /* msg header */ + 16 /* table header */ + 3 * 8 /* envelopes */ +
                         8 /* epoch_end */ + 16 /* updates vector header */;

  std::vector<fuchsia_ui_observation_geometry::ViewTreeSnapshot> updates;

  // Send pending snapshots to the client in a chronological order and clear the deque. If the size
  // of the response exceeds ZX_CHANNEL_MAX_MSG_BYTES, drop the oldest ViewTreeSnapshot in the
  // response.
  while (!view_tree_snapshots_.empty() && response_size < ZX_CHANNEL_MAX_MSG_BYTES) {
    const size_t snapshot_size = MeasureViewTreeSnapshot(view_tree_snapshots_.back());
    if (response_size + snapshot_size < ZX_CHANNEL_MAX_MSG_BYTES) {
      response_size += snapshot_size;
      // The absence of a views vector in |ViewTreeSnapshot| indicates that a view overflow has
      // occurred.
      if (!view_tree_snapshots_.back().views().has_value()) {
        error_ |= fuchsia_ui_observation_geometry::Error::kViewsOverflow;
      }
      updates.push_back(std::move(view_tree_snapshots_.back()));
      view_tree_snapshots_.pop_back();
    } else {
      error_ |= fuchsia_ui_observation_geometry::Error::kChannelOverflow;
      break;
    }
  }

  std::reverse(updates.begin(), updates.end());
  watch_response.updates(std::move(updates));

  if (error_ != fuchsia_ui_observation_geometry::Error{}) {
    watch_response.error(error_);
  }

  fidl::Arena arena;
  pending_completer_->Reply(fidl::ToWire(arena, watch_response));

  // Clear the pending_completer_ and reset the state for subsequent Watch() calls.
  Reset();
}

void GeometryProvider::ProviderEndpoint::CloseChannel() {
  if (binding_.has_value()) {
    binding_->Close(ZX_ERR_BAD_STATE);
  }
}

void GeometryProvider::ProviderEndpoint::Reset() {
  pending_completer_.reset();
  view_tree_snapshots_.clear();
  error_ = fuchsia_ui_observation_geometry::Error{};
}

}  // namespace view_tree
