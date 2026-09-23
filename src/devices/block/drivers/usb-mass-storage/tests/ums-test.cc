// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <dirent.h>
#include <endian.h>
#include <errno.h>
#include <fidl/fuchsia.hardware.block.volume/cpp/wire.h>
#include <fidl/fuchsia.hardware.usb.peripheral/cpp/wire.h>
#include <fidl/fuchsia.hardware.usb.virtual.bus/cpp/wire.h>
#include <fidl/fuchsia.storage.block/cpp/wire.h>
#include <lib/component/incoming/cpp/clone.h>
#include <lib/component/incoming/cpp/directory.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/component/incoming/cpp/service.h>
#include <lib/ddk/platform-defs.h>
#include <lib/device-watcher/cpp/device-watcher.h>
#include <lib/fdio/cpp/caller.h>
#include <lib/fdio/directory.h>
#include <lib/fdio/spawn.h>
#include <lib/fdio/watcher.h>
#include <lib/fit/defer.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/usb-virtual-bus-launcher/usb-virtual-bus-launcher.h>
#include <sys/stat.h>
#include <unistd.h>
#include <zircon/processargs.h>
#include <zircon/syscalls.h>

#include <memory>
#include <string_view>
#include <vector>

#include <fbl/string.h>
#include <usb/usb.h>
#include <zxtest/zxtest.h>

#include "src/storage/lib/block_client/cpp/remote_block_device.h"

namespace usb_virtual_bus {
namespace {

using usb_virtual::BusLauncher;

zx_status_t BRead(fidl::UnownedClientEnd<fuchsia_storage_block::Block> device, void* buffer,
                  size_t buffer_size, size_t offset) {
  return block_client::SingleReadBytes(device, buffer, buffer_size, offset);
}

zx_status_t BWrite(fidl::UnownedClientEnd<fuchsia_storage_block::Block> device, void* buffer,
                   size_t buffer_size, size_t offset) {
  return block_client::SingleWriteBytes(device, buffer, buffer_size, offset);
}

namespace usb_peripheral = fuchsia_hardware_usb_peripheral;

constexpr const char kManufacturer[] = "Google";
constexpr const char kProduct[] = "USB test drive";
constexpr const char kSerial[] = "ebfd5ad49d2a";

template <typename T>
void ValidateResult(const T& result) {
  ASSERT_OK(result.status());
  ASSERT_OK(result.value().status);
}

usb_peripheral::wire::DeviceDescriptor GetDeviceDescriptor() {
  usb_peripheral::wire::DeviceDescriptor device_desc = {};
  device_desc.bcd_usb = htole16(0x0200);
  device_desc.b_device_class = 0;
  device_desc.b_device_sub_class = 0;
  device_desc.b_device_protocol = 0;
  device_desc.b_max_packet_size0 = 64;
  // idVendor and idProduct are filled in later
  device_desc.bcd_device = htole16(0x0100);
  // iManufacturer; iProduct and iSerialNumber are filled in later
  device_desc.b_num_configurations = 1;

  device_desc.manufacturer = fidl::StringView(kManufacturer);
  device_desc.product = fidl::StringView(kProduct);
  device_desc.serial = fidl::StringView(kSerial);

  device_desc.id_vendor = htole16(0x18D1);
  device_desc.id_product = htole16(0xA021);
  return device_desc;
}

class UmsTest : public zxtest::Test {
 protected:
  void SetUp() override {
    auto bus = BusLauncher::Create({
        fuchsia_component_test::Capability::WithService(fuchsia_component_test::Service{
            {.name = fuchsia_hardware_block_volume::Service::Name}}),
    });
    ASSERT_OK(bus.status_value());
    bus_ = std::move(bus.value());
    ASSERT_NO_FATAL_FAILURE(Connect());
  }

  void TearDown() override {
    ASSERT_OK(bus_->ClearPeripheralDeviceFunctions());

    ASSERT_OK(bus_->Disable());
  }

  void Disconnect() {
    ASSERT_OK(bus_->ClearPeripheralDeviceFunctions());
    ASSERT_OK(bus_->Disconnect());
  }

