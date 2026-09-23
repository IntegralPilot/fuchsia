// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async-loop/cpp/loop.h>
#include <lib/driver/testing/cpp/driver_runtime.h>
#include <lib/fit/defer.h>
#include <lib/sync/cpp/completion.h>

#include <gtest/gtest.h>
#include <sdk/lib/syslog/cpp/log_settings.h>

#include "src/lib/testing/predicates/status.h"
#include "test_util.h"

namespace network {
namespace testing {

class MacDeviceTest : public ::testing::Test {
 public:
  void SetUp() override {
    // enable full tracing for tests, easier to debug problems.
    fuchsia_logging::LogSettingsBuilder()
        .WithMinLogSeverity(fuchsia_logging::LogSeverity::Trace)
        .BuildAndInitialize();
    CreateDispatcher();
  }

  void TearDown() override {
    if (device_) {
      sync_completion_t completion;
      device_->Teardown([&completion]() { sync_completion_signal(&completion); });
      sync_completion_wait(&completion, ZX_TIME_INFINITE);
    }
    dispatcher_.ShutdownAsync();
    dispatcher_shutdown_.Wait();
  }

  zx_status_t CreateDevice() {
    if (device_) {
      return ZX_ERR_INTERNAL;
    }
    zx::result device = impl_.CreateChild(dispatcher_.get());
    if (device.is_ok()) {
      device_ = std::move(device.value());
    }
    return device.status_value();
  }

  fidl::WireSyncClient<netdev::MacAddressing> OpenInstance() { return OpenInstance(device_.get()); }

  fidl::WireSyncClient<netdev::MacAddressing> OpenInstance(MacAddrDeviceInterface* device) {
    auto [client_end, server_end] = fidl::Endpoints<netdev::MacAddressing>::Create();
    if (!loop_) {
      loop_ = std::make_unique<async::Loop>(&kAsyncLoopConfigNeverAttachToThread);
      EXPECT_OK(loop_->StartThread("test-thread"));
    }
    EXPECT_OK(device->Bind(loop_->dispatcher(), std::move(server_end)));
    return fidl::WireSyncClient(std::move(client_end));
  }

  void CreateDispatcher() {
    zx::result dispatcher = fdf::UnsynchronizedDispatcher::Create(
        {}, "", [this](fdf_dispatcher_t*) { dispatcher_shutdown_.Signal(); });
    ASSERT_OK(dispatcher.status_value());
    dispatcher_ = std::move(*dispatcher);
  }

