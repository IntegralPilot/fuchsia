// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_FLATLAND_GLOBAL_MATRIX_DATA_H_
#define SRC_UI_SCENIC_LIB_FLATLAND_GLOBAL_MATRIX_DATA_H_

#include "src/ui/scenic/lib/flatland/flatland_types.h"
#include "src/ui/scenic/lib/flatland/global_topology_data.h"
#include "src/ui/scenic/lib/flatland/transform_handle.h"
#include "src/ui/scenic/lib/flatland/uber_struct.h"

namespace flatland {

// The list of global matrices for a particular global topology. Each entry is the global matrix
// (i.e. relative to the root TransformHandle) of the transform in the corresponding position of
// the |topology_vector| supplied to ComputeGlobalMatrices().
using GlobalMatrixVector = std::vector<glm::mat3>;

// The list of global transform clip regions for a particular global topology.
using GlobalTransformClipRegionVector = std::vector<TransformClipRegion>;

// The set of per-transform hit regions for a particular global topology.
using GlobalHitRegionsMap = std::unordered_map<TransformHandle, std::vector<flatland::HitRegion>>;

const extern TransformClipRegion kUnclippedRegion;

// Computes the global transform matrix for each transform in |global_topology| using the local
// matrices in the |uber_structs|. If a transform doesn't have a local matrix present in the
// appropriate UberStruct, this function assumes that transform's local matrix is the identity
// matrix.
GlobalMatrixVector ComputeGlobalMatrices(
    const GlobalTopologyData::TopologyVector& global_topology,
    const GlobalTopologyData::ParentIndexVector& parent_indices,
    const UberStruct::InstanceMap& uber_structs);
void ComputeGlobalMatrices(GlobalMatrixVector& output,
                           const GlobalTopologyData::TopologyVector& global_topology,
                           const GlobalTopologyData::ParentIndexVector& parent_indices,
                           const UberStruct::InstanceMap& uber_structs);

// Gathers the clip regions for each transform in |global_topology| using the local clip regions in
// the |uber_structs|.  If a transform doesn't have clip regions present in the appropriate
// UberStruct, this function assumes the region is null.  Since clip regions are specified in the
// local space of the transform they are attached to, this function transforms those into global
// clip regions before returning them.  This requires the global matrix vector to be passed along as
// a parameter.
GlobalTransformClipRegionVector ComputeGlobalTransformClipRegions(
    const GlobalTopologyData::TopologyVector& global_topology,
    const GlobalTopologyData::ParentIndexVector& parent_indices,
    const GlobalMatrixVector& matrix_vector, const UberStruct::InstanceMap& uber_structs);
void ComputeGlobalTransformClipRegions(GlobalTransformClipRegionVector& output,
                                       const GlobalTopologyData::TopologyVector& global_topology,
                                       const GlobalTopologyData::ParentIndexVector& parent_indices,
                                       const GlobalMatrixVector& matrix_vector,
                                       const UberStruct::InstanceMap& uber_structs);

// Aggregates the set of local hit regions for each transform in |global_topology| into a map of
// global hit regions. This process involves two steps: first, convert all hit regions which are
// in each transform's local space into world space, and then clip the hit regions to the
// transform's clip region.
GlobalHitRegionsMap ComputeGlobalHitRegions(
    const GlobalTopologyData::TopologyVector& global_topology,
    const GlobalTopologyData::ParentIndexVector& parent_indices,
    const GlobalMatrixVector& matrix_vector, const UberStruct::InstanceMap& uber_structs);

// Constructs a SrcToDest by projecting the destination (display) rectangle into screen space
// using the given matrix and clipping it to the provided clip region.  The source (texture) region
// is clipped proportionally, using the flip and orientation properties to map the screen-space clip
// boundaries back to the correct source axes and edges.
SrcToDest CreateSrcToDest(const glm::mat3& matrix, const TransformClipRegion& clip,
                          const types::RectangleF& src,
                          const fuchsia_ui_composition::ImageFlip image_flip);
}  // namespace flatland

#endif  // SRC_UI_SCENIC_LIB_FLATLAND_GLOBAL_MATRIX_DATA_H_
