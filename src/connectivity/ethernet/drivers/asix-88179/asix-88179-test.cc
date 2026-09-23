// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <endian.h>
#include <fcntl.h>
#include <fidl/fuchsia.hardware.ax88179/cpp/wire.h>
#include <lib/async-loop/testing/cpp/real_loop.h>
#include <lib/component/incoming/cpp/directory.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/component/incoming/cpp/service_member_watcher.h>
#include <lib/device-watcher/cpp/device-watcher.h>
#include <lib/fdio/cpp/caller.h>
#include <lib/fzl/vmo-mapper.h>
#include <lib/usb-virtual-bus-launcher/usb-virtual-bus-launcher.h>
#include <lib/zx/clock.h>

#include <fbl/string.h>
#include <usb/cdc.h>
#include <usb/usb.h>
#include <zxtest/zxtest.h>

#include "asix-88179-regs.h"
#include "src/connectivity/lib/network-device/cpp/network_device_client.h"

namespace usb_ax88179 {
namespace {

using usb_virtual::BusLauncher;

namespace ax88179 = fuchsia_hardware_ax88179;

class UsbAx88179Test : public zxtest::Test, loop_fixture::RealLoop {
 public:
  void SetUp() override {
    auto bus = BusLauncher::Create({
        fuchsia_component_test::Capability::WithService(
            fuchsia_component_test::Service{{.name = ax88179::Service::Name}}),
    });
    ASSERT_OK(bus.status_value());
    bus_ = std::move(bus.value());
    ASSERT_NO_FATAL_FAILURE(InitUsbAx88179(&dev_path_));
  }

  void TearDown() override {
    ASSERT_OK(bus_->ClearPeripheralDeviceFunctions());

    ASSERT_OK(bus_->Disable());
  }

  void InitUsbAx88179(fbl::String* dev_path) {
    namespace usb_peripheral = fuchsia_hardware_usb_peripheral;
    using ConfigurationDescriptor =
        ::fidl::VectorView<fuchsia_hardware_usb_peripheral::wire::FunctionDescriptor>;

    usb_peripheral::wire::DeviceDescriptor device_desc = {};
    device_desc.bcd_usb = htole16(0x0200);
    device_desc.b_max_packet_size0 = 64;
    device_desc.bcd_device = htole16(0x0100);
    device_desc.b_num_configurations = 1;

    usb_peripheral::wire::FunctionDescriptor usb_ax88179_desc = {
        .interface_class = USB_CLASS_COMM,
        .interface_subclass = USB_CDC_SUBCLASS_ETHERNET,
        .interface_protocol = 0,
    };

    device_desc.id_vendor = htole16(ASIX_VID);
    device_desc.id_product = htole16(AX88179_PID);
    std::vector<ConfigurationDescriptor> config_descs;
    std::vector<usb_peripheral::wire::FunctionDescriptor> function_descs;
    function_descs.push_back(usb_ax88179_desc);
    ConfigurationDescriptor config_desc;
    config_desc =
        fidl::VectorView<usb_peripheral::wire::FunctionDescriptor>::FromExternal(function_descs);
    config_descs.push_back(std::move(config_desc));
    ASSERT_OK(bus_->SetupPeripheralDevice(std::move(device_desc), std::move(config_descs)));

    fdio_cpp::UnownedFdioCaller caller(bus_->GetRootFd());
    {
      zx::result directory = component::OpenDirectoryAt(caller.directory(), "class/network");
      ASSERT_OK(directory);
      zx::result watch_result = device_watcher::WatchDirectoryForItems(
          directory.value(), [&dev_path](std::string_view devpath) {
            *dev_path = fbl::String::Concat({"class/network/", devpath});
            return std::monostate{};
          });
      ASSERT_OK(watch_result);
    }
  }

