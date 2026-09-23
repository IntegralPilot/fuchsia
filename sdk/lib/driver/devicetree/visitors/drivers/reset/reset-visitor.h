// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_RESET_RESET_VISITOR_H_
#define LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_RESET_RESET_VISITOR_H_

#include <fidl/fuchsia.hardware.reset/cpp/fidl.h>
#include <lib/driver/devicetree/visitors/driver-visitor.h>
#include <lib/driver/devicetree/visitors/property-parser.h>

#include <memory>

#include "lib/driver/devicetree/manager/node.h"

namespace reset_dt {

class ResetVisitor : public fdf_devicetree::Visitor {
 public:
  static constexpr char kResetReference[] = "resets";
  static constexpr char kResetCells[] = "#reset-cells";
  static constexpr char kResetNames[] = "reset-names";

  ResetVisitor();
  zx::result<> Visit(fdf_devicetree::Node& node,
                     const devicetree::PropertyDecoder& decoder) override;
  zx::result<> FinalizeNode(fdf_devicetree::Node& node) override;

 private:
  struct ResetController {
    fuchsia_hardware_reset::Metadata metadata;
  };

  zx::result<> AddChildNodeSpec(fdf_devicetree::Node& child, uint32_t controller_id,
                                uint32_t reset_id, const std::string& reset_name);
  zx::result<> ParseReferenceChild(fdf_devicetree::Node& child,
                                   fdf_devicetree::ReferenceNode& parent,
                                   fdf_devicetree::PropertyCells specifiers,
                                   const std::string& reset_name);

  ResetController& GetController(uint32_t node_id);

  static bool isController(
      const std::unordered_map<std::string_view, devicetree::PropertyValue>& properties) {
    return properties.contains(kResetCells);
  }

  std::map<uint32_t, ResetController> reset_controllers_;
  std::unique_ptr<fdf_devicetree::PropertyParser> reset_parser_;
};

}  // namespace reset_dt

#endif  // LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_RESET_RESET_VISITOR_H_
