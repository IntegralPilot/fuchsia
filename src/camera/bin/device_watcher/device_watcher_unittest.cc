// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.camera.test/cpp/fidl.h>
#include <fidl/fuchsia.camera2.hal/cpp/fidl.h>
#include <fidl/fuchsia.camera3/cpp/fidl.h>
#include <fidl/fuchsia.hardware.camera/cpp/fidl.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/syslog/cpp/macros.h>

#include "src/camera/bin/device_watcher/device_instance.h"
#include "src/lib/testing/loop_fixture/test_loop_fixture.h"

class DeviceWatcherTest : public gtest::TestLoopFixture {
 protected:
  DeviceWatcherTest() = default;
  void SetUp() override {
    auto watcher = component::Connect<fuchsia_camera3::DeviceWatcher>();
    ASSERT_TRUE(watcher.is_ok());
    watcher_ =
        fidl::Client<fuchsia_camera3::DeviceWatcher>(std::move(watcher.value()), dispatcher());

    auto tester = component::Connect<fuchsia_camera_test::DeviceWatcherTester>();
    ASSERT_TRUE(tester.is_ok());
    tester_ = fidl::Client<fuchsia_camera_test::DeviceWatcherTester>(std::move(tester.value()),
                                                                     dispatcher());

    RunLoopUntilIdle();
  }

  void TearDown() override {
    tester_ = fidl::Client<fuchsia_camera_test::DeviceWatcherTester>();
    watcher_ = fidl::Client<fuchsia_camera3::DeviceWatcher>();
    RunLoopUntilIdle();
  }

  fidl::Client<fuchsia_camera3::DeviceWatcher> watcher_;
  fidl::Client<fuchsia_camera_test::DeviceWatcherTester> tester_;
};

constexpr uint16_t kFakeVendorId = 0xFFFF;
constexpr uint16_t kFakeProductId = 0xABCD;

class FakeCamera : public fidl::Server<fuchsia_hardware_camera::Device>,
                   public fidl::Server<fuchsia_camera2_hal::Controller> {
 public:
  explicit FakeCamera(fidl::ServerEnd<fuchsia_hardware_camera::Device> request,
                      async_dispatcher_t* dispatcher)
      : dispatcher_(dispatcher) {
    camera_binding_.emplace(dispatcher, std::move(request), this, [](fidl::UnbindInfo) {});
  }

  void GetChannel(GetChannelRequest& request, GetChannelCompleter::Sync& completer) override {}

  void GetChannel2(GetChannel2Request& request, GetChannel2Completer::Sync& completer) override {
    controller_binding_.emplace(dispatcher_, std::move(request.server_end()), this,
                                [](fidl::UnbindInfo) {});
  }

  void GetDebugChannel(GetDebugChannelRequest& request,
                       GetDebugChannelCompleter::Sync& completer) override {}

  void GetNextConfig(GetNextConfigCompleter::Sync& completer) override {}

  void CreateStream(CreateStreamRequest& request, CreateStreamCompleter::Sync& completer) override {
  }

  void EnableStreaming(EnableStreamingCompleter::Sync& completer) override {}

  void DisableStreaming(DisableStreamingCompleter::Sync& completer) override {}

  void GetDeviceInfo(GetDeviceInfoCompleter::Sync& completer) override {
    fuchsia_camera2::DeviceInfo info;
    info.vendor_id(kFakeVendorId);
    info.product_id(kFakeProductId);
    completer.Reply({{.info = std::move(info)}});
  }

 private:
  async_dispatcher_t* dispatcher_;
  std::optional<fidl::ServerBinding<fuchsia_hardware_camera::Device>> camera_binding_;
  std::optional<fidl::ServerBinding<fuchsia_camera2_hal::Controller>> controller_binding_;
};

// TODO(https://fxbug.dev/42130510): fix device_watcher_test flake
TEST_F(DeviceWatcherTest, DISABLED_WatchDevicesFindsCameras) {
  auto camera_endpoints = fidl::CreateEndpoints<fuchsia_hardware_camera::Device>();
  FakeCamera fake(std::move(camera_endpoints->server), dispatcher());
  ASSERT_TRUE(tester_->InjectDevice({{.camera = std::move(camera_endpoints->client)}}).is_ok());
  std::set<uint64_t> cameras;

  // Wait until the watcher has discovered the real camera and the injected fake camera.
  constexpr uint32_t kExpectedCameras = 2;
  while (!HasFailure() && cameras.size() < kExpectedCameras) {
    bool watch_devices_returned = false;
    watcher_->WatchDevices().Then(
        [&](const fidl::Result<fuchsia_camera3::DeviceWatcher::WatchDevices>& result) {
          ASSERT_TRUE(result.is_ok());
          for (const auto& event : result.value().events()) {
            if (event.Which() == fuchsia_camera3::WatchDevicesEvent::Tag::kAdded) {
              EXPECT_EQ(cameras.find(event.added().value()), cameras.end());
              cameras.insert(event.added().value());
            }
            EXPECT_FALSE(event.Which() == fuchsia_camera3::WatchDevicesEvent::Tag::kRemoved);
          }
          watch_devices_returned = true;
        });
    while (!HasFailure() && !watch_devices_returned) {
      RunLoopUntilIdle();
    }
  }
  ASSERT_EQ(cameras.size(), kExpectedCameras);

  // Ensure that a second watcher client is given the same cameras.
  auto watcher2_result = component::Connect<fuchsia_camera3::DeviceWatcher>();
  ASSERT_TRUE(watcher2_result.is_ok());
  auto watcher2 = fidl::Client<fuchsia_camera3::DeviceWatcher>(std::move(watcher2_result.value()),
                                                               dispatcher());

  while (!HasFailure() && !cameras.empty()) {
    bool watch_devices_returned = false;
    watcher2->WatchDevices().Then(
        [&](const fidl::Result<fuchsia_camera3::DeviceWatcher::WatchDevices>& result) {
          ASSERT_TRUE(result.is_ok());
          for (const auto& event : result.value().events()) {
            ASSERT_TRUE(event.Which() == fuchsia_camera3::WatchDevicesEvent::Tag::kAdded);
            auto it = cameras.find(event.added().value());
            ASSERT_NE(it, cameras.end());
            cameras.erase(it);
          }
          watch_devices_returned = true;
        });
    while (!HasFailure() && !watch_devices_returned) {
      RunLoopUntilIdle();
    }
  }
}
