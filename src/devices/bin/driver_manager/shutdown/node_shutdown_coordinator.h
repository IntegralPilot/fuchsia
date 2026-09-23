// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BIN_DRIVER_MANAGER_SHUTDOWN_NODE_SHUTDOWN_COORDINATOR_H_
#define SRC_DEVICES_BIN_DRIVER_MANAGER_SHUTDOWN_NODE_SHUTDOWN_COORDINATOR_H_

#include <lib/async_patterns/cpp/task_scope.h>
#include <stdint.h>

#include <memory>
#include <optional>
#include <random>
#include <string>
#include <utility>

#include "src/devices/bin/driver_manager/node_types.h"

namespace driver_manager {

class Node;
struct NodeInfo;
class NodeRemovalTracker;

using NodeId = uint32_t;

enum class RemovalSet : uint8_t {
  kAll,      // Remove the boot drivers and the package drivers
  kPackage,  // Remove the package drivers
};

// Represents the actions of the node after shutdown is completed.
enum class ShutdownIntent : uint8_t {
  kRemoval,          // Removes the node from the topology. The default shutdown intention.
  kRestart,          // Restarts the node and attempts to bind it to a new driver.
  kRebindComposite,  // Removes the composite node from the topology for rebind. Only invoked if the
                     // node is a composite.
  kQuarantine,       // If a node's driver fails to start, we shutdown the driver but not the node.
                     // The node should be quarantined, which means we won't start its driver again.
                     // Eventually if the driver is disabled, the node will restart and be able to
                     // find a new driver match, or become an orphan until a new driver is
                     // registered that matches it.
};

// Bridge class for node-related interactions.
class NodeShutdownBridge {
 public:
  // Returns a pair that contains the node's component moniker and collection. Used to
  // register the node to the NodeRemovalTracker.
  virtual NodeInfo GetRemovalTrackerInfo() = 0;

  // Sends a Stop request to the driver. Invoked when transitioning to kWaitingOnDriver.
  virtual void StopDriver() = 0;

  // Closes the driver component connection to signal to CF that the component has
  // stopped and ensures the component is destroyed. Invoked when transitioning to
  // kWaitingOnDriverComponent.
  virtual void StopDriverComponent() = 0;

  // Sends a component destroy request to CF in certain conditions. We don't always want
  // to destroy the component so this will return false if we are keeping the component around.
  virtual bool MaybeDestroyDriverComponent() = 0;

  // Clean up work before shutdown is complete. Invoked when transitioning to kStopped.
  // The implementation is expected to invoke the callback once the node finished shutdown.
  // Once the callback is invoked, the node state changes to to kStopped and the node removal
  // tracker is notified.
  virtual void FinishShutdown(fit::callback<void()> shutdown_callback) = 0;

  virtual bool IsPendingBind() const = 0;

  virtual bool HasChildren() const = 0;

  virtual bool HasDriver() const = 0;

  virtual bool HasDriverComponent() const = 0;

  virtual bool HasDriverComponentController() const = 0;
};

// Coordinates and keeps track of the node's shutdown process.
class NodeShutdownCoordinator {
 public:
  explicit NodeShutdownCoordinator(std::string_view name, NodeShutdownBridge* bridge,
                                   async_dispatcher_t* dispatcher, bool enable_test_shutdown_delays,
                                   std::weak_ptr<std::mt19937> rng_gen);

  ~NodeShutdownCoordinator() = default;

  static const char* NodeStateAsString(NodeState state);

  // Begin the removal process for a Node. This function ensures that a Node is
  // only removed after all of its children are removed. It also ensures that
  // a Node is only removed after the driver that is bound to it has been stopped.
  // This is safe to call multiple times.
  // There are multiple reasons a Node's removal will be started:
  //   - The system is being stopped.
  //   - The Node had an unexpected error or disconnect
  // During a system stop, Remove is expected to be called twice:
  // once with |removal_set| == kPackage, and once with |removal_set| == kAll.
  // Errors and disconnects that are unrecoverable should call Remove(kAll, nullptr).
  static void Remove(std::shared_ptr<Node> node, RemovalSet removal_set,
                     NodeRemovalTracker* removal_tracker);

  // Reset the shutdown values. Should only be called after shutdown is complete.
  void ResetShutdown();

  // Functions that check and transition the node's state.
  void CheckNodeState();
  void CheckWaitingOnDriverBind();
  void CheckWaitingOnChildren();
  void CheckWaitingOnDriver();
  void CheckWaitingOnDriverComponent();
  void CheckStopped();
  void CheckWaitingOnDestroy();

  // Invokes |action|, which is the actions for transitioning to the next
  // another state. If |kEnableShutdownDelay| is true and generates a randomized
  // delay, then the action is invoked asynchronously with the delay.
  void PerformTransition(fit::function<void()> action);

  void UpdateAndNotifyState(NodeState state);
  void NotifyRemovalTracker();

  bool IsShuttingDown() const;

  const char* NodeStateAsString() const {
    return NodeShutdownCoordinator::NodeStateAsString(node_state_);
  }

  NodeState node_state() const { return node_state_; }

  // TODO(https://fxbug.dev/42082343): Handle shutdown intent priority. Currently it's possible
  // for rebind or restart to override a a removal in process.
  void set_shutdown_intent(ShutdownIntent intent) { shutdown_intent_ = intent; }
  ShutdownIntent shutdown_intent() const { return shutdown_intent_; }

 private:
  void SetRemovalTracker(NodeRemovalTracker* tracker);

  // Generates a random test delay in milliseconds. Returns std::nullopt if
  // |enable_test_shutdown_delays_| is false.
  std::optional<uint32_t> GenerateTestDelayMs();

  std::string name_;

  std::optional<NodeId> removal_id_;

  // Only set in system shutdown.
  NodeRemovalTracker* removal_tracker_ = nullptr;

  NodeState node_state_ = NodeState::kRunning;

  ShutdownIntent shutdown_intent_ = ShutdownIntent::kRemoval;

  // Owner. Must outlive NodeShutdownCoordinator.
  NodeShutdownBridge* bridge_;

  // Set to true when the NodeShutdownCoordinator is in the process of transitioning
  // node states. Only set if |enable_test_shutdown_delays_| is enabled. Used to prevent
  // multiple state transitions from happening at the same time.
  bool is_transition_pending_ = false;

  bool enable_test_shutdown_delays_;

  // Randomizer for injecting delays in shutdown. Only used when |enable_test_shutdown_delays_|
  // is true.
  std::weak_ptr<std::mt19937> rng_gen_;
  std::uniform_int_distribution<uint64_t> distribution_;

  async_patterns::TaskScope tasks_;
};

}  // namespace driver_manager

#endif  // SRC_DEVICES_BIN_DRIVER_MANAGER_SHUTDOWN_NODE_SHUTDOWN_COORDINATOR_H_
