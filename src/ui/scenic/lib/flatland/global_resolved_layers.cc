// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/flatland/global_resolved_layers.h"

#include <lib/syslog/cpp/macros.h>
#include <lib/trace/event.h>

#include <algorithm>
#include <cmath>

#include "src/ui/scenic/lib/flatland/global_matrix_data.h"

#include <glm/gtc/constants.hpp>
#include <glm/gtc/type_ptr.hpp>

namespace flatland {

void CullLayersInPlace(std::vector<flatland::ResolvedLayer>* layers_in_out, uint64_t display_width,
                       uint64_t display_height) {
  TRACE_DURATION("gfx", "CullLayersInPlace");
  FX_DCHECK(layers_in_out);
  auto is_occluder = [display_width, display_height](const flatland::ResolvedLayer& layer) -> bool {
    // Only cull if the rect is opaque.
    auto is_opaque = layer.blend_mode == flatland::BlendMode::kReplace();

    // If the rect is full screen (or larger), and opaque, clear the output vectors.
    return (is_opaque && layer.geometry.dest.x() <= 0 && layer.geometry.dest.y() <= 0 &&
            layer.geometry.dest.width() >= static_cast<float>(display_width) &&
            layer.geometry.dest.height() >= static_cast<float>(display_height));
  };

  // Find the index of the last occluder.
  size_t occluder_index = 0;
  for (size_t i = 0; i < layers_in_out->size(); i++) {
    if (is_occluder((*layers_in_out)[i])) {
      occluder_index = i;
    }
  }

  // Move all of the remaining renderable data into the output vectors. Entries get erased
  // if they occur before the last occluder index, or if the geometry at that entry is empty.
  const auto is_geometry_empty = [](const flatland::SrcToDest& geometry) {
    return geometry.dest.width() <= 0.f || geometry.dest.height() <= 0.f;
  };

  layers_in_out->erase(
      std::remove_if(layers_in_out->begin(), layers_in_out->end(),
                     [index = static_cast<size_t>(0), occluder_index,
                      &is_geometry_empty](const flatland::ResolvedLayer& layer) mutable {
                       auto curr_index = index++;
                       return curr_index < occluder_index || is_geometry_empty(layer.geometry);
                     }),
      layers_in_out->end());
}

// Decomposes the internal 8-way RotateFlip into a FIDL (Orientation, ImageFlip) pair.  This is the
// inverse of `types::RotateFlip::From(orientation, flip)` in the sense that the orientation/flip
// obtained from `DecomposeRotateFlip()` can be passed to `types::RotateFlip::From()` to obtain the
// original `RotateFlip`.
//
// The display path (in display_compositor.cc) recomposes these components back
// into a single RotateFlip via types::RotateFlip::From(orientation, flip). This
// decomposition must invert it exactly (verified by DecomposeRotateFlipTest.InvertsRotateFlipFrom).
// Step 150 removes this split entirely by carrying the unified RotateFlip all the way to the leaf.
std::pair<fuchsia_ui_composition::Orientation, fuchsia_ui_composition::ImageFlip>
DecomposeRotateFlip(types::RotateFlip rf) {
  using fuchsia_ui_composition::ImageFlip;
  using fuchsia_ui_composition::Orientation;
  switch (rf.enum_value()) {
    case types::RotateFlip::Enum::kIdentity:
      return {Orientation::kCcw0Degrees, ImageFlip::kNone};
    case types::RotateFlip::Enum::kReflectX:
      return {Orientation::kCcw0Degrees, ImageFlip::kUpDown};
    case types::RotateFlip::Enum::kReflectY:
      return {Orientation::kCcw0Degrees, ImageFlip::kLeftRight};
    case types::RotateFlip::Enum::kRotateCcw180:
      return {Orientation::kCcw180Degrees, ImageFlip::kNone};
    case types::RotateFlip::Enum::kRotateCcw90:
      return {Orientation::kCcw90Degrees, ImageFlip::kNone};
    // NOTE: it might look like a mismatch between "reflect across X-axis" and "flip left/right".
    // However, the Flatland API applies image flip before rotation, whereas `RotateFlip` matches
    // the display coordinator convention of rotating before flipping.  This non-commutativity means
    // that the flip-axis must also be rotated, hence the apparent discrepancy.  The same applies to
    // `kRotateCcw90ReflectY`.
    case types::RotateFlip::Enum::kRotateCcw90ReflectX:
      return {Orientation::kCcw90Degrees, ImageFlip::kLeftRight};
    case types::RotateFlip::Enum::kRotateCcw90ReflectY:
      return {Orientation::kCcw90Degrees, ImageFlip::kUpDown};
    case types::RotateFlip::Enum::kRotateCcw270:
      return {Orientation::kCcw270Degrees, ImageFlip::kNone};
  }
  FX_NOTREACHED();
}

// Identical to Flatland::MatrixData::GetOrientationAngle (flatland.cc).  This copy will be deleted
// at step 160, which instead derives the rotation from a cached per-node decode. Angles are
// negative because in view-space coordinates (+y downward), a positive mathematical
// rotation is visually clockwise. Thus, CCW orientations require negative angles.
static float GetOrientationAngle(fuchsia_ui_composition::Orientation orientation) {
  using fuchsia_ui_composition::Orientation;
  switch (orientation) {
    case Orientation::kCcw0Degrees:
      return 0.f;
    case Orientation::kCcw90Degrees:
      return -glm::half_pi<float>();
    case Orientation::kCcw180Degrees:
      return -glm::pi<float>();
    case Orientation::kCcw270Degrees:
      return -glm::three_over_two_pi<float>();
  }
  FX_NOTREACHED();
}

// Adapted from Flatland::MatrixData::RecomputeMatrix() (flatland.cc).  This copy will be deleted
// at step 160, where the placement becomes closed-form.  Builds the layer's local placement matrix
// from the provided `display_rect` and `orientation`.
//   - translation: from `display_rect` origin
//   - rotation: from `orientation`
//   - scale: from `display_rect` width/height
//
// The result is equivalent to creating separate translation/rotation/scale matrices, and returning
// T*R*S.
//
// This matrix is composed with the node's global matrix and handed to CreateSrcToDest, which
// decodes it straight back into an (origin, extent, orientation) SrcToDest.  This redundant
// manufacture-then-decode round-trip is deliberate here, to temporarily reuse the shared legacy
// decode (until the step 160 cleanup).
glm::mat3 GetLayerLocalMatrix(const types::Rectangle& display_rect,
                              fuchsia_ui_composition::Orientation orientation) {
  // Manually compose the matrix rather than use glm transformations since the order of operations
  // is always the same. glm matrices are column-major, so are indexed like:
  //   0 3 6
  //   1 4 7
  //   2 5 8
  glm::mat3 result(glm::uninitialize);
  float* vals = static_cast<float*>(glm::value_ptr(result));

  // Translation in the third column.
  vals[6] = static_cast<float>(display_rect.x());
  vals[7] = static_cast<float>(display_rect.y());

  // Rotation and scale combined into the first two columns.
  const float angle = GetOrientationAngle(orientation);
  const float s = sin(angle);
  const float c = cos(angle);

  const float scale_x = static_cast<float>(display_rect.width());
  const float scale_y = static_cast<float>(display_rect.height());

  vals[0] = c * scale_x;
  vals[1] = s * scale_x;
  vals[3] = -1.f * s * scale_y;
  vals[4] = c * scale_y;

  // Bottom row is constant (0, 0, 1).
  vals[2] = 0.f;
  vals[5] = 0.f;
  vals[8] = 1.f;

  return result;
}

// Helper/adaptor which generates a layer's global matrix in order to pass it to the legacy
// `CreateSrcToDest()` helper (which will be deleted in step 160).
static std::optional<SrcToDest> ComputeClippedLayerGeometry(
    const glm::mat3& node_global_matrix, const TransformClipRegion& node_clip_region,
    const types::Rectangle& display_rect, fuchsia_ui_composition::Orientation orientation,
    fuchsia_ui_composition::ImageFlip flip, const types::RectangleF& unclipped_src) {
  glm::mat3 composed_matrix = node_global_matrix * GetLayerLocalMatrix(display_rect, orientation);
  SrcToDest clipped_geometry =
      CreateSrcToDest(composed_matrix, node_clip_region, unclipped_src, flip);
  if (clipped_geometry.dest.width() <= 0.f || clipped_geometry.dest.height() <= 0.f) {
    return std::nullopt;
  }
  return clipped_geometry;
}

// Encapsulates the difference between how Flatland1 and Flatland2 APIs treat REPLACE blend mode
// when `opacity < 1`; `pin_replace` is the selector for this differing behavior.
//
// In Flatland1 *for images only*, RGB is scaled and blend stays REPLACE (fade toward "black").
//
// Flatland2, and Flatland1 for solid color fills, "demote" REPLACE to PREMULTIPLIED_ALPHA,
// so there is no visual discontinuity at `opacity == 0` (where the layer is treated as invisible);
// this allows e.g. a window manager to fade out a child app even if it uses REPLACE.
//
// NOTE: `effective_opacity` combines layer opacity with inherited transform opacity.  It does not
// involve "content opacity", neither the alpha of a solid color fill, nor the alpha channel of
// image pixels.
// TODO(https://fxbug.dev/523371761): ratified DESIGN-blend_mode_and_opacity
// decision must match the behavior implemented here.
ResolvedBlend ResolveBlendAndOpacity(types::BlendMode stored_blend, float effective_opacity,
                                     bool pin_replace) {
  types::BlendMode blend_mode = stored_blend;
  if (blend_mode == types::BlendMode::kReplace() && effective_opacity < 1.f && !pin_replace) {
    blend_mode = types::BlendMode::kPremultipliedAlpha();
  }
  if (blend_mode == types::BlendMode::kStraightAlpha()) {
    return ResolvedBlend{
        .blend_mode = blend_mode,
        .multiply_color = {1.f, 1.f, 1.f, effective_opacity},
    };
  }
  return ResolvedBlend{
      .blend_mode = blend_mode,
      .multiply_color = {effective_opacity, effective_opacity, effective_opacity,
                         effective_opacity},
  };
}

void ComputeGlobalResolvedLayers(std::vector<ResolvedLayer>& output,
                                 const GlobalTopologyData& topology,
                                 const UberStruct::InstanceMap& snapshot,
                                 const std::vector<glm::mat3>& global_matrices,
                                 const std::vector<TransformClipRegion>& clip_regions,
                                 const GlobalOpacityVector& inherited_opacities) {
  TRACE_DURATION("gfx", "ComputeGlobalResolvedLayers");
  FX_DCHECK(topology.topology_vector.size() == global_matrices.size());
  FX_DCHECK(topology.topology_vector.size() == clip_regions.size());
  FX_DCHECK(topology.topology_vector.size() == inherited_opacities.size());

  output.clear();
  if (topology.topology_vector.empty()) {
    return;
  }

  // Note: after step 160 it will no longer be necessary to iterate through the entire topology
  // to find the layer stacks; they will be cached in the common case.
  for (size_t i = 0; i < topology.topology_vector.size(); ++i) {
    const TransformHandle& handle = topology.topology_vector[i];
    const glm::mat3& node_global_matrix = global_matrices[i];
    const TransformClipRegion& node_clip_region = clip_regions[i];
    const float inherited_opacity = inherited_opacities[i];
    if (inherited_opacity == 0.f) {
      // Invisible.
      continue;
    }

    auto uber_struct_kv = snapshot.find(handle.GetInstanceId());
    if (uber_struct_kv == snapshot.end()) {
      FX_DCHECK(false) << "no corresponding UberStruct for global topology entry: " << handle;
      continue;
    }
    const auto& uber_struct = uber_struct_kv->second;
    FX_CHECK(uber_struct->flatland_version == 1u || uber_struct->flatland_version == 2u)
        << "unknown UberStruct::flatland_version: " << uber_struct->flatland_version;

    auto layer_stack_it = uber_struct->layer_stacks.find(handle);
    if (layer_stack_it == uber_struct->layer_stacks.end()) {
      // Topology entry doesn't correspond to a layer stack.
      continue;
    }

    // Helper lambda to append to `output` a `ResolvedLayer` corresponding to an image layer,
    // or to skip it e.g. if completely clipped.
    auto process_image_layer = [inherited_opacity, &node_global_matrix, &node_clip_region, i,
                                &output, flatland_version = uber_struct->flatland_version](
                                   const UberStructLayer& layer) {
      // Skip invalid image.
      const auto& image = std::get<UberStructLayer::ImageModeProperties>(layer.content);
      if (image.image_id == allocation::kInvalidImageId) {
        return;
      }

      auto [orientation, flip] = DecomposeRotateFlip(image.transform);
      auto clipped_geometry = ComputeClippedLayerGeometry(node_global_matrix, node_clip_region,
                                                          layer.common.display_rect, orientation,
                                                          flip, image.sample_rect);
      if (!clipped_geometry) {
        return;
      }

      const auto [blend_mode, multiply_color] =
          ResolveBlendAndOpacity(layer.common.blend_mode, layer.common.opacity * inherited_opacity,
                                 /*pin_replace=*/flatland_version == 1);

      output.push_back(ResolvedLayer{
          .geometry = *clipped_geometry,
          .multiply_color = multiply_color,
          .blend_mode = blend_mode,
          .content =
              ResolvedLayer::ImageContent{
                  .image_id = image.image_id,
                  .width = image.image_width,
                  .height = image.image_height,
              },
          .topology_index = static_cast<int32_t>(i),
      });
    };

    // Helper lambda to append to `output` a `ResolvedLayer` corresponding to a solid color layer,
    // or to skip it e.g. if completely clipped.
    auto process_solid_color_layer = [inherited_opacity, &node_global_matrix, &node_clip_region, i,
                                      &output](const UberStructLayer& layer) {
      auto clipped_geometry = ComputeClippedLayerGeometry(
          node_global_matrix, node_clip_region, layer.common.display_rect,
          fuchsia_ui_composition::Orientation::kCcw0Degrees,
          fuchsia_ui_composition::ImageFlip::kNone, types::RectangleF());
      if (!clipped_geometry) {
        return;
      }

      // In the UberStructLayer, a solid's `color` is straight (non-premultiplied) RGBA.
      // However, downstream of here in the ResolvedLayer, the blend mode must match the
      // encoded content.  Because blending premultiplied content is slightly more efficient,
      // we adjust STRAIGHT_ALPHA to PREMULTIPLIED_ALPHA here; the corresponding adjustment
      // is made to `content_color` below.  NOTE: this optimization is only applicable to
      // solid color content, because image content pixels cannot be mutated analogously.
      const types::BlendMode normalized_blend =
          layer.common.blend_mode == types::BlendMode::kStraightAlpha()
              ? types::BlendMode::kPremultipliedAlpha()
              : layer.common.blend_mode;

      const auto [blend_mode, multiply_color] = ResolveBlendAndOpacity(
          normalized_blend, layer.common.opacity * inherited_opacity, /*pin_replace=*/false);

      // The blend mode computed above will not be STRAIGHT_ALPHA, so we need to compute
      // the premultiplied `content_color` from the straight-alpha color received from the
      // Flatland session.  See DESIGN-solid_fill_encoding (ratified).
      // TODO(https://fxbug.dev/523371761): the ratified DESIGN-blend_mode_and_opacity
      // decision must match the behavior implemented here.
      FX_DCHECK(blend_mode != types::BlendMode::kStraightAlpha());
      const auto& solid = std::get<UberStructLayer::SolidColorModeProperties>(layer.content);
      const float a = solid.color[3];
      const std::array<float, 4> content_color = {solid.color[0] * a, solid.color[1] * a,
                                                  solid.color[2] * a, a};

      output.push_back(ResolvedLayer{
          .geometry = *clipped_geometry,
          .multiply_color = multiply_color,
          .blend_mode = blend_mode,
          .content =
              ResolvedLayer::SolidColorContent{
                  .color = content_color,
              },
          .topology_index = static_cast<int32_t>(i),
      });
    };

    // For every layer in the stack, process it according to its content type, and (if the layer
    // isn't invisible for some reason) emit a `ResolvedLayer` into `output`.
    for (const auto& layer_handle : layer_stack_it->second) {
      auto layer_it = uber_struct->layers.find(layer_handle);
      FX_CHECK(layer_it != uber_struct->layers.end());
      const auto& layer = layer_it->second;

      if (layer.common.display_rect.width() <= 0 || layer.common.display_rect.height() <= 0 ||
          layer.common.opacity == 0.f) {
        // Invisible.
        continue;
      }

      if (std::holds_alternative<UberStructLayer::ImageModeProperties>(layer.content)) {
        process_image_layer(layer);
      } else if (std::holds_alternative<UberStructLayer::SolidColorModeProperties>(layer.content)) {
        process_solid_color_layer(layer);
      }
      static_assert(3 == std::variant_size_v<decltype(UberStructLayer::content)>,
                    "Must handle all UberStructLayer content types");
    }
  }
}

GlobalOpacityVector ComputeGlobalOpacityValues(
    const GlobalTopologyData::TopologyVector& global_topology,
    const GlobalTopologyData::ParentIndexVector& parent_indices,
    const UberStruct::InstanceMap& uber_structs) {
  GlobalOpacityVector output;
  ComputeGlobalOpacityValues(output, global_topology, parent_indices, uber_structs);
  return output;
}

void ComputeGlobalOpacityValues(GlobalOpacityVector& output,
                                const GlobalTopologyData::TopologyVector& global_topology,
                                const GlobalTopologyData::ParentIndexVector& parent_indices,
                                const UberStruct::InstanceMap& uber_structs) {
  TRACE_DURATION("gfx", "ComputeGlobalOpacityValues");
  FX_DCHECK(global_topology.size() == parent_indices.size());

  output.clear();
  if (global_topology.empty()) {
    return;
  }

  output.reserve(global_topology.size());

  // The root entry's parent pointer points to itself, so special case it.
  const auto& root_handle = global_topology.front();
  const auto root_uber_struct_kv = uber_structs.find(root_handle.GetInstanceId());
  FX_DCHECK(root_uber_struct_kv != uber_structs.end());

  const auto root_opacity_kv = root_uber_struct_kv->second->local_opacity_values.find(root_handle);
  if (root_opacity_kv == root_uber_struct_kv->second->local_opacity_values.end()) {
    output.emplace_back(1.f);
  } else {
    output.emplace_back(root_opacity_kv->second);
  }

  for (size_t i = 1; i < global_topology.size(); ++i) {
    const TransformHandle& handle = global_topology[i];
    const size_t parent_index = parent_indices[i];

    // Every entry in the global topology comes from an UberStruct.
    const auto uber_stuct_kv = uber_structs.find(handle.GetInstanceId());
    FX_DCHECK(uber_stuct_kv != uber_structs.end());

    const auto opacity_kv = uber_stuct_kv->second->local_opacity_values.find(handle);
    if (opacity_kv == uber_stuct_kv->second->local_opacity_values.end()) {
      output.emplace_back(output[parent_index]);
    } else {
      output.emplace_back(output[parent_index] * opacity_kv->second);
    }
  }
}

}  // namespace flatland
