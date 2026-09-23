// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>

class Child3Driver : public fdf::DriverBase2 {
 public:
  Child3Driver() : fdf::DriverBase2("child-3") {}

  zx::result<> Start(fdf::DriverContext context) override {
    std::vector<fuchsia_driver_framework::NodeProperty2> properties = {};
    zx::result result = AddChild("child-3", properties, {});
    if (result.is_error()) {
      return result.take_error();
    }
    return zx::ok();
  }
};

FUCHSIA_DRIVER_EXPORT2(Child3Driver);
