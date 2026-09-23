// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BIN_DRIVER_MANAGER_COMPOSITE_PARENT_SET_COLLECTOR_H_
#define SRC_DEVICES_BIN_DRIVER_MANAGER_COMPOSITE_PARENT_SET_COLLECTOR_H_

#include <fidl/fuchsia.driver.development/cpp/fidl.h>
#include <fidl/fuchsia.driver.framework/cpp/fidl.h>
#include <fidl/fuchsia.driver.index/cpp/wire.h>

#include <vector>

#include "src/devices/bin/driver_manager/node.h"

namespace driver_manager {
class Resource;

using NodeWkPtr = std::weak_ptr<Node>;
using ResourceWkPtr = std::weak_ptr<Resource>;

// |ParentSetCollector| wraps functionality for collecting multiple parent nodes for composites.
// The parent set starts out empty and gets nodes added to it until it is complete. Once complete
// it will return a vector containing all the parent node pointers.
class ParentSetCollector {
 public:
  explicit ParentSetCollector(size_t size, std::string_view driver_host_name_for_colocation)
      : parents_(size),
        parent_properties_(size),
        driver_host_name_for_colocation_(driver_host_name_for_colocation) {}

  void BindToComposite(std::vector<std::string> parent_names, uint32_t primary_index) {
    parent_names_ = std::move(parent_names);
    primary_index_ = primary_index;
  }

  bool HasCompositeInfo() const {
    return primary_index_ != std::nullopt && parent_names_ != std::nullopt;
  }

  // Only a weak_ptr of the resource is stored by this class (until collection in TryToAssemble).
  zx::result<> AddResource(
      uint32_t index, const std::vector<fuchsia_driver_framework::NodeProperty2>& node_properties,
      ResourceWkPtr resource);

  void ReleaseNodes();

  // Check if all parents are found. If so, then create and return the composite node. If the
  // node is already created, return ZX_ERR_ALREADY_EXISTS.
  zx::result<std::shared_ptr<Node>> TryToAssemble(std::string_view name, NodeManager* node_manager,
                                                  async_dispatcher_t* dispatcher);

  fuchsia_driver_development::wire::CompositeNodeInfo GetCompositeInfo(
      fidl::AnyArena& arena,
      const std::optional<fuchsia_driver_framework::CompositeInfo>& composite_info) const;

  fidl::VectorView<fidl::StringView> GetParentTopologicalPaths(fidl::AnyArena& arena) const;

  fidl::VectorView<fidl::StringView> GetParentMonikers(fidl::AnyArena& arena) const;

  const std::optional<ResourceWkPtr>& get(uint32_t index) const { return parents_[index]; }

  std::optional<std::weak_ptr<Node>> completed_composite_node() const {
    return completed_composite_node_;
  }

  size_t size() const { return parents_.size(); }

  // Exposed for testing.
  const std::vector<std::optional<ResourceWkPtr>>& parents() const { return parents_; }

 private:
  // Resources are stored as weak_ptrs. Only when trying to collect the completed set are they
  // locked into shared_ptrs and validated to not be null.
  std::vector<std::optional<ResourceWkPtr>> parents_;

  std::vector<fuchsia_driver_framework::NodePropertyEntry2> parent_properties_;

  std::optional<uint32_t> primary_index_;
  std::optional<std::vector<std::string>> parent_names_;

  std::string driver_host_name_for_colocation_;

  // Contains a weak pointer to the composite node when the parent set is assembled.
  std::optional<std::weak_ptr<driver_manager::Node>> completed_composite_node_;
};

}  // namespace driver_manager

#endif  // SRC_DEVICES_BIN_DRIVER_MANAGER_COMPOSITE_PARENT_SET_COLLECTOR_H_
