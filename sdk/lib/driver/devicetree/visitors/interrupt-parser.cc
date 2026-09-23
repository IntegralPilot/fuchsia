// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/devicetree/devicetree.h>
#include <lib/driver/devicetree/visitors/interrupt-parser.h>
#include <lib/driver/devicetree/visitors/property-parser.h>
#include <lib/driver/logging/cpp/logger.h>
#include <zircon/errors.h>

#include <cstdint>
#include <optional>
#include <utility>
#include <vector>

namespace fdf_devicetree {

namespace {

Properties MakeInterruptProperties() {
  Properties props = {};
  props.emplace_back(std::make_unique<ReferenceProperty>(InterruptParser::kInterruptsExtended,
                                                         InterruptParser::kInterruptCells,
                                                         /*required=*/false));
  props.emplace_back(
      std::make_unique<StringListProperty>(InterruptParser::kInterruptNames, /*required=*/false));
  props.emplace_back(std::make_unique<StringListProperty>(
      InterruptParser::kFuchsiaInterruptWakeVectors, /*required=*/false));
  return props;
}

}  // namespace

InterruptParser::InterruptParser() : PropertyParser(MakeInterruptProperties()) {}

zx::result<ParsedProperties> InterruptParser::Parse(Node& node) {
  zx::result interrupt_values = PropertyParser::Parse(node);
  if (interrupt_values.is_error()) {
    fdf::error("Interrupts-extended parser failed for node '{}' - {}", node.name(),
               interrupt_values);

    return interrupt_values.take_error();
  }

  // "interrupts-extended" takes precedence over "interrupts". Return if kInterruptsExtended
  // exists.
  if (interrupt_values->Get<References>(kInterruptsExtended)) {
    return zx::ok(*interrupt_values);
  }

  // Return early if there are no "interrupts" property for this node.
  auto interrupts_property = node.properties().find(kInterrupts);
  if (interrupts_property == node.properties().end()) {
    return zx::ok(*interrupt_values);
  }

  // Find the interrupt parent. Start the search at the current node's parent, as nodes cannot be
  // their own interrupt parent.
  ReferenceNode interrupt_parent(nullptr);
  ParentNode current(node.parent());
  // Traverse the parent chain upwards until interrupt parent or interrupt controller is
  // encountered.
  while (current) {
    auto parent_phandle = current.GetProperty<uint32_t>("interrupt-parent");
    if (parent_phandle.is_ok()) {
      auto result = node.GetReferenceNode(*parent_phandle);
      if (result.is_error()) {
        fdf::error("Failed to get reference node for phandle {} - {} ", *parent_phandle, result);

        return result.take_error();
      }
      interrupt_parent = *result;
      break;
    }
    if (parent_phandle.status_value() != ZX_ERR_NOT_FOUND) {
      return parent_phandle.take_error();
    }

    if (current.GetProperty<bool>("interrupt-controller")) {
      interrupt_parent = current.MakeReferenceNode();
      break;
    }
    current = current.parent();
  }

  if (!interrupt_parent) {
    fdf::error("Interrupt parent not found for node '{}'", node.name());

    return zx::error(ZX_ERR_NOT_FOUND);
  }

  auto cell_width_prop = interrupt_parent.properties().find(kInterruptCells);
  if (cell_width_prop == current.properties().end()) {
    fdf::error(
        "Could not find the interrupt cells property in the in interrupt parent '{}' for node '{}'",
        interrupt_parent.name(), node.name());

    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  auto cell_width = cell_width_prop->second.AsUint32();
  if (!cell_width) {
    fdf::error("Invalid interrupt cells property in the in interrupt parent '{}' for node '{}'",
               interrupt_parent.name(), node.name());

    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  size_t cell_count = interrupts_property->second.AsBytes().size_bytes() / sizeof(uint32_t);

  if ((cell_count % cell_width.value()) != 0) {
    fdf::error(
        "Invalid number of interrupt elements in node '{}. Interrupt cell size is {} and there are {} extra entries.",
        node.name(), cell_width.value(), cell_count % cell_width.value());

    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  std::vector<Reference> interrupt_references;
  for (size_t offset = 0; offset < cell_count; offset += cell_width.value()) {
    PropertyCells interrupt = interrupts_property->second.AsBytes().subspan(
        offset * sizeof(uint32_t), (*cell_width) * sizeof(uint32_t));
    interrupt_references.emplace_back(interrupt_parent, interrupt);
  }

  interrupt_values->AddProperty(kInterruptsExtended, std::move(interrupt_references));

  if (interrupt_values->Get<std::vector<std::string>>(kInterruptNames)) {
    const size_t interrupt_count = interrupt_values->Get<References>(kInterruptsExtended)->size();
    const size_t interrupt_name_count =
        interrupt_values->Get<std::vector<std::string>>(kInterruptNames)->size();
    if (interrupt_count != interrupt_name_count) {
      fdf::error("Number of interrupts ({}) doesn't match number of interrupt-names ({})",
                 interrupt_count, interrupt_name_count);

      return zx::error(ZX_ERR_INVALID_ARGS);
    }
  }

  return zx::ok(*interrupt_values);
}

}  // namespace fdf_devicetree
