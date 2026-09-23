// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/flatland/transform_graph.h"

#include <lib/syslog/cpp/macros.h>
#include <lib/trace/event.h>

#include <algorithm>

#include "src/ui/scenic/lib/utils/logging.h"

namespace flatland {

TransformGraph::TransformGraph() : TransformGraph(0) {}

TransformGraph::TransformGraph(TransformHandle::InstanceId instance_id)
    : instance_id_(instance_id) {}

TransformHandle TransformGraph::CreateTransform() {
  FX_DCHECK(is_valid_);
  TransformHandle retval(instance_id_, next_transform_id_++);
  FX_DCHECK(!working_set_.contains(retval));
  working_set_.insert(retval);
  live_set_.insert(retval);
  return retval;
}

bool TransformGraph::ReleaseTransform(TransformHandle handle) {
  FX_DCHECK(is_valid_);
  auto iter = working_set_.find(handle);
  if (iter == working_set_.end()) {
    return false;
  }

  working_set_.erase(iter);
  return true;
}

void TransformGraph::ClearChildren(TransformHandle parent) {
  FX_DCHECK(is_valid_);
  FX_DCHECK(working_set_.contains(parent));
  auto iter = children_.find(parent);
  if (iter != children_.end()) {
    iter->second.normal_children.clear();
    if (!iter->second.priority_child.has_value()) {
      children_.erase(iter);
    }
  }
}

bool TransformGraph::AddChild(TransformHandle parent, TransformHandle child) {
  FX_DCHECK(is_valid_);
  FX_DCHECK(working_set_.contains(parent));

  auto& node = children_[parent];
  auto& vec = node.normal_children;
  if (std::find(vec.begin(), vec.end(), child) != vec.end()) {
    FLATLAND_VERBOSE_LOG << "TransformGraph::AddChild() parent=" << parent << "  child=" << child
                         << "  failure!";
    return false;
  }

  vec.reserve(4);
  vec.push_back(child);
  FLATLAND_VERBOSE_LOG << "TransformGraph::AddChild() parent=" << parent << "  child=" << child
                       << "  success!";

  return true;
}

bool TransformGraph::RemoveChild(TransformHandle parent, TransformHandle child) {
  FX_DCHECK(is_valid_);
  FX_DCHECK(working_set_.contains(parent));

  auto iter = children_.find(parent);
  if (iter == children_.end()) {
    FLATLAND_VERBOSE_LOG << "TransformGraph::RemoveChild() parent=" << parent << "  child=" << child
                         << "  failure!";
    return false;
  }

  auto& vec = iter->second.normal_children;
  auto vec_iter = std::find(vec.begin(), vec.end(), child);
  if (vec_iter == vec.end()) {
    FLATLAND_VERBOSE_LOG << "TransformGraph::RemoveChild() parent=" << parent << "  child=" << child
                         << "  failure!";
    return false;
  }

  vec.erase(vec_iter);
  if (vec.empty() && !iter->second.priority_child.has_value()) {
    children_.erase(iter);
  }

  FLATLAND_VERBOSE_LOG << "TransformGraph::RemoveChild() parent=" << parent << "  child=" << child
                       << "  success!";

  return true;
}

bool TransformGraph::ReplaceChildren(TransformHandle parent,
                                     std::span<const TransformHandle> new_children) {
  FX_DCHECK(is_valid_);
  FX_DCHECK(working_set_.contains(parent));

  if (new_children.empty()) {
    ClearChildren(parent);
    return true;
  }

  // Use unordered_set to verify uniqueness of child TransformHandles.
  // Stack-backed std::pmr::unordered_set avoids heap allocations.
  alignas(std::max_align_t) char buffer[1024];
  std::pmr::monotonic_buffer_resource stack_resource(buffer, sizeof(buffer));
  std::pmr::unordered_set<TransformHandle> unique_children(&stack_resource);
  unique_children.reserve(new_children.size());
  for (auto child : new_children) {
    auto res = unique_children.insert(child);
    // Do not allow duplicate handles in `new_children`.
    if (!res.second) {
      return false;
    }
  }

  auto& normal_children = children_[parent].normal_children;
  normal_children.clear();
  normal_children.reserve(std::max<size_t>(4, new_children.size()));
  size_t verbose_child_count = 0;
  for (auto child : new_children) {
    FLATLAND_VERBOSE_LOG << "TransformGraph::ReplaceChildren() parent=" << parent << "  child-"
                         << verbose_child_count++ << "=" << child << "  success!";
    normal_children.push_back(child);
  }

  return true;
}

void TransformGraph::SetPriorityChild(TransformHandle parent, TransformHandle child) {
  FX_DCHECK(is_valid_);
  FX_DCHECK(working_set_.contains(parent));

  children_[parent].priority_child = child;
}

void TransformGraph::ClearPriorityChild(TransformHandle parent) {
  FX_DCHECK(is_valid_);
  FX_DCHECK(working_set_.contains(parent));

  auto iter = children_.find(parent);
  if (iter != children_.end()) {
    iter->second.priority_child.reset();
    if (iter->second.normal_children.empty()) {
      children_.erase(iter);
    }
  }
}

void TransformGraph::ResetGraph(TransformHandle exception) {
  FX_DCHECK(working_set_.contains(exception));
  working_set_.clear();
  working_set_.insert(exception);
  children_.clear();
  is_valid_ = true;
}

TransformGraph::TopologyData TransformGraph::ComputeAndCleanup(
    TransformHandle start, uint64_t max_iterations, std::pmr::memory_resource* output_resource) {
  TRACE_DURATION("gfx", "TransformGraph::ComputeAndCleanup");
  FX_DCHECK(is_valid_);
  FX_DCHECK(working_set_.contains(start));

  TopologyData data;
  data.sorted_transforms = TopologyVector(output_resource);

  // Swap all the live nodes into the dead set, so we can pull them out as we visit them.
  std::swap(live_set_, data.dead_transforms);

  // Track visited nodes across traversals using a PMR set backed by a stack buffer and resource
  // fallback.
  alignas(std::max_align_t) char buffer[4096];
  std::pmr::monotonic_buffer_resource stack_resource(buffer, sizeof(buffer));
  std::pmr::unordered_set<TransformHandle> visited(&stack_resource);

  // Compute the topological set starting from the start transform.
  Traverse(start, children_, nullptr, &data.cyclical_edges, max_iterations - data.iterations,
           data.sorted_transforms);
  data.iterations += data.sorted_transforms.size();
  for (auto [transform, child_count] : data.sorted_transforms) {
    visited.insert(transform);
    data.dead_transforms.erase(transform);
    live_set_.insert(transform);
  }

  // Compute the topological set starting from every working set transform, for cleanup purposes.
  TopologyVector working_transforms(&stack_resource);
  for (auto transform : working_set_) {
    if (visited.contains(transform)) {
      continue;
    }
    working_transforms.clear();
    Traverse(transform, children_, &visited, &data.cyclical_edges, max_iterations - data.iterations,
             working_transforms);
    data.iterations += working_transforms.size();
    for (auto [transform, child_count] : working_transforms) {
      visited.insert(transform);
      data.dead_transforms.erase(transform);
      live_set_.insert(transform);
    }
  }

  // Cleanup child state for all dead nodes.
  for (auto transform : data.dead_transforms) {
    children_.erase(transform);
  }

  if (data.iterations >= max_iterations) {
    is_valid_ = false;
  }

  return data;
}

TransformGraph::ChildIterator TransformGraph::ChildIterator::ForParent(
    const PriorityChildMap& children, TransformHandle parent) {
  auto iter = children.find(parent);
  if (iter != children.end()) {
    return ChildIterator{.node = &iter->second, .index = 0};
  }
  return ChildIterator{.node = nullptr, .index = 0};
}

TransformHandle TransformGraph::ChildIterator::GetAndAdvance() {
  FX_DCHECK(HasNext());
  if (node->priority_child.has_value()) {
    if (index == 0) {
      index++;
      return *node->priority_child;
    }
    return node->normal_children[index++ - 1];
  }
  return node->normal_children[index++];
}

uint64_t TransformGraph::ChildIterator::ChildCount() const {
  if (!node) {
    return 0;
  }
  return (node->priority_child.has_value() ? 1 : 0) + node->normal_children.size();
}

void TransformGraph::Traverse(TransformHandle start, const PriorityChildMap& children,
                              const std::pmr::unordered_set<TransformHandle>* prev_visited,
                              ChildMap* cycles, uint64_t max_length,
                              TopologyVector& out_topology_vector) {
  // If the start node is already visited, return it immediately as a leaf without traversing.
  if (prev_visited && prev_visited->contains(start)) {
    out_topology_vector.push_back({start, 0});
    return;
  }

  // Reserve capacity for the return vector to prevent reallocations during traversal.
  out_topology_vector.reserve(std::min(working_set_.size(), max_length));

  // Create a 4KB stack buffer for local vector allocations, with resource as upstream fallback.
  alignas(std::max_align_t) char buffer[4096];
  std::pmr::monotonic_buffer_resource stack_resource(buffer, sizeof(buffer));

  std::pmr::vector<ChildIterator> iterator_stack(&stack_resource);
  std::pmr::vector<TransformHandle> ancestors(&stack_resource);
  std::pmr::vector<uint64_t> parent_indices(&stack_resource);

  // Add the starting handle to the output, and initialize our state.
  ChildIterator start_iter = ChildIterator::ForParent(children, start);
  out_topology_vector.push_back({start, start_iter.ChildCount()});
  iterator_stack.push_back(start_iter);
  ancestors.push_back(start);
  parent_indices.push_back(0);

  // Iterate until we're done, or until we run out of space
  while (!iterator_stack.empty() && out_topology_vector.size() < max_length) {
    auto& current_iter = iterator_stack.back();

    // If we're at the end of this iterator, pop to the parent iterator.
    if (!current_iter.HasNext()) {
      iterator_stack.pop_back();
      ancestors.pop_back();
      parent_indices.pop_back();
      continue;
    }

    const TransformHandle child = current_iter.GetAndAdvance();

    // Search from the bottom of the stack (since it's more likely), looking for a cycle.
    if (std::find(ancestors.crbegin(), ancestors.crend(), child) != ancestors.crend()) {
      FX_DCHECK(cycles);
      FX_DCHECK(!parent_indices.empty());
      cycles->insert({out_topology_vector[parent_indices.back()].handle, child});
    } else if (prev_visited && prev_visited->contains(child)) {
      // If the child has already been visited in a prior traversal, record it with a child count
      // of 0 rather than expanding its children. When `prev_visited` is non-empty, the resulting
      // topology vector is used for dead-transform bookkeeping. Setting the child count to 0
      // ensures the returned TopologyVector remains structurally well-formed (since its children
      // are not expanded here) while avoiding redundant re-traversal of visited subtrees.
      out_topology_vector.push_back({child, 0});
    } else {
      // If the child is not part of a cycle and unvisited, add it to the sorted list and stacks.
      parent_indices.push_back(out_topology_vector.size());
      ChildIterator child_iter = ChildIterator::ForParent(children, child);
      iterator_stack.push_back(child_iter);
      out_topology_vector.push_back({child, child_iter.ChildCount()});
      ancestors.push_back(child);
    }
  }
}

}  // namespace flatland
