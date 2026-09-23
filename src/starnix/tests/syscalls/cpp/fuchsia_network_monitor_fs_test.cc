// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <sys/mount.h>

#include <string>

#include <gmock/gmock.h>
#include <gtest/gtest.h>
#include <linux/capability.h>
#include <rapidjson/document.h>

#include "src/lib/files/directory.h"
#include "src/lib/files/file.h"
#include "src/lib/files/path.h"
#include "src/starnix/tests/syscalls/cpp/capabilities_helper.h"
#include "src/starnix/tests/syscalls/cpp/test_helper.h"

class NmfsTest : public ::testing::Test {
 public:
  void SetUp() override {
    // Attempt to create the directory regardless, since its existence
    // does not necessarily mean that the filesystem was
    // successfully mounted.
    files::CreateDirectory("/tmp/fuchsia_network_monitor");

    int mount_result =
        mount(nullptr, "/tmp/fuchsia_network_monitor", "fuchsia_network_monitor_fs", 0, nullptr);
    if (mount_result == -1) {
      GTEST_SKIP() << "Can't mount fuchsia_network_monitor_fs";
    }
    ASSERT_EQ(mount_result, 0);
  }
};

bool IsValidJsonString(const std::string& json_string) {
  if (json_string.empty()) {
    return false;
  }

  rapidjson::Document document;
  document.Parse(json_string.c_str());

  return !document.HasParseError();
}

void ExpectFileWriteSuccess(const std::string& file_location, const std::string& json_string,
                            const std::string& err_msg) {
  EXPECT_TRUE(IsValidJsonString(json_string)) << "improper JSON format";
  EXPECT_TRUE(files::WriteFile(file_location, json_string)) << err_msg;
}

void ExpectFileWriteFailure(const std::string& file_location, const std::string& json_string,
                            const std::string& err_msg) {
  EXPECT_TRUE(IsValidJsonString(json_string)) << "improper JSON format";
  EXPECT_FALSE(files::WriteFile(file_location, json_string)) << err_msg;
}

