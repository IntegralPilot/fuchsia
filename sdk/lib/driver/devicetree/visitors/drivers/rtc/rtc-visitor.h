// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_RTC_RTC_VISITOR_H_
#define LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_RTC_RTC_VISITOR_H_

#include <lib/driver/devicetree/visitors/driver-visitor.h>
#include <lib/driver/devicetree/visitors/property-parser.h>

#include <set>

#include "lib/driver/devicetree/manager/node.h"

namespace rtc_dt {

// TODO(https://fxbug.dev/473553516): Assuming that there is only one RTC available for now.
class RtcVisitor : public fdf_devicetree::Visitor {
 public:
  static constexpr char kRtcReference[] = "rtcs";

  RtcVisitor();
  zx::result<> Visit(fdf_devicetree::Node& node,
                     const devicetree::PropertyDecoder& decoder) override;

 private:
  zx::result<> ParseReferenceChild(fdf_devicetree::Node& child,
                                   fdf_devicetree::ReferenceNode& parent);

  std::unique_ptr<fdf_devicetree::PropertyParser> rtc_parser_;
};

}  // namespace rtc_dt

#endif  // LIB_DRIVER_DEVICETREE_VISITORS_DRIVERS_RTC_RTC_VISITOR_H_