 protected:
  fdf_testing::DriverRuntime driver_runtime_;
  FakeMacDeviceImpl impl_;
  // Loop whose dispatcher is used to create client instances.
  // The loop is created lazily in `OpenInstance` to avoid spawning threads on tests that do not
  // instantiate clients.
  std::unique_ptr<async::Loop> loop_;
  std::shared_ptr<MacAddrDeviceInterface> device_;
  fdf::Dispatcher dispatcher_;
  libsync::Completion dispatcher_shutdown_;
};

TEST_F(MacDeviceTest, GetAddress) {
  ASSERT_OK(CreateDevice());
  fidl::WireSyncClient<netdev::MacAddressing> client = OpenInstance();
  fidl::WireResult result = client->GetUnicastAddress();
  ASSERT_OK(result.status());
  ASSERT_EQ(result.value().address.octets, impl_.mac().octets);
}

TEST_F(MacDeviceTest, UnrecognizedMode) {
  // set some arbitrary not supported mode:
  impl_.features().supported_modes(netdriver::wire::SupportedMacFilterMode(0));
  ASSERT_STATUS(CreateDevice(), ZX_ERR_NOT_SUPPORTED);
}

TEST_F(MacDeviceTest, EmptyMode) {
  // set an empty set for supported modes
  impl_.features().supported_modes(netdriver::wire::SupportedMacFilterMode(0));
  ASSERT_STATUS(CreateDevice(), ZX_ERR_NOT_SUPPORTED);
}

TEST_F(MacDeviceTest, StartupModeFilter) {
  ASSERT_OK(CreateDevice());
  ASSERT_EQ(impl_.mode(), netdev::wire::MacFilterMode::kMulticastFilter);
}

TEST_F(MacDeviceTest, StartupModeMcastPromiscuous) {
  impl_.features().supported_modes(netdriver::wire::SupportedMacFilterMode::kMulticastPromiscuous |
                                   netdriver::wire::SupportedMacFilterMode::kPromiscuous);
  ASSERT_OK(CreateDevice());
  ASSERT_EQ(impl_.mode(), netdev::wire::MacFilterMode::kMulticastPromiscuous);
}

TEST_F(MacDeviceTest, StartupModePromiscuous) {
  impl_.features().supported_modes(netdriver::wire::SupportedMacFilterMode::kPromiscuous);
  ASSERT_OK(CreateDevice());
  ASSERT_EQ(impl_.mode(), netdev::wire::MacFilterMode::kPromiscuous);
}

TEST_F(MacDeviceTest, SetBadMode) {
  impl_.features().supported_modes(netdriver::wire::SupportedMacFilterMode::kMulticastFilter);
  ASSERT_OK(CreateDevice());
  fidl::WireSyncClient<netdev::MacAddressing> client = OpenInstance();

  fidl::WireResult result = client->SetMode(netdev::wire::MacFilterMode::kPromiscuous);
  ASSERT_OK(result.status());
  ASSERT_STATUS(result.value().status, ZX_ERR_NOT_SUPPORTED);
}

TEST_F(MacDeviceTest, SetPromiscuous) {
  ASSERT_OK(CreateDevice());
  fidl::WireSyncClient<netdev::MacAddressing> client = OpenInstance();

  fidl::WireResult result = client->SetMode(netdev::wire::MacFilterMode::kPromiscuous);
  ASSERT_OK(result.status());
  ASSERT_OK(result.value().status);
  ASSERT_OK(impl_.WaitConfigurationChanged());
  ASSERT_EQ(impl_.mode(), netdev::MacFilterMode::kPromiscuous);
  ASSERT_TRUE(impl_.addresses().empty());
}

TEST_F(MacDeviceTest, SetMulticastPromiscuous) {
  ASSERT_OK(CreateDevice());
  fidl::WireSyncClient<netdev::MacAddressing> client = OpenInstance();

  fidl::WireResult result = client->SetMode(netdev::wire::MacFilterMode::kMulticastPromiscuous);
  ASSERT_OK(result.status());
  ASSERT_OK(result.value().status);
  ASSERT_OK(impl_.WaitConfigurationChanged());
  ASSERT_EQ(impl_.mode(), netdev::wire::MacFilterMode::kMulticastPromiscuous);
  ASSERT_TRUE(impl_.addresses().empty());
}

TEST_F(MacDeviceTest, InvalidMulticastAddress) {
  ASSERT_OK(CreateDevice());
  fidl::WireSyncClient<netdev::MacAddressing> client = OpenInstance();

  MacAddress addr{{0x00, 0x01, 0x02, 0x03, 0x04, 0x05}};
  fidl::WireResult add = client->AddMulticastAddress(addr);
  ASSERT_OK(add.status());
  ASSERT_STATUS(add.value().status, ZX_ERR_INVALID_ARGS);

  // same thing should happen for RemoveMulticastAddress:
  fidl::WireResult remove = client->RemoveMulticastAddress(addr);
  ASSERT_OK(remove.status());
  ASSERT_STATUS(remove.value().status, ZX_ERR_INVALID_ARGS);
}

TEST_F(MacDeviceTest, AddRemoveMulticastFilter) {
  ASSERT_OK(CreateDevice());
  ASSERT_EQ(impl_.mode(), netdev::wire::MacFilterMode::kMulticastFilter);

  fidl::WireSyncClient<netdev::MacAddressing> client = OpenInstance();

  MacAddress addr{{0x01, 0x01, 0x02, 0x03, 0x04, 0x05}};
  fidl::WireResult add = client->AddMulticastAddress(addr);
  ASSERT_OK(add.status());
  ASSERT_OK(add.value().status);
  ASSERT_OK(impl_.WaitConfigurationChanged());
  ASSERT_EQ(impl_.mode(), netdev::wire::MacFilterMode::kMulticastFilter);
  ASSERT_EQ(1u, impl_.addresses().size());
  ASSERT_EQ(impl_.addresses().begin()->octets, addr.octets);

  fidl::WireResult remove = client->RemoveMulticastAddress(addr);
  ASSERT_OK(remove.status());
  ASSERT_OK(impl_.WaitConfigurationChanged());
  ASSERT_EQ(impl_.mode(), netdev::wire::MacFilterMode::kMulticastFilter);
  ASSERT_TRUE(impl_.addresses().empty());
}

TEST_F(MacDeviceTest, OverflowsIntoMulticastPromiscuous) {
  ASSERT_OK(CreateDevice());
  ASSERT_EQ(impl_.mode(), netdev::wire::MacFilterMode::kMulticastFilter);
  fidl::WireSyncClient<netdev::MacAddressing> client = OpenInstance();

  for (size_t i = 0; i < impl_.features().multicast_filter_count() + 1; i++) {
    MacAddress addr{{0x01, 0x00, 0x00, 0x00, 0x00, static_cast<unsigned char>(i)}};
    fidl::WireResult result = client->AddMulticastAddress(addr);
    ASSERT_OK(result.status());
    ASSERT_OK(result.value().status);
    ASSERT_OK(impl_.WaitConfigurationChanged());
  }
  ASSERT_EQ(impl_.mode(), netdev::wire::MacFilterMode::kMulticastPromiscuous);
  ASSERT_TRUE(impl_.addresses().empty());
}

TEST_F(MacDeviceTest, MostPermissiveClientWins) {
  ASSERT_OK(CreateDevice());
  ASSERT_EQ(impl_.mode(), netdev::wire::MacFilterMode::kMulticastFilter);

  fidl::WireSyncClient<netdev::MacAddressing> cli1 = OpenInstance();

  fidl::WireSyncClient<netdev::MacAddressing> cli2 = OpenInstance();

  MacAddress addr{{0x01, 0x00, 0x00, 0x00, 0x00, 0x02}};
  {
    fidl::WireResult result = cli1->AddMulticastAddress(addr);
    ASSERT_OK(result.status());
    ASSERT_OK(result.value().status);
    ASSERT_OK(impl_.WaitConfigurationChanged());
    ASSERT_EQ(impl_.mode(), netdev::wire::MacFilterMode::kMulticastFilter);
    ASSERT_EQ(impl_.addresses().size(), 1ul);
  }
  {
    fidl::WireResult result = cli2->SetMode(netdev::wire::MacFilterMode::kPromiscuous);
    ASSERT_OK(result.status());
    ASSERT_OK(result.value().status);
    ASSERT_OK(impl_.WaitConfigurationChanged());
    ASSERT_EQ(impl_.mode(), netdev::wire::MacFilterMode::kPromiscuous);
    ASSERT_TRUE(impl_.addresses().empty());
  }
  // Remove second instance and check that the mode fell back to the first one.
  cli2 = {};
  ASSERT_OK(impl_.WaitConfigurationChanged());
  ASSERT_EQ(impl_.mode(), netdev::wire::MacFilterMode::kMulticastFilter);
  ASSERT_EQ(impl_.addresses().size(), 1ul);
}

TEST_F(MacDeviceTest, FallsBackToDefaultMode) {
  ASSERT_OK(CreateDevice());
  ASSERT_EQ(impl_.mode(), netdev::wire::MacFilterMode::kMulticastFilter);

  fidl::WireSyncClient<netdev::MacAddressing> client = OpenInstance();

  fidl::WireResult result = client->SetMode(netdev::wire::MacFilterMode::kPromiscuous);
  ASSERT_OK(result.status());
  ASSERT_OK(result.value().status);
  ASSERT_OK(impl_.WaitConfigurationChanged());
  ASSERT_EQ(impl_.mode(), netdev::wire::MacFilterMode::kPromiscuous);

  // close the instance and check that we fell back to the default mode.
  client = {};
  ASSERT_OK(impl_.WaitConfigurationChanged());
  ASSERT_EQ(impl_.mode(), netdev::wire::MacFilterMode::kMulticastFilter);
  ASSERT_TRUE(impl_.addresses().empty());
}

// Regression test for https://fxbug.dev/517998440
TEST_F(MacDeviceTest, MulticastFilterCountGreaterThanMaxMacFilter) {
  // Configure the fake device to report a multicast filter count larger than the FIDL limit (64).
  impl_.features().multicast_filter_count(netdriver::wire::kMaxMacFilter + 10);
  impl_.features().supported_modes(netdriver::SupportedMacFilterMode::kMulticastFilter);

  ASSERT_OK(CreateDevice());
  ASSERT_EQ(impl_.mode(), netdev::wire::MacFilterMode::kMulticastFilter);

  fidl::WireSyncClient<netdev::MacAddressing> cli1 = OpenInstance();
  fidl::WireSyncClient<netdev::MacAddressing> cli2 = OpenInstance();

  // Add 33 unique multicast addresses on client 1.
  for (size_t i = 0; i < 33; i++) {
    MacAddress addr{{0x01, 0x00, 0x00, 0x00, 0x00, static_cast<unsigned char>(i)}};
    fidl::WireResult result = cli1->AddMulticastAddress(addr);
    ASSERT_OK(result.status());
    ASSERT_OK(result.value().status);
    ASSERT_OK(impl_.WaitConfigurationChanged());
  }

  // Add 32 unique multicast addresses on client 2 (total 65 addresses).
  for (size_t i = 0; i < 32; i++) {
    MacAddress addr{{0x01, 0x00, 0x00, 0x00, 0x00, static_cast<unsigned char>(100 + i)}};
    fidl::WireResult result = cli2->AddMulticastAddress(addr);
    // This should succeed. If the bug is present, the 32nd addition (65th total)
    // will fail because the internal FIDL call SetMode fails and closes the channel.
    ASSERT_OK(result.status());
    ASSERT_OK(result.value().status);
    ASSERT_OK(impl_.WaitConfigurationChanged());
  }

  // Verify that the driver capped the filter list at kMaxMacFilter (64)
  // instead of sending 65 addresses.
  ASSERT_EQ(impl_.addresses().size(), netdriver::wire::kMaxMacFilter);
}

TEST_F(MacDeviceTest, DriverUnbindDoesNotBlockTeardown) {
  // Device teardown attempts to run `SetMode` on the driver after unbinding
  // each client, but unbinding the driver may cause `SetMode` to fail or not to
  // return at all. Race the two operations and check that the latter doesn't
  // block the former.
  for (int i = 0; i < 1000; ++i) {
    FakeMacDeviceImpl impl;
    zx::result device = impl.CreateChild(dispatcher_.get());
    ASSERT_OK(device);
    fidl::WireSyncClient<netdev::MacAddressing> client = OpenInstance(device.value().get());

    // Unbind the driver from the device interface so that address consolidation
    // fails.
    impl.Unbind();

    // Teardown must gracefully handle the potential consolidation failure(s).
    sync_completion_t completion;
    device->Teardown([&completion]() { sync_completion_signal(&completion); });
    ASSERT_OK(sync_completion_wait(&completion, zx::sec(5).get()));
  }
}

}  // namespace testing
}  // namespace network
