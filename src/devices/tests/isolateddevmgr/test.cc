// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.board.test/cpp/wire.h>
#include <fidl/fuchsia.device.manager.test/cpp/wire.h>
#include <lib/ddk/metadata.h>
#include <lib/ddk/platform-defs.h>
#include <lib/driver-integration-test/fixture.h>
#include <lib/fdio/fd.h>
#include <lib/fdio/fdio.h>
#include <stdio.h>
#include <stdlib.h>
#include <zircon/syscalls.h>

#include <sstream>
#include <string>
#include <vector>

#include <zxtest/zxtest.h>

using driver_integration_test::IsolatedDevmgr;

namespace {

class IsolatedDevMgrTest : public zxtest::Test {
 protected:
  static constexpr char kPlatformDeviceName1[board_test::kNameLengthMax] = "metadata-test-1";
  static const std::vector<uint8_t> kMetadata1;
  static const board_test::DeviceEntry kDeviceEntry1;

  static constexpr char kPlatformDeviceName2[board_test::kNameLengthMax] = "metadata-test-2";
  static const std::vector<uint8_t> kMetadata2;
  static const board_test::DeviceEntry kDeviceEntry2;

  static board_test::DeviceEntry CreateEntry(const char* name, uint32_t vid, uint32_t pid,
                                             uint32_t did, cpp20::span<const uint8_t> metadata) {
    board_test::DeviceEntry entry = {};
    strlcpy(entry.name, name, sizeof(entry.name));
    entry.vid = vid;
    entry.pid = pid;
    entry.did = did;
    entry.metadata_size = metadata.size();
    entry.metadata = metadata.data();
    return entry;
  }

  static std::string GetDevice1Path() {
    std::ostringstream path;
    path << "sys/platform/" << kPlatformDeviceName1 << "/metadata-test";
    return path.str();
  }

  static std::string GetDevice2Path() {
    std::ostringstream path;
    path << "sys/platform/" << kPlatformDeviceName2 << "/metadata-test";
    return path.str();
  }

  static void CheckMetadata(fidl::WireSyncClient<fuchsia_device_manager_test::Metadata>& client,
                            const std::vector<uint8_t>& expected_metadata) {
    fidl::WireResult result = client->GetMetadata(
        fidl::StringView::FromExternal(fuchsia_board_test::wire::kPdevMetadataId));
    ASSERT_OK(result.status());
    fidl::VectorView<uint8_t> received_metadata = std::move(result->data);
    ASSERT_EQ(received_metadata.size(), expected_metadata.size());

    EXPECT_BYTES_EQ(received_metadata.data(), expected_metadata.data(), expected_metadata.size());
  }

  IsolatedDevmgr& devmgr() { return devmgr_; }

 private:
  IsolatedDevmgr devmgr_;
};

const std::vector<uint8_t> IsolatedDevMgrTest::kMetadata1 = {1, 2, 3, 4, 5};
const board_test::DeviceEntry IsolatedDevMgrTest::kDeviceEntry1 = IsolatedDevMgrTest::CreateEntry(
    IsolatedDevMgrTest::kPlatformDeviceName1, PDEV_VID_TEST, PDEV_PID_METADATA_TEST,
    PDEV_DID_TEST_CHILD_1, IsolatedDevMgrTest::kMetadata1);

const std::vector<uint8_t> IsolatedDevMgrTest::kMetadata2 = {7, 6, 5, 4, 3, 2, 1};
const board_test::DeviceEntry IsolatedDevMgrTest::kDeviceEntry2 = IsolatedDevMgrTest::CreateEntry(
    IsolatedDevMgrTest::kPlatformDeviceName2, PDEV_VID_TEST, PDEV_PID_METADATA_TEST,
    PDEV_DID_TEST_CHILD_2, IsolatedDevMgrTest::kMetadata2);

TEST_F(IsolatedDevMgrTest, MetadataOneDriverTest) {
  // Set the driver arguments.
  IsolatedDevmgr::Args args;
  args.device_list.push_back(kDeviceEntry1);

  // Create the isolated Devmgr.
  zx_status_t status = IsolatedDevmgr::Create(&args, &devmgr());
  ASSERT_OK(status);

  // Wait for Metadata-test driver to be created
  zx::result channel =
      device_watcher::RecursiveWaitForFile(devmgr().devfs_root().get(), GetDevice1Path().c_str());
  ASSERT_OK(channel.status_value());

  fidl::WireSyncClient client{
      fidl::ClientEnd<fuchsia_device_manager_test::Metadata>(std::move(channel.value()))};
  ASSERT_NO_FATAL_FAILURE(CheckMetadata(client, kMetadata1));
}

TEST_F(IsolatedDevMgrTest, MetadataTwoDriverTest) {
  // Set the driver arguments.
  IsolatedDevmgr::Args args;
  args.device_list.push_back(kDeviceEntry1);
  args.device_list.push_back(kDeviceEntry2);

  // Create the isolated Devmgr.
  zx_status_t status = IsolatedDevmgr::Create(&args, &devmgr());
  ASSERT_OK(status);

  struct MetadataTest {
    std::string path;
    std::vector<uint8_t> metadata;
  };

  std::vector<MetadataTest> tests{
      {
          .path = GetDevice1Path(),
          .metadata = kMetadata1,
      },
      {
          .path = GetDevice2Path(),
          .metadata = kMetadata2,
      },
  };

  for (MetadataTest& test : tests) {
    SCOPED_TRACE(test.path);
    zx::result channel =
        device_watcher::RecursiveWaitForFile(devmgr().devfs_root().get(), test.path.c_str());
    ASSERT_OK(channel.status_value());
    fidl::WireSyncClient client{
        fidl::ClientEnd<fuchsia_device_manager_test::Metadata>(std::move(channel.value()))};
    ASSERT_NO_FATAL_FAILURE(CheckMetadata(client, test.metadata));
  }
}

}  // namespace
