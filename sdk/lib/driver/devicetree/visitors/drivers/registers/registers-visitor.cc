// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "registers-visitor.h"

#include <fidl/fuchsia.hardware.registers/cpp/fidl.h>
#include <lib/ddk/metadata.h>
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

namespace registers_dt {

namespace {
using fuchsia_hardware_registers::Mask;
using fuchsia_hardware_registers::MaskEntry;
using fuchsia_hardware_registers::Metadata;
using fuchsia_hardware_registers::RegistersMetadataEntry;

// Would like to add RegisterCellsV1 with only offsets as well.
class RegisterCells {
 public:
  explicit RegisterCells(fdf_devicetree::PropertyCells cells) : register_cells_(cells, 1, 1, 2) {}

  // 1st cell denotes the offset from the MMIO.
  uint32_t mmio_offset() { return static_cast<uint32_t>(*register_cells_[0][0]); }

  // 2nd cell denotes the size of the register in bytes.
  uint32_t size() { return static_cast<uint32_t>(*register_cells_[0][1]); }

  // 3rd cell denotes the mask for the register.
  zx::result<Mask> mask() {
    uint64_t mask = mask_as_u64();

    uint32_t mask_size = size();
    switch (mask_size) {
      case 1:
        return zx::ok(Mask::WithR8(static_cast<uint8_t>(mask)));
      case 2:
        return zx::ok(Mask::WithR16(static_cast<uint16_t>(mask)));
      case 4:
        return zx::ok(Mask::WithR32(static_cast<uint32_t>(mask)));
      case 8:
        return zx::ok(Mask::WithR64(mask));
      default:
        break;
    }

    fdf::error("Invalid mask size {}", mask_size);

    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  uint64_t mask_as_u64() { return *register_cells_[0][2]; }

 private:
  using RegisterElement = devicetree::PropEncodedArrayElement<3>;
  devicetree::PropEncodedArray<RegisterElement> register_cells_;
};

}  // namespace

RegistersVisitor::RegistersVisitor() : DriverVisitor({"fuchsia,registers"}) {
  fdf_devicetree::Properties properties = {};
  properties.emplace_back(
      std::make_unique<fdf_devicetree::StringListProperty>(kRegisterNames, /* required */ false));
  properties.emplace_back(std::make_unique<fdf_devicetree::ReferenceProperty>(
      kRegisterReference, kRegisterCells, /* required */ false));
  register_parser_ = std::make_unique<fdf_devicetree::PropertyParser>(std::move(properties));
}

zx::result<> RegistersVisitor::Visit(fdf_devicetree::Node& node,
                                     const devicetree::PropertyDecoder& decoder) {
  // Call register parser on all nodes, not just the register device node as we need to parse
  // register reference properties.
  auto parser_output = register_parser_->Parse(node);
  if (parser_output.is_error()) {
    return parser_output.take_error();
  }

  auto registers = parser_output->Get<fdf_devicetree::References>(kRegisterReference);
  if (!registers) {
    return zx::ok();
  }

  auto register_names = parser_output->Get<std::vector<std::string>>(kRegisterNames);

  for (uint32_t index = 0; index < registers->size(); index++) {
    auto& reference = (*registers)[index];
    if (is_match(reference.reference_node().properties())) {
      std::optional<std::string> name;
      if (register_names && index < register_names->size()) {
        name = (*register_names)[index];
      }
      auto result =
          ParseRegisterChild(node, reference.reference_node(), reference.property_cells(), name);
      if (result.is_error()) {
        return result.take_error();
      }
    }
  }
  return zx::ok();
}

zx::result<> RegistersVisitor::AddChildNodeSpec(fdf_devicetree::Node& child,
                                                std::optional<std::string> register_name) {
  std::vector bind_rules = {
      fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.registers.Service"),
  };
  std::vector bind_properties = {
      fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.registers.Service"),
  };

  if (register_name) {
    bind_rules.emplace_back(fdf::MakeAcceptBindRule(bind_fuchsia::NAME, *register_name));
    bind_properties.emplace_back(fdf::MakeProperty2(bind_fuchsia::NAME, *register_name));
  } else {
    bind_rules.emplace_back(fdf::MakeAcceptBindRule(bind_fuchsia::NAME, child.fdf_name()));
  }

  auto register_node = fuchsia_driver_framework::ParentSpec2{{bind_rules, bind_properties}};

  child.AddNodeSpec(register_node);
  return zx::ok();
}

zx::result<> RegistersVisitor::ParseRegisterChild(fdf_devicetree::Node& child,
                                                  fdf_devicetree::ReferenceNode& parent,
                                                  fdf_devicetree::PropertyCells specifiers,
                                                  std::optional<std::string> register_name) {
  auto controller_iter = register_controllers_.find(*parent.phandle());
  if (controller_iter == register_controllers_.end()) {
    register_controllers_[*parent.phandle()] = RegisterController();
  }
  auto& controller = register_controllers_[*parent.phandle()];

  if (!controller.overlap_check_on) {
    auto overlap_property = parent.properties().find("overlap_check_on");
    if (overlap_property == parent.properties().end()) {
      fdf::debug(
          "Registers parent '{}' does not have a overlap_check_on property. Assuming default false.",
          parent.name());

      controller.overlap_check_on = false;
    } else {
      controller.overlap_check_on = true;
    }
  }

  if (!register_name) {
    fdf::debug("Register reference '{}' does not have a register name, will be using child name.",
               child.name());
  }

  // Check if a register for the name already exists.
  std::string name = register_name.value_or(child.fdf_name());
  auto register_it = controller.registers.find(name);
  RegistersMetadataEntry register_entry =
      register_it != controller.registers.end() ? register_it->second : RegistersMetadataEntry();

  auto cells = RegisterCells(specifiers);
  MaskEntry mask;
  zx::result mask_value = cells.mask();
  if (mask_value.is_error()) {
    fdf::error("Register reference '{}' has invalid mask value.", child.name());

    return mask_value.take_error();
  }

  mask.mask(mask_value.value());
  mask.mmio_offset(cells.mmio_offset());
  mask.count(1);
  mask.overlap_check_on(controller.overlap_check_on);

  fdf::debug("Mask added to controller '{}'- mask {:#x}({}) offset {:#x} overlap {} bind name '{}'",
             parent.name(), cells.mask_as_u64(), cells.size(), *mask.mmio_offset(),
             *mask.overlap_check_on(), name);

  if (!register_entry.masks()) {
    register_entry.masks() = std::vector<MaskEntry>();
  }

  register_entry.masks()->push_back(std::move(mask));
  register_entry.name(name);

  // For now supporting only one mmio region per devicetree node.
  register_entry.mmio_id(0);
  controller.registers[name] = register_entry;

  // Add register bind rules only once per register name.
  if (register_it == controller.registers.end()) {
    return AddChildNodeSpec(child, register_name);
  }

  return zx::ok();
}

zx::result<> RegistersVisitor::DriverFinalizeNode(fdf_devicetree::Node& node) {
  if (node.phandle()) {
    auto controller = register_controllers_.find(*node.phandle());
    if (controller == register_controllers_.end()) {
      fdf::info("Register controller '{}' is not being used. Not adding any metadata for it.",
                node.name());

      return zx::ok();
    }

    std::vector<RegistersMetadataEntry> registers;
    for (const auto& register_entry : controller->second.registers) {
      registers.push_back(register_entry.second);
    }

    Metadata registers_metadata = {{registers}};

    fit::result encoded = fidl::Persist(registers_metadata);
    if (encoded.is_error()) {
      fdf::error("Failed to persist data - {}.", encoded.error_value().FormatDescription());

      return zx::ok();
    }

    fuchsia_hardware_platform_bus::Metadata metadata = {{
        .id = Metadata::kSerializableName,
        .data = encoded.value(),
    }};

    node.AddMetadata(std::move(metadata));

    fdf::debug("Registers metadata added for node '{}'", node.name());
  }

  return zx::ok();
}

}  // namespace registers_dt

REGISTER_DEVICETREE_VISITOR(registers_dt::RegistersVisitor);