  void Connect() {
    using ConfigurationDescriptor =
        ::fidl::VectorView<fuchsia_hardware_usb_peripheral::wire::FunctionDescriptor>;
    usb_peripheral::wire::FunctionDescriptor ums_function_desc = {
        .interface_class = USB_CLASS_MSC,
        .interface_subclass = USB_SUBCLASS_MSC_SCSI,
        .interface_protocol = USB_PROTOCOL_MSC_BULK_ONLY,
    };

    std::vector<usb_peripheral::wire::FunctionDescriptor> function_descs;
    function_descs.push_back(ums_function_desc);
    std::vector<ConfigurationDescriptor> config_descs;
    config_descs.emplace_back(
        fidl::VectorView<usb_peripheral::wire::FunctionDescriptor>::FromExternal(function_descs));

    ASSERT_OK(bus_->SetupPeripheralDevice(GetDeviceDescriptor(), std::move(config_descs)));
  }

  fbl::String GetTestdevPath() {
    zx::result directory = component::OpenDirectoryAt(
        bus_->GetExposedDir(), fuchsia_hardware_block_volume::Service::Name,
        fuchsia_io::wire::Flags::kProtocolDirectory | fuchsia_io::wire::kPermReadable);
    if (directory.is_error()) {
      return fbl::String("");
    }

    zx::result watch_result = device_watcher::WatchDirectoryForItems<fbl::String>(
        directory.value(), [this](std::string_view name) -> std::optional<fbl::String> {
          if (name == "." || name == "..") {
            return std::nullopt;
          }
          last_known_devpath_ = fbl::String(name.data(), name.size());
          return last_known_devpath_;
        });
    if (watch_result.is_ok()) {
      return last_known_devpath_;
    }
    return fbl::String("");
  }

  // Waits for the block device to be removed.
  void WaitForRemove() {
    if (last_known_devpath_.length() == 0) {
      return;
    }

    zx::result directory = component::OpenDirectoryAt(
        bus_->GetExposedDir(), fuchsia_hardware_block_volume::Service::Name,
        fuchsia_io::wire::Flags::kProtocolDirectory | fuchsia_io::wire::kPermReadable);
    if (directory.is_error()) {
      last_known_devpath_ = fbl::String("");
      return;
    }

    int dir_fd = -1;
    zx_status_t status = fdio_fd_create(directory.value().TakeChannel().release(), &dir_fd);
    if (status != ZX_OK) {
      last_known_devpath_ = fbl::String("");
      return;
    }
    auto close_fd = fit::defer([dir_fd]() { close(dir_fd); });

    std::unique_ptr<device_watcher::DirWatcher> watcher;
    status = device_watcher::DirWatcher::Create(dir_fd, &watcher);
    ASSERT_OK(status);

    const char* c_path = last_known_devpath_.c_str();
    if (c_path == nullptr || c_path[0] == '\0') {
      return;
    }
    std::string_view name_view(c_path);

    for (int i = 0; i < 2000; i++) {
      // 1. Check if the instance is already gone using fstatat.
      struct stat st;
      if (fstatat(dir_fd, c_path, &st, 0) < 0) {
        if (errno == ENOENT) {
          last_known_devpath_ = fbl::String("");
          return;
        }
      }

      // 2. Wait up to 50ms for a removal event.
      status = watcher->WaitForRemoval(name_view, zx::msec(50));
      if (status == ZX_OK) {
        last_known_devpath_ = fbl::String("");
        return;
      }

      if (status != ZX_ERR_TIMED_OUT) {
        ASSERT_OK(status);
      }
    }
    FAIL("Timed out waiting for device removal after 100 seconds");
  }

