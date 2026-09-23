// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_FLATLAND_GLOBAL_RESOLVED_LAYERS_H_
#define SRC_UI_SCENIC_LIB_FLATLAND_GLOBAL_RESOLVED_LAYERS_H_

#include <vector>

#include "src/ui/scenic/lib/allocation/image_metadata.h"
#include "src/ui/scenic/lib/flatland/flatland_types.h"
#include "src/ui/scenic/lib/flatland/global_topology_data.h"
#include "src/ui/scenic/lib/flatland/uber_struct.h"

#include <glm/mat3x3.hpp>

namespace flatland {

// The list of global opacity values for a particular global topology.  Each entry is the
// global opacity value (i.e. relative to the root TransformHandle) of the transform in the
// corresponding position of the `topology_vector` supplied to `ComputeGlobalOpacityValues()`.
using GlobalOpacityVector = std::vector<float>;

// Computes a list of global opacity values for the global topology.
GlobalOpacityVector ComputeGlobalOpacityValues(
    const GlobalTopologyData::TopologyVector& global_topology,
    const GlobalTopologyData::ParentIndexVector& parent_indices,
    const UberStruct::InstanceMap& uber_structs);
void ComputeGlobalOpacityValues(GlobalOpacityVector& output,
                                const GlobalTopologyData::TopologyVector& global_topology,
                                const GlobalTopologyData::ParentIndexVector& parent_indices,
                                const UberStruct::InstanceMap& uber_structs);

// Computes the resolved layers list for the global topology.
// Walks |topology| in DFS order; for each node whose UberStruct has a
// layer_stacks entry, emits one ResolvedLayer per visible stack layer.
void ComputeGlobalResolvedLayers(std::vector<ResolvedLayer>& output,
                                 const GlobalTopologyData& topology,
                                 const UberStruct::InstanceMap& snapshot,
                                 const std::vector<glm::mat3>& global_matrices,
                                 const std::vector<TransformClipRegion>& clip_regions,
                                 const GlobalOpacityVector& inherited_opacities);

// Helper which returns a new vector instead of taking the output vector as an argument.
inline std::vector<ResolvedLayer> ComputeGlobalResolvedLayers(
    const GlobalTopologyData& topology, const UberStruct::InstanceMap& snapshot,
    const std::vector<glm::mat3>& global_matrices,
    const std::vector<TransformClipRegion>& clip_regions,
    const GlobalOpacityVector& inherited_opacities) {
  std::vector<ResolvedLayer> output;
  ComputeGlobalResolvedLayers(output, topology, snapshot, global_matrices, clip_regions,
                              inherited_opacities);
  return output;
}

// Simple culling algorithm that checks if any of the input rectangles cover the entire display,
// and if so, culls all rectangles that came before them (since rectangles are implicitly sorted
// according to depth, with the first entry being the furthest back, this has the effect of
// eliminating all rectangles behind the full-screen one). Also culls any rectangle that has
// no size (width is zero, or height is zero).
void CullLayersInPlace(std::vector<flatland::ResolvedLayer>* layers_in_out, uint64_t display_width,
                       uint64_t display_height);

// Exposed for testing. Inverts RotateFlip::From(Orientation, ImageFlip).
std::pair<fuchsia_ui_composition::Orientation, fuchsia_ui_composition::ImageFlip>
DecomposeRotateFlip(types::RotateFlip rf);

// Exposed for testing.
glm::mat3 GetLayerLocalMatrix(const types::Rectangle& display_rect,
                              fuchsia_ui_composition::Orientation orientation);

// Exposed for testing. Return type for `ResolveBlendAndOpacity()` helper.
struct ResolvedBlend {
  types::BlendMode blend_mode;
  std::array<float, 4> multiply_color;
};

// Exposed for testing. Encapsulates the difference between how Flatland1 and Flatland2 APIs treat
// REPLACE blend mode when `opacity < 1`; `pin_replace` is the selector for this differing behavior.
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
                                     bool pin_replace);

}  // namespace flatland

#endif  // SRC_UI_SCENIC_LIB_FLATLAND_GLOBAL_RESOLVED_LAYERS_H_
