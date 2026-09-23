// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_USB_USB_PHY_USB_PHY_VISITOR_H_
#define LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_USB_USB_PHY_USB_PHY_VISITOR_H_

#include <lib/driver/devicetree/manager/visitor.h>
#include <lib/driver/devicetree/visitors/property-parser.h>

#include <string_view>

namespace usb_phy_visitor_dt {

class UsbPhyVisitor : public fdf_devicetree::Visitor {
 public:
  static constexpr char kPhys[] = "phys";
  static constexpr char kPhyNames[] = "phy-names";
  static constexpr char kPhyCells[] = "#phy-cells";

  UsbPhyVisitor();
  zx::result<> Visit(fdf_devicetree::Node& node,
                     const devicetree::PropertyDecoder& decoder) override;

 private:
  bool is_match(const std::string& name);

  zx::result<> AddChildNodeSpec(fdf_devicetree::Node& child, std::string_view phy_name);

  std::unique_ptr<fdf_devicetree::PropertyParser> parser_;
};

}  // namespace usb_phy_visitor_dt

#endif  // LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_USB_USB_PHY_USB_PHY_VISITOR_H_
