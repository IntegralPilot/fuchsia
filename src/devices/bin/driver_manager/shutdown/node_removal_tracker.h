// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BIN_DRIVER_MANAGER_SHUTDOWN_NODE_REMOVAL_TRACKER_H_
#define SRC_DEVICES_BIN_DRIVER_MANAGER_SHUTDOWN_NODE_REMOVAL_TRACKER_H_

#include <lib/async/cpp/task.h>
#include <lib/fit/function.h>
#include <lib/zx/time.h>

#include <string>
#include <unordered_map>
#include <unordered_set>

#include "src/devices/bin/driver_manager/node_types.h"

namespace driver_manager {
class Node;

using NodeId = uint32_t;

struct NodeInfo {
  std::string name;
  std::string driver_url;
  Collection collection;
  NodeState state;
  std::weak_ptr<Node> node;
};

class NodeRemovalTracker {
 public:
  // Wait time between checking which nodes have not finished being removed.
  static constexpr zx::duration kRemovalCheckDelay = zx::sec(15);

  // Maximum number of times we check for nodes that have not being removed before considering the
  // removal process timed out.
  static constexpr uint32_t kMaxRemovalCheckCount = 3;

  explicit NodeRemovalTracker(async_dispatcher_t* dispatcher) : dispatcher_(dispatcher) {}

  NodeId RegisterNode(NodeInfo node);
  void Notify(NodeId id, NodeState state);

  void FinishEnumeration();

  void set_pkg_callback(fit::callback<void()> callback);
  void set_all_callback(fit::callback<void()> callback);

  void SetOnRemovalTimeoutCallback(fit::callback<void()> callback);

 private:
  void CheckRemoval();

  void CheckRemovalDone();

  size_t remaining_pkg_node_count() const { return remaining_pkg_nodes_.size(); }

  size_t remaining_node_count() const {
    return remaining_pkg_nodes_.size() + remaining_non_pkg_nodes_.size();
  }

  bool fully_enumerated_ = false;
  NodeId next_node_id_ = 0;

  uint32_t timeout_count_ = 0;

  std::unordered_set<NodeId> remaining_pkg_nodes_;
  std::unordered_set<NodeId> remaining_non_pkg_nodes_;
  std::unordered_map<NodeId, NodeInfo> nodes_;

  // Callback invoked when all package drivers have been removed.
  fit::callback<void()> pkg_callback_;

  // Callback invoked when all drivers have been removed.
  fit::callback<void()> all_callback_;

  // Callback invoked when waiting on the nodes to be removed has timed out.
  fit::callback<void()> on_removal_timeout_callback_;

  async_dispatcher_t* const dispatcher_;

  // Task used to dump diagnostic data if node removal is hanging. The task is started when
  // FinishEnumeration() is called and canceled when removal is complete.
  async::TaskClosureMethod<NodeRemovalTracker, &NodeRemovalTracker::CheckRemoval>
      check_removal_task_{this};
};

}  // namespace driver_manager
#endif  // SRC_DEVICES_BIN_DRIVER_MANAGER_SHUTDOWN_NODE_REMOVAL_TRACKER_H_
