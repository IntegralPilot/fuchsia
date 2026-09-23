// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <dirent.h>
#include <fcntl.h>
#include <fidl/fuchsia.driver.test/cpp/wire.h>
#include <fidl/fuchsia.hardware.nand/cpp/wire.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/device-watcher/cpp/device-watcher.h>
#include <lib/fdio/fd.h>
#include <limits.h>
#include <stdio.h>
#include <stdlib.h>

#include <algorithm>
#include <iterator>
#include <set>
#include <utility>

#include <bind/fuchsia/platform/cpp/bind.h>
#include <fbl/string.h>
#include <fbl/unique_fd.h>
#include <gtest/gtest.h>
#include <ramdevice-client/ramnand.h>

#include "src/lib/testing/predicates/status.h"

namespace {

fuchsia_hardware_nand::wire::RamNandInfo BuildConfig() {
  return {
      .nand_info = {.page_size = 4096,
                    .pages_per_block = 4,
                    .num_blocks = 5,
                    .ecc_bits = 6,
                    .oob_size = 0,
                    .nand_class = fuchsia_hardware_nand::wire::Class::kTest,
                    .partition_guid{}},
  };
}

}  // namespace

namespace ram_nand::testing {

class RamNandIntegrationTest : public ::testing::Test {
 public:
  void SetUp() override {
    // Connect to DriverTestRealm.
    zx::result client_end = component::Connect<fuchsia_driver_test::Realm>();
    ASSERT_OK(client_end);
    fidl::WireSyncClient client(std::move(client_end.value()));

    // Start DriverTestRealm.
    fidl::Arena arena;
    const fidl::WireResult result =
        client->Start(fuchsia_driver_test::wire::RealmArgs::Builder(arena)
                          .root_driver("fuchsia-boot:///platform-bus#meta/platform-bus.cm")
                          .software_devices(std::vector{fuchsia_driver_test::wire::SoftwareDevice{
                              .device_name = "ram-nand",
                              .device_id = bind_fuchsia_platform::BIND_PLATFORM_DEV_DID_RAM_NAND,
                          }})
                          .Build());
    ASSERT_OK(result.status());
    ASSERT_TRUE(result->is_ok());

    // Wait for the ram-nand driver to be bound.
    zx::result channel = device_watcher::RecursiveWaitForFile(ramdevice_client::RamNand::kBasePath);
    ASSERT_OK(channel);
  }
};

class NandDevice {
 public:
  static zx::result<NandDevice> Create(
      fuchsia_hardware_nand::wire::RamNandInfo config = BuildConfig()) {
    zx::result ram_nand = ramdevice_client::RamNand::Create(std::move(config));
    if (ram_nand.is_error()) {
      return ram_nand.take_error();
    }
    return zx::ok(NandDevice(std::move(ram_nand.value())));
  }

  NandDevice(const NandDevice&) = delete;
  NandDevice& operator=(const NandDevice&) = delete;

  NandDevice(NandDevice&&) = default;
  NandDevice& operator=(NandDevice&&) = default;

  ~NandDevice() = default;

  const char* path() { return ram_nand_.path(); }
  const char* filename() { return ram_nand_.filename(); }

 private:
  explicit NandDevice(ramdevice_client::RamNand ram_nand) : ram_nand_(std::move(ram_nand)) {}

  ramdevice_client::RamNand ram_nand_;
};

TEST_F(RamNandIntegrationTest, TrivialLifetime) {
  std::unique_ptr<device_watcher::DirWatcher> watcher;
  fbl::unique_fd dir_fd(open(ramdevice_client::RamNand::kBasePath, O_RDONLY | O_DIRECTORY));
  ASSERT_TRUE(dir_fd);
  ASSERT_EQ(device_watcher::DirWatcher::Create(dir_fd.get(), &watcher), ZX_OK);

  fbl::String path;
  fbl::String filename;
  {
    zx::result result = NandDevice::Create();
    ASSERT_OK(result.status_value());
    NandDevice& device = result.value();
    path = fbl::String(device.path());
    filename = fbl::String(device.filename());
  }
  ASSERT_EQ(watcher->WaitForRemoval(filename, zx::sec(5)), ZX_OK);

  fbl::unique_fd found(open(path.c_str(), O_RDWR));
  ASSERT_FALSE(found);
}

TEST_F(RamNandIntegrationTest, ExportConfig) {
  fuchsia_hardware_nand::wire::RamNandInfo config = BuildConfig();
  config.export_nand_config = true;

  zx::result device = NandDevice::Create(std::move(config));
  ASSERT_OK(device.status_value());
}

TEST_F(RamNandIntegrationTest, ExportPartitions) {
  fuchsia_hardware_nand::wire::RamNandInfo config = BuildConfig();
  config.export_partition_map = true;

  zx::result device = NandDevice::Create(std::move(config));
  ASSERT_OK(device.status_value());
}

TEST_F(RamNandIntegrationTest, CreateFailure) {
  fuchsia_hardware_nand::wire::RamNandInfo config = BuildConfig();
  config.nand_info.num_blocks = 0;

  zx::result device = NandDevice::Create(std::move(config));
  ASSERT_STATUS(device.status_value(), ZX_ERR_INVALID_ARGS);
}

namespace {
std::set<std::string> GetNandDevices() {
  std::set<std::string> devices;
  DIR* dir = opendir("/dev/class/nand");
  if (!dir) {
    return devices;
  }
  struct dirent* de;
  while ((de = readdir(dir)) != nullptr) {
    if (std::string_view(de->d_name) == "." || std::string_view(de->d_name) == "..") {
      continue;
    }
    devices.insert(de->d_name);
  }
  closedir(dir);
  return devices;
}
}  // namespace

TEST_F(RamNandIntegrationTest, ServiceInstancesRemoved) {
  std::set<std::string> before = GetNandDevices();

  std::string class_filename;
  {
    zx::result result = NandDevice::Create();
    ASSERT_OK(result.status_value());

    // Wait for the new device to appear in /dev/class/nand.
    bool found = false;
    for (int i = 0; i < 50; ++i) {
      std::set<std::string> after = GetNandDevices();
      std::vector<std::string> diff;
      std::set_difference(after.begin(), after.end(), before.begin(), before.end(),
                          std::back_inserter(diff));
      if (!diff.empty()) {
        class_filename = diff[0];
        found = true;
        break;
      }
      zx::nanosleep(zx::deadline_after(zx::msec(100)));
    }
    ASSERT_TRUE(found) << "Timeout waiting for class node to appear";
  }

  // Synchronously check that the class node is gone.
  std::string path = std::string("/dev/class/nand/") + class_filename;
  fbl::unique_fd fd(open(path.c_str(), O_RDONLY));
  EXPECT_FALSE(fd) << "Class node still exists after destruction: " << path;
  EXPECT_EQ(errno, ENOENT);
}

// Force rebuild comment.
}  // namespace ram_nand::testing
