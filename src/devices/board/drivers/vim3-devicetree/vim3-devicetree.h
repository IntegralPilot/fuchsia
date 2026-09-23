// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BOARD_DRIVERS_VIM3_DEVICETREE_VIM3_DEVICETREE_H_
#define SRC_DEVICES_BOARD_DRIVERS_VIM3_DEVICETREE_VIM3_DEVICETREE_H_

#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/devicetree/manager/manager.h>
#include <lib/driver/devicetree/visitors/registry.h>

#include <memory>
#include <optional>

namespace vim3_dt {

// Vim3 board driver based on device tree
class Vim3Devicetree : public fdf::DriverBase2 {
 public:
  Vim3Devicetree() : fdf::DriverBase2("vim3-devicetree") {}

  zx::result<> Start(fdf::DriverContext context) final;

 private:
  fidl::SyncClient<fuchsia_driver_framework::Node> node_;
};

}  // namespace vim3_dt

#endif  // SRC_DEVICES_BOARD_DRIVERS_VIM3_DEVICETREE_VIM3_DEVICETREE_H_
