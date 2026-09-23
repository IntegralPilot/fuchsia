// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/bin/driver_manager/pending_node_manager.h"

#include <fidl/fuchsia.driver.framework/cpp/wire.h>
#include <lib/fit/defer.h>
#include <zircon/assert.h>
#include <zircon/status.h>

#include "src/devices/bin/driver_manager/node.h"
#include "src/devices/bin/driver_manager/node_types.h"
#include "src/devices/bin/driver_manager/resource.h"
#include "src/devices/lib/log/log.h"

namespace driver_manager {

namespace {

struct ResourceWithProperties {
  std::shared_ptr<Resource> resource;
  std::unordered_map<std::string, fuchsia_driver_framework::NodePropertyValue> properties;
};

bool ResourceMatchesSelector(
    const Resource& resource,
    const std::unordered_map<std::string, fuchsia_driver_framework::NodePropertyValue>& res_props,
    const std::vector<PendingNodeManager::PreParsedOffer>& parsed_offers,
    const fuchsia_driver_framework::Selector& selector) {
  // Match the offers in the selector.
  for (const auto& sel_proc : parsed_offers) {
    bool found = false;
    for (const auto& res_offer : resource.offers()) {
      if (res_offer.service_name == sel_proc.service_name &&
          res_offer.transport == sel_proc.transport) {
        found = true;
        break;
      }
    }
    if (!found) {
      return false;
    }
  }

  // Match the include properties in the selector.
  if (selector.include_properties().has_value()) {
    for (const auto& sel_prop : selector.include_properties().value()) {
      auto it = res_props.find(sel_prop.key());
      if (it == res_props.end() || it->second != sel_prop.value()) {
        return false;
      }
    }
  }

  // Reject if the exclude properties are present in the resource.
  if (selector.exclude_properties().has_value()) {
    for (const auto& sel_prop : selector.exclude_properties().value()) {
      auto it = res_props.find(sel_prop.key());
      if (it != res_props.end() && it->second == sel_prop.value()) {
        return false;
      }
    }
  }

  return true;
}

bool TryResolveDependencies(PendingNodeManager::PendingNode& pending,
                            const std::vector<ResourceWithProperties>& resources) {
  if (!pending.node.dependencies().has_value() || pending.node.dependencies()->empty()) {
    return true;
  }
  const auto& deps = pending.node.dependencies().value();
  std::vector<std::shared_ptr<Resource>> matched(deps.size(), nullptr);
  std::vector<bool> resource_used(resources.size(), false);

  auto match_dep = [&](auto& self, size_t dep_idx) -> bool {
    if (dep_idx == matched.size()) {
      return true;
    }
    const auto& dep = deps[dep_idx];
    if (!dep.selector().has_value()) {
      return self(self, dep_idx + 1);
    }
    for (size_t res_idx = 0; res_idx < resources.size(); ++res_idx) {
      if (resource_used[res_idx]) {
        continue;
      }

      if (ResourceMatchesSelector(*resources[res_idx].resource, resources[res_idx].properties,
                                  pending.parsed_offers_per_dependency[dep_idx],
                                  dep.selector().value())) {
        resource_used[res_idx] = true;
        matched[dep_idx] = resources[res_idx].resource;
        if (self(self, dep_idx + 1)) {
          return true;
        }
        resource_used[res_idx] = false;
        matched[dep_idx] = nullptr;
      }
    }
    return false;
  };

  if (match_dep(match_dep, 0)) {
    pending.resolved_resources = std::move(matched);
    return true;
  }
  return false;
}

}  // namespace

PendingNodeManager::PendingNodeManager(NodeManager* node_manager, async_dispatcher_t* dispatcher)
    : node_manager_(node_manager), dispatcher_(dispatcher) {}

fit::result<fuchsia_driver_framework::NodeError> PendingNodeManager::AddNode(
    fuchsia_driver_framework::Node2 node,
    fidl::ServerEnd<fuchsia_driver_framework::NodeController> controller,
    fidl::ServerEnd<fuchsia_driver_framework::Node> node_ref) {
  ZX_ASSERT_MSG(node.dependencies().has_value() && !node.dependencies()->empty(),
                "Node %s must have dependencies to be added to PendingNodeManager",
                node.name().value_or("").c_str());
  fidl::Arena arena;
  std::string name = node.name().value_or("");
  std::vector<fuchsia_driver_framework::ParentSpec2> parents;
  std::vector<std::vector<PreParsedOffer>> parsed_offers_per_dependency;

  parsed_offers_per_dependency.reserve(node.dependencies()->size());
  for (const auto& dep : node.dependencies().value()) {
    std::vector<fuchsia_driver_framework::BindRule2> bind_rules;
    std::vector<fuchsia_driver_framework::NodeProperty2> properties;
    std::vector<PreParsedOffer> parsed_offers;

    if (dep.selector().has_value()) {
      const auto& selector = dep.selector().value();
      if (selector.include_properties().has_value()) {
        for (const auto& prop : selector.include_properties().value()) {
          bind_rules.push_back(fuchsia_driver_framework::BindRule2{{
              .key = prop.key(),
              .condition = fuchsia_driver_framework::Condition::kAccept,
              .values = {prop.value()},
          }});
          properties.push_back(fuchsia_driver_framework::NodeProperty2{{
              .key = prop.key(),
              .value = prop.value(),
          }});
        }
      }
      if (selector.exclude_properties().has_value()) {
        for (const auto& prop : selector.exclude_properties().value()) {
          bind_rules.push_back(fuchsia_driver_framework::BindRule2{{
              .key = prop.key(),
              .condition = fuchsia_driver_framework::Condition::kReject,
              .values = {prop.value()},
          }});
        }
      }
      if (selector.offers().has_value()) {
        for (const auto& offer : selector.offers().value()) {
          auto processed = ProcessNodeOffer(offer, Collection::kNone, "");
          if (processed.is_error()) {
            return fit::error(processed.error_value());
          }
          const auto& node_offer = processed.value();
          std::string expected_value =
              std::format("{}.{}", node_offer.service_name, node_offer.transport);
          bind_rules.push_back(fuchsia_driver_framework::BindRule2{{
              .key = node_offer.service_name,
              .condition = fuchsia_driver_framework::Condition::kAccept,
              .values = {fuchsia_driver_framework::NodePropertyValue::WithStringValue(
                  expected_value)},
          }});
          properties.push_back(fuchsia_driver_framework::NodeProperty2{{
              .key = node_offer.service_name,
              .value = fuchsia_driver_framework::NodePropertyValue::WithStringValue(expected_value),
          }});
          parsed_offers.push_back(PreParsedOffer{
              .service_name = node_offer.service_name,
              .transport = node_offer.transport,
          });
        }
      }
    }
    if (dep.tags().has_value()) {
      for (const auto& tag : dep.tags().value()) {
        properties.push_back(fuchsia_driver_framework::NodeProperty2{{
            .key = tag.key(),
            .value = tag.value(),
        }});
      }
    }
    parents.push_back(fuchsia_driver_framework::ParentSpec2{{
        .bind_rules = std::move(bind_rules),
        .properties = std::move(properties),
    }});
    parsed_offers_per_dependency.push_back(std::move(parsed_offers));
  }

  auto pending = std::make_shared<PendingNode>();
  pending->node = std::move(node);
  pending->controller = std::move(controller);
  pending->node_ref = std::move(node_ref);
  pending->parsed_offers_per_dependency = std::move(parsed_offers_per_dependency);
  pending->parents = parents;

  std::weak_ptr<PendingNode> pending_weak = pending;
  auto& pending_ref = *pending;
  pending_nodes_.push_back(std::move(pending));

  node_manager_->RequestMatchPendingNode(
      fidl::ToWire(arena, pending_ref.parents), [this, pending_weak](auto& result) mutable {
        auto pending_ptr = pending_weak.lock();
        if (!pending_ptr) {
          return;
        }

        if (!result.ok()) {
          fdf_log::error("DriverIndex::MatchPendingNode failed: {}", result.status());
        } else if (result->is_error()) {
          fdf_log::error("DriverIndex::MatchPendingNode returned error: {}",
                         zx_status_get_string(result->error_value()));
        } else {
          auto response = result.value();
          if (response->has_driver()) {
            pending_ptr->matched_driver = fidl::ToNatural(response->driver());
          }
        }

        node_manager_->TryResolvePendingNodes();
      });
  return fit::ok();
}

void PendingNodeManager::TryResolvePendingNodes(
    const std::unordered_map<ResourceId, std::weak_ptr<Resource>>& multibind_resources) {
  std::vector<ResourceWithProperties> resources;
  resources.reserve(multibind_resources.size());
  for (const auto& [id, resource_weak] : multibind_resources) {
    if (auto resource = resource_weak.lock()) {
      std::unordered_map<std::string, fuchsia_driver_framework::NodePropertyValue> res_props;
      res_props.reserve(resource->properties().size());
      for (const auto& res_prop : resource->properties()) {
        res_props.emplace(res_prop.key(), res_prop.value());
      }
      resources.push_back(ResourceWithProperties{
          .resource = std::move(resource),
          .properties = std::move(res_props),
      });
    }
  }

  // Resolve dependencies for each pending node. If the node is instantiated,
  // remove it from pending_nodes_.
  for (auto it = pending_nodes_.begin(); it != pending_nodes_.end();) {
    if (TryResolveDependencies(**it, resources)) {
      auto result = InstantiateNode(*it);
      if (result.is_error()) {
        it = pending_nodes_.erase(it);
      } else if (result.value()) {
        it = pending_nodes_.erase(it);
      } else {
        ++it;
      }
    } else {
      ++it;
    }
  }
}

zx::result<bool> PendingNodeManager::InstantiateNode(std::shared_ptr<PendingNode> pending) {
  std::vector<fuchsia_driver_framework::NodePropertyEntry2> parent_properties;
  std::vector<std::string> parents_names;
  std::vector<std::weak_ptr<Resource>> dependencies;

  bool has_matched_driver = pending->matched_driver.has_value();
  bool owned_by_parent = pending->node_ref.is_valid();

  if (!has_matched_driver && !owned_by_parent) {
    return zx::ok(false);
  }

  std::vector<std::string> matched_parent_names;
  uint32_t primary_index = 0;
  std::string driver_url;
  fuchsia_driver_framework::DriverPackageType package_type =
      fuchsia_driver_framework::DriverPackageType::kBase;

  if (has_matched_driver) {
    const auto& match = pending->matched_driver.value();
    if (match.parent_names().has_value()) {
      matched_parent_names = match.parent_names().value();
    }
    primary_index = match.primary_parent_index().value_or(0);
    if (match.composite_driver().has_value() &&
        match.composite_driver()->driver_info().has_value()) {
      const auto& driver_info = match.composite_driver()->driver_info().value();
      driver_url = driver_info.url().value_or("");
      package_type =
          driver_info.package_type().value_or(fuchsia_driver_framework::DriverPackageType::kBase);
    }
  }

  for (size_t i = 0; i < pending->resolved_resources.size(); ++i) {
    auto resource = pending->resolved_resources[i];
    std::string p_name = has_matched_driver && i < matched_parent_names.size()
                             ? matched_parent_names[i]
                             : resource->name();
    parents_names.push_back(p_name);
    dependencies.push_back(resource);

    std::vector<fuchsia_driver_framework::NodeProperty2> props = resource->properties();
    const auto& dep = pending->node.dependencies().value()[i];
    if (dep.tags().has_value()) {
      for (const auto& tag : dep.tags().value()) {
        fuchsia_driver_framework::NodeProperty2 prop{{
            .key = tag.key(),
            .value = tag.value(),
        }};
        props.push_back(std::move(prop));
      }
    }

    parent_properties.push_back(fuchsia_driver_framework::NodePropertyEntry2{{
        .name = p_name,
        .properties = std::move(props),
    }});
  }

  auto composite_res = Node::CreateCompositeNode(
      pending->node.name().value(), std::move(dependencies), std::move(parents_names),
      parent_properties, node_manager_, dispatcher_, "", primary_index);
  if (composite_res.is_error()) {
    fdf_log::error("Failed to create composite node for Node2: {}",
                   zx_status_get_string(composite_res.error_value()));
    return composite_res.take_error();
  }

  auto node = composite_res.value();
  if (pending->controller.is_valid()) {
    node->SetController(std::move(pending->controller));
  }

  if (pending->node_ref.is_valid()) {
    node->SetOwnedByParent(std::move(pending->node_ref));
  } else {
    if (has_matched_driver && !driver_url.empty()) {
      auto start_result = node_manager_->StartDriver(*node, driver_url, package_type);
      if (start_result.is_error()) {
        fdf_log::error("Failed to start driver '{}': {}", node->name(),
                       zx_status_get_string(start_result.error_value()));
      }
    }
  }
  return zx::ok(true);
}

void PendingNodeManager::MatchPendingNodesWithoutDriver() {
  for (auto& pending : pending_nodes_) {
    if (pending->matched_driver.has_value()) {
      continue;
    }

    std::weak_ptr<PendingNode> pending_weak = pending;
    fidl::Arena arena;
    node_manager_->RequestMatchPendingNode(
        fidl::ToWire(arena, pending->parents), [this, pending_weak](auto& result) mutable {
          auto pending_ptr = pending_weak.lock();
          if (!pending_ptr) {
            return;
          }

          if (!result.ok()) {
            fdf_log::error("DriverIndex::MatchPendingNode failed: {}", result.status());
          } else if (result->is_error()) {
            fdf_log::info("DriverIndex::MatchPendingNode returned error: {}",
                          zx_status_get_string(result->error_value()));
          } else {
            auto response = result.value();
            if (response->has_driver()) {
              pending_ptr->matched_driver = fidl::ToNatural(response->driver());
            }
          }

          node_manager_->TryResolvePendingNodes();
        });
  }
}

}  // namespace driver_manager
