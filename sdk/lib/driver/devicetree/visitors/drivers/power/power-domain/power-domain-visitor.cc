// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "power-domain-visitor.h"

#include <lib/driver/component/cpp/composite_node_spec.h>
#include <lib/driver/component/cpp/node_properties.h>
#include <lib/driver/devicetree/visitors/registration.h>
#include <lib/driver/logging/cpp/logger.h>

#include <vector>

#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/power/cpp/bind.h>

namespace power_domain_visitor_dt {

class PowerDomainCells {
 public:
  explicit PowerDomainCells(fdf_devicetree::PropertyCells cells)
      : power_domain_cells_(cells, PowerDomainVisitor::kPowerDomainCellsSize) {}

  // 1st cell denotes the domain id.
  uint32_t domain_id() { return static_cast<uint32_t>(*power_domain_cells_[0][0]); }

 private:
  using PowerDomainCell =
      devicetree::PropEncodedArrayElement<PowerDomainVisitor::kPowerDomainCellsSize>;
  devicetree::PropEncodedArray<PowerDomainCell> power_domain_cells_;
};

PowerDomainVisitor::PowerDomainVisitor() {
  fdf_devicetree::Properties properties = {};
  properties.emplace_back(std::make_unique<fdf_devicetree::ReferenceProperty>(
      kPowerDomains, kPowerDomainCells, /* required */ false));
  properties.emplace_back(std::make_unique<fdf_devicetree::StringListProperty>(
      kPowerDomainNames, /* required */ false));
  parser_ = std::make_unique<fdf_devicetree::PropertyParser>(std::move(properties));
}

zx::result<> PowerDomainVisitor::Visit(fdf_devicetree::Node& node,
                                       const devicetree::PropertyDecoder& decoder) {
  auto parser_output = parser_->Parse(node);
  if (parser_output.is_error()) {
    fdf::error("Power domain visitor failed for node '{}' : {}", node.name(), parser_output);

    return parser_output.take_error();
  }

  auto power_domains = parser_output->Get<fdf_devicetree::References>(kPowerDomains);
  if (!power_domains) {
    return zx::ok();
  }

  auto power_domain_names = parser_output->Get<std::vector<std::string>>(kPowerDomainNames);

  for (uint32_t index = 0; index < power_domains->size(); index++) {
    auto& domain_reference = (*power_domains)[index];
    std::optional<std::string_view> name;
    if (power_domain_names && index < power_domain_names->size()) {
      name = (*power_domain_names)[index];
    }
    if (zx::result result = ParseReferenceChild(node, domain_reference.reference_node(),
                                                domain_reference.property_cells(), name);
        result.is_error()) {
      return result;
    }
  }

  return zx::ok();
}

zx::result<> PowerDomainVisitor::AddChildNodeSpec(fdf_devicetree::Node& child, uint32_t domain_id,
                                                  uint32_t node_id, bool is_full_power,
                                                  std::optional<std::string_view> name) {
  auto bind_rules = std::vector<fuchsia_driver_framework::BindRule2>();
  auto properties = std::vector<fuchsia_driver_framework::NodeProperty2>();

  if (is_full_power) {
    bind_rules.push_back(
        fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.power.Service"));
    bind_rules.push_back(fdf::MakeAcceptBindRule(bind_fuchsia_power::POWER_DOMAIN, domain_id));
    properties.push_back(
        fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.power.Service"));
    properties.push_back(fdf::MakeProperty2(bind_fuchsia_power::POWER_DOMAIN, domain_id));
  } else {
    bind_rules.push_back(
        fdf::MakeAcceptBindRule(bind_fuchsia::SERVICE, "fuchsia.hardware.powerdomain.Service"));
    bind_rules.push_back(fdf::MakeAcceptBindRule(bind_fuchsia_power::POWER_DOMAIN, domain_id));
    bind_rules.push_back(
        fdf::MakeAcceptBindRule(bind_fuchsia_power::POWER_DOMAIN_NODE_ID, node_id));
    properties.push_back(
        fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.powerdomain.Service"));
    properties.push_back(fdf::MakeProperty2(bind_fuchsia_power::POWER_DOMAIN, domain_id));
  }

  if (name) {
    properties.push_back(fdf::MakeProperty2(bind_fuchsia::NAME, *name));
  }

  auto power_node = fuchsia_driver_framework::ParentSpec2{{
      .bind_rules = bind_rules,
      .properties = properties,
  }};
  fdf::debug("Added power domain (id: {}, node_id: {}) parent to node '{}'", domain_id, node_id,
             child.name());

  child.AddNodeSpec(power_node);
  return zx::ok();
}

PowerDomainVisitor::PowerController& PowerDomainVisitor::GetController(
    fdf_devicetree::Phandle phandle) {
  const auto [controller_iter, success] = power_controllers_.insert({phandle, PowerController()});
  return controller_iter->second;
}

uint32_t PowerDomainVisitor::GetNextUniqueId() { return next_unique_id_++; }

zx::result<> PowerDomainVisitor::ParseReferenceChild(fdf_devicetree::Node& child,
                                                     fdf_devicetree::ReferenceNode& parent,
                                                     fdf_devicetree::PropertyCells specifiers,
                                                     std::optional<std::string_view> name) {
  auto& controller = GetController(*parent.phandle());

  if (specifiers.size_bytes() != kPowerDomainCellsSize * sizeof(uint32_t)) {
    fdf::error("Power domain reference '{}' has incorrect number of specifiers ({}) - expected {}.",
               child.name(), specifiers.size_bytes() / sizeof(uint32_t), kPowerDomainCellsSize);

    return zx::error(ZX_ERR_NOT_FOUND);
  }

  bool is_full_power = parent.GetProperty<bool>(kLegacyPower);
  controller.is_full_power = is_full_power;

  auto cells = PowerDomainCells(specifiers);
  uint32_t domain_id = cells.domain_id();
  uint32_t node_id = GetNextUniqueId();

  if (is_full_power) {
    fuchsia_hardware_power::Domain domain;
    domain.id() = domain_id;

    if (!controller.full_domain_info) {
      controller.full_domain_info.emplace();
      controller.full_domain_info->domains() = std::vector<fuchsia_hardware_power::Domain>();
    }

    auto it = std::find_if(controller.full_domain_info->domains()->begin(),
                           controller.full_domain_info->domains()->end(),
                           [&domain](const fuchsia_hardware_power::Domain& entry) {
                             return entry.id() == domain.id();
                           });
    if (it == controller.full_domain_info->domains()->end()) {
      controller.full_domain_info->domains()->push_back(domain);
      fdf::debug("Power domain added (id: {}) added to controller '{}'", domain_id, parent.name());
    }
  } else {
    fuchsia_hardware_powerdomain::PowerDomain domain;
    domain.id() = domain_id;
    domain.node_id() = node_id;
    if (name) {
      domain.name() = std::string(*name);
    }

    if (!controller.basic_domain_info) {
      controller.basic_domain_info.emplace();
      controller.basic_domain_info->domains() =
          std::vector<fuchsia_hardware_powerdomain::PowerDomain>();
    }

    // Always push back for basic power domain to support multiple clients.
    controller.basic_domain_info->domains()->push_back(domain);
    fdf::debug("Basic power domain added (id: {}, node_id: {}) added to controller '{}'", domain_id,
               node_id, parent.name());
  }

  return AddChildNodeSpec(child, domain_id, node_id, is_full_power, name);
}

zx::result<> PowerDomainVisitor::FinalizeNode(fdf_devicetree::Node& node) {
  if (!node.phandle() || (power_controllers_.find(*node.phandle()) == power_controllers_.end())) {
    return zx::ok();
  }

  auto controller = power_controllers_.at(*node.phandle());

  if (controller.is_full_power) {
    if (controller.full_domain_info && controller.full_domain_info->domains()) {
      const fit::result persisted_domain_info = fidl::Persist(*controller.full_domain_info);
      if (!persisted_domain_info.is_ok()) {
        fdf::error("Failed to persist Power domain metadata for node {}: {}", node.name(),
                   persisted_domain_info.error_value().FormatDescription());

        return zx::error(persisted_domain_info.error_value().status());
      }
      fuchsia_hardware_platform_bus::Metadata controller_metadata = {{
          .id = fuchsia_hardware_power::DomainMetadata::kSerializableName,
          .data = persisted_domain_info.value(),
      }};
      node.AddMetadata(std::move(controller_metadata));
      fdf::debug("Power domain metadata added to node '{}'", node.name());
    }
  } else {
    if (controller.basic_domain_info && controller.basic_domain_info->domains()) {
      const fit::result persisted_domain_info = fidl::Persist(*controller.basic_domain_info);
      if (!persisted_domain_info.is_ok()) {
        fdf::error("Failed to persist Basic Power domain metadata for node {}: {}", node.name(),
                   persisted_domain_info.error_value().FormatDescription());

        return zx::error(persisted_domain_info.error_value().status());
      }
      fuchsia_hardware_platform_bus::Metadata controller_metadata = {{
          .id = fuchsia_hardware_powerdomain::DomainMetadata::kSerializableName,
          .data = persisted_domain_info.value(),
      }};
      node.AddMetadata(std::move(controller_metadata));
      fdf::debug("Basic Power domain metadata added to node '{}'", node.name());
    }
  }

  return zx::ok();
}

}  // namespace power_domain_visitor_dt

REGISTER_DEVICETREE_VISITOR(power_domain_visitor_dt::PowerDomainVisitor);
