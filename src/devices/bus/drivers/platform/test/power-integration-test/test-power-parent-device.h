// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BUS_DRIVERS_PLATFORM_TEST_POWER_INTEGRATION_TEST_TEST_POWER_PARENT_DEVICE_H_
#define SRC_DEVICES_BUS_DRIVERS_PLATFORM_TEST_POWER_INTEGRATION_TEST_TEST_POWER_PARENT_DEVICE_H_

#include <fidl/fuchsia.hardware.power/cpp/fidl.h>
#include <fidl/fuchsia.power.broker/cpp/fidl.h>
#include <lib/driver/component/cpp/driver_base2.h>

namespace fake_parent_device {

class FakeParentServer : public fidl::WireServer<fuchsia_hardware_power::PowerTokenProvider> {
 public:
  explicit FakeParentServer(std::string element_name) : element_name_(std::move(element_name)) {}
  void GetToken(GetTokenCompleter::Sync& completer) override;
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_hardware_power::PowerTokenProvider> md,
      fidl::UnknownMethodCompleter::Sync& completer) override;

 private:
  std::string element_name_;
};

class FakeParent : public fdf::DriverBase2 {
 public:
  FakeParent() : fdf::DriverBase2("fake-parent") {}

  zx::result<> Start(fdf::DriverContext context) override;

 private:
  fidl::ServerBindingGroup<fuchsia_hardware_power::PowerTokenProvider> bindings_;
  fidl::WireSyncClient<fuchsia_driver_framework::Node> node_;
  fidl::WireSyncClient<fuchsia_driver_framework::NodeController> controller_;
  fidl::WireSyncClient<fuchsia_driver_framework::NodeController> child_controller_;
  std::unique_ptr<FakeParentServer> server_;
  std::unique_ptr<FakeParentServer> server2_;
  fidl::WireClient<fuchsia_power_broker::Topology> topology_client_;
};

}  // namespace fake_parent_device

#endif  // SRC_DEVICES_BUS_DRIVERS_PLATFORM_TEST_POWER_INTEGRATION_TEST_TEST_POWER_PARENT_DEVICE_H_
