// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/flatland/global_matrix_data.h"

#include <lib/syslog/cpp/macros.h>
#include <lib/trace/event.h>

#include <cmath>

#include "src/ui/scenic/lib/flatland/flatland_types.h"

#include <glm/gtc/epsilon.hpp>
#include <glm/gtc/matrix_access.hpp>

namespace flatland {

constexpr TransformClipRegion kUnclippedRegion({.x = -(std::numeric_limits<int32_t>::max() / 2),
                                                .y = -(std::numeric_limits<int32_t>::max() / 2),
                                                .width = std::numeric_limits<int32_t>::max(),
                                                .height = std::numeric_limits<int32_t>::max()});

namespace {

using fuchsia_ui_composition::Orientation;

// TODO(https://fxbug.dev/426028969): `types::RectangleF` exists now; consider using it here after
// adding helpers such as `Overlap()` or `Intersect(...).IsEmpty()`.  One concern is that we heavily
// use `glm::vec2` including matrix multiplication, so we want to verify that CPU performance isn't
// affected by switching formats.
bool Overlap(const TransformClipRegion& clip, const glm::vec2& origin, const glm::vec2& extent) {
  if (clip == kUnclippedRegion)
    return true;
  const types::Point2 opposite = clip.opposite();
  if (origin.x > static_cast<float>(opposite.x()))
    return false;
  if (origin.y > static_cast<float>(opposite.y()))
    return false;
  if (origin.x + extent.x < static_cast<float>(clip.x()))
    return false;
  if (origin.y + extent.y < static_cast<float>(clip.y()))
    return false;
  return true;
}

// TODO(https://fxbug.dev/426028969): add `types::RectangleF` and use it here.  An `Intersect`
// would be handy, too.
std::pair<glm::vec2, glm::vec2> ClipRectangle(const TransformClipRegion& clip,
                                              const glm::vec2& origin, const glm::vec2& extent) {
  if (!Overlap(clip, origin, extent)) {
    return {glm::vec2(0), glm::vec2(0)};
  }

  glm::vec2 result_origin, result_extent;
  result_origin.x = std::max(float(clip.x()), origin.x);
  result_extent.x = std::min(float(clip.x() + clip.width()), origin.x + extent.x) - result_origin.x;

  result_origin.y = std::max(float(clip.y()), origin.y);
  result_extent.y =
      std::min(float(clip.y() + clip.height()), origin.y + extent.y) - result_origin.y;

  return {result_origin, result_extent};
}

std::array<glm::vec3, 4> ConvertRectToVerts(types::Rectangle rect) {
  return {glm::vec3(static_cast<float>(rect.x()), static_cast<float>(rect.y()), 1),
          glm::vec3(static_cast<float>(rect.x() + rect.width()), static_cast<float>(rect.y()), 1),
          glm::vec3(static_cast<float>(rect.x() + rect.width()),
                    static_cast<float>(rect.y() + rect.height()), 1),
          glm::vec3(static_cast<float>(rect.x()), static_cast<float>(rect.y() + rect.height()), 1)};
}

std::array<glm::vec3, 4> ConvertRectFToVerts(const types::RectangleF& rect) {
  return {glm::vec3(rect.x(), rect.y(), 1), glm::vec3(rect.x() + rect.width(), rect.y(), 1),
          glm::vec3(rect.x() + rect.width(), rect.y() + rect.height(), 1),
          glm::vec3(rect.x(), rect.y() + rect.height(), 1)};
}

// Template to handle both vec2 and vec3 inputs.
template <typename T>
types::Rectangle ConvertVertsToRect(const std::array<T, 4>& verts) {
  return types::Rectangle({.x = static_cast<int32_t>(verts[0].x),
                           .y = static_cast<int32_t>(verts[0].y),
                           .width = static_cast<int32_t>(fabs(verts[1].x - verts[0].x)),
                           .height = static_cast<int32_t>(fabs(verts[2].y - verts[1].y))});
}

types::RectangleF ConvertVertsToRectF(const std::array<glm::vec2, 4>& verts) {
  return types::RectangleF({.x = verts[0].x,
                            .y = verts[0].y,
                            .width = fabs(verts[1].x - verts[0].x),
                            .height = fabs(verts[2].y - verts[1].y)});
}

// Assume that the 4 vertices represent a rectangle, and are provided in clockwise order,
// starting at the top-left corner. Return a tuple of the transformed vertices as well as
// those same transformed vertices reordered so that they are in clockwise order starting
// at the top-left corner.
std::pair<std::array<glm::vec2, 4>, std::array<glm::vec2, 4>> MatrixMultiplyVerts(
    const glm::mat3& matrix, const std::array<glm::vec3, 4>& in_verts) {
  const std::array<glm::vec2, 4> verts = {
      matrix * in_verts[0],
      matrix * in_verts[1],
      matrix * in_verts[2],
      matrix * in_verts[3],
  };

  float min_x = FLT_MAX, min_y = FLT_MAX;
  float max_x = std::numeric_limits<float>::lowest(), max_y = std::numeric_limits<float>::lowest();
  for (uint32_t i = 0; i < 4; i++) {
    min_x = std::min(min_x, verts[i].x);
    min_y = std::min(min_y, verts[i].y);
    max_x = std::max(max_x, verts[i].x);
    max_y = std::max(max_y, verts[i].y);
  }

  return {verts,
          {
              glm::vec2(min_x, min_y),  // top_left
              glm::vec2(max_x, min_y),  // top_right
              glm::vec2(max_x, max_y),  // bottom_right
              glm::vec2(min_x, max_y),  // bottom_left
          }};
}

types::Rectangle MatrixMultiplyRect(const glm::mat3& matrix, types::Rectangle rect) {
  return ConvertVertsToRect(std::get<1>(MatrixMultiplyVerts(matrix, ConvertRectToVerts(rect))));
}

types::RectangleF MatrixMultiplyRectF(const glm::mat3& matrix, types::RectangleF rect) {
  return ConvertVertsToRectF(std::get<1>(MatrixMultiplyVerts(matrix, ConvertRectFToVerts(rect))));
}

}  // namespace

SrcToDest CreateSrcToDest(const glm::mat3& matrix, const TransformClipRegion& clip,
                          const types::RectangleF& src,
                          const fuchsia_ui_composition::ImageFlip image_flip) {
  // The local space of the renderable has its top-left origin point at (0,0) and grows
  // downward and to the right, so that the bottom-right point is at (1,1). We apply
  // the matrix to the four points that represent this unit square to get the points in
  // the global coordinate space.
  //
  // Note that the verts provided are 2D homogenous coordinates, so the third value is always equal
  // to 1. These are NOT 3D vectors with x, y, z values.
  auto [verts, reordered_verts] = MatrixMultiplyVerts(matrix, {
                                                                  glm::vec3(0, 0, 1),
                                                                  glm::vec3(1, 0, 1),
                                                                  glm::vec3(1, 1, 1),
                                                                  glm::vec3(0, 1, 1),
                                                              });

  // Will equal the index of the vert located at the origin in the reordered verts.
  int vert_index = 0;
  bool vert_index_set = false;
  for (uint32_t i = 0; i < 4; i++) {
    if (glm::all(glm::epsilonEqual(reordered_verts[0], verts[i], 0.001f))) {
      vert_index = i;
      vert_index_set = true;
      break;
    }
  }

  FX_DCHECK(vert_index_set) << "Expected |vert_index| to be set";

  // Maps the calculated |vert_index| value to the global Orientation specified by the matrix. Note
  // this conversion only considers orientation and not reflections. Reflections are a property of
  // Image Content only, not Transforms (or Viewports), and so are not handled here.
  constexpr Orientation kIndexToOrientation[4] = {
      // If |vert_index| = 0, then the list is in the same order (no rotation).
      Orientation::kCcw0Degrees,
      // If |vert_index| = 1, then the verts have been rotated by 90 degrees (top-left is now
      // top-right).
      Orientation::kCcw90Degrees,
      // If |vert_index| = 2, then the verts have been rotated by 180 degrees (top-left is now
      // bottom-right).
      Orientation::kCcw180Degrees,
      // If |vert_index| = 3, then the verts have been rotated by 270 degrees (top-left is now
      // bottom-left).
      Orientation::kCcw270Degrees};

  const Orientation orientation = kIndexToOrientation[vert_index];
  const types::RotateFlip transform = types::RotateFlip::From(orientation, image_flip);

  // Grab the origin, extent and orientation of the rectangle.
  auto origin = reordered_verts[0];
  auto extent = reordered_verts[2] - reordered_verts[0];
  FX_CHECK(extent.x >= 0.f && extent.y >= 0.f);

  // Now clip the origin and extent based on the clip rectangle.
  auto [clipped_origin, clipped_extent] = ClipRectangle(clip, origin, extent);

  if (origin == clipped_origin && extent == clipped_extent) {
    // If no clipping happened, we can leave the source rect as is and return.
    const types::RectangleF clipped_dest({
        .x = clipped_origin.x,
        .y = clipped_origin.y,
        .width = clipped_extent.x,
        .height = clipped_extent.y,
    });
    return SrcToDest(src, clipped_dest, transform);
  }
  if (clipped_origin == glm::vec2(0) && clipped_extent == glm::vec2(0)) {
    // The entire rectangle is outside of the clip region.
    return SrcToDest(types::RectangleF({.x = 0.f, .y = 0.f, .width = 0.f, .height = 0.f}),
                     types::RectangleF({.x = 0.f, .y = 0.f, .width = 0.f, .height = 0.f}),
                     transform);
  }

  // The rectangle was clipped, so we also have to clip the source rectangle.
  const float x_lerp = glm::clamp((clipped_origin.x - origin.x) / extent.x, 0.f, 1.f);
  const float y_lerp = glm::clamp((clipped_origin.y - origin.y) / extent.y, 0.f, 1.f);
  const float w_lerp =
      glm::clamp((clipped_origin.x + clipped_extent.x - origin.x) / extent.x, 0.f, 1.f);
  const float h_lerp =
      glm::clamp((clipped_origin.y + clipped_extent.y - origin.y) / extent.y, 0.f, 1.f);

  // Map the dst-space clip ratios onto the source rect's axes.
  //
  // The clip ran in dst (screen) space, yielding four edge ratios. The source
  // sub-rectangle is the clipped dst mapped back through the leaf transform
  // (orientation + flip), so each ratio re-attaches to a source edge through that
  // transform, NOT one-to-one:
  //   * 0/180:  dst-x -> source-u, dst-y -> source-v   (axes aligned)
  //   * 90/270: dst-x -> source-v, dst-y -> source-u   (axes swapped)
  //   * flip mirrors which END of the chosen axis each ratio shrinks.
  // This is the same dependence the old four-corner path had (`rotated_u`/`rotated_v`
  // plus the `flip_idx` reorder), reduced to a per-axis edge assignment. The same
  // orientation + image_flip are combined into the stored RotateFlip above.
  float u_min_ratio = 0.f, u_max_ratio = 1.f;
  float v_min_ratio = 0.f, v_max_ratio = 1.f;

  switch (orientation) {
    case Orientation::kCcw0Degrees:
      u_min_ratio = x_lerp;
      u_max_ratio = w_lerp;
      v_min_ratio = y_lerp;
      v_max_ratio = h_lerp;
      break;
    case Orientation::kCcw90Degrees:
      u_min_ratio = 1.f - h_lerp;
      u_max_ratio = 1.f - y_lerp;
      v_min_ratio = x_lerp;
      v_max_ratio = w_lerp;
      break;
    case Orientation::kCcw180Degrees:
      u_min_ratio = 1.f - w_lerp;
      u_max_ratio = 1.f - x_lerp;
      v_min_ratio = 1.f - h_lerp;
      v_max_ratio = 1.f - y_lerp;
      break;
    case Orientation::kCcw270Degrees:
      u_min_ratio = y_lerp;
      u_max_ratio = h_lerp;
      v_min_ratio = 1.f - w_lerp;
      v_max_ratio = 1.f - x_lerp;
      break;
  }

  if (image_flip == fuchsia_ui_composition::ImageFlip::kLeftRight) {
    const float new_u_min = 1.f - u_max_ratio;
    const float new_u_max = 1.f - u_min_ratio;
    u_min_ratio = new_u_min;
    u_max_ratio = new_u_max;
  } else if (image_flip == fuchsia_ui_composition::ImageFlip::kUpDown) {
    const float new_v_min = 1.f - v_max_ratio;
    const float new_v_max = 1.f - v_min_ratio;
    v_min_ratio = new_v_min;
    v_max_ratio = new_v_max;
  }

  const types::RectangleF clipped_src({
      .x = src.x() + u_min_ratio * src.width(),
      .y = src.y() + v_min_ratio * src.height(),
      .width = (u_max_ratio - u_min_ratio) * src.width(),
      .height = (v_max_ratio - v_min_ratio) * src.height(),
  });

  const types::RectangleF clipped_dest({
      .x = clipped_origin.x,
      .y = clipped_origin.y,
      .width = clipped_extent.x,
      .height = clipped_extent.y,
  });

  return SrcToDest(clipped_src, clipped_dest, transform);
}

GlobalMatrixVector ComputeGlobalMatrices(
    const GlobalTopologyData::TopologyVector& global_topology,
    const GlobalTopologyData::ParentIndexVector& parent_indices,
    const UberStruct::InstanceMap& uber_structs) {
  GlobalMatrixVector output;
  ComputeGlobalMatrices(output, global_topology, parent_indices, uber_structs);
  return output;
}

void ComputeGlobalMatrices(GlobalMatrixVector& output,
                           const GlobalTopologyData::TopologyVector& global_topology,
                           const GlobalTopologyData::ParentIndexVector& parent_indices,
                           const UberStruct::InstanceMap& uber_structs) {
  TRACE_DURATION("gfx", "ComputeGlobalMatrices");

  output.clear();
  if (global_topology.empty()) {
    return;
  }

  output.reserve(global_topology.size());

  // The root entry's parent pointer points to itself, so special case it.
  const auto& root_handle = global_topology.front();
  const auto root_uber_struct_kv = uber_structs.find(root_handle.GetInstanceId());
  FX_DCHECK(root_uber_struct_kv != uber_structs.end());

  const auto root_matrix_kv = root_uber_struct_kv->second->local_matrices.find(root_handle);

  if (root_matrix_kv == root_uber_struct_kv->second->local_matrices.end()) {
    output.emplace_back(glm::mat3());
  } else {
    const auto& matrix = root_matrix_kv->second;
    output.emplace_back(matrix);
  }

  for (size_t i = 1; i < global_topology.size(); ++i) {
    const TransformHandle& handle = global_topology[i];
    const size_t parent_index = parent_indices[i];

    // Every entry in the global topology comes from an UberStruct.
    const auto uber_struct_kv = uber_structs.find(handle.GetInstanceId());
    FX_DCHECK(uber_struct_kv != uber_structs.end());

    const auto matrix_kv = uber_struct_kv->second->local_matrices.find(handle);

    if (matrix_kv == uber_struct_kv->second->local_matrices.end()) {
      // This is *definitely* safe because we reserve storage above, so there is no chance of
      // reallocation.  However, a close reading of the C++ spec requires this to be safe even
      // with reallocation.
      output.emplace_back(output[parent_index]);
    } else {
      // See comment above.  This is safe even without a close reading of the C++ spec, because the
      // argument is computed before `emplace_back()` is called.
      output.emplace_back(output[parent_index] * matrix_kv->second);
    }
  }
}

GlobalTransformClipRegionVector ComputeGlobalTransformClipRegions(
    const GlobalTopologyData::TopologyVector& global_topology,
    const GlobalTopologyData::ParentIndexVector& parent_indices,
    const GlobalMatrixVector& matrix_vector, const UberStruct::InstanceMap& uber_structs) {
  GlobalTransformClipRegionVector output;
  ComputeGlobalTransformClipRegions(output, global_topology, parent_indices, matrix_vector,
                                    uber_structs);
  return output;
}

void ComputeGlobalTransformClipRegions(GlobalTransformClipRegionVector& output,
                                       const GlobalTopologyData::TopologyVector& global_topology,
                                       const GlobalTopologyData::ParentIndexVector& parent_indices,
                                       const GlobalMatrixVector& matrix_vector,
                                       const UberStruct::InstanceMap& uber_structs) {
  TRACE_DURATION("gfx", "ComputeGlobalTransformClipRegions");
  FX_DCHECK(global_topology.size() == parent_indices.size());
  FX_DCHECK(global_topology.size() == matrix_vector.size());

  output.clear();
  if (global_topology.empty()) {
    return;
  }

  output.reserve(global_topology.size());

  // The root entry's parent pointer points to itself, so special case it.
  const auto& root_handle = global_topology.front();
  const auto root_uber_struct_kv = uber_structs.find(root_handle.GetInstanceId());
  FX_DCHECK(root_uber_struct_kv != uber_structs.end());

  const auto root_regions_kv = root_uber_struct_kv->second->local_clip_regions.find(root_handle);

  // Process the root separately from the rest of the tree.
  if (root_regions_kv == root_uber_struct_kv->second->local_clip_regions.end()) {
    output.emplace_back(kUnclippedRegion);
  } else {
    output.emplace_back(MatrixMultiplyRect(matrix_vector[0], root_regions_kv->second));
  }

  for (size_t i = 1; i < global_topology.size(); ++i) {
    const TransformHandle& handle = global_topology[i];
    const size_t parent_index = parent_indices[i];
    auto parent_clip = output[parent_index];

    // Every entry in the global topology comes from an UberStruct.
    const auto uber_stuct_kv = uber_structs.find(handle.GetInstanceId());
    FX_DCHECK(uber_stuct_kv != uber_structs.end());
    const auto regions_kv = uber_stuct_kv->second->local_clip_regions.find(handle);

    // A clip region is bounded to that of its parent region. If the current clip region
    // is empty, then it defaults to that of its parent. Otherwise, we must find the
    // intersection of the parent clip region and the current clip region, in the global
    // coordinate space.
    if (regions_kv == uber_stuct_kv->second->local_clip_regions.end()) {
      output.emplace_back(parent_clip);
    } else {
      // Calculate the global position of the current clip region.
      auto curr_clip = MatrixMultiplyRect(matrix_vector[i], regions_kv->second);

      // Calculate the intersection of the current clip with its parent.
      glm::vec2 curr_origin = {curr_clip.x(), curr_clip.y()};
      glm::vec2 curr_extent = {curr_clip.width(), curr_clip.height()};
      auto [clipped_origin, clipped_extent] = ClipRectangle(parent_clip, curr_origin, curr_extent);

      // Add the intersection to the global clip vector.
      output.emplace_back(TransformClipRegion({.x = static_cast<int>(clipped_origin.x),
                                               .y = static_cast<int>(clipped_origin.y),
                                               .width = static_cast<int>(clipped_extent.x),
                                               .height = static_cast<int>(clipped_extent.y)}));
    }
  }
}

GlobalHitRegionsMap ComputeGlobalHitRegions(
    const GlobalTopologyData::TopologyVector& global_topology,
    const GlobalTopologyData::ParentIndexVector& parent_indices,
    const GlobalMatrixVector& matrix_vector, const UberStruct::InstanceMap& uber_structs) {
  TRACE_DURATION("gfx", "ComputeGlobalHitRegions");
  FX_DCHECK(global_topology.size() == parent_indices.size());
  FX_DCHECK(global_topology.size() == matrix_vector.size());

  GlobalHitRegionsMap global_hit_regions;

  for (size_t i = 0; i < global_topology.size(); ++i) {
    const TransformHandle& handle = global_topology[i];

    // Every entry in the global topology comes from an UberStruct.
    const auto uber_struct_kv = uber_structs.find(handle.GetInstanceId());
    FX_DCHECK(uber_struct_kv != uber_structs.end());

    const auto& local_hit_regions_map = uber_struct_kv->second->local_hit_regions_map;
    const auto regions_vec_kv = local_hit_regions_map.find(handle);

    if (regions_vec_kv != local_hit_regions_map.end()) {
      auto& hit_regions = global_hit_regions[handle];
      hit_regions.reserve(regions_vec_kv->second.size());
      for (const auto& local_hit_region : regions_vec_kv->second) {
        if (local_hit_region.is_finite()) {
          // Usually: calculate the global position of the current hit region.
          auto global_rect = MatrixMultiplyRectF(matrix_vector[i], local_hit_region.region());
          hit_regions.emplace_back(global_rect, local_hit_region.interaction());
        } else {
          // Special case: preserve sentinel value for infinite hit region.
          hit_regions.push_back(local_hit_region);
        }
      }
    }
  }

  return global_hit_regions;
}

}  // namespace flatland
