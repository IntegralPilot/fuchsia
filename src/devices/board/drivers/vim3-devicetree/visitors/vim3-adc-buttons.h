// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BOARD_DRIVERS_VIM3_DEVICETREE_VISITORS_VIM3_ADC_BUTTONS_H_
#define SRC_DEVICES_BOARD_DRIVERS_VIM3_DEVICETREE_VISITORS_VIM3_ADC_BUTTONS_H_

#include <lib/driver/devicetree/visitors/driver-visitor.h>

namespace vim3_dt {

// The |Vim3AdcButtonsVisitor| overrides metadata and composite spec for adc buttons node.
// TODO(https://fxbug.dev/340928876) : Create devicetree schema and generic visitor for this
// metadata and dependency on adc, and remove this workaround.
class Vim3AdcButtonsVisitor : public fdf_devicetree::DriverVisitor {
 public:
  explicit Vim3AdcButtonsVisitor()
      : DriverVisitor({"fuchsia,adc-buttons", "amlogic,meson-g12a-saradc"}) {}
  zx::result<> DriverVisit(fdf_devicetree::Node& node,
                           const devicetree::PropertyDecoder& decoder) override;

 private:
  zx::result<> AddAdcButtonsMetadata(fdf_devicetree::Node& node);
};

}  // namespace vim3_dt

#endif  // SRC_DEVICES_BOARD_DRIVERS_VIM3_DEVICETREE_VISITORS_VIM3_ADC_BUTTONS_H_
