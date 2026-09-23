// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_USB_AML_USB_PHY_AML_USB_PHY_VISITOR_H_
#define LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_USB_AML_USB_PHY_AML_USB_PHY_VISITOR_H_

#include <lib/driver/devicetree/visitors/driver-visitor.h>
#include <lib/driver/devicetree/visitors/property-parser.h>

namespace aml_usb_phy_visitor_dt {

class AmlUsbPhyVisitor : public fdf_devicetree::DriverVisitor {
 public:
  static constexpr char kPllSettings[] = "pll-settings";
  static constexpr char kDrModes[] = "dr_modes";
  static constexpr char kRegNames[] = "reg-names";
  static constexpr const char kCompatible[] = "compatible";

  AmlUsbPhyVisitor();

  zx::result<> DriverVisit(fdf_devicetree::Node& node,
                           const devicetree::PropertyDecoder& decoder) override;

 private:
  std::unique_ptr<fdf_devicetree::PropertyParser> parser_;
};

}  // namespace aml_usb_phy_visitor_dt

#endif  // LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_USB_AML_USB_PHY_AML_USB_PHY_VISITOR_H_
