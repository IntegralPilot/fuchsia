// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <dirent.h>
#include <endian.h>
#include <fcntl.h>
#include <fidl/fuchsia.hardware.serial/cpp/wire.h>
#include <fidl/fuchsia.hardware.usb.peripheral/cpp/wire.h>
#include <fidl/fuchsia.hardware.usb.virtual.bus/cpp/wire.h>
#include <fidl/fuchsia.io/cpp/wire.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/ddk/platform-defs.h>
#include <lib/fdio/cpp/caller.h>
#include <lib/fdio/directory.h>
#include <lib/fdio/watcher.h>
#include <lib/hid/boot.h>
#include <lib/usb-virtual-bus-launcher/usb-virtual-bus-launcher.h>
#include <sys/stat.h>
#include <unistd.h>
#include <zircon/syscalls.h>

#include <ctime>

#include <fbl/string.h>
#include <usb/usb.h>
#include <zxtest/zxtest.h>

namespace usb_virtual_bus {
namespace {

zx_status_t WaitForAnyFile(int dirfd, int event, const char* name, void* cookie) {
  if (std::string_view{name} == ".") {
    return ZX_OK;
  }
  if (event != WATCH_EVENT_ADD_FILE) {
    return ZX_OK;
  }
  if (*name) {
    *reinterpret_cast<fbl::String*>(cookie) = fbl::String(name);
    return ZX_ERR_STOP;
  } else {
    return ZX_OK;
  }
}

using usb_virtual::BusLauncher;

class FtdiTest : public zxtest::Test {
 public:
  void SetUp() override {
    auto bus = BusLauncher::Create();
    ASSERT_OK(bus.status_value());
    bus_ = std::move(bus.value());

    ASSERT_NO_FATAL_FAILURE(InitFtdi(devpath_));
  }

  void TearDown() override {
    ASSERT_OK(bus_->ClearPeripheralDeviceFunctions());

    ASSERT_OK(bus_->Disable());
  }

 protected:
  // Initialize an FTDI USB device. Asserts on failure.
  void InitFtdi(fbl::String& devpath) {
    using ConfigurationDescriptor =
        ::fidl::VectorView<fuchsia_hardware_usb_peripheral::wire::FunctionDescriptor>;
    namespace usb_peripheral = fuchsia_hardware_usb_peripheral;

    usb_peripheral::wire::DeviceDescriptor device_desc = {};
    device_desc.bcd_usb = htole16(0x0200);
    device_desc.b_max_packet_size0 = 64;
    device_desc.bcd_device = htole16(0x0100);
    device_desc.b_num_configurations = 1;

    // Setting FTDI Vendor
    device_desc.id_vendor = htole16(0x403);
    // Setting 232H product
    device_desc.id_product = htole16(0x6014);

    usb_peripheral::wire::FunctionDescriptor ftdi_function_desc = {
        .interface_class = USB_CLASS_VENDOR,
        .interface_subclass = USB_SUBCLASS_VENDOR,
        .interface_protocol = USB_PROTOCOL_TEST_FTDI,
    };

    std::vector<usb_peripheral::wire::FunctionDescriptor> function_descs;
    function_descs.push_back(ftdi_function_desc);
    std::vector<ConfigurationDescriptor> config_descs;
    config_descs.emplace_back(
        fidl::VectorView<usb_peripheral::wire::FunctionDescriptor>::FromExternal(function_descs));
    ASSERT_OK(bus_->SetupPeripheralDevice(std::move(device_desc), std::move(config_descs)));

    fbl::unique_fd fd;
    ASSERT_OK(fdio_open3_fd_at(bus_->GetRootFd(), "class/serial",
                               static_cast<uint64_t>(fuchsia_io::wire::kPermReadable),
                               fd.reset_and_get_address()));
    ASSERT_STATUS(fdio_watch_directory(fd.get(), WaitForAnyFile, ZX_TIME_INFINITE, &devpath),
                  ZX_ERR_STOP);
    devpath = fbl::String::Concat({fbl::String("class/serial/"), devpath});
  }

  std::optional<BusLauncher> bus_;
  fbl::String devpath_;
};

// TODO(https://fxbug.dev/434354165): Disabled due to flakes.
TEST_F(FtdiTest, DISABLED_ReadAndWriteTest) {
  zx::result result = component::ConnectAt<fuchsia_hardware_serial::DeviceProxy>(
      fdio_cpp::UnownedFdioCaller(bus_->GetRootFd()).directory(), devpath_.c_str());
  ASSERT_OK(result.status_value());
  auto [client_end, server] = fidl::Endpoints<fuchsia_hardware_serial::Device>::Create();
  ASSERT_OK(fidl::WireCall(result.value())->GetChannel(std::move(server)));

  auto assert_read_with_timeout = [&client_end = client_end](cpp20::span<uint8_t> write_data) {
    for (zx::time deadline = zx::deadline_after(zx::sec(5));
         zx::clock::get_monotonic() < deadline;) {
      const fidl::WireResult result = fidl::WireCall(client_end)->Read();
      ASSERT_OK(result.status());
      const fit::result response = result.value();
      ASSERT_TRUE(response.is_ok(), "%s", zx_status_get_string(response.error_value()));
      cpp20::span data = response.value()->data.get();
      if (data.empty()) {
        continue;
      }
      ASSERT_EQ(data.size_bytes(), write_data.size_bytes());
      ASSERT_BYTES_EQ(data.data(), write_data.data(), write_data.size_bytes());
      return;
    }
    FAIL("timed out");
  };

  {
    uint8_t write_data[] = {1, 2, 3};
    const fidl::WireResult result =
        fidl::WireCall(client_end)->Write(fidl::VectorView<uint8_t>::FromExternal(write_data));
    ASSERT_OK(result.status());
    const fit::result response = result.value();
    ASSERT_TRUE(response.is_ok(), "%s", zx_status_get_string(response.error_value()));
    ASSERT_NO_FATAL_FAILURE(assert_read_with_timeout(cpp20::span(write_data)));
  }

  {
    uint8_t write_data[] = {5, 4, 3, 2, 1};
    const fidl::WireResult result =
        fidl::WireCall(client_end)->Write(fidl::VectorView<uint8_t>::FromExternal(write_data));
    ASSERT_OK(result.status());
    const fit::result response = result.value();
    ASSERT_TRUE(response.is_ok(), "%s", zx_status_get_string(response.error_value()));
    ASSERT_NO_FATAL_FAILURE(assert_read_with_timeout(cpp20::span(write_data)));
  }
}

}  // namespace
}  // namespace usb_virtual_bus
