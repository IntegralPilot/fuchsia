// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_USB_DWC2_DWC2_VISITOR_H_
#define LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_USB_DWC2_DWC2_VISITOR_H_

#include <lib/driver/devicetree/visitors/driver-visitor.h>
#include <lib/driver/devicetree/visitors/property-parser.h>

namespace dwc2_visitor_dt {

class Dwc2Visitor : public fdf_devicetree::DriverVisitor {
 public:
  static constexpr char kGRxFifoSize[] = "g-rx-fifo-size";
  static constexpr char kGNpTxFifoSize[] = "g-np-tx-fifo-size";
  static constexpr char kGTxFifoSize[] = "g-tx-fifo-size";
  static constexpr char kGTurnaroundTime[] = "g-turnaround-time";
  static constexpr char kDmaBurstLen[] = "dma-burst-len";

  Dwc2Visitor();
  zx::result<> DriverVisit(fdf_devicetree::Node& node,
                           const devicetree::PropertyDecoder& decoder) override;

 private:
  std::unique_ptr<fdf_devicetree::PropertyParser> parser_;
};

}  // namespace dwc2_visitor_dt

#endif  // LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_USB_DWC2_DWC2_VISITOR_H_