TEST_F(NmfsTest, NmfsCoreNetworkFileWriteFailure) {
  // Any string that is not the expected JSON format should not result in a write.
  EXPECT_FALSE(files::WriteFile("/tmp/fuchsia_network_monitor/1", "test"))
      << "File contents must be in the proper JSON format";

  EXPECT_FALSE(files::WriteFile("/tmp/fuchsia_network_monitor/1", "1"))
      << "File contents must be in the proper JSON format";

  // The name of the file must be an unsigned integer.
  ExpectFileWriteFailure("/tmp/fuchsia_network_monitor/test",
                         R"({
    "version": "V1",
    "netid": 1,
    "mark": 56,
    "handle": 78,
    "dnsv4": [],
    "dnsv6": []
})",
                         "File name must be parseable as an unsigned integer, was string");
  ExpectFileWriteFailure("/tmp/fuchsia_network_monitor/test",
                         R"({
    "version": "V1",
    "netid": 1,
    "mark": 56,
    "handle": 78,
    "dnsv4": [],
    "dnsv6": []
})",
                         "File name must be parseable as an unsigned integer, was float");

  // The version must be a currently supported enum type.
  ExpectFileWriteFailure("/tmp/fuchsia_network_monitor/1",
                         R"({
    "version": "V9999",
    "netid": 1,
    "mark": 56,
    "handle": 78,
    "dnsv4": [],
    "dnsv6": []
})",
                         "Version must match a currently supported version");

  // The netid must match the integer provided for the file name.
  ExpectFileWriteFailure("/tmp/fuchsia_network_monitor/1",
                         R"({
    "version": "V1",
    "netid": 2,
    "mark": 56,
    "handle": 78,
    "dnsv4": [],
    "dnsv6": []
})",
                         "Network id must match the name of the file");

  // The netid must be a non-negative integer.
  ExpectFileWriteFailure("/tmp/fuchsia_network_monitor/1",
                         R"({
    "version": "V1",
    "netid": 1.0,
    "mark": 56,
    "handle": 78,
    "dnsv4": [],
    "dnsv6": []
})",
                         "Network id must be parseable as an unsigned integer, was float");

  ExpectFileWriteFailure("/tmp/fuchsia_network_monitor/1",
                         R"({
    "version": "V1",
    "netid": -1,
    "mark": 56,
    "handle": 78,
    "dnsv4": [],
    "dnsv6": []
})",
                         "Network id must be parseable as an unsigned integer, was negative");

  // The mark must be a non-negative integer.
  ExpectFileWriteFailure("/tmp/fuchsia_network_monitor/1",
                         R"({
    "version": "V1",
    "netid": 1,
    "mark": 56.0,
    "handle": 78,
    "dnsv4": [],
    "dnsv6": []
})",
                         "Mark must be parseable as an unsigned integer, was float");

  ExpectFileWriteFailure("/tmp/fuchsia_network_monitor/1",
                         R"({
    "version": "V1",
    "netid": 1,
    "mark": -56,
    "handle": 78,
    "dnsv4": [],
    "dnsv6": []
})",
                         "Mark must be parseable as an unsigned integer, was negative");

  // The handle must be a non-negative integer.
  ExpectFileWriteFailure("/tmp/fuchsia_network_monitor/1",
                         R"({
    "version": "V1",
    "netid": 1,
    "mark": 56,
    "handle": 78.0,
    "dnsv4": [],
    "dnsv6": []
})",
                         "Handle must be parseable as an unsigned integer, was float");

  ExpectFileWriteFailure("/tmp/fuchsia_network_monitor/1",
                         R"({
    "version": "V1",
    "netid": 1,
    "mark": 56,
    "handle": -78,
    "dnsv4": [],
    "dnsv6": []
})",
                         "Handle must be parseable as an unsigned integer, was negative");

  // The DNS fields must be arrays with correctly formatted addresses.
  ExpectFileWriteFailure("/tmp/fuchsia_network_monitor/1",
                         R"({
    "version": "V1",
    "netid": 1,
    "mark": 56,
    "handle": 78,
    "dnsv4": ["foo"],
    "dnsv6": []
})",
                         "DNS v4 addresses must be in the proper format");
  ExpectFileWriteFailure("/tmp/fuchsia_network_monitor/1",
                         R"({
    "version": "V1",
    "netid": 1,
    "mark": 56,
    "handle": 78,
    "dnsv4": [],
    "dnsv6": ["foo"]
})",
                         "DNS v6 addresses must be in the proper format");
  ExpectFileWriteFailure("/tmp/fuchsia_network_monitor/1",
                         R"({
    "version": "V1",
    "netid": 1,
    "mark": 56,
    "handle": 78,
    "dnsv4": "192.0.2.0",
    "dnsv6": []
})",
                         "DNS v4 addresses must be provided in an array");
  ExpectFileWriteFailure("/tmp/fuchsia_network_monitor/1",
                         R"({
    "version": "V1",
    "netid": 1,
    "mark": 56,
    "handle": 78,
    "dnsv4": [],
    "dnsv6": "2001:db8::"
})",
                         "DNS v6 addresses must be provided in an array");
  ExpectFileWriteFailure("/tmp/fuchsia_network_monitor/1",
                         R"({
    "version": "V1",
    "netid": 1,
    "mark": 56,
    "handle": 78,
    "dnsv4": [],
    "dnsv6": ["192.0.2.0"]
})",
                         "DNS v4 addresses must be present in the v6 address list");
  ExpectFileWriteFailure("/tmp/fuchsia_network_monitor/1",
                         R"({
    "version": "V1",
    "netid": 1,
    "mark": 56,
    "handle": 78,
    "dnsv4": ["2001:db8::"],
    "dnsv6": []
})",
                         "DNS v6 addresses must be present in the v4 address list");

  // All fields must be provided.
  ExpectFileWriteFailure("/tmp/fuchsia_network_monitor/1",
                         R"({
    "netid": 1,
    "mark": 56,
    "handle": 78,
    "dnsv4": [],
    "dnsv6": []
})",
                         "All fields must be provided in JSON, missing version");
  ExpectFileWriteFailure("/tmp/fuchsia_network_monitor/1",
                         R"({
    "version": "V1",
    "mark": 56,
    "handle": 78,
    "dnsv4": [],
    "dnsv6": []
})",
                         "All fields must be provided in JSON, missing netid");
  ExpectFileWriteFailure("/tmp/fuchsia_network_monitor/1",
                         R"({
    "version": "V1",
    "netid": 1,
    "handle": 78,
    "dnsv4": [],
    "dnsv6": []
})",
                         "All fields must be provided in JSON, missing mark");
  ExpectFileWriteFailure("/tmp/fuchsia_network_monitor/1",
                         R"({
    "version": "V1",
    "netid": 1,
    "mark": 56,
    "dnsv4": [],
    "dnsv6": []
})",
                         "All fields must be provided in JSON, missing handle");
  ExpectFileWriteFailure("/tmp/fuchsia_network_monitor/1",
                         R"({
    "version": "V1",
    "netid": 1,
    "mark": 56,
    "handle": 78,
    "dnsv6": []
})",
                         "All fields must be provided in JSON, missing dnsv4");
  ExpectFileWriteFailure("/tmp/fuchsia_network_monitor/1",
                         R"({
    "version": "V1",
    "netid": 1,
    "mark": 56,
    "handle": 78,
    "dnsv4": []
})",
                         "All fields must be provided in JSON, missing dnsv6");
}

