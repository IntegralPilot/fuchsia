// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_POWER_POWER_ELEMENT_POWER_ELEMENT_VISITOR_H_
#define LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_POWER_POWER_ELEMENT_POWER_ELEMENT_VISITOR_H_

#include <fidl/fuchsia.hardware.power/cpp/fidl.h>
#include <lib/driver/devicetree/manager/visitor.h>
#include <lib/driver/devicetree/visitors/property-parser.h>

namespace power_element_visitor_dt {

class PowerElementVisitor : public fdf_devicetree::Visitor {
 public:
  static constexpr char kLevel[] = "level";
  static constexpr char kLevelDependencies[] = "level-dependencies";
  static constexpr char kTargetLevel[] = "target-level";
  static constexpr char kLatencyUs[] = "latency-us";

  PowerElementVisitor();
  zx::result<> Visit(fdf_devicetree::Node& node,
                     const devicetree::PropertyDecoder& decoder) override;

 private:
  // Checks if the node is a power element.
  static bool IsMatch(fdf_devicetree::Node& node);

  // Extract power element name from node name.
  // Example: For a node with name "device-x-wakeup-element", it will return
  // "device-x-wakeup".
  static std::optional<std::string> GetElementName(const std::string& node_name);

  // Extract power level name from node name.
  // Example: For a node with name "off-level", it will return
  // "off".
  static std::optional<std::string> GetLevelName(const std::string& node_name);

  // Return the power element corresponding to the level reference node.
  // The power element could be an System Activity Governor (SAG) power element.
  // |fuchsia_hardware_power::ParentElement| is a union of SAG and non-SAG power elements.
  static std::optional<fuchsia_hardware_power::ParentElement> GetParentElementFromLevelRef(
      fdf_devicetree::ReferenceNode& level_in_parent);

  // Given |child_name|, |parent| and |type|, return the power dependency entry if one exists. If it
  // does not exist, a new entry is added and returned.
  static fuchsia_hardware_power::PowerDependency& GetPowerDependency(
      fuchsia_hardware_power::PowerElementConfiguration& element_config,
      const std::string& child_name, const fuchsia_hardware_power::ParentElement& parent);

  // Helper method that returns the power element configuration for a given power element node.
  zx::result<fuchsia_hardware_power::PowerElementConfiguration> ParsePowerElement(
      fdf_devicetree::Node& node);

  // Helper method to parse power level node of a power element. The level information is updated in
  // the |element_config|.
  zx::result<> ParseLevel(fdf_devicetree::Node& node,
                          fuchsia_hardware_power::PowerElementConfiguration& element_config);

  // Helper method to parse level transition table. The table information is added to |level|.
  zx::result<> ParseTransitionTable(fdf_devicetree::Node& node,
                                    fuchsia_hardware_power::PowerLevel& level);

  std::unique_ptr<fdf_devicetree::PropertyParser> level_parser_;
  std::unique_ptr<fdf_devicetree::PropertyParser> transition_parser_;
  std::unique_ptr<fdf_devicetree::PropertyParser> reference_parser_;
};

}  // namespace power_element_visitor_dt

#endif  // LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_POWER_POWER_ELEMENT_POWER_ELEMENT_VISITOR_H_
