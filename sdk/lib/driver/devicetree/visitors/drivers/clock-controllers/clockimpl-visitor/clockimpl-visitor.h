// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_CLOCK_CONTROLLERS_CLOCKIMPL_VISITOR_CLOCKIMPL_VISITOR_H_
#define LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_CLOCK_CONTROLLERS_CLOCKIMPL_VISITOR_CLOCKIMPL_VISITOR_H_

#include <fidl/fuchsia.hardware.clockimpl/cpp/fidl.h>
#include <lib/driver/devicetree/visitors/driver-visitor.h>
#include <lib/driver/devicetree/visitors/property-parser.h>

#include <cstdint>
#include <memory>
#include <string_view>
#include <vector>

namespace clock_impl_dt {

class ClockImplVisitor : public fdf_devicetree::Visitor {
 public:
  static constexpr char kClockReference[] = "clocks";
  static constexpr char kClockCells[] = "#clock-cells";
  static constexpr char kClockNames[] = "clock-names";
  static constexpr char kAssignedClocks[] = "assigned-clocks";
  static constexpr char kAssignedClockParents[] = "assigned-clock-parents";
  static constexpr char kAssignedClockRates[] = "assigned-clock-rates";

  ClockImplVisitor();

  zx::result<> FinalizeNode(fdf_devicetree::Node& node) override;

  zx::result<> Visit(fdf_devicetree::Node& node,
                     const devicetree::PropertyDecoder& decoder) override;

 private:
  struct ClockController {
#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
    fuchsia_hardware_clockimpl::ClockIdsMetadata clock_nodes_metadata;
#endif
    fuchsia_hardware_clockimpl::InitMetadata init_metadata;
  };

  // Return an existing or a new instance of ClockController.
  ClockController& GetController(fdf_devicetree::Phandle phandle);

  // Helper to parse nodes with a reference to clock-controller in "clocks" property.
  zx::result<> ParseReferenceChild(fdf_devicetree::Node& child,
                                   fdf_devicetree::ReferenceNode& parent,
                                   fdf_devicetree::PropertyCells specifiers,
                                   std::optional<std::string_view> clock_name);

  // Helper to parse clock init hog to produce fuchsia_hardware_clockimpl::InitStep.
  zx::result<> ParseInitChild(fdf_devicetree::Node& child, fdf_devicetree::ReferenceNode& parent,
                              fdf_devicetree::PropertyCells specifiers,
                              std::optional<uint32_t> clock_rate,
                              std::optional<fdf_devicetree::Reference> clock_parent);

  zx::result<> AddChildNodeSpec(fdf_devicetree::Node& child, uint32_t node_id,
                                std::optional<std::string_view> clock_name);

  zx::result<> AddInitChildNodeSpec(fdf_devicetree::Node& child);

  uint32_t GetNextUniqueId();

  bool is_match(const fdf_devicetree::Node& node);

  std::map<fdf_devicetree::Phandle, ClockController> clock_controllers_;
  std::unique_ptr<fdf_devicetree::PropertyParser> clock_parser_;

  uint32_t next_unique_id_ = 0;
};

}  // namespace clock_impl_dt

#endif  // LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_CLOCK_CONTROLLERS_CLOCKIMPL_VISITOR_CLOCKIMPL_VISITOR_H_