TEST_F(NmfsTest, NmfsV2NetworkFileWriteFailure) {
  ExpectFileWriteFailure("/tmp/fuchsia_network_monitor/1",
                         R"({
    "version": "V2",
    "netid": 1,
    "mark": 56,
    "handle": 78,
    "dnsv4": [],
    "dnsv6": [],
    "transports": 0,
    "capabilities": []
})",
                         "Transport list must be provided in an array");

  ExpectFileWriteFailure("/tmp/fuchsia_network_monitor/1",
                         R"({
    "version": "V2",
    "netid": 1,
    "mark": 56,
    "handle": 78,
    "dnsv4": [],
    "dnsv6": [],
    "transports": [],
    "capabilities": 0
})",
                         "Capabilities list must be provided in an array");

  // All fields must be provided.
  ExpectFileWriteFailure("/tmp/fuchsia_network_monitor/1",
                         R"({
    "version": "V2",
    "netid": 1,
    "mark": 56,
    "handle": 78,
    "dnsv4": [],
    "dnsv6": [],
    "capabilities": [],
    "name": "",
    "addrv4": false,
    "addrv6": false,
    "defaultv4": false,
    "defaultv6": false
})",
                         "All fields must be provided in JSON, missing transports");

  ExpectFileWriteFailure("/tmp/fuchsia_network_monitor/1",
                         R"({
    "version": "V2",
    "netid": 1,
    "mark": 56,
    "handle": 78,
    "dnsv4": [],
    "dnsv6": [],
    "transports": [],
    "name": "",
    "addrv4": false,
    "addrv6": false,
    "defaultv4": false,
    "defaultv6": false
})",
                         "All fields must be provided in JSON, missing capabilities");

  ExpectFileWriteFailure("/tmp/fuchsia_network_monitor/1",
                         R"({
    "version": "V2",
    "netid": 1,
    "mark": 56,
    "handle": 78,
    "dnsv4": [],
    "dnsv6": [],
    "transports": [],
    "capabilities": [],
    "addrv6": false,
    "addrv6": false,
    "defaultv4": false,
    "defaultv6": false
})",
                         "All fields must be provided in JSON, missing name");

  ExpectFileWriteFailure("/tmp/fuchsia_network_monitor/1",
                         R"({
    "version": "V2",
    "netid": 1,
    "mark": 56,
    "handle": 78,
    "dnsv4": [],
    "dnsv6": [],
    "transports": [],
    "capabilities": [],
    "name": "",
    "addrv6": false,
    "defaultv4": false,
    "defaultv6": false
})",
                         "All fields must be provided in JSON, missing addrv4");

  ExpectFileWriteFailure("/tmp/fuchsia_network_monitor/1",
                         R"({
    "version": "V2",
    "netid": 1,
    "mark": 56,
    "handle": 78,
    "dnsv4": [],
    "dnsv6": [],
    "transports": [],
    "capabilities": [],
    "name": "",
    "addrv4": false,
    "defaultv4": false,
    "defaultv6": false
})",
                         "All fields must be provided in JSON, missing addrv6");

  ExpectFileWriteFailure("/tmp/fuchsia_network_monitor/1",
                         R"({
    "version": "V2",
    "netid": 1,
    "mark": 56,
    "handle": 78,
    "dnsv4": [],
    "dnsv6": [],
    "transports": [],
    "capabilities": [],
    "name": "",
    "addrv4": false,
    "addrv6": false,
    "defaultv6": false
})",
                         "All fields must be provided in JSON, missing defaultv4");

  ExpectFileWriteFailure("/tmp/fuchsia_network_monitor/1",
                         R"({
    "version": "V2",
    "netid": 1,
    "mark": 56,
    "handle": 78,
    "dnsv4": [],
    "dnsv6": [],
    "transports": [],
    "capabilities": [],
    "name": "",
    "addrv4": false,
    "addrv6": false,
    "defaultv4": false
})",
                         "All fields must be provided in JSON, missing defaultv6");
}

