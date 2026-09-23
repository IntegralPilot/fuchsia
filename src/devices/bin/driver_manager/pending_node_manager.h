// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BIN_DRIVER_MANAGER_PENDING_NODE_MANAGER_H_
#define SRC_DEVICES_BIN_DRIVER_MANAGER_PENDING_NODE_MANAGER_H_

#include <fidl/fuchsia.driver.framework/cpp/fidl.h>
#include <lib/async/dispatcher.h>
#include <lib/fit/result.h>
#include <lib/zx/result.h>

#include <memory>
#include <unordered_map>
#include <vector>

#include "src/devices/bin/driver_manager/node_types.h"

namespace driver_manager {

class Node;
class NodeManager;
class Resource;

class PendingNodeManager {
 public:
  struct PreParsedOffer {
    std::string service_name;
    OfferTransport transport;
  };

  struct PendingNode {
    fuchsia_driver_framework::Node2 node;
    fidl::ServerEnd<fuchsia_driver_framework::NodeController> controller;
    fidl::ServerEnd<fuchsia_driver_framework::Node> node_ref;
    std::vector<std::shared_ptr<Resource>> resolved_resources;
    std::optional<fuchsia_driver_framework::CompositeDriverMatch> matched_driver;
    std::vector<std::vector<PreParsedOffer>> parsed_offers_per_dependency;
    std::vector<fuchsia_driver_framework::ParentSpec2> parents;
  };

  PendingNodeManager(NodeManager* node_manager, async_dispatcher_t* dispatcher);

  fit::result<fuchsia_driver_framework::NodeError> AddNode(
      fuchsia_driver_framework::Node2 node,
      fidl::ServerEnd<fuchsia_driver_framework::NodeController> controller,
      fidl::ServerEnd<fuchsia_driver_framework::Node> node_ref);

  void TryResolvePendingNodes(
      const std::unordered_map<ResourceId, std::weak_ptr<Resource>>& multibind_resources);

  void MatchPendingNodesWithoutDriver();

  // Exposed for testing.
  const std::vector<std::shared_ptr<PendingNode>>& pending_nodes() const { return pending_nodes_; }

 private:
  // Attempts to instantiate the pending node.
  // Returns true if the composite node was successfully assembled and added to the node tree.
  // Returns false if the pending node does not have a matched driver yet.
  // Returns an error if composite node creation fails.
  zx::result<bool> InstantiateNode(std::shared_ptr<PendingNode> pending);

  NodeManager* node_manager_;
  async_dispatcher_t* dispatcher_;
  std::vector<std::shared_ptr<PendingNode>> pending_nodes_;
};

}  // namespace driver_manager

#endif  // SRC_DEVICES_BIN_DRIVER_MANAGER_PENDING_NODE_MANAGER_H_
