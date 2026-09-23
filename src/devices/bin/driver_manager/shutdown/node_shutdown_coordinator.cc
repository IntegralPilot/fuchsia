// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/bin/driver_manager/shutdown/node_shutdown_coordinator.h"

#include "src/devices/bin/driver_manager/node.h"
#include "src/devices/bin/driver_manager/shutdown/node_removal_tracker.h"
#include "src/devices/lib/log/log.h"

namespace driver_manager {

namespace {

// The range of test delay time in milliseconds.
constexpr uint32_t kMinTestDelayMs = 0;
constexpr uint32_t kMaxTestDelayMs = 5;

}  // namespace

NodeShutdownCoordinator::NodeShutdownCoordinator(std::string_view name, NodeShutdownBridge* bridge,
                                                 async_dispatcher_t* dispatcher,
                                                 bool enable_test_shutdown_delays,
                                                 std::weak_ptr<std::mt19937> rng_gen)
    : name_(name),
      bridge_(bridge),
      enable_test_shutdown_delays_(enable_test_shutdown_delays),
      rng_gen_(rng_gen),
      distribution_(kMinTestDelayMs, kMaxTestDelayMs),
      tasks_(dispatcher) {}

void NodeShutdownCoordinator::Remove(std::shared_ptr<Node> node, RemovalSet removal_set,
                                     NodeRemovalTracker* removal_tracker) {
  std::stack<std::shared_ptr<Node>> nodes_to_check_for_removal;
  std::stack<std::pair<std::shared_ptr<Node>, RemovalSet>> nodes =
      std::stack<std::pair<std::shared_ptr<Node>, RemovalSet>>({{node, removal_set}});
  while (!nodes.empty()) {
    std::pair<std::shared_ptr<Node>, RemovalSet> removal_record = nodes.top();
    nodes.pop();
    std::shared_ptr<Node> node = removal_record.first;
    RemovalSet removal_set = removal_record.second;

    NodeShutdownCoordinator& node_shutdown_coordinator = node->GetNodeShutdownCoordinator();
    if (!removal_tracker && node_shutdown_coordinator.removal_tracker_) {
      // TODO(https://fxbug.dev/42066485): Change this to an error when we track shutdown steps
      // better.
      fdf_log::debug("Untracked Node::Remove() called on {}, indicating an error during shutdown",
                     node->MakeTopologicalPath());
    }

    if (removal_tracker) {
      node_shutdown_coordinator.SetRemovalTracker(removal_tracker);
    }

    fdf_log::debug("Remove called on Node: {}", node->name());
    // Two cases where we will transition state and take action:
    // Removing kAll, and state is Running, Prestop, or Stop.
    // Removing kPkg, and state is Running
    if ((node_shutdown_coordinator.node_state() != NodeState::kPrestop &&
         node_shutdown_coordinator.node_state() != NodeState::kStopped &&
         node_shutdown_coordinator.node_state() != NodeState::kRunning) ||
        (node_shutdown_coordinator.node_state() == NodeState::kPrestop &&
         removal_set == RemovalSet::kPackage)) {
      if (node->parents().size() <= 1) {
        fdf_log::debug("Node::Remove() {} called late, already in state {}",
                       node->MakeComponentMoniker(), node_shutdown_coordinator.NodeStateAsString());
      }
      continue;
    }

    // Now, the cases where we do something:
    // Set the new state
    if (removal_set == RemovalSet::kPackage &&
        (node->collection() == Collection::kBoot || node->collection() == Collection::kNone)) {
      node_shutdown_coordinator.node_state_ = NodeState::kPrestop;
    } else {
      node_shutdown_coordinator.node_state_ = NodeState::kWaitingOnDriverBind;
      // Either removing kAll, or is package driver and removing kPackage.
      // All children should be removed regardless as they block removal of this node.
      removal_set = RemovalSet::kAll;
    }

    // Propagate removal message to children
    node_shutdown_coordinator.NotifyRemovalTracker();

    // Ask each of our children to remove themselves.
    for (auto& child : node->children()) {
      fdf_log::debug("Node: {} calling remove on child: {}", node->name(), child->name());
      nodes.emplace(child, removal_set);
      child->SetShouldDestroy();
    }
    nodes_to_check_for_removal.push(std::move(node));
  }

  while (!nodes_to_check_for_removal.empty()) {
    nodes_to_check_for_removal.top()->GetNodeShutdownCoordinator().CheckNodeState();
    nodes_to_check_for_removal.pop();
  }
}

void NodeShutdownCoordinator::ResetShutdown() {
  ZX_ASSERT_MSG(node_state_ == NodeState::kDestroyed || node_state_ == NodeState::kStopped,
                "NodeShutdownCoordinator::ResetShutdown called in invalid node state: %s",
                NodeStateAsString());
  node_state_ = NodeState::kRunning;
  shutdown_intent_ = ShutdownIntent::kRemoval;
}

void NodeShutdownCoordinator::CheckNodeState() {
  if (is_transition_pending_) {
    return;
  }

  switch (node_state_) {
    case NodeState::kRunning:
    case NodeState::kPrestop:
    case NodeState::kDestroyed:
      return;
    case NodeState::kWaitingOnDriverBind: {
      CheckWaitingOnDriverBind();
      return;
    }
    case NodeState::kWaitingOnChildren: {
      CheckWaitingOnChildren();
      return;
    }
    case NodeState::kWaitingOnDriver: {
      CheckWaitingOnDriver();
      return;
    }
    case NodeState::kWaitingOnDriverComponent: {
      CheckWaitingOnDriverComponent();
      return;
    }
    case NodeState::kStopped: {
      CheckStopped();
      return;
    }
    case NodeState::kWaitingOnDestroy: {
      CheckWaitingOnDestroy();
      return;
    }
  }
}

void NodeShutdownCoordinator::CheckWaitingOnDriverBind() {
  ZX_ASSERT(!is_transition_pending_);
  ZX_ASSERT_MSG(
      node_state_ == NodeState::kWaitingOnDriverBind,
      "NodeShutdownCoordinator::CheckWaitingOnDriverBind called in invalid node state: %s",
      NodeStateAsString());
  // Remain on this state if the node's driver is still in the binding state.
  if (bridge_->IsPendingBind()) {
    return;
  }
  PerformTransition([this]() mutable { UpdateAndNotifyState(NodeState::kWaitingOnChildren); });
}

void NodeShutdownCoordinator::CheckWaitingOnChildren() {
  ZX_ASSERT(!is_transition_pending_);
  ZX_ASSERT_MSG(node_state_ == NodeState::kWaitingOnChildren,
                "NodeShutdownCoordinator::CheckWaitingOnChildren called in invalid node state: %s",
                NodeStateAsString());
  // Remain on this state if the node still has children.
  if (bridge_->HasChildren()) {
    return;
  }
  PerformTransition([this]() mutable {
    bridge_->StopDriver();
    UpdateAndNotifyState(NodeState::kWaitingOnDriver);
  });
}

void NodeShutdownCoordinator::CheckWaitingOnDriver() {
  ZX_ASSERT(!is_transition_pending_);
  ZX_ASSERT_MSG(node_state_ == NodeState::kWaitingOnDriver,
                "NodeShutdownCoordinator::CheckWaitingOnDriver called in invalid node state: %s",
                NodeStateAsString());

  // Remain on this state if the node still has a driver set to it.
  if (bridge_->HasDriver()) {
    return;
  }
  PerformTransition([this]() mutable {
    bridge_->StopDriverComponent();
    UpdateAndNotifyState(NodeState::kWaitingOnDriverComponent);
  });
}

void NodeShutdownCoordinator::CheckWaitingOnDriverComponent() {
  ZX_ASSERT(!is_transition_pending_);
  ZX_ASSERT_MSG(
      node_state_ == NodeState::kWaitingOnDriverComponent,
      "NodeShutdownCoordinator::CheckWaitingOnDriverComponent called in invalid node state: %s",
      NodeStateAsString());

  // Remain on this state if the node still has a driver component.
  if (bridge_->HasDriverComponent()) {
    return;
  }

  PerformTransition([this]() mutable { UpdateAndNotifyState(NodeState::kStopped); });
}

void NodeShutdownCoordinator::CheckStopped() {
  ZX_ASSERT(!is_transition_pending_);
  ZX_ASSERT_MSG(node_state_ == NodeState::kStopped,
                "NodeShutdownCoordinator::CheckStopped called in invalid node state: %s",
                NodeStateAsString());

  PerformTransition([this]() mutable {
    if (bridge_->HasDriverComponentController()) {
      // Remain on this state if the node did not need to destroy an active component controller.
      bool destroy_started = bridge_->MaybeDestroyDriverComponent();
      if (!destroy_started) {
        fdf_log::debug("node {} HasDriverComponentController but not destroying", name_);
        return;
      }
    }

    UpdateAndNotifyState(NodeState::kWaitingOnDestroy);
  });
}

void NodeShutdownCoordinator::CheckWaitingOnDestroy() {
  ZX_ASSERT(!is_transition_pending_);
  ZX_ASSERT_MSG(node_state_ == NodeState::kWaitingOnDestroy,
                "NodeShutdownCoordinator::CheckStopped called in invalid node state: %s",
                NodeStateAsString());

  // Remain on this state if the node still has a driver component controller.
  if (bridge_->HasDriverComponentController()) {
    return;
  }

  PerformTransition([this]() mutable {
    bridge_->FinishShutdown([this]() { UpdateAndNotifyState(NodeState::kDestroyed); });
  });
}

void NodeShutdownCoordinator::PerformTransition(fit::function<void()> action) {
  ZX_ASSERT(!is_transition_pending_);

  // Generate a test delay. If there is none, perform the action and return.
  // Otherwise, perform the action asynchronously with the delay.
  std::optional<uint32_t> test_delay = GenerateTestDelayMs();
  if (!test_delay) {
    action();
    return;
  }

  is_transition_pending_ = true;
  tasks_.Post([action = std::move(action), delay_amount = test_delay.value()] {
    std::this_thread::sleep_for(std::chrono::microseconds(delay_amount));
    action();
  });
}

void NodeShutdownCoordinator::UpdateAndNotifyState(NodeState state) {
  node_state_ = state;
  is_transition_pending_ = false;
  NotifyRemovalTracker();
  CheckNodeState();
}

void NodeShutdownCoordinator::NotifyRemovalTracker() {
  if (!removal_tracker_ || removal_id_ == std::nullopt) {
    return;
  }
  removal_tracker_->Notify(removal_id_.value(), node_state_);
}

bool NodeShutdownCoordinator::IsShuttingDown() const { return node_state_ != NodeState::kRunning; }

const char* NodeShutdownCoordinator::NodeStateAsString(NodeState state) {
  switch (state) {
    case NodeState::kWaitingOnDriverBind:
      return "kWaitingOnDriverBind";
    case NodeState::kRunning:
      return "kRunning";
    case NodeState::kPrestop:
      return "kPrestop";
    case NodeState::kWaitingOnChildren:
      return "kWaitingOnChildren";
    case NodeState::kWaitingOnDriver:
      return "kWaitingOnDriver";
    case NodeState::kWaitingOnDriverComponent:
      return "kWaitingOnDriverComponent";
    case NodeState::kStopped:
      return "kStopped";
    case NodeState::kWaitingOnDestroy:
      return "kWaitingOnDestroy";
    case NodeState::kDestroyed:
      return "kDestroyed";
  }
}

void NodeShutdownCoordinator::SetRemovalTracker(NodeRemovalTracker* removal_tracker) {
  ZX_ASSERT(removal_tracker);
  if (removal_tracker_) {
    // We should never have two competing trackers
    ZX_ASSERT(removal_tracker_ == removal_tracker);
    return;
  }
  removal_tracker_ = removal_tracker;
  removal_id_ = removal_tracker->RegisterNode(bridge_->GetRemovalTrackerInfo());
}

std::optional<uint32_t> NodeShutdownCoordinator::GenerateTestDelayMs() {
  if (!enable_test_shutdown_delays_) {
    return std::nullopt;
  }

  auto rng = rng_gen_.lock();
  if (!rng) {
    fdf_log::warn("Shutdown test RNG released. Unable to generate a test delay");
    return std::nullopt;
  }

  // Randomize a 20% chance for adding a shutdown delay.
  if (distribution_(*rng) % 5 == 1) {
    return distribution_(*rng);
  }
  return std::nullopt;
}

}  // namespace driver_manager
