// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_CPU_PERFORMANCE_DOMAIN_PERFORMANCE_DOMAIN_VISITOR_H_
#define LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_CPU_PERFORMANCE_DOMAIN_PERFORMANCE_DOMAIN_VISITOR_H_

#include <fidl/fuchsia.hardware.amlogic.metadata/cpp/fidl.h>
#include <lib/driver/devicetree/manager/visitor.h>
#include <lib/driver/devicetree/visitors/property-parser.h>

namespace performance_domain_visitor_dt {

class PerformanceDomainVisitor : public fdf_devicetree::Visitor {
 public:
  static constexpr char kDomainID[] = "domain-id";
  static constexpr char kCpus[] = "cpus";
  static constexpr char kOperatingPoints[] = "operating-points";
  static constexpr char kRelativePerformance[] = "relative-performance";
  static constexpr char kOperatingFrequency[] = "opp-hz";
  static constexpr char kOperatingMicrovolt[] = "opp-microvolt";

  PerformanceDomainVisitor();
  zx::result<> Visit(fdf_devicetree::Node& node,
                     const devicetree::PropertyDecoder& decoder) override;

 private:
  // Checks if the node is a performance domain node.
  static bool IsMatch(fdf_devicetree::Node& node);

  // Extract performance domain name from node name.
  // Example: For a node with name "high-performance-domain", it will return
  // "high-performance".
  static std::optional<std::string> GetDomainName(const std::string& node_name);

  zx::result<fuchsia_hardware_amlogic_metadata::PerformanceDomain> ParsePerformanceDomain(
      fdf_devicetree::Node& node,
      std::vector<fuchsia_hardware_amlogic_metadata::OperatingPoint>& opp_tables);

  zx::result<std::vector<fuchsia_hardware_amlogic_metadata::OperatingPoint>> ParseOppTable(
      fdf_devicetree::Node& node, uint32_t domain_id);

  std::unique_ptr<fdf_devicetree::PropertyParser> performance_domain_parser_;
  std::unique_ptr<fdf_devicetree::PropertyParser> opp_parser_;
};

}  // namespace performance_domain_visitor_dt

#endif  // LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_CPU_PERFORMANCE_DOMAIN_PERFORMANCE_DOMAIN_VISITOR_H_
