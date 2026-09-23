// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BOARD_DRIVERS_VIM3_DEVICETREE_VISITORS_VIM3_NNA_H_
#define SRC_DEVICES_BOARD_DRIVERS_VIM3_DEVICETREE_VISITORS_VIM3_NNA_H_

#include <lib/driver/devicetree/visitors/driver-visitor.h>

namespace vim3_dt {

// TODO(https://fxbug.dev/342174810): Remove this metadata and replace with API/composite nodes.
class Vim3NnaVisitor : public fdf_devicetree::DriverVisitor {
 public:
  explicit Vim3NnaVisitor() : DriverVisitor({"amlogic,nna"}) {}
  zx::result<> DriverVisit(fdf_devicetree::Node& node,
                           const devicetree::PropertyDecoder& decoder) override;
};

}  // namespace vim3_dt

#endif  // SRC_DEVICES_BOARD_DRIVERS_VIM3_DEVICETREE_VISITORS_VIM3_NNA_H_