  std::optional<BusLauncher> bus_;
  fbl::String last_known_devpath_;
};

TEST_F(UmsTest, ReconnectTest) {
  GetTestdevPath();
  for (size_t i = 0; i < 5; i++) {
    ASSERT_NO_FATAL_FAILURE(Disconnect());
    WaitForRemove();
    ASSERT_NO_FATAL_FAILURE(Connect());
    GetTestdevPath();
  }
}

TEST_F(UmsTest, WriteShouldBePersistedToBlockDevice) {
  uint32_t blk_size;
  std::unique_ptr<uint8_t[]> write_buffer;
  {
    fbl::String dev_instance = GetTestdevPath();
    ASSERT_FALSE(dev_instance.empty());
    zx::result client_end =
        component::ConnectAtMember<fuchsia_hardware_block_volume::Service::Volume>(
            bus_->GetExposedDir(), dev_instance.c_str());
    ASSERT_OK(client_end);
    {
      const fidl::WireResult result = fidl::WireCall(client_end.value())->GetInfo();
      ASSERT_OK(result.status());
      const fit::result response = result.value();
      ASSERT_TRUE(response.is_ok(), "%s", zx_status_get_string(response.error_value()));
      blk_size = response.value()->info.block_size;
    }

    // Allocate our buffer
    write_buffer.reset(new uint8_t[blk_size]);
    // Generate and write a pattern to the block device
    for (size_t i = 0; i < blk_size; i++) {
      write_buffer.get()[i] = static_cast<unsigned char>(i);
    }
    ASSERT_EQ(ZX_OK, BWrite(client_end.value(), write_buffer.get(), blk_size, 0));
    memset(write_buffer.get(), 0, blk_size);
  }
  // Disconnect and re-connect the block device
  ASSERT_NO_FATAL_FAILURE(Disconnect());
  ASSERT_NO_FATAL_FAILURE(Connect());
  {
    fbl::String dev_instance = GetTestdevPath();
    ASSERT_FALSE(dev_instance.empty());
    zx::result client_end =
        component::ConnectAtMember<fuchsia_hardware_block_volume::Service::Volume>(
            bus_->GetExposedDir(), dev_instance.c_str());
    ASSERT_OK(client_end);
    // Read back the pattern, which should match what was written
    // since writeback caching was disabled.
    ASSERT_EQ(ZX_OK, BRead(client_end.value(), write_buffer.get(), blk_size, 0));
    for (size_t i = 0; i < blk_size; i++) {
      ASSERT_EQ(write_buffer.get()[i], static_cast<unsigned char>(i));
    }
  }
}

TEST_F(UmsTest, BlkdevTest) {
  char errmsg[1024];
  fdio_spawn_action_t actions[1];
  actions[0] = {};
  actions[0].action = FDIO_SPAWN_ACTION_ADD_NS_ENTRY;
  zx::result exposed_dir = component::Clone(bus_->GetExposedDir());
  ASSERT_OK(exposed_dir);
  actions[0].ns.handle = exposed_dir.value().TakeChannel().release();
  actions[0].ns.prefix = "/svc2";
  fbl::String instance = GetTestdevPath();
  ASSERT_FALSE(instance.empty());
  fbl::String path = fbl::String::Concat({
      fbl::String("/svc2/"),
      fbl::String(fuchsia_hardware_block_volume::Service::Name),
      fbl::String("/"),
      instance,
      fbl::String("/volume"),
  });
  const char* argv[] = {"/pkg/bin/blktest", "-d", path.c_str(), nullptr};
  zx_handle_t process;
  ASSERT_OK(fdio_spawn_etc(zx_job_default(), FDIO_SPAWN_CLONE_ALL, "/pkg/bin/blktest", argv,
                           nullptr, 1, actions, &process, errmsg));
  uint32_t observed;
  zx_object_wait_one(process, ZX_PROCESS_TERMINATED, ZX_TIME_INFINITE, &observed);
  zx_info_process_t proc_info;
  EXPECT_OK(zx_object_get_info(process, ZX_INFO_PROCESS, &proc_info, sizeof(proc_info), nullptr,
                               nullptr));
  EXPECT_EQ(proc_info.return_code, 0);
}

}  // namespace
}  // namespace usb_virtual_bus

int main(int argc, char** argv) { return zxtest::RunAllTests(argc, argv); }
