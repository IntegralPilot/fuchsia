// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/flatland/global_topology_data.h"

#include <fuchsia/math/cpp/fidl.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/trace/event.h>

#include <optional>

#include "src/ui/scenic/lib/flatland/flatland_types.h"
#include "src/ui/scenic/lib/utils/helpers.h"
#include "src/ui/scenic/lib/utils/logging.h"

#include <glm/gtc/type_ptr.hpp>

using flatland::GlobalTopologyData;
using flatland::TransformClipRegion;
using flatland::TransformHandle;
using flatland::UberStruct;

namespace {

using ViewRefMap =
    std::unordered_map<flatland::TransformHandle, std::shared_ptr<const flatland::ViewRef>>;

struct pair_hash {
  size_t operator()(const std::pair<flatland::TransformHandle, uint64_t>& p) const noexcept {
    return std::hash<flatland::TransformHandle>{}(p.first) ^ std::hash<uint64_t>{}(p.second);
  }
};

std::optional<zx_koid_t> GetViewRefKoid(const flatland::TransformHandle& handle,
                                        const ViewRefMap& view_ref_map) {
  const auto kv = view_ref_map.find(handle);
  if (kv == view_ref_map.end() || kv->second == nullptr) {
    return std::nullopt;
  }

  return kv->second->koid();
}

std::optional<size_t> GetViewRefIndex(zx_koid_t view_ref_koid,
                                      const std::vector<flatland::TransformHandle>& transforms,
                                      const ViewRefMap& view_refs) {
  TRACE_DURATION("gfx", "flatland::HitTest[GetViewRefIndex]");
  for (const auto& [transform, view_ref] : view_refs) {
    if (view_ref != nullptr && view_ref_koid == view_ref->koid()) {
      // Found |view_ref_koid|, now calculate the index of its root transform in |transforms|.
      for (size_t start = 0; start < transforms.size(); ++start) {
        if (transforms[start] == transform) {
          return start;
        }
      }
      FX_DCHECK(false) << "view ref root transform not in transforms vector";
    }
  }

  // |view_ref_koid| was not found.
  return std::nullopt;
}

// Returns the last index (exclusive) of the subtree rooted at |start|.
//
// Prerequisite: |start| was returned from `GetViewRefIndex()`
size_t GetSubtreeEndIndex(size_t start, const std::vector<flatland::TransformHandle>& transforms,
                          const std::vector<size_t>& parent_indices) {
  TRACE_DURATION("gfx", "flatland::HitTest[GetSubtreeEndIndex]");
  FX_DCHECK(start < transforms.size()) << "precondition";

  // We need to be careful about the case where start == 0, since in that case hitting the global
  // root and hitting start are identical. It is simpler to handle this case explicitly, and then in
  // the loop below we have this additional guarantee.
  if (start == 0) {
    return transforms.size();
  }

  // |end| is an exclusive index.
  size_t end = start + 1;

  // This is an O(n logn) op. We can make it O(n) if performance needs dictate.
  while (end < transforms.size()) {
    // Do an ancestor check to see if the current transform is a descendant of |start_node|.
    size_t cur_idx = end;
    while (cur_idx != start && cur_idx != 0) {
      cur_idx = parent_indices[cur_idx];
    }

    // We have ran up the ancestor tree until we hit the root of the entire tree - exit.
    if (cur_idx == 0) {
      break;
    }

    ++end;
  }

  return end;
}

// Converts a 3x3 (2D) matrix to its 4x4 (3D) analog.
// xx, xy, yx, yy represent scale/rotation, T for translation.
//
// xx xy Tx
// yx yy TY
// 00 00 01
//
// ...becomes...
//
// xx xy 00 TX
// yx yy 00 TY
// 00 00 01 00
// 00 00 00 01
glm::mat4 Convert2DTransformTo3D(glm::mat3 in_matrix) {
  // Construct identity matrix.
  glm::mat4 out_matrix(1.f);
  // Assign the rotation and scale values to the 2x2 submatrix.
  out_matrix[0][0] = in_matrix[0][0];
  out_matrix[0][1] = in_matrix[0][1];
  out_matrix[1][0] = in_matrix[1][0];
  out_matrix[1][1] = in_matrix[1][1];
  // Assign the translation values to the final column.
  out_matrix[3][0] = in_matrix[2][0];
  out_matrix[3][1] = in_matrix[2][1];

  return out_matrix;
}

// Wrapper function to check against implementation's sentinel value for infinite regions.
bool HitRegionContainsPoint(const flatland::HitRegion& region, float x, float y) {
  if (!region.is_finite()) {
    return true;
  }
  return region.region().Contains({.x = x, .y = y});
}

// Easier-to-read input data to HitTest() below.
struct HitTestingData {
  const GlobalTopologyData::TopologyVector transforms;
  const GlobalTopologyData::ParentIndexVector parent_indices;
  const std::vector<flatland::TransformHandle> root_transforms;
  const ViewRefMap view_refs;
  const flatland::HitRegions hit_regions;
  const std::vector<flatland::TransformClipRegion> global_clip_regions;
};

view_tree::SubtreeHitTestResult HitTest(const HitTestingData& data, zx_koid_t start_node,
                                        glm::vec2 world_point, bool is_semantic_hit_test) {
  TRACE_DURATION("gfx", "flatland::HitTest");
  const auto& [transforms, parent_indices, root_transforms, view_refs, hit_regions,
               global_clip_regions] = data;
  FX_DCHECK(transforms.size() == parent_indices.size());
  FX_DCHECK(transforms.size() == global_clip_regions.size());
  FX_DCHECK(transforms.size() == root_transforms.size());

  size_t start = 0, end = 0;
  if (auto result = GetViewRefIndex(start_node, transforms, view_refs)) {
    start = *result;
    end = GetSubtreeEndIndex(start, transforms, parent_indices);
  } else {
    // Start node not in view tree.
    return view_tree::SubtreeHitTestResult{};
  }

  FX_DCHECK(0 <= start && start < end && end <= transforms.size());

  const float x = world_point[0];
  const float y = world_point[1];

  std::vector<zx_koid_t> hits = {};
  hits.reserve(end - start);

  for (size_t i = start; i < end; ++i) {
    const auto& transform = transforms[i];
    const auto& root_transform = root_transforms[i];

    const auto clip_region = types::RectangleF::From(global_clip_regions[i]);

    // Skip anonymous views.
    if (const auto local_root = view_refs.find(root_transform);
        local_root != view_refs.end() && local_root->second != nullptr) {
      const auto& view_ref_koid = local_root->second->koid();
      // Skip views without hit regions.
      if (const auto hit_region_vec = hit_regions.find(transform);
          hit_region_vec != hit_regions.end()) {
        for (const auto& region : hit_region_vec->second) {
          const bool semantically_invisible =
              region.interaction() ==
              fuchsia_ui_composition::HitTestInteraction::kSemanticallyInvisible;

          // Deliver a hit in all cases except for when it is a semantic hit test and the region
          // is semantically invisible.
          if (is_semantic_hit_test && semantically_invisible) {
            continue;
          }

          // Instead of clipping the hit region with the clip region, simply check if the hit
          // point is in both.
          if (HitRegionContainsPoint(region, x, y) && clip_region.Contains({.x = x, .y = y})) {
            hits.push_back(view_ref_koid);
            break;
          }
        }
      }
    }
  }

  std::reverse(hits.begin(), hits.end());
  return view_tree::SubtreeHitTestResult{.hits = std::move(hits)};
}

// Returns whether the transform at |index| has an anonymous ancestor.
bool HasAnonymousAncestor(const size_t index, const size_t root_index, const ViewRefMap& view_refs,
                          const GlobalTopologyData::TopologyVector& topology_vector,
                          const GlobalTopologyData::ParentIndexVector& parent_indices) {
  if (index == root_index) {
    return false;
  }

  size_t parent_index = parent_indices[index];
  while (parent_index != root_index) {
    // A transform that has an entry in the ViewRefMap is a view, but a nullptr entry is an
    // anonymous view.
    const auto parent_transform_handle = topology_vector[parent_index];
    const auto it = view_refs.find(parent_transform_handle);
    if (it != view_refs.end() && it->second == nullptr) {
      return true;
    }

    parent_index = parent_indices[parent_index];
  }

  return false;
}

// Returns the index and ViewRef koid first node in the topology with a ViewRef set.
// If none is found it returns std::nullopt, indicating an empty ViewTree.
std::optional<std::pair<size_t, zx_koid_t>> FindRoot(
    const GlobalTopologyData::TopologyVector& topology_vector, const ViewRefMap& view_refs) {
  for (size_t index = 0; index < topology_vector.size(); ++index) {
    // TODO(https://fxbug.dev/42060740): Make sure the root view is not anonymous?
    if (const auto koid = GetViewRefKoid(topology_vector[index], view_refs)) {
      return std::make_pair(index, *koid);
    }
  }

  return std::nullopt;
}

// Finds the parent of the node at |index| by looking upwards until a View is found.
// Returns ZX_KOID INVALID if no valid parent is found. (The root has no parent)
zx_koid_t FindParentView(const size_t index, const zx_koid_t view_ref_koid, const zx_koid_t root,
                         const GlobalTopologyData::TopologyVector& topology_vector,
                         const GlobalTopologyData::ParentIndexVector& parent_indices,
                         const ViewRefMap& view_refs) {
  zx_koid_t parent_koid = ZX_KOID_INVALID;
  if (view_ref_koid != root) {
    size_t parent_index = parent_indices[index];
    while (!view_refs.contains(topology_vector[parent_index])) {
      parent_index = parent_indices[parent_index];
    }
    parent_koid = GetViewRefKoid(topology_vector[parent_index], view_refs).value();
  }
  return parent_koid;
}

// Returns the bounding box of |transform_handle| by findings the clip regions specified by its
// View's parent.
//
// See //docs/development/graphics/scenic/concepts/view_bounds.md for conceptual documentation on
// how view bounds, null/infinitely small bounds, and hit-testing interact.
view_tree::BoundingBox ComputeBoundingBox(
    const TransformHandle transform_handle, const UberStruct::InstanceMap& uber_structs,
    const std::unordered_map<TransformHandle, TransformHandle>& child_to_parent_transform_map) {
  constexpr view_tree::BoundingBox kEmptyBox{.min = {0, 0}, .max = {0, 0}};

  // LLM Style Note: the previous version of this code used nested if-statements, more difficult for
  // the human eye to parse than current "flattened" style.  Unless tradeoffs dictate otherwise,
  // this style is preferred.

  // We only need the global clip region from the immediate parent, because it is already the
  // intersection with all parent clip regions.
  const auto it = child_to_parent_transform_map.find(transform_handle);
  if (it == child_to_parent_transform_map.end()) {
    return kEmptyBox;
  }
  const TransformHandle parent_transform_handle = it->second;

  const auto uber_struct_it = uber_structs.find(parent_transform_handle.GetInstanceId());
  if (uber_struct_it == uber_structs.end()) {
    return kEmptyBox;
  }
  const auto& local_clip_regions = uber_struct_it->second->local_clip_regions;

  const auto parent_clip_region_it = local_clip_regions.find(parent_transform_handle);
  if (parent_clip_region_it == local_clip_regions.end()) {
    return kEmptyBox;
  }

  // TODO(https://fxbug.dev/465563593): There might be a bug here.  fxr/643755 is the first CL
  // to compute the bounding box; previously it was hardcoded to:
  //      const auto full_screen_bounding_box = view_tree::BoundingBox{
  //          .min = {0, 0},
  //          .max = {display_width, display_height},
  //
  // Ever since fxr/643755, the clip extent has been computed dynamically, but the origin has
  // always been assumed to be (0,0).  Through many refactorings, the general approach has not
  // changed.
  //
  // We've never noticed this to cause problems, but we might just be getting lucky.  Certainly,
  // it seems e.g. a tiling WM should have child views whose global clip regions have a non-zero
  // origin.  Until we can investigate this properly, this DCHECK will serve as a canary.
  const types::Rectangle& rect = parent_clip_region_it->second;
  FX_DCHECK(rect.origin() == types::Point2({.x = 0, .y = 0}));

  return {.min = {0, 0},
          .max = {static_cast<float>(rect.width()), static_cast<float>(rect.height())}};
}

// Return value struct for ComputeViewTree().
struct ViewTreeData {
  std::unordered_map<zx_koid_t, view_tree::ViewNode> view_tree;
  std::unordered_set<TransformHandle> implicitly_anonymous_views;
};

// Computes and returns the ViewTree plus a list of implicit anonymous views (named views that are
// part of an anonymous subtree) based on GlobalTopologyData.
ViewTreeData ComputeViewTree(const zx_koid_t root, const size_t root_index,
                             const GlobalTopologyData::TopologyVector& topology_vector,
                             const GlobalTopologyData::ParentIndexVector& parent_indices,
                             const ViewRefMap& view_refs,
                             const UberStruct::InstanceMap& uber_structs,
                             const std::vector<glm::mat3>& global_matrix_vector,
                             const std::unordered_map<TransformHandle, TransformHandle>&
                                 link_child_to_parent_transform_map) {
  TRACE_DURATION("gfx", "flatland::ComputeViewTree");
  ViewTreeData output;
  for (size_t i = root_index; i < topology_vector.size(); ++i) {
    const auto& transform_handle = topology_vector.at(i);
    const auto view_ref_it = view_refs.find(transform_handle);
    // Transforms without ViewRefs are not Views and can be skipped.
    if (view_ref_it == view_refs.end()) {
      continue;
    }

    const auto& view_ref = view_ref_it->second;
    // Anonymous views can be skipped.
    if (view_ref == nullptr) {
      continue;
    }

    // If any node in the ancestor chain is anonymous then the View should marked "unconnected".
    if (HasAnonymousAncestor(i, root_index, view_refs, topology_vector, parent_indices)) {
      output.implicitly_anonymous_views.emplace(transform_handle);
      continue;
    }

    std::string debug_name;
    if (const auto uber_it = uber_structs.find(transform_handle.GetInstanceId());
        uber_it != uber_structs.end()) {
      debug_name = uber_it->second->debug_name;
    }

    const zx_koid_t parent_koid =
        FindParentView(i, view_ref->koid(), root, topology_vector, parent_indices, view_refs);
    const view_tree::BoundingBox bounding_box =
        ComputeBoundingBox(transform_handle, uber_structs, link_child_to_parent_transform_map);

    output.view_tree.emplace(
        view_ref->koid(), view_tree::ViewNode{.parent = parent_koid,
                                              .bounding_box = bounding_box,
                                              .local_from_world_transform = glm::inverse(
                                                  Convert2DTransformTo3D(global_matrix_vector[i])),
                                              .view_ref = view_ref,
                                              .debug_name = debug_name});
  }

  // Fill in the children by deriving it from the parents of each node.
  for (const auto& [koid, view_node] : output.view_tree) {
    if (view_node.parent != ZX_KOID_INVALID) {
      output.view_tree.at(view_node.parent).children.emplace(koid);
    }
  }

  return output;
}

}  // namespace