TEST_F(NmfsTest, NmfsCannotCreateDir) {
  EXPECT_FALSE(files::CreateDirectory("/tmp/fuchsia_network_monitor/2"));
}

TEST_F(NmfsTest, NmfsNetworkFileWriteSuccessNoAddresses) {
  ExpectFileWriteSuccess("/tmp/fuchsia_network_monitor/3",
                         R"({
    "version": "V1",
    "netid": 3,
    "mark": 56,
    "handle": 78,
    "dnsv4": [],
    "dnsv6": []
  })",
                         "All JSON fields must be provided with proper formatting");
  // File output should be able to be rewritten to the file without formatting changes.
  std::string network_info;
  EXPECT_TRUE(files::ReadFileToString("/tmp/fuchsia_network_monitor/3", &network_info))
      << "Network file should be readable";
  EXPECT_TRUE(files::WriteFile("/tmp/fuchsia_network_monitor/3", network_info))
      << "Contents from read should be writeable to the same file";
}

TEST_F(NmfsTest, NmfsNetworkFileWriteSuccessV4Addresses) {
  ExpectFileWriteSuccess("/tmp/fuchsia_network_monitor/4",
                         R"({
    "version": "V1",
    "netid": 4,
    "mark": 56,
    "handle": 78,
    "dnsv4": ["192.0.2.0", "192.0.2.1"],
    "dnsv6": []
  })",
                         "All JSON fields must be provided with proper formatting");
  // File output should be able to be rewritten to the file without formatting changes.
  std::string network_info;
  EXPECT_TRUE(files::ReadFileToString("/tmp/fuchsia_network_monitor/4", &network_info))
      << "Network file should be readable";
  EXPECT_TRUE(files::WriteFile("/tmp/fuchsia_network_monitor/4", network_info))
      << "Contents from read should be writeable to the same file";
}

