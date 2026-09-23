// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_INPUT_TOUCHSCREEN_FOCALTECH_FOCALTECH_VISITOR_H_
#define LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_INPUT_TOUCHSCREEN_FOCALTECH_FOCALTECH_VISITOR_H_

#include <lib/driver/devicetree/visitors/property-parser.h>

#include "lib/driver/devicetree/visitors/driver-visitor.h"

namespace focaltech_visitor_dt {

class FocaltechVisitor : public fdf_devicetree::DriverVisitor {
 public:
  static constexpr char kCompatible[] = "compatible";
  static constexpr char kNeedsFirmware[] = "focaltech,needs-firmware";

  FocaltechVisitor();
  zx::result<> DriverVisit(fdf_devicetree::Node& node,
                           const devicetree::PropertyDecoder& decoder) override;

 private:
  std::unique_ptr<fdf_devicetree::PropertyParser> parser_;
};

}  // namespace focaltech_visitor_dt

#endif  // LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_INPUT_TOUCHSCREEN_FOCALTECH_FOCALTECH_VISITOR_H_