namespace flatland {

void GlobalTopologyData::Clear() {
  TRACE_DURATION("gfx", "GlobalTopologyVector::Clear");
  {
    TRACE_DURATION("gfx", "GlobalTopologyVector::Clear[vectors]");
    topology_vector.clear();
    parent_indices.clear();
  }
}

bool GlobalTopologyData::IsCleared() const {
  return topology_vector.empty() && parent_indices.empty();
}

// static
GlobalTopologyData GlobalTopologyData::ComputeGlobalTopologyData(
    const UberStruct::InstanceMap& uber_structs, const LinkTopologyMap& links,
    TransformHandle::InstanceId link_instance_id, TransformHandle root) {
  GlobalTopologyData output;
  ComputeGlobalTopologyData(output, uber_structs, links, link_instance_id, root);
  return output;
}

// static
void GlobalTopologyData::ComputeGlobalTopologyData(GlobalTopologyData& output,
                                                   const UberStruct::InstanceMap& uber_structs,
                                                   const LinkTopologyMap& links,
                                                   TransformHandle::InstanceId link_instance_id,
                                                   TransformHandle root) {
  TRACE_DURATION("gfx", "flatland::ComputeGlobalTopologyData");
  FX_DCHECK(output.IsCleared());
  // There should never be an UberStruct for the |link_instance_id|.
  FX_DCHECK(!uber_structs.contains(link_instance_id));

#ifdef USE_FLATLAND_VERBOSE_LOGGING
  {
    std::ostringstream str;
    str << "ComputeGlobalTopologyData(): Dumping UberStructs:\n";
    for (auto& kv : uber_structs) {
      str << *kv.second << "...................\n";
    }
    FLATLAND_VERBOSE_LOG << str.str();
  }
#endif

  // This is a stack of vector "iterators". We store the raw index, instead of an iterator, so that
  // we can do index comparisons.
  std::vector<std::pair<const TransformGraph::TopologyVector&, /*local_index=*/size_t>>
      vector_stack;
  // This is a stack of global parent indices and the number of children left to process for that
  // parent.
  struct ParentChildIterator {
    size_t parent_index = 0;
    size_t children_left = 0;
  };
  std::vector<ParentChildIterator> parent_counts;

  auto& [topology_vector, parent_indices] = output;

  // If we don't have the root in the map, the topology will be empty.
  const auto root_uber_struct_kv = uber_structs.find(root.GetInstanceId());
  if (root_uber_struct_kv != uber_structs.cend()) {
    vector_stack.emplace_back(root_uber_struct_kv->second->local_topology, 0);
  }

  // Helper lambda to resolve a link handle (viewport) or skip it if broken.
  // Resolving a link pushes its target session's subtree local topology vector onto
  // `vector_stack` to continue DFS traversal. Both outcomes skip the rest of the
  // outer loop body, modifying the traversal stack and iterator state directly.
  auto resolve_or_skip_link = [&links, &uber_structs, &parent_counts,
                               &vector_stack](const TransformHandle& link_handle) {
    // NOTE: there are a number of cases below where the link is skipped:
    //   - the link doesn't exist
    //   - the link exists, but there is no corresponding UberStruct
    //   - the link exists and there is an UberStruct, but the UberStruct root doesn't match
    //
    // In all of these cases, special care must be taken when the skipped link was the parent's last
    // expected child (children_left == 0), because there are no subsequent siblings to trigger the
    // standard pop cleanup.  We must manually pop the parent here to prevent it from being stranded
    // on the stack, which would fail the final DFS validation checks.

    // Skip the link if it doesn't exist.
    const auto link_kv = links.find(link_handle);
    if (link_kv == links.end()) {
      FLATLAND_VERBOSE_LOG << "GlobalTopologyData link doesn't exist for handle=" << link_handle
                           << ", skipping ";

      // No next sibling; must manually pop the parent (see comment above).
      if (parent_counts.back().children_left == 0) {
        parent_counts.pop_back();
      }
      return;
    }
    const TransformHandle& link_transform = link_kv->second;

    // Skip the link if its target UberStruct doesn't exist.
    const auto uber_struct_kv = uber_structs.find(link_transform.GetInstanceId());
    if (uber_struct_kv == uber_structs.end()) {
      FLATLAND_VERBOSE_LOG << "GlobalTopologyData link doesn't exist for instance_id="
                           << link_transform.GetInstanceId() << ", skipping";

      // No next sibling; must manually pop the parent (see comment above).
      if (parent_counts.back().children_left == 0) {
        parent_counts.pop_back();
      }
      return;
    }

    // Skip the link if its target UberStruct has a mismatched root handle (e.g. if the new
    // UberStruct hasn't been registered yet).
    const auto& new_vector = uber_struct_kv->second->local_topology;
    FX_DCHECK(!new_vector.empty()) << "Valid UberStructs cannot have empty local_topology";
    if (new_vector[0].handle != link_transform) {
      FLATLAND_VERBOSE_LOG << "GlobalTopologyData link mismatch with "
                              "existing UberStruct ("
                           << new_vector[0].handle << " vs. " << link_transform << "), skipping";

      // No next sibling; must manually pop the parent (see comment above).
      if (parent_counts.back().children_left == 0) {
        parent_counts.pop_back();
      }
      return;
    }

    // Thanks to one-view-per-session semantics, we should never cycle through the
    // topological vectors, so we don't need to handle cycles.  We DCHECK here just to be sure.
    FX_DCHECK(std::find_if(vector_stack.cbegin(), vector_stack.cend(), [&](const auto& entry) {
                return entry.first == new_vector;
              }) == vector_stack.cend());

    // The link is now resolved!  The resolved link actually resulted in a new nested
    // topology subtree.  We increment parent's children_left because this link now expands
    // into the child topology vector.
    ++parent_counts.back().children_left;

    vector_stack.emplace_back(new_vector, 0);
  };

  // Main DFS traversal loop to produce the flattened topological scene graph.
  while (!vector_stack.empty()) {
    auto& [vector, iterator_index] = vector_stack.back();

    // If we are finished with a vector, pop back to the previous vector.
    if (iterator_index >= vector.size()) {
      FX_DCHECK(iterator_index == vector.size());
      vector_stack.pop_back();
      continue;
    }

    const TransformGraph::TopologyEntry& current_entry = vector[iterator_index];

    FLATLAND_VERBOSE_LOG << "GlobalTopologyData processing current_entry=" << current_entry.handle
                         << "  child-count=" << current_entry.child_count;
    ++iterator_index;

    // Mark that a child has been processed for the latest parent.
    if (!parent_counts.empty()) {
      FLATLAND_VERBOSE_LOG << "      GlobalTopologyData       parent_counts size="
                           << parent_counts.size()
                           << "  parent=" << topology_vector[parent_counts.back().parent_index]
                           << "  remaining-children=" << parent_counts.back().children_left;

      FX_DCHECK(parent_counts.back().children_left > 0);
      --parent_counts.back().children_left;
    } else {
      // Only expect to see this at the root of the topology, I think.  Is this worth putting a
      // permanent check in?
      FLATLAND_VERBOSE_LOG << "GlobalTopologyData       no parent";
    }

    // If we are processing a link transform, resolve or skip it and move to the next entry.
    if (current_entry.handle.GetInstanceId() == link_instance_id) {
      resolve_or_skip_link(current_entry.handle);
      continue;
    }

    // Push the current transform and update the "iterator".
    const size_t new_parent_index = topology_vector.size();
    topology_vector.push_back(current_entry.handle);

    parent_indices.push_back(parent_counts.empty() ? 0 : parent_counts.back().parent_index);

    // If this entry was the last child for the previous parent, pop that off the stack.
    if (!parent_counts.empty() && parent_counts.back().children_left == 0) {
      parent_counts.pop_back();
    }

    // If this entry has children, push it onto the parent stack.
    if (current_entry.child_count != 0) {
      parent_counts.push_back({new_parent_index, current_entry.child_count});
    }
  }

// Validates that every child of every parent was processed. If the last handle processed was an
// unresolved link handle, its parent will be the only thing left on the stack with 0 children to
// avoid extra unnecessary cleanup logic.
#ifndef NDEBUG
  if (parent_counts.size() > 1 ||
      (parent_counts.size() == 1 && parent_counts.back().children_left != 0)) {
    std::ostringstream str;
    str << "Error while generating GlobalTopologyData (failed parent_counts validation)\n"
        << "Dumping parent_counts vector:\n";
    for (size_t i = 0; i < parent_counts.size(); ++i) {
      str << "i: " << i << "  index: " << parent_counts[i].parent_index
          << "  parent: " << topology_vector[parent_counts[i].parent_index]
          << "  child-count: " << parent_counts[i].children_left << std::endl;
    }
    FX_LOGS(FATAL) << str.str();
  }
  FX_CHECK(parent_counts.empty() ||
           (parent_counts.size() == 1 && parent_counts.back().children_left == 0));
#endif
}

std::unique_ptr<view_tree::SubtreeSnapshot> GlobalTopologyData::GenerateViewTreeSnapshot(
    const GlobalTopologyData& data, const UberStruct::InstanceMap& uber_structs,
    HitRegions hit_regions, std::vector<TransformClipRegion> global_clip_regions,
    const std::vector<glm::mat3>& global_matrix_vector,
    const std::unordered_map<TransformHandle, TransformHandle>&
        link_child_to_parent_transform_map) {
  TRACE_DURATION("gfx", "flatland::GenerateViewTreeSnapshot");

  // Compute the view_refs map on demand from uber_structs.
  ViewRefMap view_refs;
  view_refs.reserve(uber_structs.size());
  for (const auto& [_, uber_struct] : uber_structs) {
    if (!uber_struct->local_topology.empty()) {
      view_refs.emplace(uber_struct->local_topology[0].handle, uber_struct->view_ref);
    }
  }

  const auto root_values = FindRoot(data.topology_vector, view_refs);
  if (!root_values.has_value()) {
    // No root -> Empty ViewTree.
    return std::make_unique<view_tree::SubtreeSnapshot>();
  }

  auto output = std::make_unique<view_tree::SubtreeSnapshot>();
  auto& [root, view_tree, unconnected_views, hit_tester, _] = *output;

  const auto [root_index, root_koid] = root_values.value();
  root = root_koid;
  auto [view_tree_temp, implicitly_anonymous_views] =
      ComputeViewTree(root_koid, root_index, data.topology_vector, data.parent_indices, view_refs,
                      uber_structs, global_matrix_vector, link_child_to_parent_transform_map);
  view_tree = std::move(view_tree_temp);

  // Unconnected_views = all non-anonymous views (those with ViewRefs) not in the ViewTree.
  {
    TRACE_DURATION("gfx", "flatland::GenerateViewTreeSnapshot[unconnected_views]");
    for (const auto& [_, view_ref] : view_refs) {
      if (view_ref != nullptr) {
        if (!view_tree.contains(view_ref->koid())) {
          unconnected_views.emplace(view_ref->koid());
        }
      }
    }
  }

  // Copy all non-anonymous ViewRefs from the ViewRefMap.
  ViewRefMap named_view_refs;
  {
    TRACE_DURATION("gfx", "flatland::GenerateViewTreeSnapshot[named_views]");
    named_view_refs.reserve(view_refs.size());
    for (auto& [key, value] : view_refs) {
      if (!implicitly_anonymous_views.contains(key)) {
        named_view_refs.emplace(key, value);
      }
    }
  }

  // Compute the parallel vector of local root transforms for hit testing.
  std::vector<TransformHandle> root_transforms;
  root_transforms.reserve(data.topology_vector.size());
  for (const auto& transform : data.topology_vector) {
    const auto it = uber_structs.find(transform.GetInstanceId());
    FX_DCHECK(it != uber_structs.end());
    root_transforms.push_back(it->second->local_topology[0].handle);
  }

  // Note: The ViewTree represents a snapshot of the scene at a specific time. Because of this it's
  // important that it contains no references to live data. This means the hit testing closure must
  // contain only plain values or data with value semantics like shared_ptr<const>, to ensure that
  // it's safe to call from any thread.
  {
    TRACE_DURATION("gfx", "flatland::GenerateViewTreeSnapshot[hit_tester]");
    hit_tester = [hit_test_data = HitTestingData{
                      .transforms = data.topology_vector,
                      .parent_indices = data.parent_indices,
                      .root_transforms = std::move(root_transforms),
                      .view_refs = std::move(named_view_refs),
                      .hit_regions = std::move(hit_regions),
                      .global_clip_regions = std::move(global_clip_regions),
                  }](zx_koid_t start_node, glm::vec2 world_point, bool is_semantic_hit_test) {
      return HitTest(hit_test_data, start_node, world_point, is_semantic_hit_test);
    };
  }

  return output;
}

GlobalTopologyData::ChildCountVector GlobalTopologyData::ComputeChildCountVector() const {
  if (parent_indices.empty()) {
    return {};
  }
  ChildCountVector child_counts(parent_indices.size(), 0);
  // Start at index 1 because the root at index 0 has no parent to add children to.
  for (size_t i = 1; i < parent_indices.size(); ++i) {
    child_counts[parent_indices[i]]++;
  }
  return child_counts;
}

}  // namespace flatland
