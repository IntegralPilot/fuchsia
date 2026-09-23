// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/component/cpp/node_add_args.h>

#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/test/platform/cpp/bind.h>

class Child2Driver : public fdf::DriverBase2 {
 public:
  Child2Driver() : fdf::DriverBase2("test-child-2") {}

  zx::result<> Start(fdf::DriverContext context) override {
    auto properties = std::vector{
        fdf::MakeProperty2(bind_fuchsia::PLATFORM_DEV_VID,
                           bind_fuchsia_test_platform::BIND_PLATFORM_DEV_VID_TEST),
        fdf::MakeProperty2(bind_fuchsia::PLATFORM_DEV_PID,
                           bind_fuchsia_test_platform::BIND_PLATFORM_DEV_PID_PBUS_TEST),
        fdf::MakeProperty2(bind_fuchsia::PLATFORM_DEV_DID,
                           bind_fuchsia_test_platform::BIND_PLATFORM_DEV_DID_CHILD_4),
    };
    zx::result result = AddChild("child-4", properties, {});
    if (result.is_error()) {
      return result.take_error();
    }
    return zx::ok();
  }
};

FUCHSIA_DRIVER_EXPORT2(Child2Driver);
