// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "spmi-visitor.h"

#include <lib/driver/component/cpp/node_properties.h>
#include <lib/driver/devicetree/visitors/common-types.h>
#include <lib/driver/devicetree/visitors/registration.h>
#include <lib/driver/logging/cpp/logger.h>
#include <stdlib.h>

#include <utility>

#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/spmi/cpp/bind.h>

#include "spmi.h"

namespace {

constexpr char kSpmisPropertyName[] = "spmis";

template <typename T>
zx::result<std::pair<uint32_t, uint32_t>> GetAddressAndSizeCells(const T& node) {
  auto address_cells = node.template GetProperty<uint32_t>("#address-cells");
  if (address_cells.is_error()) {
    return address_cells.take_error();
  }

  auto size_cells = node.template GetProperty<uint32_t>("#size-cells");
  if (size_cells.is_error()) {
    return size_cells.take_error();
  }

  return zx::ok(std::pair<uint32_t, uint32_t>(*address_cells, *size_cells));
}

std::vector<std::string> GetRegNames(const fdf_devicetree::ChildNode& node) {
  auto reg_names = node.GetProperty<std::vector<std::string>>("reg-names");
  if (reg_names.is_ok()) {
    return *reg_names;
  }

  return {};
}

}  // namespace

namespace spmi_dt {

SpmiVisitor::SpmiVisitor() {
  fdf_devicetree::Properties spmi_properties = {};
  spmi_properties.emplace_back(std::make_unique<fdf_devicetree::ReferenceProperty>(
      kSpmisPropertyName, 0u, /*required=*/false));
  spmi_parser_ = std::make_unique<fdf_devicetree::PropertyParser>(std::move(spmi_properties));
}

zx::result<> SpmiVisitor::Visit(fdf_devicetree::Node& node,
                                const devicetree::PropertyDecoder& decoder) {
  if (zx::result<> result = ParseController(node); result.is_error()) {
    return result.take_error();
  }
  return ParseReferenceProperty(node);
}

zx::result<> SpmiVisitor::FinalizeNode(fdf_devicetree::Node& node) {
  if (spmi_nodes_.contains(node.id())) {
    if (zx::result<> result = FinalizeSpmiNode(spmi_nodes_[node.id()], node); result.is_error()) {
      return result.take_error();
    }
  }

  if (spmi_references_.contains(node.id())) {
    return FinalizeReferences(spmi_references_[node.id()], node);
  }

  return zx::ok();
}

zx::result<> SpmiVisitor::ParseReferenceProperty(fdf_devicetree::Node& node) {
  zx::result properties = spmi_parser_->Parse(node);
  if (properties.is_error()) {
    fdf::error("Failed to parse node \"{}\"", node.name());

    return properties.take_error();
  }
  auto spmis = properties->Get<fdf_devicetree::References>(kSpmisPropertyName);
  if (!spmis) {
    return zx::ok();
  }

  if (spmi_references_.contains(node.id())) {
    fdf::error("Duplicate ID for SPMI reference node \"{}\"", node.name());

    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  std::set<uint32_t>& spmi_references = spmi_references_[node.id()];
  for (auto& reference : *spmis) {
    if (!reference.reference_node()) {
      fdf::error("Failed to parse SPMI reference for node \"{}\"", node.name());

      return zx::error(ZX_ERR_INVALID_ARGS);
    }

    auto [it, _] = spmi_nodes_.try_emplace(reference.reference_node().id());
    SpmiNode& spmi_node = it->second;
    if (spmi_node.has_reference_property) {
      if (spmi_references.contains(reference.reference_node().id())) {
        // Ignore duplicate reference property entries.
        continue;
      }

      fdf::error("Multiple reference properties for SPMI node \"{}\"",
                 reference.reference_node().name());

      return zx::error(ZX_ERR_ALREADY_EXISTS);
    }
    spmi_node.has_reference_property = true;

    spmi_references.insert(reference.reference_node().id());
  }
  return zx::ok();
}

zx::result<> SpmiVisitor::ParseController(fdf_devicetree::Node& node) {
  const uint32_t controller_id = node.id();

  if (!node.name().starts_with("spmi@")) {
    return zx::ok();
  }

  const auto cells = GetAddressAndSizeCells(node);
  if (cells.is_error() || *cells != std::pair<uint32_t, uint32_t>{2, 0}) {
    fdf::error("Invalid #address-cells or #size-cells for SPMI controller \"{}\"", node.name());

    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  fuchsia_hardware_spmi::ControllerInfo controller{{.id = controller_id}};

  uint16_t used_target_ids = 0;
  for (fdf_devicetree::ChildNode& child : node.children()) {
    auto reg = child.GetProperty<std::vector<uint32_t>>("reg");
    if (reg.is_error()) {
      fdf::error("SPMI target \"{}\" has no reg property: {}", child.name(), reg);

      return reg.take_error();
    }

    if (reg->empty() || reg->size() % 2 != 0) {
      fdf::error("SPMI target \"{}\" reg property has invalid size: {}", child.name(), reg->size());

      return zx::error(ZX_ERR_INVALID_ARGS);
    }

    std::vector<std::string> reg_names = GetRegNames(child);
    if (!reg_names.empty() && reg_names.size() != reg->size() / 2) {
      fdf::error("SPMI target \"{}\" has mismatched reg and reg-names properties", child.name());

      return zx::error(ZX_ERR_INVALID_ARGS);
    }

    for (size_t i = 0; i < reg->size(); i += 2) {
      const uint32_t target_id = (*reg)[i];
      const uint32_t target_type = (*reg)[i + 1];

      if (target_id >= fuchsia_hardware_spmi::kMaxTargets) {
        fdf::error("SPMI target ID {} for \"{}\" out of range", target_id, child.name());

        return zx::error(ZX_ERR_ALREADY_EXISTS);
      }
      if (target_type != SPMI_USID) {
        fdf::error("Unsupported SPMI target type {} for \"{}\"", target_type, child.name());

        return zx::error(ZX_ERR_NOT_SUPPORTED);
      }

      if (used_target_ids & (1 << target_id)) {
        fdf::error("Duplicate SPMI target ID {} for \"{}\"", target_id, child.name());

        return zx::error(ZX_ERR_ALREADY_EXISTS);
      }

      used_target_ids |= (1 << target_id);

      std::optional<std::string> target_name;
      if (!reg_names.empty()) {
        target_name = reg_names[i / 2];
      }

      zx::result<fuchsia_hardware_spmi::TargetInfo> target =
          ParseTarget(controller_id, target_id, target_name, child);
      if (target.is_error()) {
        return target.take_error();
      }

      if (!controller.targets()) {
        controller.targets().emplace();
      }
      controller.targets()->push_back(*std::move(target));
    }
  }

  fit::result<fidl::Error, std::vector<uint8_t>> metadata = fidl::Persist(controller);
  if (metadata.is_error()) {
    fdf::error("Failed to persist SPMI controller metadata: {}",
               metadata.error_value().FormatDescription());

    return zx::error(metadata.error_value().status());
  }

  fuchsia_hardware_platform_bus::Metadata pbus_metadata{{
      .id = fuchsia_hardware_spmi::ControllerInfo::kSerializableName,
      .data = std::move(metadata.value()),
  }};
  node.AddMetadata(std::move(pbus_metadata));

  return zx::ok();
}

zx::result<fuchsia_hardware_spmi::TargetInfo> SpmiVisitor::ParseTarget(
    uint32_t controller_id, uint32_t target_id, std::optional<std::string> target_name,
    fdf_devicetree::ChildNode& node) {
  node.set_register_type(fdf_devicetree::RegisterType::kSpmi);

  fuchsia_hardware_spmi::TargetInfo target{{
      .id = target_id,
      .display_name = node.fdf_name(),
  }};
  if (target_name) {
    target.name() = *target_name;
  }

  // SPMI target nodes might have children that are not SPMI sub-targets (register blocks), but are
  // client devices on a child bus (I2C client devices, if SPMI target is also I2C controller).
  //
  // To support this, check if child nodes match the standard SPMI sub-target register cell format
  // (this expects #address-cells = 1 and #size-cells = 1). If they do not match (e.g. I2C uses
  // #address-cells = 1 and #size-cells = 0), assume children are non-SPMI client devices.
  //
  // In this case, treat the SPMI target as a SPMI leaf node. This allows the target to be parsed by
  // the SPMI visitor, while its children are processed by other visitors (e.g. I2C visitor).
  bool has_sub_targets = !node.GetNode()->children().empty();
  if (has_sub_targets) {
    const zx::result<std::pair<uint32_t, uint32_t>> cells = GetAddressAndSizeCells(node);
    if (cells.is_error() || *cells != std::pair<uint32_t, uint32_t>{1, 1}) {
      // The children are not SPMI sub-targets (e.g. they might be I2C/SPI clients).
      // Treat this target as having no sub-targets at the SPMI level.
      has_sub_targets = false;
    }
  }

  if (has_sub_targets) {
    auto reg = node.GetProperty<std::vector<uint32_t>>("reg");
    if (reg.is_ok() && reg->size() > 2) {
      fdf::error("SPMI target \"{}\" has multiple SIDs and sub-targets, which is not allowed",
                 node.name());
      return zx::error(ZX_ERR_INVALID_ARGS);
    }
  }

  if (!has_sub_targets) {
    // This target has no sub-target children, so create a node spec for it.
    fuchsia_driver_framework::ParentSpec2 target_spec{{
        .bind_rules =
            {
                fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE,
                                        "fuchsia.hardware.spmi.TargetService"),
                fdf::MakeAcceptBindRule(bind_fuchsia_spmi::CONTROLLER_ID, controller_id),
                fdf::MakeAcceptBindRule(bind_fuchsia::ID, target_id),
            },
        .properties =
            {
                fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.spmi.TargetService"),
                fdf::MakeProperty2(bind_fuchsia::ID, target_id),
            },
    }};

    if (target_name) {
      target_spec.properties().push_back(fdf::MakeProperty2(bind_fuchsia::NAME, *target_name));
    }

    spmi_nodes_[node.id()].parent_specs.push_back(std::move(target_spec));
    return zx::ok(target);
  }

  std::vector<fuchsia_hardware_spmi::SubTargetInfo> sub_targets_info;
  for (fdf_devicetree::ChildNode& child : node.GetNode()->children()) {
    zx::result<std::vector<fuchsia_hardware_spmi::SubTargetInfo>> sub_target_regions =
        ParseSubTarget(controller_id, target, child);
    if (sub_target_regions.is_error()) {
      return sub_target_regions.take_error();
    }
    sub_targets_info.insert(sub_targets_info.end(), sub_target_regions->begin(),
                            sub_target_regions->end());
  }

  target.sub_targets().emplace(std::move(sub_targets_info));
  return zx::ok(target);
}

zx::result<std::vector<fuchsia_hardware_spmi::SubTargetInfo>> SpmiVisitor::ParseSubTarget(
    uint32_t controller_id, const fuchsia_hardware_spmi::TargetInfo& parent,
    fdf_devicetree::ChildNode& node) {
  ZX_DEBUG_ASSERT(parent.id());

  node.set_register_type(fdf_devicetree::RegisterType::kSpmi);

  auto reg = node.GetProperty<std::vector<uint32_t>>("reg");
  if (reg.is_error()) {
    fdf::error("SPMI sub-target \"{}\" has no reg property: {}", node.name(), reg);

    return reg.take_error();
  }

  if (reg->empty() || reg->size() % 2 != 0) {
    fdf::error("SPMI sub-target \"{}\" has invalid reg size {}", node.name(), reg->size());

    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  std::vector<std::string> reg_names = GetRegNames(node);
  if (!reg_names.empty() && reg_names.size() != reg->size() / 2) {
    fdf::error("SPMI sub-target \"{}\" has mismatched reg and reg-names properties", node.name());

    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  std::vector<fuchsia_driver_framework::ParentSpec2>& parent_specs =
      spmi_nodes_[node.id()].parent_specs;

  std::vector<fuchsia_hardware_spmi::SubTargetInfo> sub_targets;
  for (size_t i = 0; i < reg->size(); i += 2) {
    const uint32_t address = (*reg)[i];
    const uint32_t size = (*reg)[i + 1];

    uint32_t address_plus_size{};
    const bool overflow = add_overflow(address, size, &address_plus_size);
    if (size == 0 || overflow || address_plus_size > UINT16_MAX + 1) {
      fdf::error("SPMI sub-target \"{}\" has invalid address ({:#04x}) or size ({})", node.name(),
                 address, size);

      return zx::error(ZX_ERR_INVALID_ARGS);
    }

    fuchsia_hardware_spmi::SubTargetInfo sub_target{{
        .address = static_cast<uint16_t>(address),
        .size = size,
        .display_name = node.fdf_name(),
    }};

    fuchsia_driver_framework::ParentSpec2 sub_target_spec{{
        .bind_rules =
            {
                fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE,
                                        "fuchsia.hardware.spmi.SubTargetService"),
                fdf::MakeAcceptBindRule(bind_fuchsia_spmi::CONTROLLER_ID, controller_id),
                fdf::MakeAcceptBindRule(bind_fuchsia_spmi::TARGET_ID,
                                        static_cast<uint32_t>(*parent.id())),
                fdf::MakeAcceptBindRule(bind_fuchsia::ID, address),
            },
        .properties =
            {
                fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.spmi.SubTargetService"),
                fdf::MakeProperty2(bind_fuchsia_spmi::TARGET_ID,
                                   static_cast<uint32_t>(*parent.id())),
                fdf::MakeProperty2(bind_fuchsia::ID, address),
            },
    }};

    if (!reg_names.empty()) {
      const std::string_view sub_target_name = reg_names[i / 2];
      sub_target.name() = sub_target_name;
      sub_target_spec.properties().push_back(
          fdf::MakeProperty2(bind_fuchsia::NAME, sub_target_name));
    }

    sub_targets.push_back(std::move(sub_target));
    parent_specs.push_back(std::move(sub_target_spec));
  }

  return zx::ok(sub_targets);
}

zx::result<> SpmiVisitor::FinalizeSpmiNode(const SpmiNode& spmi_node, fdf_devicetree::Node& node) {
  if (spmi_node.has_reference_property) {
    auto compatible = node.GetProperty<std::string>("compatible");
    if (compatible.is_ok()) {
      fdf::error("SPMI node \"{}\" has a compatible property and is referenced by other nodes",
                 node.name());

      return zx::error(ZX_ERR_INVALID_ARGS);
    }
    if (compatible.is_error() && compatible.status_value() != ZX_ERR_NOT_FOUND) {
      return compatible.take_error();
    }

    return zx::ok();
  }

  // Only add parents for the SPMI node if it does not appear in a reference property.
  if (spmi_node.parent_specs.empty()) {
    fdf::error("No parent specs found for SPMI node \"{}\"", node.name());

    return zx::error(ZX_ERR_NOT_FOUND);
  }

  for (const fuchsia_driver_framework::ParentSpec2& parent_spec : spmi_node.parent_specs) {
    node.AddNodeSpec(parent_spec);
  }

  return zx::ok();
}

zx::result<> SpmiVisitor::FinalizeReferences(const std::set<uint32_t>& spmi_references,
                                             fdf_devicetree::Node& node) {
  for (const uint32_t spmi_id : spmi_references) {
    if (!spmi_nodes_.contains(spmi_id) || spmi_nodes_[spmi_id].parent_specs.empty()) {
      fdf::error("No SPMI target or sub-target found for reference property in node \"{}\"",
                 node.name());

      return zx::error(ZX_ERR_NOT_FOUND);
    }

    const SpmiNode& spmi_node = spmi_nodes_[spmi_id];
    if (!spmi_node.has_reference_property) {
      fdf::error("SPMI node is not marked as having a reference property for node \"{}\"",
                 node.name());

      return zx::error(ZX_ERR_BAD_STATE);
    }
    for (const fuchsia_driver_framework::ParentSpec2& parent_spec : spmi_node.parent_specs) {
      node.AddNodeSpec(parent_spec);
    }
  }
  return zx::ok();
}

}  // namespace spmi_dt

REGISTER_DEVICETREE_VISITOR(spmi_dt::SpmiVisitor);