TEST_F(NmfsTest, NmfsNetworkFileWriteSuccessV6Addresses) {
  ExpectFileWriteSuccess("/tmp/fuchsia_network_monitor/5",
                         R"({
    "version": "V1",
    "netid": 5,
    "mark": 56,
    "handle": 78,
    "dnsv4": [],
    "dnsv6": ["2001:db8::", "2001:db8::1"]
  })",
                         "All JSON fields must be provided with proper formatting");
  // File output should be able to be rewritten to the file without formatting changes.
  std::string network_info;
  EXPECT_TRUE(files::ReadFileToString("/tmp/fuchsia_network_monitor/5", &network_info))
      << "Network file should be readable";
  EXPECT_TRUE(files::WriteFile("/tmp/fuchsia_network_monitor/5", network_info))
      << "Contents from read should be writeable to the same file";
}

TEST_F(NmfsTest, NmfsDefaultNetworkFile) {
  EXPECT_FALSE(files::WriteFile("/tmp/fuchsia_network_monitor/default", "6"))
      << "The associated network file must be populated first prior to writing to the default file";

  std::string empty_default;
  EXPECT_FALSE(files::ReadFileToString("/tmp/fuchsia_network_monitor/default", &empty_default))
      << "The default network id file must not be readable";

  // Create the network and set it as the default.
  ExpectFileWriteSuccess("/tmp/fuchsia_network_monitor/6",
                         R"({
    "version": "V1",
    "netid": 6,
    "mark": 56,
    "handle": 78,
    "dnsv4": [],
    "dnsv6": []
  })",
                         "All JSON fields must be provided with proper formatting");
  EXPECT_FALSE(files::WriteFile("/tmp/fuchsia_network_monitor/default", "9999"))
      << "The default network id file must not accept a network id that does not exist";
  EXPECT_FALSE(files::WriteFile("/tmp/fuchsia_network_monitor/default", "null"))
      << "The default network id file must not accept a null network id";
  EXPECT_TRUE(files::WriteFile("/tmp/fuchsia_network_monitor/default", "6"))
      << "The default network id file must accept the network id of a populated network";

  std::string populated_default;
  EXPECT_TRUE(files::ReadFileToString("/tmp/fuchsia_network_monitor/default", &populated_default))
      << "The default network id file must be readable";
  EXPECT_EQ(populated_default, "6") << "The default network id must be the network id that was set";

  EXPECT_FALSE(files::DeletePath("/tmp/fuchsia_network_monitor/6", /*recursive=*/false))
      << "The default network id must be unset before the network can be removed";
  EXPECT_TRUE(files::DeletePath("/tmp/fuchsia_network_monitor/default", /*recursive=*/false))
      << "The default network id must be removable";
  EXPECT_TRUE(files::DeletePath("/tmp/fuchsia_network_monitor/6", /*recursive=*/false))
      << "The default network id must be removable now that it is not the default";
}

TEST_F(NmfsTest, NmfsV2NetworkFileWriteSuccessTransportList) {
  ExpectFileWriteSuccess("/tmp/fuchsia_network_monitor/7",
                         R"({
    "version": "V2",
    "netid": 7,
    "mark": 56,
    "handle": 78,
    "dnsv4": [],
    "dnsv6": [],
    "transports": [0, 1, 2, 3],
    "capabilities": [],
    "name": "test",
    "addrv4": true,
    "addrv6": false,
    "defaultv4": false,
    "defaultv6": false
  })",
                         "All JSON fields must be provided with proper formatting");
  // File output should be able to be rewritten to the file without formatting changes.
  std::string network_info;
  EXPECT_TRUE(files::ReadFileToString("/tmp/fuchsia_network_monitor/7", &network_info))
      << "Network file should be readable";
  EXPECT_TRUE(files::WriteFile("/tmp/fuchsia_network_monitor/7", network_info))
      << "Contents from read should be writeable to the same file";
}

