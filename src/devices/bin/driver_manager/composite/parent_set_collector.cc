// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/bin/driver_manager/composite/parent_set_collector.h"

#include <fidl/fuchsia.driver.framework/cpp/fidl.h>

#include "src/devices/bin/driver_manager/resource.h"
#include "src/devices/lib/log/log.h"

namespace driver_manager {

zx::result<> ParentSetCollector::AddResource(
    uint32_t index, const std::vector<fuchsia_driver_framework::NodeProperty2>& node_properties,
    ResourceWkPtr resource) {
  ZX_ASSERT(HasCompositeInfo());
  ZX_ASSERT(index < parents_.size());

  if (parents_[index] != std::nullopt && !parents_[index]->expired()) {
    return zx::error(ZX_ERR_ALREADY_BOUND);
  }
  parents_[index] = std::move(resource);
  parent_properties_[index] =
      fuchsia_driver_framework::NodePropertyEntry2(parent_names_.value()[index], node_properties);

  if (auto resource_ptr = parents_[index]->lock(); resource_ptr) {
    if (resource_ptr->is_self_resource()) {
      if (auto node_ptr = resource_ptr->owner().lock(); node_ptr) {
        node_ptr->MarkAsCompositeParent();
      }
    }
  }

  return zx::ok();
}

void ParentSetCollector::ReleaseNodes() {
  for (auto& resource : parents_) {
    if (resource == std::nullopt) {
      continue;
    }
    if (auto resource_ptr = resource->lock(); resource_ptr) {
      if (resource_ptr->is_self_resource()) {
        if (auto node_ptr = resource_ptr->owner().lock(); node_ptr) {
          node_ptr->UnmarkAsCompositeParent();
        }
      }
    }
    resource.reset();
  }
}

zx::result<std::shared_ptr<Node>> ParentSetCollector::TryToAssemble(
    std::string_view name, NodeManager* node_manager, async_dispatcher_t* dispatcher) {
  ZX_ASSERT(HasCompositeInfo());
  if (completed_composite_node_ && !completed_composite_node_->expired()) {
    return zx::error(ZX_ERR_ALREADY_EXISTS);
  }

  std::vector<ResourceWkPtr> parent_resources;
  parent_resources.reserve(parents_.size());
  for (auto& resource_opt : parents_) {
    if (resource_opt == std::nullopt) {
      return zx::error(ZX_ERR_SHOULD_WAIT);
    }
    parent_resources.push_back(resource_opt.value());
  }

  auto result = Node::CreateCompositeNode(
      std::string(name), parent_resources, parent_names_.value(), parent_properties_, node_manager,
      dispatcher, driver_host_name_for_colocation_, primary_index_.value());
  if (result.is_error()) {
    return result.take_error();
  }

  fdf_log::info("Built composite node '{}' for completed composite node spec", name);
  completed_composite_node_.emplace(result.value());
  return zx::ok(result.value());
}

fuchsia_driver_development::wire::CompositeNodeInfo ParentSetCollector::GetCompositeInfo(
    fidl::AnyArena& arena,
    const std::optional<fuchsia_driver_framework::CompositeInfo>& composite_info) const {
  namespace fdd = fuchsia_driver_development;

  auto composite_node_info = fdd::wire::CompositeNodeInfo::Builder(arena);

  if (composite_info.has_value()) {
    composite_node_info.composite(fdd::wire::CompositeInfo::WithComposite(
        arena, fidl::ToWire(arena, composite_info.value())));
  }

  if (!HasCompositeInfo()) {
    fidl::VectorView<fidl::StringView> parent_topological_paths(arena, size());
    composite_node_info.parent_topological_paths(parent_topological_paths);
    composite_node_info.parent_monikers(fidl::VectorView<fidl::StringView>(arena, size()));
    return composite_node_info.Build();
  }

  composite_node_info.parent_topological_paths(GetParentTopologicalPaths(arena));
  composite_node_info.parent_monikers(GetParentMonikers(arena));

  std::optional<NodeWkPtr> composite_node = completed_composite_node();
  if (composite_node) {
    if (auto node_ptr = composite_node->lock(); node_ptr) {
      composite_node_info.topological_path(node_ptr->MakeTopologicalPath());
      composite_node_info.moniker(fidl::StringView(arena, node_ptr->MakeComponentMoniker()));
    }
  }
  return composite_node_info.Build();
}

fidl::VectorView<fidl::StringView> ParentSetCollector::GetParentTopologicalPaths(
    fidl::AnyArena& arena) const {
  fidl::VectorView<fidl::StringView> parent_topological_paths(arena, parents_.size());
  for (uint32_t i = 0; i < parents_.size(); i++) {
    if (parents_[i] == std::nullopt) {
      parent_topological_paths[i] = fidl::StringView();
      continue;
    }

    if (auto resource = parents_[i]->lock(); resource) {
      if (auto node = resource->owner().lock(); node) {
        parent_topological_paths[i] = fidl::StringView(arena, node->MakeTopologicalPath());
        continue;
      }
    }
    parent_topological_paths[i] = fidl::StringView();
  }
  return parent_topological_paths;
}

fidl::VectorView<fidl::StringView> ParentSetCollector::GetParentMonikers(
    fidl::AnyArena& arena) const {
  fidl::VectorView<fidl::StringView> parent_monikers(arena, parents_.size());
  for (uint32_t i = 0; i < parents_.size(); i++) {
    if (parents_[i] == std::nullopt) {
      parent_monikers[i] = fidl::StringView();
      continue;
    }

    if (auto resource = parents_[i]->lock(); resource) {
      if (auto node = resource->owner().lock(); node) {
        parent_monikers[i] = fidl::StringView(arena, node->MakeComponentMoniker());
        continue;
      }
    }
    parent_monikers[i] = fidl::StringView();
  }
  return parent_monikers;
}

}  // namespace driver_manager
