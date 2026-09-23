// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_POWER_POWER_DOMAIN_POWER_DOMAIN_VISITOR_H_
#define LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_POWER_POWER_DOMAIN_POWER_DOMAIN_VISITOR_H_

#include <fidl/fuchsia.hardware.power/cpp/fidl.h>
#include <fidl/fuchsia.hardware.powerdomain/cpp/fidl.h>
#include <lib/driver/devicetree/manager/visitor.h>
#include <lib/driver/devicetree/visitors/property-parser.h>

#include <optional>
#include <string_view>

namespace power_domain_visitor_dt {

class PowerDomainVisitor : public fdf_devicetree::Visitor {
 public:
  static constexpr char kPowerDomainCells[] = "#power-domain-cells";
  static constexpr uint32_t kPowerDomainCellsSize = 1;
  static constexpr char kPowerDomains[] = "power-domains";
  static constexpr char kPowerDomainNames[] = "power-domain-names";
  static constexpr char kLegacyPower[] = "fuchsia,legacy-power";

  PowerDomainVisitor();
  zx::result<> Visit(fdf_devicetree::Node& node,
                     const devicetree::PropertyDecoder& decoder) override;

  zx::result<> FinalizeNode(fdf_devicetree::Node& node) override;

 private:
  struct PowerController {
    std::optional<fuchsia_hardware_power::DomainMetadata> full_domain_info;
    std::optional<fuchsia_hardware_powerdomain::DomainMetadata> basic_domain_info;
    bool is_full_power = false;
  };

  // Return an existing or a new instance of PowerController.
  PowerController& GetController(fdf_devicetree::Phandle phandle);

  // Helper to parse nodes with a reference to power-controller in "power-domains" property.
  zx::result<> ParseReferenceChild(fdf_devicetree::Node& child,
                                   fdf_devicetree::ReferenceNode& parent,
                                   fdf_devicetree::PropertyCells specifiers,
                                   std::optional<std::string_view> name = std::nullopt);

  uint32_t GetNextUniqueId();

  zx::result<> AddChildNodeSpec(fdf_devicetree::Node& child, uint32_t domain_id, uint32_t node_id,
                                bool is_full_power,
                                std::optional<std::string_view> name = std::nullopt);

  std::unique_ptr<fdf_devicetree::PropertyParser> parser_;
  // Mapping of power controller Phandle to its info.
  std::map<fdf_devicetree::Phandle, PowerController> power_controllers_;
  uint32_t next_unique_id_ = 1;
};

}  // namespace power_domain_visitor_dt

#endif  // LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_POWER_POWER_DOMAIN_POWER_DOMAIN_VISITOR_H_