TEST_F(NmfsTest, NmfsV2NetworkFileWriteSuccessCapabilityList) {
  ExpectFileWriteSuccess("/tmp/fuchsia_network_monitor/8",
                         R"({
    "version": "V2",
    "netid": 8,
    "mark": 56,
    "handle": 78,
    "dnsv4": [],
    "dnsv6": [],
    "transports": [],
    "capabilities": [0, 1, 2, 3],
    "name": "test",
    "addrv4": true,
    "addrv6": false,
    "defaultv4": false,
    "defaultv6": false
  })",
                         "All JSON fields must be provided with proper formatting");
  // File output should be able to be rewritten to the file without formatting changes.
  std::string network_info;
  EXPECT_TRUE(files::ReadFileToString("/tmp/fuchsia_network_monitor/8", &network_info))
      << "Network file should be readable";
  EXPECT_TRUE(files::WriteFile("/tmp/fuchsia_network_monitor/8", network_info))
      << "Contents from read should be writeable to the same file";
}

TEST_F(NmfsTest, NmfsPermissionsAndCapabilities) {
  ASSERT_TRUE(test_helper::HasCapability(CAP_NET_ADMIN)) << "Test requires CAP_NET_ADMIN initially";

  test_helper::ForkHelper fork_helper;

  // Try to create a node as unprivileged (without CAP_NET_ADMIN).
  fork_helper.RunInForkedProcess([&]() {
    test_helper::UnsetCapabilityEffective(CAP_NET_ADMIN);
    // File creation should fail with EPERM because we lack CAP_NET_ADMIN capability.
    EXPECT_FALSE(files::WriteFile("/tmp/fuchsia_network_monitor/9", R"({
      "version": "V1",
      "netid": 9,
      "mark": 56,
      "handle": 78,
      "dnsv4": [],
      "dnsv6": []
    })"));
    EXPECT_EQ(errno, EPERM);
  });
  ASSERT_TRUE(fork_helper.WaitForChildren());

  // Creation should succeed with CAP_NET_ADMIN.
  ExpectFileWriteSuccess("/tmp/fuchsia_network_monitor/9", R"({
    "version": "V1",
    "netid": 9,
    "mark": 56,
    "handle": 78,
    "dnsv4": [],
    "dnsv6": []
  })",
                         "Creation should succeed with CAP_NET_ADMIN");

  // Try to unlink or write to the existing file as unprivileged (without CAP_NET_ADMIN).
  fork_helper.RunInForkedProcess([&]() {
    test_helper::UnsetCapabilityEffective(CAP_NET_ADMIN);
    // Try to unlink the file (should fail with EPERM).
    EXPECT_EQ(unlink("/tmp/fuchsia_network_monitor/9"), -1);
    EXPECT_EQ(errno, EPERM);

    // Try to write to the existing file (should fail with EPERM).
    EXPECT_FALSE(files::WriteFile("/tmp/fuchsia_network_monitor/9", "{}"));
    EXPECT_EQ(errno, EPERM);
  });
  ASSERT_TRUE(fork_helper.WaitForChildren());

  // Test default file unlink and write with/without CAP_NET_ADMIN.
  EXPECT_TRUE(files::WriteFile("/tmp/fuchsia_network_monitor/default", "9"));

  fork_helper.RunInForkedProcess([&]() {
    test_helper::UnsetCapabilityEffective(CAP_NET_ADMIN);
    // Try to set default (should fail with EPERM).
    EXPECT_FALSE(files::WriteFile("/tmp/fuchsia_network_monitor/default", "9"));
    EXPECT_EQ(errno, EPERM);

    // Try to unlink default (should fail with EPERM).
    EXPECT_EQ(unlink("/tmp/fuchsia_network_monitor/default"), -1);
    EXPECT_EQ(errno, EPERM);
  });
  ASSERT_TRUE(fork_helper.WaitForChildren());

  // Clean up.
  EXPECT_EQ(unlink("/tmp/fuchsia_network_monitor/default"), 0) << strerror(errno);
  EXPECT_EQ(unlink("/tmp/fuchsia_network_monitor/9"), 0) << strerror(errno);
}
