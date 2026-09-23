// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BUS_DRIVERS_PLATFORM_TEST_POWER_INTEGRATION_TEST_TEST_POWER_CHILD_DEVICE_H_
#define SRC_DEVICES_BUS_DRIVERS_PLATFORM_TEST_POWER_INTEGRATION_TEST_TEST_POWER_CHILD_DEVICE_H_

#include <fidl/fuchsia.power.broker/cpp/fidl.h>
#include <lib/driver/component/cpp/driver_base2.h>

namespace fake_child_device {

class FakeChild : public fdf::DriverBase2 {
 public:
  FakeChild() : fdf::DriverBase2("fake-child") {}

  ~FakeChild() override = default;

  zx::result<> Start(fdf::DriverContext context) override;

 private:
  fidl::WireSyncClient<fuchsia_driver_framework::Node> node_;
  fidl::WireSyncClient<fuchsia_driver_framework::NodeController> controller_;
  fidl::ClientEnd<fuchsia_power_broker::Lessor> lessor_;
};
}  // namespace fake_child_device

#endif  // SRC_DEVICES_BUS_DRIVERS_PLATFORM_TEST_POWER_INTEGRATION_TEST_TEST_POWER_CHILD_DEVICE_H_
