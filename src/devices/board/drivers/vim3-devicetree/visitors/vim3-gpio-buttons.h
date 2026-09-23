// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BOARD_DRIVERS_VIM3_DEVICETREE_VISITORS_VIM3_GPIO_BUTTONS_H_
#define SRC_DEVICES_BOARD_DRIVERS_VIM3_DEVICETREE_VISITORS_VIM3_GPIO_BUTTONS_H_

#include <lib/driver/devicetree/visitors/driver-visitor.h>

namespace vim3_dt {

// The |Vim3GpioButtonsVisitor| overrides metadata for gpio buttons node.
// TODO(https://fxbug.dev/341211742) : Create devicetree schema and generic visitor for this
// metadata, and remove this workaround.
class Vim3GpioButtonsVisitor : public fdf_devicetree::DriverVisitor {
 public:
  explicit Vim3GpioButtonsVisitor() : DriverVisitor({"fuchsia,gpio-buttons"}) {}
  zx::result<> DriverVisit(fdf_devicetree::Node& node,
                           const devicetree::PropertyDecoder& decoder) override;
};

}  // namespace vim3_dt

#endif  // SRC_DEVICES_BOARD_DRIVERS_VIM3_DEVICETREE_VISITORS_VIM3_GPIO_BUTTONS_H_
