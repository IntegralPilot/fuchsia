// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "performance-domain-visitor.h"

#include <lib/driver/devicetree/visitors/registration.h>
#include <lib/driver/logging/cpp/logger.h>

#include <cstdint>
#include <regex>
#include <vector>

namespace performance_domain_visitor_dt {

PerformanceDomainVisitor::PerformanceDomainVisitor() {
  fdf_devicetree::Properties domain_properties = {};
  domain_properties.emplace_back(std::make_unique<fdf_devicetree::Uint32Property>(kDomainID, true));
  domain_properties.emplace_back(
      std::make_unique<fdf_devicetree::Uint32ArrayProperty>(kCpus, true));
  domain_properties.emplace_back(
      std::make_unique<fdf_devicetree::Uint32Property>(kRelativePerformance, true));
  domain_properties.emplace_back(
      std::make_unique<fdf_devicetree::ReferenceProperty>(kOperatingPoints, 0u, true));

  performance_domain_parser_ =
      std::make_unique<fdf_devicetree::PropertyParser>(std::move(domain_properties));

  fdf_devicetree::Properties opp_properties = {};
  opp_properties.emplace_back(
      std::make_unique<fdf_devicetree::Uint64Property>(kOperatingFrequency, true));
  opp_properties.emplace_back(
      std::make_unique<fdf_devicetree::Uint32Property>(kOperatingMicrovolt, true));

  opp_parser_ = std::make_unique<fdf_devicetree::PropertyParser>(std::move(opp_properties));
}

zx::result<> PerformanceDomainVisitor::Visit(fdf_devicetree::Node& node,
                                             const devicetree::PropertyDecoder& decoder) {
  if (!IsMatch(node)) {
    return zx::ok();
  }

  auto device_node = node.parent().GetNode();

  std::vector<fuchsia_hardware_amlogic_metadata::PerformanceDomain> performance_domains;
  std::vector<fuchsia_hardware_amlogic_metadata::OperatingPoint> opp_tables;

  for (auto& child : node.children()) {
    zx::result performance_domain = ParsePerformanceDomain(*child.GetNode(), opp_tables);
    if (performance_domain.is_error()) {
      return performance_domain.take_error();
    }

    fdf::debug("Added performance domain '{}' to node '{}'.", performance_domain->name(),
               device_node->name());

    performance_domains.push_back(std::move(performance_domain.value()));
  }

  if (!performance_domains.empty()) {
    const fuchsia_hardware_amlogic_metadata::CpuMetadata cpu_metadata(
        {.performance_domains = std::move(performance_domains),
         .operating_points = std::move(opp_tables)});

    fit::result persisted_metadata = fidl::Persist(cpu_metadata);
    if (!persisted_metadata.is_ok()) {
      fdf::error("Failed to persist metadata: {}",
                 persisted_metadata.error_value().FormatDescription());

      return zx::error(persisted_metadata.error_value().status());
    }

    fuchsia_hardware_platform_bus::Metadata metadata({
        .id = fuchsia_hardware_amlogic_metadata::CpuMetadata::kSerializableName,
        .data = std::move(persisted_metadata.value()),
    });
    device_node->AddMetadata(std::move(metadata));
  }

  return zx::ok();
}

bool PerformanceDomainVisitor::IsMatch(fdf_devicetree::Node& node) {
  return node.parent() && node.name() == "performance-domains";
}

std::optional<std::string> PerformanceDomainVisitor::GetDomainName(const std::string& node_name) {
  std::smatch match;
  std::regex name_regex("(^[a-zA-Z0-9-]*)-domain$");
  if (std::regex_search(node_name, match, name_regex) && match.size() == 2) {
    return match[1];
  }
  return std::nullopt;
}

zx::result<fuchsia_hardware_amlogic_metadata::PerformanceDomain>
PerformanceDomainVisitor::ParsePerformanceDomain(
    fdf_devicetree::Node& node,
    std::vector<fuchsia_hardware_amlogic_metadata::OperatingPoint>& opp_tables) {
  auto parser_output = performance_domain_parser_->Parse(node);
  if (parser_output.is_error()) {
    fdf::error("Performance domain visitor failed for node '{}' : {}", node.name(), parser_output);

    return parser_output.take_error();
  }

  fuchsia_hardware_amlogic_metadata::PerformanceDomain performance_domain;
  performance_domain.id() = parser_output->Get<uint32_t>(kDomainID).value();
  performance_domain.relative_performance() =
      static_cast<uint8_t>(parser_output->Get<uint32_t>(kRelativePerformance).value());
  performance_domain.core_count() =
      static_cast<uint32_t>(parser_output->Get<std::vector<uint32_t>>(kCpus)->size());

  auto operating_points = parser_output->Get<fdf_devicetree::References>(kOperatingPoints);
  if (operating_points->size() > 1) {
    fdf::warn("Node '{}' has {} operating points, but only the first will be used.", node.name(),
              operating_points->size());
  }

  auto opp_table =
      ParseOppTable(*operating_points->at(0).reference_node().GetNode(), performance_domain.id());
  if (opp_table.is_error()) {
    return opp_table.take_error();
  }
  opp_tables.insert(opp_tables.end(), opp_table->begin(), opp_table->end());

  const std::optional domain_name = GetDomainName(node.name());

  if (!domain_name) {
    fdf::error("Performance domain has invalid node name '{}'.", node.name());

    return zx::error(ZX_ERR_INVALID_ARGS);
  }
  performance_domain.name() = domain_name.value();
  return zx::ok(performance_domain);
}

zx::result<std::vector<fuchsia_hardware_amlogic_metadata::OperatingPoint>>
PerformanceDomainVisitor::ParseOppTable(fdf_devicetree::Node& node, uint32_t domain_id) {
  std::vector<fuchsia_hardware_amlogic_metadata::OperatingPoint> opp_table;
  for (auto& child : node.children()) {
    auto parser_output = opp_parser_->Parse(*child.GetNode());
    if (parser_output.is_error()) {
      fdf::error("Operating point visitor failed for node '{}' : {}", child.name(), parser_output);

      return parser_output.take_error();
    }

    opp_table.push_back(fuchsia_hardware_amlogic_metadata::OperatingPoint(
        {.freq_hz =
             static_cast<uint32_t>(parser_output->Get<uint64_t>(kOperatingFrequency).value()),
         .volt_uv = parser_output->Get<uint32_t>(kOperatingMicrovolt).value(),
         .pd_id = domain_id}));
  }
  return zx::ok(opp_table);
}

}  // namespace performance_domain_visitor_dt

REGISTER_DEVICETREE_VISITOR(performance_domain_visitor_dt::PerformanceDomainVisitor);
