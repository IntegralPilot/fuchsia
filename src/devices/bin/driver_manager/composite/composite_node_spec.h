// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BIN_DRIVER_MANAGER_COMPOSITE_COMPOSITE_NODE_SPEC_H_
#define SRC_DEVICES_BIN_DRIVER_MANAGER_COMPOSITE_COMPOSITE_NODE_SPEC_H_

#include <fidl/fuchsia.driver.development/cpp/wire.h>
#include <fidl/fuchsia.driver.index/cpp/fidl.h>
#include <lib/inspect/cpp/inspector.h>

#include "src/devices/bin/driver_manager/composite/parent_set_collector.h"
#include "src/devices/bin/driver_manager/resource.h"

namespace driver_manager {
class Node;
class Resource;

using NodeWkPtr = std::weak_ptr<Node>;
using RemoveCompositeNodeCallback = fit::callback<void(zx::result<>)>;

struct CompositeNodeSpecCreateInfo {
  std::string name;
  std::vector<fuchsia_driver_framework::ParentSpec2> parents;
  std::string driver_host_name_for_colocation;
};

// This partially abstract class represents a composite node spec and is responsible for managing
// its state and composite node. The CompositeNodeSpec class will manage the state of its bound
// nodes while its subclasses manage the composite node under the spec.
class CompositeNodeSpec {
 public:
  explicit CompositeNodeSpec(CompositeNodeSpecCreateInfo create_info,
                             async_dispatcher_t* dispatcher, NodeManager* node_manager);

  virtual ~CompositeNodeSpec() = default;

  // Called when CompositeNodeManager receives a MatchedNodeRepresentation.
  // Return ZX_ERR_ALREADY_BOUND if it's already bound. If the composite is complete, return
  // a pointer to the new node. Otherwise, return a std::nullopt. The lifetime of this
  // node object is managed by the parent nodes. Virtual for testing.
  virtual zx::result<std::optional<NodeWkPtr>> BindParent(
      fuchsia_driver_framework::wire::CompositeParent composite_parent,
      const ResourceWkPtr& resource);

  virtual fuchsia_driver_development::wire::CompositeNodeInfo GetCompositeInfo(
      fidl::AnyArena& arena) const;

  void RecordInspect(inspect::Node& spec_inspect_node) const;

  // Remove the underlying composite node and unmatch all of its parents. Called for
  // rebind. Virtual for testing.
  virtual void Remove(RemoveCompositeNodeCallback callback);

  const std::vector<fuchsia_driver_framework::ParentSpec2>& parent_specs() const {
    return parent_specs_;
  }

  const std::string& name() const { return name_; }
  const std::string& driver_host_name_for_colocation() const {
    return driver_host_name_for_colocation_;
  }

  std::optional<NodeWkPtr> completed_composite_node() const {
    return parent_set_collector_.completed_composite_node();
  }

  // Exposed for testing.
  virtual const std::vector<std::optional<ResourceWkPtr>>& GetParentResources() const {
    return parent_set_collector_.parents();
  }

  virtual std::vector<std::optional<NodeWkPtr>> GetParentNodes() const {
    std::vector<std::optional<NodeWkPtr>> parent_nodes;
    parent_nodes.reserve(parent_set_collector_.parents().size());
    for (const auto& parent_resource : parent_set_collector_.parents()) {
      if (parent_resource == std::nullopt) {
        parent_nodes.push_back(std::nullopt);
      } else if (auto resource = parent_resource->lock()) {
        parent_nodes.push_back(resource->owner());
      } else {
        parent_nodes.push_back(std::nullopt);
      }
    }
    return parent_nodes;
  }

 private:
  std::string name_;
  std::string driver_host_name_for_colocation_;

  ParentSetCollector parent_set_collector_;

  std::string driver_url_;

  async_dispatcher_t* const dispatcher_;
  NodeManager* node_manager_;

  std::vector<fuchsia_driver_framework::ParentSpec2> parent_specs_;

  // Store our composite_info for easy responses to GetCompositeInfo.
  // This is set the first time |BindParentImpl| is called.
  std::optional<fuchsia_driver_framework::CompositeInfo> composite_info_ = std::nullopt;
};

}  // namespace driver_manager

#endif  // SRC_DEVICES_BIN_DRIVER_MANAGER_COMPOSITE_COMPOSITE_NODE_SPEC_H_
