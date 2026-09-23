// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_USB_TESTING_USB_VIRTUAL_BUS_LAUNCHER_INCLUDE_LIB_USB_VIRTUAL_BUS_LAUNCHER_USB_VIRTUAL_BUS_LAUNCHER_H_
#define SRC_DEVICES_USB_TESTING_USB_VIRTUAL_BUS_LAUNCHER_INCLUDE_LIB_USB_VIRTUAL_BUS_LAUNCHER_USB_VIRTUAL_BUS_LAUNCHER_H_

#include <fidl/fuchsia.hardware.usb.peripheral/cpp/wire.h>
#include <fidl/fuchsia.hardware.usb.virtual.bus/cpp/wire.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/driver_test_realm/realm_builder/cpp/builder.h>
#include <lib/sys/component/cpp/testing/realm_builder.h>
#include <lib/zx/result.h>

#include <vector>

#include <fbl/unique_fd.h>

namespace usb_virtual {

using ConfigurationDescriptor =
    ::fidl::VectorView<fuchsia_hardware_usb_peripheral::wire::FunctionDescriptor>;
using fuchsia_hardware_usb_peripheral::wire::DeviceDescriptor;

// Helper class that launches an driver_test_realm with a virtual USB bus for tests.
class BusLauncher {
 public:
  BusLauncher(BusLauncher&& other) = default;
  BusLauncher& operator=(BusLauncher&& other) = default;

  BusLauncher(const BusLauncher&) = delete;
  BusLauncher& operator=(const BusLauncher&) = delete;

  virtual ~BusLauncher();

  // Create the driver_test_realm, wait for it to start, then enable the virtual USB bus.
  static zx::result<BusLauncher> Create(
      std::vector<fuchsia_component_test::Capability> exposes = {});

  // Set up a USB peripheral device with the given descriptors. See fuchsia.hardware.usb.peripheral
  // for more information. Waits for the functions to be registered and triggers a connect event on
  // the virtual bus.
  [[nodiscard]] zx_status_t SetupPeripheralDevice(
      DeviceDescriptor&& device_desc, std::vector<ConfigurationDescriptor> config_descs);

  // Asks the peripheral device to clear its functions and waits for the FunctionsCleared event.
  [[nodiscard]] zx_status_t ClearPeripheralDeviceFunctions();

  // Get a file descriptor to the root of driver_test_realm's devfs.
  int GetRootFd();

  // Get the exposed directory of the driver_test_realm's root node.
  fidl::UnownedClientEnd<fuchsia_io::Directory> GetExposedDir();

  // Disable the virtual bus.
  [[nodiscard]] zx_status_t Disable();

  // Enable the virtual bus.
  [[nodiscard]] zx_status_t Enable();

  // Disconnect the virtual bus.
  [[nodiscard]] zx_status_t Disconnect();

 private:
  BusLauncher(component_testing::RealmRoot realm, fbl::unique_fd devfs_root)
      : realm_(std::move(realm)), devfs_root_(std::move(devfs_root)) {}

  std::unique_ptr<async::Loop> loop_;
  component_testing::RealmRoot realm_;
  fbl::unique_fd devfs_root_;
  fidl::WireSyncClient<fuchsia_hardware_usb_peripheral::Device> peripheral_;
  fidl::WireSyncClient<fuchsia_hardware_usb_virtual_bus::Bus> virtual_bus_;
};

}  // namespace usb_virtual

#endif  // SRC_DEVICES_USB_TESTING_USB_VIRTUAL_BUS_LAUNCHER_INCLUDE_LIB_USB_VIRTUAL_BUS_LAUNCHER_USB_VIRTUAL_BUS_LAUNCHER_H_
