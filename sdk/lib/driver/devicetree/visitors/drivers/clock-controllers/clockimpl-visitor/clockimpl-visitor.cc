// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "clockimpl-visitor.h"

#include <fidl/fuchsia.hardware.clockimpl/cpp/fidl.h>
#include <lib/driver/component/cpp/composite_node_spec.h>
#include <lib/driver/component/cpp/node_properties.h>
#include <lib/driver/devicetree/visitors/registration.h>
#include <lib/driver/logging/cpp/logger.h>
#include <zircon/assert.h>
#include <zircon/errors.h>

#include <cstdint>
#include <memory>
#include <set>
#include <utility>

#include <bind/fuchsia/clock/cpp/bind.h>
#include <bind/fuchsia/cpp/bind.h>

namespace clock_impl_dt {

namespace {
using fuchsia_hardware_clockimpl::InitCall;
using fuchsia_hardware_clockimpl::InitStep;

class ClockCells {
 public:
  explicit ClockCells(fdf_devicetree::PropertyCells cells) : clock_cells_(cells, 1) {}

  // 1st cell denotes the clock ID.
  uint32_t id() { return static_cast<uint32_t>(*clock_cells_[0][0]); }

 private:
  using ClockElement = devicetree::PropEncodedArrayElement<1>;
  devicetree::PropEncodedArray<ClockElement> clock_cells_;
};

}  // namespace

ClockImplVisitor::ClockImplVisitor() {
  fdf_devicetree::Properties properties = {};
  properties.emplace_back(
      std::make_unique<fdf_devicetree::StringListProperty>(kClockNames, /* required */ false));
  properties.emplace_back(std::make_unique<fdf_devicetree::ReferenceProperty>(
      kClockReference, kClockCells, /* required */ false));
  properties.emplace_back(std::make_unique<fdf_devicetree::ReferenceProperty>(
      kAssignedClocks, kClockCells, /* required */ false));
  properties.emplace_back(std::make_unique<fdf_devicetree::ReferenceProperty>(
      kAssignedClockParents, kClockCells, /* required */ false));
  properties.emplace_back(
      std::make_unique<fdf_devicetree::Uint32ArrayProperty>(kAssignedClockRates,
                                                            /* required */ false));
  clock_parser_ = std::make_unique<fdf_devicetree::PropertyParser>(std::move(properties));
}

bool ClockImplVisitor::is_match(const fdf_devicetree::Node& node) {
  auto clock_cells = node.GetProperty<uint32_t>(kClockCells);
  if (clock_cells.is_error()) {
    return false;
  }

  return *clock_cells == 1;
}

uint32_t ClockImplVisitor::GetNextUniqueId() { return next_unique_id_++; }

zx::result<> ClockImplVisitor::Visit(fdf_devicetree::Node& node,
                                     const devicetree::PropertyDecoder& decoder) {
  zx::result parser_output = clock_parser_->Parse(node);
  if (parser_output.is_error()) {
    fdf::error("Clock visitor failed for node '{}' : {}", node.name(), parser_output);

    return parser_output.take_error();
  }

  // Parse clocks and clock-names
  if (auto clocks = parser_output->Get<fdf_devicetree::References>(kClockReference)) {
    auto clock_names = parser_output->Get<std::vector<std::string>>(kClockNames);
    if (!clock_names && clocks->size() != 1u) {
      fdf::error(
          "Clock reference '{}' does not have valid clock names property. Name is required to generate bind rules, especially when more than one clock is referenced.",
          node.name());

      return zx::error(ZX_ERR_INVALID_ARGS);
    }

    for (uint32_t index = 0; index < clocks->size(); index++) {
      auto& reference = (*clocks)[index];
      if (is_match(*reference.reference_node().GetNode())) {
        std::optional<std::string_view> name;
        if (clock_names) {
          name = (*clock_names)[index];
        }
        auto result =
            ParseReferenceChild(node, reference.reference_node(), reference.property_cells(), name);
        if (result.is_error()) {
          return result.take_error();
        }
      }
    }
  }

  // Parse assigned-clocks and related properties.
  if (auto assigned_clocks = parser_output->Get<fdf_devicetree::References>(kAssignedClocks)) {
    auto clock_parents = parser_output->Get<fdf_devicetree::References>(kAssignedClockParents);
    if (clock_parents && clock_parents->size() > assigned_clocks->size()) {
      fdf::error("Assigned clock parents in '{}' has more entries than assigned clocks.",
                 node.name());

      return zx::error(ZX_ERR_INVALID_ARGS);
    }

    auto clock_rates = parser_output->Get<std::vector<uint32_t>>(kAssignedClockRates);
    if (clock_rates && clock_rates->size() > assigned_clocks->size()) {
      fdf::error("Assigned clock rates in '{}' has more entries than assigned clocks.",
                 node.name());

      return zx::error(ZX_ERR_INVALID_ARGS);
    }

    // Track the clock controllers referenced so that we can add bind rule only once per controller.
    std::set<uint32_t> init_controllers;
    for (uint32_t index = 0; index < assigned_clocks->size(); index++) {
      auto& reference = (*assigned_clocks)[index];
      if (is_match(*reference.reference_node().GetNode())) {
        std::optional<fdf_devicetree::Reference> parent;
        if (clock_parents && index < clock_parents->size()) {
          parent = (*clock_parents)[index];
        }
        std::optional<uint32_t> rate;
        if (clock_rates && index < clock_rates->size()) {
          rate = (*clock_rates)[index];
        }
        auto result = ParseInitChild(node, reference.reference_node(), reference.property_cells(),
                                     rate, parent);
        if (result.is_error()) {
          return result.take_error();
        }

        if (init_controllers.find(reference.reference_node().id()) == init_controllers.end()) {
          result = AddInitChildNodeSpec(node);
          if (result.is_error()) {
            return result.take_error();
          }
          init_controllers.insert(reference.reference_node().id());
        }
      }
    }
  }

  return zx::ok();
}

zx::result<> ClockImplVisitor::AddChildNodeSpec(fdf_devicetree::Node& child, uint32_t node_id,
                                                std::optional<std::string_view> clock_name) {
  auto clock_node = fuchsia_driver_framework::ParentSpec2{{
      .bind_rules =
          {
              fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.clock.Service"),
              fdf::MakeAcceptBindRule(bind_fuchsia::ID, node_id),
          },
      .properties =
          {
              fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.clock.Service"),
          },
  }};

  if (clock_name) {
    clock_node.properties().push_back(
        fdf::MakeProperty2(bind_fuchsia::NAME, std::string(*clock_name)));
  }

  child.AddNodeSpec(clock_node);
  return zx::ok();
}

zx::result<> ClockImplVisitor::AddInitChildNodeSpec(fdf_devicetree::Node& child) {
  auto clock_init_node = fuchsia_driver_framework::ParentSpec2{{
      .bind_rules = {fdf::MakeAcceptBindRule(bind_fuchsia::INIT_STEP,
                                             bind_fuchsia_clock::BIND_INIT_STEP_CLOCK)},
      .properties =
          {
              fdf::MakeProperty2(bind_fuchsia::INIT_STEP, bind_fuchsia_clock::BIND_INIT_STEP_CLOCK),
          },
  }};
  child.AddNodeSpec(clock_init_node);
  return zx::ok();
}

ClockImplVisitor::ClockController& ClockImplVisitor::GetController(
    fdf_devicetree::Phandle phandle) {
  if (!clock_controllers_.contains(phandle)) {
    clock_controllers_[phandle] = ClockController();
  }
  return clock_controllers_[phandle];
}

zx::result<> ClockImplVisitor::ParseReferenceChild(fdf_devicetree::Node& child,
                                                   fdf_devicetree::ReferenceNode& parent,
                                                   fdf_devicetree::PropertyCells specifiers,
                                                   std::optional<std::string_view> clock_name) {
  if (specifiers.size_bytes() != 1 * sizeof(uint32_t)) {
    fdf::error("Clock reference '{}' has incorrect number of clock specifiers ({}) - expected 1.",
               child.name(), specifiers.size_bytes() / sizeof(uint32_t));

    return zx::error(ZX_ERR_NOT_FOUND);
  }

  auto cells = ClockCells(specifiers);
  const uint32_t clock_id = cells.id();
  const uint32_t node_id = GetNextUniqueId();
  const std::string name_string = clock_name ? std::string(*clock_name) : "<anonymous>";

  fdf::debug("Clock ID added - Unique ID {}, Clock ID {:#x} name '{}' to controller '{}'", node_id,
             clock_id, name_string, parent.name());

#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
  auto& controller = GetController(*parent.phandle());
  auto& clock_nodes = controller.clock_nodes_metadata.clock_nodes();
  if (!clock_nodes.has_value()) {
    clock_nodes.emplace(std::vector<fuchsia_hardware_clockimpl::ClockNodeDescriptor>{});
  }
  clock_nodes.value().emplace_back(fuchsia_hardware_clockimpl::ClockNodeDescriptor{{
      .clock_id = clock_id,
      .node_id = node_id,
      .name = name_string,
  }});
#endif

  return AddChildNodeSpec(child, node_id, clock_name);
}

zx::result<> ClockImplVisitor::ParseInitChild(
    fdf_devicetree::Node& child, fdf_devicetree::ReferenceNode& parent,
    fdf_devicetree::PropertyCells specifiers, std::optional<uint32_t> clock_rate,
    std::optional<fdf_devicetree::Reference> clock_parent) {
  auto& controller = GetController(*parent.phandle());
  auto clock = ClockCells(specifiers);

  if ((clock_rate && *clock_rate != 0) || clock_parent) {
    controller.init_metadata.steps().push_back({{clock.id(), InitCall::WithDisable({})}});
  }

  if (clock_parent) {
    auto parent_clock = ClockCells(clock_parent->property_cells());
    controller.init_metadata.steps().push_back(
        {{clock.id(), InitCall::WithInputIdx(parent_clock.id())}});
    fdf::debug("Clock parent set to {} for clock ID {} by '{}'.", parent_clock.id(), clock.id(),
               child.name());
  }

  if (clock_rate) {
    // Skip setting rates for 0 as per the clock bindings.
    if (*clock_rate != 0) {
      controller.init_metadata.steps().push_back({{clock.id(), InitCall::WithRateHz(*clock_rate)}});

      fdf::debug("Clock initial rate set to {} for clock ID {} by '{}'.", *clock_rate, clock.id(),
                 child.name());
    }
  }

  controller.init_metadata.steps().push_back({{clock.id(), InitCall::WithEnable({})}});

  return zx::ok();
}

zx::result<> ClockImplVisitor::FinalizeNode(fdf_devicetree::Node& node) {
  // Check that it is indeed a clock-controller that we support.
  if (!is_match(node)) {
    return zx::ok();
  }

  if (node.phandle()) {
    auto controller = clock_controllers_.find(*node.phandle());
    if (controller == clock_controllers_.end()) {
      fdf::info("Clock controller '{}' is not being used. Not adding any metadata for it.",
                node.name());

      return zx::ok();
    }

    if (!controller->second.init_metadata.steps().empty()) {
      const fit::result encoded_metadata = fidl::Persist(controller->second.init_metadata);
      if (!encoded_metadata.is_ok()) {
        fdf::error("Failed to encode clock init metadata: {}",
                   encoded_metadata.error_value().FormatDescription());

        return zx::error(encoded_metadata.error_value().status());
      }

      node.AddMetadata({{
          .id = fuchsia_hardware_clockimpl::InitMetadata::kSerializableName,
          .data = std::move(encoded_metadata.value()),
      }});

      fdf::debug("Clock init steps metadata added to node '{}'", node.name());
    }

#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
    const auto& clock_nodes = controller->second.clock_nodes_metadata.clock_nodes();
    if (clock_nodes.has_value() && !clock_nodes.value().empty()) {
      const fit::result encoded_clock_id_metadata =
          fidl::Persist(controller->second.clock_nodes_metadata);
      if (!encoded_clock_id_metadata.is_ok()) {
        fdf::error("Failed to encode clock ID's: {}",
                   encoded_clock_id_metadata.error_value().FormatDescription());

        return zx::error(encoded_clock_id_metadata.error_value().status());
      }
      fuchsia_hardware_platform_bus::Metadata metadata = {{
          .id = fuchsia_hardware_clockimpl::wire::ClockIdsMetadata::kSerializableName,
          .data = encoded_clock_id_metadata.value(),
      }};
      node.AddMetadata(std::move(metadata));

      fdf::debug("Clock ID's metadata added to node '{}'", node.name());
    }
#endif
  }

  return zx::ok();
}

}  // namespace clock_impl_dt

REGISTER_DEVICETREE_VISITOR(clock_impl_dt::ClockImplVisitor);
