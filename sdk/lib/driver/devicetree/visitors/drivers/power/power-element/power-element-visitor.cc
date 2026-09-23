// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "power-element-visitor.h"

#include <lib/driver/devicetree/visitors/registration.h>
#include <lib/driver/logging/cpp/logger.h>

#include <algorithm>
#include <optional>
#include <regex>
#include <string>
#include <vector>

namespace {
using fuchsia_hardware_power::CpuPowerElement;
using fuchsia_hardware_power::LevelTuple;
using fuchsia_hardware_power::ParentElement;
using fuchsia_hardware_power::PowerDependency;
using fuchsia_hardware_power::PowerElement;
using fuchsia_hardware_power::PowerElementConfiguration;
using fuchsia_hardware_power::PowerLevel;
using fuchsia_hardware_power::SagElement;
using fuchsia_hardware_power::Transition;
}  // namespace

namespace power_element_visitor_dt {

PowerElementVisitor::PowerElementVisitor() {
  fdf_devicetree::Properties level_properties = {};
  level_properties.emplace_back(
      std::make_unique<fdf_devicetree::Uint32Property>(kLevel, /* required */ true));
  level_properties.emplace_back(std::make_unique<fdf_devicetree::ReferenceProperty>(
      kLevelDependencies, 0u, /* required */ false));
  level_parser_ = std::make_unique<fdf_devicetree::PropertyParser>(std::move(level_properties));

  fdf_devicetree::Properties transition_properties = {};
  transition_properties.emplace_back(
      std::make_unique<fdf_devicetree::Uint32Property>(kTargetLevel, true));
  transition_properties.emplace_back(
      std::make_unique<fdf_devicetree::Uint32Property>(kLatencyUs, /* required */ false));
  transition_parser_ =
      std::make_unique<fdf_devicetree::PropertyParser>(std::move(transition_properties));

  fdf_devicetree::Properties reference_properties = {};
  reference_properties.emplace_back(
      std::make_unique<fdf_devicetree::ReferenceProperty>("power-elements", 0u, false));
  reference_parser_ =
      std::make_unique<fdf_devicetree::PropertyParser>(std::move(reference_properties));
}

std::optional<std::string> PowerElementVisitor::GetElementName(const std::string& node_name) {
  std::smatch match;
  std::regex name_regex("(^[a-zA-Z0-9-]*)-element$");
  if (std::regex_search(node_name, match, name_regex) && match.size() == 2) {
    return match[1];
  }
  return std::nullopt;
}

zx::result<> PowerElementVisitor::Visit(fdf_devicetree::Node& node,
                                        const devicetree::PropertyDecoder& decoder) {
  auto result = reference_parser_->Parse(node);
  if (result.is_error()) {
    return zx::ok();
  }
  auto refs = result->Get<fdf_devicetree::References>("power-elements");
  if (!refs) {
    return zx::ok();
  }
  auto power_elements_node = (*refs)[0].reference_node().GetNode();
  for (auto& child : power_elements_node->children()) {
    auto element_config = ParsePowerElement(*child.GetNode());
    if (element_config.is_error()) {
      return element_config.take_error();
    }
    fdf::debug("Added power element '{}' to node '{}'.", *element_config->element()->name(),
               node.name());

    node.AddPowerConfig(*element_config);
  }

  return zx::ok();
}

zx::result<PowerElementConfiguration> PowerElementVisitor::ParsePowerElement(
    fdf_devicetree::Node& node) {
  PowerElementConfiguration element_config;
  element_config.element() = PowerElement();
  element_config.element()->name() = GetElementName(node.name());

  if (!element_config.element()->name()) {
    fdf::error("Power element has invalid node name '{}'.", node.name());

    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  auto children = node.children();
  if (children.size() != 1u || children[0].name() != "power-levels") {
    fdf::error(
        "Power element has invalid child nodes '{}'. Expecting a single child node named 'power-levels'.",
        node.name());

    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  element_config.element()->levels() = std::vector<PowerLevel>();

  // Parse the power-level nodes.
  auto power_level_nodes = children[0].GetNode()->children();
  for (auto& child : power_level_nodes) {
    zx::result result = ParseLevel(*child.GetNode(), element_config);
    if (result.is_error()) {
      return result.take_error();
    }
  }
  return zx::ok(element_config);
}

std::optional<std::string> PowerElementVisitor::GetLevelName(const std::string& node_name) {
  std::smatch match;
  std::regex name_regex("(^[a-zA-Z0-9-]*)-level$");
  if (std::regex_search(node_name, match, name_regex) && match.size() == 2) {
    return match[1];
  }
  return std::nullopt;
}

std::optional<ParentElement> PowerElementVisitor::GetParentElementFromLevelRef(
    fdf_devicetree::ReferenceNode& level_in_parent) {
  // Reference node points to a specific power level node. The parent will be power-levels node.
  // It's parent will be the power element node.
  if (!level_in_parent.parent() /*power-levels node*/ ||
      !level_in_parent.parent().parent() /*power element x*/ ||
      !level_in_parent.parent().parent().parent() /*power-elements node*/) {
    fdf::error("Power level reference node '{}' should be under a power-element node.",
               level_in_parent.name());

    return std::nullopt;
  }

  // |level_in_parent| points to a x-element/power-levels/x-level. Get the pointer to the power
  // element.
  auto power_element = level_in_parent.parent().parent();
  auto power_elements_node = power_element.parent();

  // Check if the power element is part of a specific device type.

  if (power_elements_node.name() == "sag-power-elements") {
    if (power_element.name() == "execution-state-element") {
      return ParentElement::WithSag(SagElement::kExecutionState);
    }
    if (power_element.name() == "application-activity-element") {
      return ParentElement::WithSag(SagElement::kApplicationActivity);
    }
    fdf::error("Power level reference node '{}' is an invalid SAG element '{}'.",
               level_in_parent.name(), power_element.name());

    return std::nullopt;
  }

  if (power_elements_node.name() == "cpu-power-elements") {
    if (power_element.name() == "cpu-element") {
      return ParentElement::WithCpuControl(CpuPowerElement::kCpu);
    }
  }

  auto parent_name = GetElementName(power_element.name());
  if (!parent_name) {
    fdf::error("Power level reference node '{}' has an invalid element name '{}'.",
               level_in_parent.name(), power_element.name());

    return std::nullopt;
  }

  return ParentElement::WithInstanceName(*parent_name);
}

PowerDependency& PowerElementVisitor::GetPowerDependency(PowerElementConfiguration& element_config,
                                                         const std::string& child_name,
                                                         const ParentElement& parent) {
  if (!element_config.dependencies()) {
    element_config.dependencies() = std::vector<PowerDependency>();
  }

  // Check if the dependency already exists.
  for (auto& dependency : *element_config.dependencies()) {
    if (dependency.child() == child_name && dependency.parent() == parent) {
      return dependency;
    }
  }

  PowerDependency dependency;
  dependency.child() = child_name;
  dependency.parent() = parent;
  dependency.level_deps() = std::vector<LevelTuple>();
  element_config.dependencies()->push_back(dependency);
  return element_config.dependencies()->back();
}

zx::result<> PowerElementVisitor::ParseLevel(fdf_devicetree::Node& node,
                                             PowerElementConfiguration& element_config) {
  PowerLevel level;
  level.name() = GetLevelName(node.name());

  if (!level.name()) {
    fdf::error("Power element has invalid node name '{}'.", node.name());

    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  zx::result parser_output = level_parser_->Parse(node);
  if (parser_output.is_error()) {
    fdf::error("Power level parse failed for node '{}' : {}", node.name(), parser_output);

    return parser_output.take_error();
  }

  level.level() = *parser_output->Get<uint32_t>(kLevel);

  // Parse level dependencies if exists.
  if (auto dependencies = parser_output->Get<fdf_devicetree::References>(kLevelDependencies)) {
    for (auto& entry : *dependencies) {
      auto parent_level_ref = entry.reference_node();
      auto parent_element = GetParentElementFromLevelRef(parent_level_ref);
      if (!parent_element) {
        return zx::error(ZX_ERR_INVALID_ARGS);
      }

      PowerDependency& dependency =
          GetPowerDependency(element_config, *element_config.element()->name(), *parent_element);

      LevelTuple level_tuple;
      level_tuple.child_level() = level.level();

      auto parent_level = parent_level_ref.GetProperty<uint32_t>(kLevel);
      if (parent_level.is_error()) {
        fdf::error("Power level reference node '{}' has no level property: {}",
                   parent_level_ref.name(), parent_level);

        return parent_level.take_error();
      }
      level_tuple.parent_level() = *parent_level;

      dependency.level_deps()->push_back(level_tuple);
    }
  }

  // Parse transition table if exists.
  if (!node.children().empty()) {
    if (node.children().size() != 1u || node.children()[0].name() != "level-transition-table") {
      fdf::error(
          "Power level has invalid child nodes '{}'. Expecting a single child node named 'level-transition-table'.",
          node.name());

      return zx::error(ZX_ERR_INVALID_ARGS);
    }

    zx::result result = ParseTransitionTable(*node.children()[0].GetNode(), level);
    if (result.is_error()) {
      return result.take_error();
    }
  }

  element_config.element()->levels()->push_back(level);
  return zx::ok();
}

zx::result<> PowerElementVisitor::ParseTransitionTable(fdf_devicetree::Node& node,
                                                       PowerLevel& level) {
  level.transitions() = std::vector<Transition>();
  for (auto& child : node.children()) {
    auto parser_output = transition_parser_->Parse(*child.GetNode());
    if (parser_output.is_error()) {
      fdf::error("Failed to parse transition entry '{}'", child.name());

      return parser_output.take_error();
    }
    Transition transition;
    transition.target_level() = *parser_output->Get<uint32_t>(kTargetLevel);
    if (auto latency = parser_output->Get<uint32_t>(kLatencyUs)) {
      transition.latency_us() = *latency;
    }
    level.transitions()->push_back(transition);
  }
  return zx::ok();
}

}  // namespace power_element_visitor_dt

REGISTER_DEVICETREE_VISITOR(power_element_visitor_dt::PowerElementVisitor);
