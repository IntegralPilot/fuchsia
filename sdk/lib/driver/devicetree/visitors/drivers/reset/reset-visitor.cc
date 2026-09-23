// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "reset-visitor.h"

#include <lib/driver/component/cpp/composite_node_spec.h>
#include <lib/driver/component/cpp/node_properties.h>
#include <lib/driver/devicetree/visitors/common-types.h>
#include <lib/driver/devicetree/visitors/multivisitor.h>
#include <lib/driver/devicetree/visitors/registration.h>
#include <lib/driver/logging/cpp/logger.h>
#include <zircon/assert.h>

#include <cstdint>
#include <memory>
#include <optional>
#include <string>
#include <utility>
#include <vector>

#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/reset/cpp/bind.h>

namespace reset_dt {

ResetVisitor::ResetVisitor() {
  fdf_devicetree::Properties properties = {};
  properties.emplace_back(std::make_unique<fdf_devicetree::StringListProperty>(kResetNames));
  properties.emplace_back(
      std::make_unique<fdf_devicetree::ReferenceProperty>(kResetReference, kResetCells));
  reset_parser_ = std::make_unique<fdf_devicetree::PropertyParser>(std::move(properties));
}

zx::result<> ResetVisitor::Visit(fdf_devicetree::Node& node,
                                 const devicetree::PropertyDecoder& decoder) {
  auto parser_output = reset_parser_->Parse(node);
  if (parser_output.is_error()) {
    return parser_output.take_error();
  }

  std::optional<fdf_devicetree::References> reset_references =
      parser_output->Get<fdf_devicetree::References>(kResetReference);

  if (!reset_references.has_value()) {
    return zx::ok();
  }

  const size_t count = reset_references->size();
  const auto& names_property = parser_output->Get<std::vector<std::string>>(kResetNames);
  if (!names_property) {
    fdf::error("Reset reference '{}' error: `resets` property present but `reset-names` missing",
               node.name());

    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  const size_t names_count = names_property->size();
  if (names_count != count) {
    fdf::error(
        "Node '{}' error. `resets` ({}) and `reset-names` ({}) must have the same number of elements.",
        node.name(), count, names_count);

    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  for (uint32_t index = 0; index < count; index++) {
    auto& reference = reset_references->at(index);
    auto& name = names_property->at(index);

    auto result =
        ParseReferenceChild(node, reference.reference_node(), reference.property_cells(), name);
    if (result.is_error()) {
      return result.take_error();
    }
  }
  return zx::ok();
}

ResetVisitor::ResetController& ResetVisitor::GetController(uint32_t node_id) {
  if (!reset_controllers_.contains(node_id)) {
    reset_controllers_[node_id] = ResetController();
  }
  return reset_controllers_[node_id];
}

zx::result<> ResetVisitor::ParseReferenceChild(fdf_devicetree::Node& child,
                                               fdf_devicetree::ReferenceNode& parent,
                                               fdf_devicetree::PropertyCells specifiers,
                                               const std::string& reset_name) {
  auto& controller = GetController(parent.id());

  // If there are 0 reset cells that means this reset provider only has one reset toggle and its
  // id is vacuously 0.
  uint32_t reset_id = 0;

  if (specifiers.size_bytes() == sizeof(uint32_t)) {
    devicetree::PropertyValue pv(specifiers);
    if (!pv.AsUint32()) {
      fdf::error("Node ({}) failed to parse resets id nodes", child.name());

      return zx::error(ZX_ERR_INVALID_ARGS);
    }
    reset_id = *pv.AsUint32();
  } else if (specifiers.size_bytes() != 0) {
    fdf::error("Reset reference cell for node {} must be 0 or 4 bytes, actually = {}", child.name(),
               specifiers.size_bytes());

    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  const uint32_t controller_id = parent.id();
  controller.metadata.controller_id() = controller_id;

  if (!controller.metadata.resets().has_value()) {
    controller.metadata.resets() = std::vector<fuchsia_hardware_reset::ResetDescriptor>();
  }
  fuchsia_hardware_reset::ResetDescriptor desc;
  desc.reset_id() = reset_id;
  controller.metadata.resets()->push_back(std::move(desc));

  return AddChildNodeSpec(child, controller_id, reset_id, reset_name);
}

zx::result<> ResetVisitor::AddChildNodeSpec(fdf_devicetree::Node& child, uint32_t controller_id,
                                            uint32_t reset_id, const std::string& reset_name) {
  std::vector bind_rules = {{
      fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.reset.Service"),
      fdf::MakeAcceptBindRule(bind_fuchsia_reset::CONTROLLER_ID, controller_id),
      fdf::MakeAcceptBindRule(bind_fuchsia_reset::RESET_ID, reset_id),
  }};

  std::vector bind_properties = {{
      fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.reset.Service"),
      fdf::MakeProperty2(bind_fuchsia::NAME, reset_name),
  }};

  auto reset_node = fuchsia_driver_framework::ParentSpec2{
      {.bind_rules = bind_rules, .properties = bind_properties}};

  child.AddNodeSpec(reset_node);
  return zx::ok();
}

zx::result<> ResetVisitor::FinalizeNode(fdf_devicetree::Node& node) {
  // Check that this is a reset provider.
  if (!isController(node.properties())) {
    return zx::ok();
  }

  auto controller = reset_controllers_.find(node.id());
  if (controller == reset_controllers_.end()) {
    fdf::warn("Found a reset controller node but no nodes refer to it '{}'", node.name());

    return zx::ok();
  }

  const fit::result persisted_metadata = fidl::Persist(controller->second.metadata);
  if (!persisted_metadata.is_ok()) {
    fdf::error("Failed to encode reset metadata for reset controller {}: {}", node.name(),
               persisted_metadata.error_value().FormatDescription());

    return zx::error(persisted_metadata.error_value().status());
  }

  fuchsia_hardware_platform_bus::Metadata reset_metadata = {{
      .id = fuchsia_hardware_reset::Metadata::kSerializableName,
      .data = std::move(persisted_metadata.value()),
  }};

  node.AddMetadata(std::move(reset_metadata));

  fdf::debug("Reset metadata added to node '{}'", node.name());

  return zx::ok();
}

}  // namespace reset_dt

REGISTER_DEVICETREE_VISITOR(reset_dt::ResetVisitor);
