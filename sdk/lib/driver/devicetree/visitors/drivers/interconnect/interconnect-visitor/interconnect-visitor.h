// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_INTERCONNECT_INTERCONNECT_VISITOR_INTERCONNECT_VISITOR_H_
#define LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_INTERCONNECT_INTERCONNECT_VISITOR_INTERCONNECT_VISITOR_H_

#include <fidl/fuchsia.hardware.interconnect/cpp/fidl.h>
#include <lib/driver/devicetree/visitors/driver-visitor.h>
#include <lib/driver/devicetree/visitors/property-parser.h>

#include <cstdint>
#include <string_view>

namespace interconnect_dt {

class InterconnectVisitor : public fdf_devicetree::Visitor {
 public:
  static constexpr char kInterconnectReference[] = "interconnects";
  static constexpr char kInterconnectNames[] = "interconnect-names";
  static constexpr char kInterconnectTags[] = "interconnect-tags";

  explicit InterconnectVisitor();

  zx::result<> FinalizeNode(fdf_devicetree::Node& node) override;

  zx::result<> Visit(fdf_devicetree::Node& node,
                     const devicetree::PropertyDecoder& decoder) override;

 private:
  struct Interconnect {
    fuchsia_hardware_interconnect::Metadata metadata;
  };

  // Return an existing or a new instance of Interconnect.
  Interconnect& GetInterconnect(fdf_devicetree::Phandle phandle);

  // Helper to parse nodes with a reference to interconnect in "interconnects" property.
  zx::result<> ParseReferenceChild(fdf_devicetree::Node& child,
                                   const fdf_devicetree::ReferenceNode& parent,
                                   fdf_devicetree::PropertyCells specifiers,
                                   std::string_view interconnect_name, std::optional<uint32_t> tag);

  zx::result<> AddChildNodeSpec(fdf_devicetree::Node& child, uint32_t id,
                                std::string_view interconnect_name);

  static bool IsMatch(std::string_view node_name);

  std::unordered_map<fdf_devicetree::Phandle, Interconnect> interconnects_;
  fdf_devicetree::PropertyParser parser_;
  uint32_t id_ = 1;
};

}  // namespace interconnect_dt

#endif  // LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_INTERCONNECT_INTERCONNECT_VISITOR_INTERCONNECT_VISITOR_H_
