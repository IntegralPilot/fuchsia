// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/bin/driver_manager/composite/composite_node_spec.h"

#include <bind/fuchsia/cpp/bind.h>

#include "src/devices/bin/driver_manager/node_property_conversion.h"
#include "src/devices/bin/driver_manager/resource.h"
#include "src/devices/lib/log/log.h"

namespace fdd = fuchsia_driver_development;

namespace driver_manager {

CompositeNodeSpec::CompositeNodeSpec(CompositeNodeSpecCreateInfo create_info,
                                     async_dispatcher_t* dispatcher, NodeManager* node_manager)
    : name_(create_info.name),
      driver_host_name_for_colocation_(create_info.driver_host_name_for_colocation),
      parent_set_collector_(create_info.parents.size(),
                            create_info.driver_host_name_for_colocation),
      dispatcher_(dispatcher),
      node_manager_(node_manager) {
  parent_specs_ = std::move(create_info.parents);
}

zx::result<std::optional<NodeWkPtr>> CompositeNodeSpec::BindParent(
    fuchsia_driver_framework::wire::CompositeParent composite_parent,
    const ResourceWkPtr& resource) {
  ZX_ASSERT(composite_parent.has_index());
  auto node_index = composite_parent.index();
  if (node_index >= parent_set_collector_.size()) {
    return zx::error(ZX_ERR_OUT_OF_RANGE);
  }

  if (!composite_info_.has_value()) {
    ZX_ASSERT(composite_parent.has_composite());
    auto composite = fidl::ToNatural(composite_parent.composite());
    composite_info_ = composite;
  }

  auto& spec = composite_info_->spec();
  auto& matched_driver = composite_info_->matched_driver();

  ZX_ASSERT(spec.has_value() && spec->name().has_value() && matched_driver.has_value() &&
            matched_driver->composite_driver().has_value() &&
            matched_driver->composite_driver()->driver_info().has_value() &&
            matched_driver->composite_driver()->driver_info()->url().has_value() &&
            matched_driver->parent_names().has_value());

  const auto& composite = matched_driver->composite_driver();
  const auto& driver_info = composite->driver_info();
  auto spec_name_value = spec->name().value();
  auto& parent_names = matched_driver->parent_names().value();
  auto& primary_index = matched_driver->primary_parent_index();

  if (!parent_set_collector_.HasCompositeInfo()) {
    parent_set_collector_.BindToComposite(parent_names, primary_index.value_or(0));
    driver_url_ = driver_info->url().value();
  }

  std::vector<fuchsia_driver_framework::NodeProperty2> properties =
      parent_specs()[composite_parent.index()].properties();

  for (const auto& prop : properties) {
    if (prop.key() == bind_fuchsia::NAME) {
      if (const auto& str_val = prop.value().string_value(); str_val.has_value()) {
        if (str_val.value() != parent_names[node_index]) {
          fdf_log::error(
              "Parent property fuchsia.NAME '{}' does not match composite parent name '{}'",
              str_val.value(), parent_names[node_index]);
          return zx::error(ZX_ERR_INVALID_ARGS);
        }
      }
    }
  }

  zx::result<> add_result =
      parent_set_collector_.AddResource(composite_parent.index(), properties, resource);
  if (add_result.is_error()) {
    return add_result.take_error();
  }

  auto composite_node = parent_set_collector_.TryToAssemble(name_, node_manager_, dispatcher_);
  if (composite_node.is_error()) {
    if (composite_node.status_value() != ZX_ERR_SHOULD_WAIT) {
      return composite_node.take_error();
    }
    return zx::ok(std::nullopt);
  }
  return zx::ok(composite_node.value());
}

void CompositeNodeSpec::Remove(RemoveCompositeNodeCallback callback) {
  parent_set_collector_.ReleaseNodes();

  // TODO(https://fxbug.dev/42075799): Once we start enforcing the multibind composite flag, move
  // the parent nodes back to the orphaned nodes if they can't multibind.
  auto node = parent_set_collector_.completed_composite_node();
  if (node && !node->expired()) {
    node->lock()->RemoveCompositeNodeForRebind(std::move(callback));
    parent_set_collector_ =
        ParentSetCollector(parent_specs_.size(), driver_host_name_for_colocation());
    driver_url_ = "";
    composite_info_.reset();
    return;
  }

  parent_set_collector_ =
      ParentSetCollector(parent_specs_.size(), driver_host_name_for_colocation());
  driver_url_ = "";
  composite_info_.reset();
  callback(zx::ok());
}

fdd::wire::CompositeNodeInfo CompositeNodeSpec::GetCompositeInfo(fidl::AnyArena& arena) const {
  if (composite_info_.has_value()) {
    return parent_set_collector_.GetCompositeInfo(arena, composite_info_);
  }
  fuchsia_driver_framework::CompositeInfo info;
  fuchsia_driver_framework::CompositeNodeSpec spec;
  spec.name(name_);
  info.spec(std::move(spec));
  return parent_set_collector_.GetCompositeInfo(arena, info);
}

void CompositeNodeSpec::RecordInspect(inspect::Node& spec_inspect_node) const {
  spec_inspect_node.RecordString("name", name_);
  if (!driver_host_name_for_colocation_.empty()) {
    spec_inspect_node.RecordString("driver_host", driver_host_name_for_colocation_);
  }
  if (!driver_url_.empty()) {
    spec_inspect_node.RecordString("driver_url", driver_url_);
  }

  auto parents_node = spec_inspect_node.CreateChild("parents");
  for (size_t p_idx = 0; p_idx < parent_specs_.size(); ++p_idx) {
    const auto& parent = parent_specs_[p_idx];
    auto parent_node = parents_node.CreateChild(std::to_string(p_idx));

    // Bind rules
    if (!parent.bind_rules().empty()) {
      auto bind_rules_node = parent_node.CreateChild("bind_rules");
      for (size_t r_idx = 0; r_idx < parent.bind_rules().size(); ++r_idx) {
        const auto& rule = parent.bind_rules()[r_idx];
        auto rule_node = bind_rules_node.CreateChild(std::to_string(r_idx));
        rule_node.RecordString("key", rule.key());
        rule_node.RecordString(
            "condition",
            rule.condition() == fuchsia_driver_framework::Condition::kAccept ? "ACCEPT" : "REJECT");

        auto values_array = rule_node.CreateStringArray("values", rule.values().size());
        for (size_t v_idx = 0; v_idx < rule.values().size(); ++v_idx) {
          const auto& val = rule.values()[v_idx];
          if (val.int_value().has_value()) {
            values_array.Set(v_idx, std::to_string(val.int_value().value()));
          } else if (val.string_value().has_value()) {
            values_array.Set(v_idx, val.string_value().value());
          } else if (val.bool_value().has_value()) {
            values_array.Set(v_idx, val.bool_value().value() ? "true" : "false");
          } else if (val.enum_value().has_value()) {
            values_array.Set(v_idx, val.enum_value().value());
          }
        }
        rule_node.Record(std::move(values_array));
        bind_rules_node.Record(std::move(rule_node));
      }
      parent_node.Record(std::move(bind_rules_node));
    }

    // Properties
    if (!parent.properties().empty()) {
      auto properties_node = parent_node.CreateChild("properties");
      for (size_t prop_idx = 0; prop_idx < parent.properties().size(); ++prop_idx) {
        const auto& prop = parent.properties()[prop_idx];
        auto entry_node = properties_node.CreateChild(std::to_string(prop_idx));
        entry_node.RecordString("key", prop.key());
        if (prop.value().int_value().has_value()) {
          entry_node.RecordUint("value", prop.value().int_value().value());
        } else if (prop.value().string_value().has_value()) {
          entry_node.RecordString("value", prop.value().string_value().value());
        } else if (prop.value().bool_value().has_value()) {
          entry_node.RecordBool("value", prop.value().bool_value().value());
        } else if (prop.value().enum_value().has_value()) {
          entry_node.RecordString("value", prop.value().enum_value().value());
        }
        properties_node.Record(std::move(entry_node));
      }
      parent_node.Record(std::move(properties_node));
    }

    parents_node.Record(std::move(parent_node));
  }
  spec_inspect_node.Record(std::move(parents_node));
}

}  // namespace driver_manager
