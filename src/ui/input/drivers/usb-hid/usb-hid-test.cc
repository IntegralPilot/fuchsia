// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <dirent.h>
#include <endian.h>
#include <fcntl.h>
#include <fidl/fuchsia.device/cpp/wire.h>
#include <fidl/fuchsia.hardware.input/cpp/wire.h>
#include <fidl/fuchsia.hardware.usb.peripheral/cpp/wire.h>
#include <fidl/fuchsia.hardware.usb.virtual.bus/cpp/wire.h>
#include <lib/component/incoming/cpp/directory.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/ddk/platform-defs.h>
#include <lib/device-watcher/cpp/device-watcher.h>
#include <lib/fdio/cpp/caller.h>
#include <lib/fdio/directory.h>
#include <lib/fit/function.h>
#include <lib/hid/boot.h>
#include <lib/usb-virtual-bus-launcher/usb-virtual-bus-launcher.h>
#include <sys/stat.h>
#include <unistd.h>
#include <zircon/status.h>
#include <zircon/syscalls.h>

#include <fbl/string.h>
#include <gtest/gtest.h>
#include <usb/usb.h>

#include "src/lib/testing/predicates/status.h"

namespace usb_virtual_bus {
namespace {

namespace fhidbus = fuchsia_hardware_hidbus;

using usb_virtual::BusLauncher;

class UsbHidTest : public ::testing::Test {
 public:
  void SetUp() override {
    zx::result bus = BusLauncher::Create();
    ASSERT_OK(bus);
    bus_ = std::move(bus.value());

    auto usb_hid_function_desc = GetConfigDescriptor();
    ASSERT_NO_FATAL_FAILURE(InitUsbHid(usb_hid_function_desc));

    fdio_cpp::UnownedFdioCaller caller(bus_->GetRootFd());
    zx::result controller =
        component::ConnectAt<fuchsia_hardware_input::Controller>(caller.directory(), devpath_);
    ASSERT_OK(controller);
    auto [device, server] = fidl::Endpoints<fuchsia_hardware_input::Device>::Create();

    fidl::OneWayStatus status = fidl::WireCall(controller.value())->OpenSession(std::move(server));
    ASSERT_OK(status.status());

    sync_client_ = fidl::WireSyncClient<fuchsia_hardware_input::Device>(std::move(device));
  }

  void TearDown() override {
    if (bus_.has_value()) {
      ASSERT_OK(bus_->ClearPeripheralDeviceFunctions());
      ASSERT_OK(bus_->Disable());
    }
  }

 protected:
  virtual fuchsia_hardware_usb_peripheral::wire::FunctionDescriptor GetConfigDescriptor() = 0;

  // Initialize a Usb HID device. Asserts on failure.
  void InitUsbHid(fuchsia_hardware_usb_peripheral::wire::FunctionDescriptor desc) {
    namespace usb_peripheral = fuchsia_hardware_usb_peripheral;
    std::vector<usb_peripheral::wire::FunctionDescriptor> function_descs = {desc};
    ASSERT_OK(bus_->SetupPeripheralDevice(
        {
            .bcd_usb = htole16(0x0200),
            .b_max_packet_size0 = 64,
            .id_vendor = htole16(0x18d1),
            .id_product = htole16(0xaf10),
            .bcd_device = htole16(0x0100),
            .b_num_configurations = 1,
        },
        {fidl::VectorView<usb_peripheral::wire::FunctionDescriptor>::FromExternal(
            function_descs)}));
    fdio_cpp::UnownedFdioCaller caller(bus_->GetRootFd());
    {
      zx::result directory = component::OpenDirectoryAt(caller.directory(), "class/input");
      ASSERT_OK(directory);
      zx::result watch_result = device_watcher::WatchDirectoryForItems(
          directory.value(), [this](std::string_view devpath) {
            devpath_ = fbl::String::Concat({"class/input/", devpath});
            return std::monostate{};
          });
      ASSERT_OK(watch_result);
    }
  }

  std::optional<BusLauncher> bus_;
  fbl::String devpath_;
  fidl::WireSyncClient<fuchsia_hardware_input::Device> sync_client_;
};

class UsbOneEndpointTest : public UsbHidTest {
 protected:
  fuchsia_hardware_usb_peripheral::wire::FunctionDescriptor GetConfigDescriptor() override {
    return fuchsia_hardware_usb_peripheral::wire::FunctionDescriptor{
        .interface_class = USB_CLASS_HID,
        .interface_subclass = 0,
        .interface_protocol = USB_PROTOCOL_TEST_HID_ONE_ENDPOINT,
    };
  }
};

TEST_F(UsbOneEndpointTest, GetDeviceIdsVidPid) {
  // Check USB device descriptor VID/PID plumbing.
  auto result = sync_client_->Query();
  ASSERT_OK(result.status());
  ASSERT_TRUE(result->is_ok());
  EXPECT_EQ(0x18d1u, result.value()->info.vendor_id());
  EXPECT_EQ(0xaf10u, result.value()->info.product_id());
}

TEST_F(UsbOneEndpointTest, SetAndGetReport) {
  uint8_t buf[sizeof(hid_boot_mouse_report_t)] = {0xab, 0xbc, 0xde};

  auto set_result = sync_client_->SetReport(fhidbus::wire::ReportType::kInput, 0,
                                            fidl::VectorView<uint8_t>::FromExternal(buf));
  auto get_result = sync_client_->GetReport(fhidbus::wire::ReportType::kInput, 0);

  ASSERT_TRUE(set_result.ok());
  ASSERT_TRUE(set_result->is_ok());

  ASSERT_TRUE(get_result.ok());
  ASSERT_TRUE(get_result->is_ok());

  ASSERT_EQ(get_result.value()->report.size(), sizeof(hid_boot_mouse_report_t));
  ASSERT_EQ(0xab, get_result.value()->report[0]);
  ASSERT_EQ(0xbc, get_result.value()->report[1]);
  ASSERT_EQ(0xde, get_result.value()->report[2]);
}

class UsbTwoEndpointTest : public UsbHidTest {
 protected:
  fuchsia_hardware_usb_peripheral::wire::FunctionDescriptor GetConfigDescriptor() override {
    return fuchsia_hardware_usb_peripheral::wire::FunctionDescriptor{
        .interface_class = USB_CLASS_HID,
        .interface_subclass = 0,
        .interface_protocol = USB_PROTOCOL_TEST_HID_TWO_ENDPOINT,
    };
  }
};

TEST_F(UsbTwoEndpointTest, SetAndGetReport) {
  uint8_t buf[sizeof(hid_boot_mouse_report_t)] = {0xab, 0xbc, 0xde};

  auto set_result = sync_client_->SetReport(fhidbus::wire::ReportType::kInput, 0,
                                            fidl::VectorView<uint8_t>::FromExternal(buf));
  auto get_result = sync_client_->GetReport(fhidbus::wire::ReportType::kInput, 0);

  ASSERT_TRUE(set_result.ok());
  ASSERT_TRUE(set_result->is_ok());

  ASSERT_TRUE(get_result.ok());
  ASSERT_TRUE(get_result->is_ok());

  ASSERT_EQ(get_result.value()->report.size(), sizeof(hid_boot_mouse_report_t));
  ASSERT_EQ(0xab, get_result.value()->report[0]);
  ASSERT_EQ(0xbc, get_result.value()->report[1]);
  ASSERT_EQ(0xde, get_result.value()->report[2]);
}

}  // namespace
}  // namespace usb_virtual_bus
