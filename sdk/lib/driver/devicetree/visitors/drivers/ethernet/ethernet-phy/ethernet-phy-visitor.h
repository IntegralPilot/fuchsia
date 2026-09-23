// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_ETHERNET_ETHERNET_PHY_ETHERNET_PHY_VISITOR_H_
#define LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_ETHERNET_ETHERNET_PHY_ETHERNET_PHY_VISITOR_H_

#include <lib/driver/devicetree/manager/visitor.h>
#include <lib/driver/devicetree/visitors/property-parser.h>

#include <string_view>

namespace eth_phy_visitor_dt {

class EthPhyVisitor : public fdf_devicetree::Visitor {
 public:
  static constexpr char kPhys[] = "phys";
  static constexpr char kPhyCells[] = "#phy-cells";

  EthPhyVisitor();
  zx::result<> Visit(fdf_devicetree::Node& node,
                     const devicetree::PropertyDecoder& decoder) override;

 private:
  bool is_match(const std::string& name);

  zx::result<> AddChildNodeSpec(fdf_devicetree::Node& child);

  std::unique_ptr<fdf_devicetree::PropertyParser> parser_;
};

}  // namespace eth_phy_visitor_dt

#endif  // LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_ETHERNET_ETHERNET_PHY_ETHERNET_PHY_VISITOR_H_
