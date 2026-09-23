// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/bin/driver_manager/power/all_drivers_element.h"

#include <memory>
#include <unordered_set>
#include <vector>

#include "src/devices/bin/driver_manager/power/power_manager.h"
#include "src/devices/lib/log/log.h"

namespace driver_manager {

namespace {

// Specifies the direction of traversal for the depth-first search helper.
enum class Direction : uint8_t {
  // Traverse downwards through children (e.g. from parent to descendants).
  kDown,
  // Traverse upwards through parents (e.g. from child to ancestors).
  kUp,
};

// Performs a generic Depth-First-Search (DFS) over the node topology starting from
// |starting_node|.
//
// |direction| controls whether we traverse downwards (children) or upwards (parents).
// |visitor| is a callback executed on each node. If it returns false, the traversal
// of the branch beneath/above the current node is pruned (adjacent nodes are not pushed).
void PerformDFS(const std::shared_ptr<const Node>& starting_node, Direction direction,
                fit::function<bool(const std::shared_ptr<const Node>&)> visitor) {
  std::unordered_set<const Node*> visited;
  std::vector<std::shared_ptr<const Node>> stack;
  visited.insert(starting_node.get());
  stack.push_back(starting_node);

  while (!stack.empty()) {
    auto current = stack.back();
    stack.pop_back();

    if (!visitor(current)) {
      continue;
    }

    if (direction == Direction::kDown) {
      for (const auto& child : current->children()) {
        if (visited.insert(child.get()).second) {
          stack.push_back(child);
        }
      }
    } else {
      for (const auto& parent_weak : current->parents()) {
        if (auto parent = parent_weak.lock()) {
          if (visited.insert(parent.get()).second) {
            stack.push_back(std::move(parent));
          }
        }
      }
    }
  }
}

// Performs a bottom-up, post-order Depth-First-Search (DFS) over the node topology
// starting from |starting_node|.
//
// The |visitor| callback is invoked on a node only after all of its reachable
// children/descendants have been visited. This is useful for bottom-up processing
// (e.g., propagating bounds or dependencies).
void PerformPostOrderDFS(const std::shared_ptr<const Node>& starting_node,
                         fit::function<void(const std::shared_ptr<const Node>&)> visitor) {
  std::unordered_set<const Node*> visited;
  std::vector<std::shared_ptr<const Node>> stack;

  stack.push_back(starting_node);
  visited.insert(starting_node.get());

  while (!stack.empty()) {
    auto curr = stack.back();

    bool pushed_child = false;
    for (const auto& child : curr->children()) {
      if (!visited.contains(child.get())) {
        visited.insert(child.get());
        stack.push_back(child);
        pushed_child = true;
        break;
      }
    }

    if (!pushed_child) {
      visitor(curr);
      stack.pop_back();
    }
  }
}

// Returns true if |start_node| has any descendants (excluding itself) that are currently
// bound to a driver.
bool HasBoundDescendants(const std::shared_ptr<const Node>& start_node) {
  bool has_bound = false;
  PerformDFS(start_node, Direction::kDown,
             [&has_bound, &start_node](const std::shared_ptr<const Node>& node) {
               if (node == start_node) {
                 // Skip initial node
                 return true;
               }

               if (has_bound) {
                 // Found in another branch, stop here.
                 return false;
               }

               if (node->is_bound()) {
                 has_bound = true;
                 return false;  // Found, prune children
               }
               return true;
             });
  return has_bound;
}

}  // namespace

AllDriversElement::AllDriversElement(PowerManager* power_manager, std::shared_ptr<Node> root)
    : root_(root), power_manager_(power_manager) {}

void AllDriversElement::AttachElement(async_dispatcher_t* dispatcher, PowerElementHandles handles) {
  element_control_ = std::move(handles.element_control);
  lessor_ = std::move(handles.lessor);
  element_runner_bindings_.AddBinding(dispatcher, std::move(handles.element_runner), this,
                                      fidl::kIgnoreBindingClosure);
}

void AllDriversElement::OnNodeBound(std::shared_ptr<const Node> node) {
  if (!transition_complete_) {
    return;
  }

  // During normal operation, every bound node takes its startup lease out. If it is not a
  // leaf node, this lease will immediately fall out of scope and be safely dropped.
  std::optional<zx::eventpair> startup_lease =
      std::const_pointer_cast<Node>(node)->TakeStartupLease();

  if (node->IsHermeticPowerTest()) {
    return;
  }

  if (HasBoundDescendants(node)) {
    // Already has bound descendants, so it is not a leaf and we want to drop its startup lease.
    return;
  }

  // This node has no bound descendants, so it is a leaf. Now we insert it as a leaf and remove any
  // formerly-leaf parents from the leaf set
  std::string topo_path = node->MakeTopologicalPath();
  leaf_driver_instances_.insert_or_assign(topo_path, node);

  if (current_level_ == 0) {
    // If we are in low power mode, we just clean up the leaf topology without
    // holding any leases (the startup lease is allowed to drop).
    // Note: we never drop the startup lease until after AllDrivers turns on because this code
    // doesn't execute until transition_complete_ is true
    RemoveAncestors(node);
    return;
  }

  if (startup_lease.has_value() && startup_lease->is_valid()) {
    // Handoff the startup lease from the leaf node to AllDriversElement.
    leases_.insert_or_assign(topo_path, std::move(*startup_lease));
    RemoveAncestors(node);
    return;
  }

  // If the startup lease is missing or consumed, actively request a new lease.
  // We use a deferred callback to ensure ancestor leases are only dropped AFTER
  // the new lease is firmly held, preventing momentary off-on power cycles.
  std::shared_ptr<fit::deferred_callback> on_lease_acquire =
      std::make_shared<fit::deferred_callback>(
          fit::defer_callback([this, node]() { RemoveAncestors(node); }));
  AcquireLease(node, on_lease_acquire);
}

void AllDriversElement::RemoveAncestors(const std::shared_ptr<const Node>& start_node) {
  PerformDFS(start_node, Direction::kUp,
             [this, &start_node](const std::shared_ptr<const Node>& node) {
               if (node == start_node) {
                 return true;  // Traverse up to parents
               }

               if (!node->is_bound()) {
                 return true;  // Traverse up to parents of this unbound node
               }

               // Found a driver-bound ancestor. We stop traversing up this branch because anything
               // remaining up should not be stored as a leaf driver instance.
               std::string topo_path = node->MakeTopologicalPath();
               if (leaf_driver_instances_.contains(topo_path)) {
                 leaf_driver_instances_.erase(topo_path);
                 leases_.erase(topo_path);
               }
               return false;  // Prune traversal up this branch
             });
}

void AllDriversElement::AcquireLease(std::shared_ptr<const Node> node,
                                     std::shared_ptr<fit::deferred_callback> deferred) {
  std::string topo_path = node->MakeTopologicalPath();

  zx::eventpair lease_token, lease_token_peer;
  if (zx::eventpair::create(0, &lease_token, &lease_token_peer) != ZX_OK) {
    fdf_log::error("Failed to create lease token for node '{}'", topo_path);
    return;
  }

  fuchsia_power_broker::LeaseSchema schema;
  schema.lease_token(std::move(lease_token_peer));
  schema.lease_name(node->name());

  zx::event power_token = node->DuplicatePowerToken();
  if (power_token.is_valid()) {
    std::vector<fuchsia_power_broker::LeaseDependency> deps;
    deps.push_back(fuchsia_power_broker::LeaseDependency{{
        .requires_token = std::move(power_token),
        .requires_level = 1,
    }});
    schema.dependencies(std::move(deps));
  }

  power_manager_->power_topology()
      ->Lease(std::move(schema))
      .Then([weak_self = weak_from_this(), topo_path, lease_token = std::move(lease_token),
             deferred = std::move(deferred)](
                fidl::Result<fuchsia_power_broker::Topology::Lease>& result) mutable {
        auto self = weak_self.lock();
        if (!self) {
          return;
        }
        if (!result.is_ok()) {
          fdf_log::error("Failed to acquire lease for node '{}': {}", topo_path,
                         result.error_value());
        } else if (self->current_level_ == 1) {
          self->leases_.insert_or_assign(topo_path, std::move(lease_token));
        }
      });
}

void AllDriversElement::ProcessStartupNode(const std::shared_ptr<const Node>& node,
                                           bool has_bound_descendant) {
  // We only process driver-bound nodes. Unbound nodes do not take startup leases.
  if (!node->is_bound()) {
    return;
  }

  // Retrieve the startup lease. It is crucial to take it here regardless of whether
  // this is a leaf or non-leaf node, so it doesn't get left behind.
  std::optional<zx::eventpair> startup_lease =
      std::const_pointer_cast<Node>(node)->TakeStartupLease();

  if (node->IsHermeticPowerTest()) {
    return;
  }

  // If this node has bound descendants, it is not a leaf node in the active topology.
  if (has_bound_descendant) {
    // The startup lease goes out of scope here and is dropped. It is safe to drop it
    // because leaf nodes (visited earlier in post-order DFS) already held their leases
    // synchronously, ensuring the parent node remains powered.
    return;
  }

  // For leaf nodes, we register them as leaf driver instances.
  std::string topo_path = node->MakeTopologicalPath();
  leaf_driver_instances_.insert_or_assign(topo_path, node);

  if (startup_lease.has_value() && startup_lease->is_valid()) {
    // Transfer the leaf startup lease directly to AllDriversElement leases_ map
    // to maintain active power holding.
    leases_.insert_or_assign(topo_path, std::move(*startup_lease));
  }
}

void AllDriversElement::CompleteStartupTransition() {
  if (current_level_ == 0) {
    return;
  }

  transition_complete_ = true;

  // Identify all leaf nodes and safely transfer their startup leases.
  if (auto root = root_.lock()) {
    // Memoization set to track which nodes have bound descendants.
    // Bottom-up post-order DFS guarantees that when we visit a node, we have already processed
    // all of its children, allowing O(1) checks for bound descendants.
    std::unordered_set<const Node*> has_bound_descendant_set;

    PerformPostOrderDFS(
        root, [this, &has_bound_descendant_set](const std::shared_ptr<const Node>& node) {
          // Check if this node has any descendants bound to a driver.
          bool has_bound_descendant = false;
          for (const auto& child : node->children()) {
            if (child->is_bound() || has_bound_descendant_set.contains(child.get())) {
              has_bound_descendant = true;
              has_bound_descendant_set.insert(node.get());
              break;
            }
          }

          ProcessStartupNode(node, has_bound_descendant);
        });
  }
}

void AllDriversElement::SetLevel(SetLevelRequestView request, SetLevelCompleter::Sync& completer) {
  current_level_ = request->level;

  if (!transition_complete_) {
    // If the initial level is 0, CompleteStartupTransition will no-op and we
    // will re-enter this method when the 1 comes in.
    CompleteStartupTransition();
    completer.Reply();
    return;
  }

  if (current_level_ == 0) {
    leases_.clear();
    completer.Reply();
    return;
  }

  ZX_ASSERT(current_level_ == 1);

  // This defer function is executed once all the AcquireLeases() calls have completed.
  auto defer = std::make_shared<fit::deferred_callback>(
      [completer = completer.ToAsync()]() mutable { completer.Reply(); });

  // When waking up from low power, we request a new lease for each leaf driver node.
  // We also use this iteration to clean up expired weak_ptrs from dead nodes.
  for (auto it = leaf_driver_instances_.begin(); it != leaf_driver_instances_.end();) {
    if (auto locked_node = it->second.lock()) {
      AcquireLease(std::move(locked_node), defer);
      ++it;
    } else {
      fdf_log::warn("Node {} has expired, removing from leaf instances.", it->first);
      leases_.erase(it->first);
      it = leaf_driver_instances_.erase(it);
    }
  }
}

void AllDriversElement::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_power_broker::ElementRunner> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {
  fdf_log::info("Unknown AllDrivers element runner method request received: {}",
                metadata.method_ordinal);
}

}  // namespace driver_manager