  void ConnectNetdeviceClient() {
    fdio_cpp::UnownedFdioCaller caller(bus_->GetRootFd());
    zx::result device =
        component::ConnectAt<fuchsia_hardware_network::Device>(caller.directory(), dev_path_);
    ASSERT_OK(device);

    client_ = std::make_unique<network::client::NetworkDeviceClient>(std::move(device.value()),
                                                                     dispatcher());
    client_->SetErrorCallback([this](zx_status_t error) {
      loop().Quit();
      ADD_FATAL_FAILURE("client failed with %s", zx_status_get_string(error));
    });

    std::optional<zx::result<std::vector<fuchsia_hardware_network::wire::PortId>>> ports;
    client_->GetPorts([&ports](auto found_ports) { ports = std::move(found_ports); });
    RunLoopUntil([&ports]() { return ports.has_value(); });
    ASSERT_OK(ports.value());
    ASSERT_EQ(ports.value().value().size(), 1ul);
    port_id_ = ports.value().value()[0];
    {
      std::optional<zx_status_t> status;
      client_->OpenSession("test_session", [&status](zx_status_t result) { status = result; });
      RunLoopUntil([&status]() { return status.has_value(); });
      ASSERT_OK(status.value());
    }
  }

  void StartDevice() {
    std::optional<zx_status_t> status;
    client_->AttachPort(port_id_, {fuchsia_hardware_network::wire::FrameType::kEthernet},
                        [&status](zx_status_t result) { status = result; });
    RunLoopUntil([&status]() { return status.has_value(); });
    ASSERT_OK(status.value());
  }

  void SetDeviceOnline() {
    component::SyncServiceMemberWatcher<ax88179::Service::Hooks> watcher(bus_->GetExposedDir());
    zx::result test_client_end = watcher.GetNextInstance(false);
    ASSERT_OK(test_client_end.status_value());
    fidl::WireSyncClient test_client{std::move(*test_client_end)};

    auto online_result = test_client->SetOnline(true);
    ASSERT_OK(online_result.status());
    auto result = online_result->status;
    ASSERT_OK(result);
  }

  zx::result<bool> GetDeviceOnline() {
    std::optional<bool> online;
    zx::result handle = client_->WatchStatus(
        port_id_, [&online](fuchsia_hardware_network::wire::PortStatus status) {
          ASSERT_TRUE(status.has_flags());
          online = static_cast<bool>(status.flags() &
                                     fuchsia_hardware_network::wire::StatusFlags::kOnline);
        });
    if (handle.is_error()) {
      return handle.take_error();
    }
    RunLoopUntil([&online]() { return online.has_value(); });
    return zx::ok(online.value());
  }

  void WaitDeviceOnline() {
    bool online = false;
    zx::result handle = client_->WatchStatus(
        port_id_, [&online](fuchsia_hardware_network::wire::PortStatus new_status) {
          EXPECT_TRUE(new_status.has_flags());
          online = static_cast<bool>(new_status.flags() &
                                     fuchsia_hardware_network::wire::StatusFlags::kOnline);
        });
    ASSERT_OK(handle);
    RunLoopUntil([&online]() { return online; });
  }

 protected:
  std::optional<BusLauncher> bus_;
  fbl::String dev_path_;
  std::unique_ptr<network::client::NetworkDeviceClient> client_;
  fuchsia_hardware_network::wire::PortId port_id_;
};

TEST_F(UsbAx88179Test, SetupShutdownTest) { ASSERT_NO_FATAL_FAILURE(); }

TEST_F(UsbAx88179Test, OfflineByDefault) {
  ASSERT_NO_FATAL_FAILURE(ConnectNetdeviceClient());

  ASSERT_NO_FATAL_FAILURE(StartDevice());

  zx::result online = GetDeviceOnline();
  ASSERT_OK(online);
  ASSERT_FALSE(online.value());
}

TEST_F(UsbAx88179Test, SetOnlineAfterStart) {
  ASSERT_NO_FATAL_FAILURE(ConnectNetdeviceClient());

  ASSERT_NO_FATAL_FAILURE(StartDevice());

  ASSERT_NO_FATAL_FAILURE(SetDeviceOnline());

  ASSERT_NO_FATAL_FAILURE(WaitDeviceOnline());
}

// This is for https://fxbug.dev/42116796#c41.
TEST_F(UsbAx88179Test, SetOnlineBeforeStart) {
  ASSERT_NO_FATAL_FAILURE(ConnectNetdeviceClient());

  ASSERT_NO_FATAL_FAILURE(SetDeviceOnline());

  ASSERT_NO_FATAL_FAILURE(StartDevice());

  ASSERT_NO_FATAL_FAILURE(WaitDeviceOnline());
}

}  // namespace
}  // namespace usb_ax88179
