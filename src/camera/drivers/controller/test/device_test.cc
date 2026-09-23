// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.camera/cpp/wire.h>
#include <fuchsia/hardware/camera/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async_patterns/testing/cpp/dispatcher_bound.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>

#include "src/camera/drivers/controller/controller_device.h"
#include "src/camera/drivers/controller/controller_protocol.h"
#include "src/camera/drivers/controller/test/constants.h"
#include "src/camera/drivers/controller/test/fake_sysmem.h"
#include "src/devices/testing/mock-ddk/mock-device.h"
#include "src/lib/testing/loop_fixture/test_loop_fixture.h"

namespace camera {
namespace {

class ControllerDeviceTest : public gtest::TestLoopFixture {
 public:
  void SetUp() override {
    ASSERT_EQ(ZX_OK, incoming_loop_.StartThread("incoming"));
    root_ = MockDevice::FakeRootParent();

    // Create sysmem fragment
    root_->AddNsProtocol<fuchsia_sysmem2::Allocator>(
        [&](auto request) { sysmem_.SyncCall(&FakeSysmem::Connect, std::move(request)); });

    auto result = ControllerDevice::Create(root_.get());
    ASSERT_TRUE(result.is_ok());
    controller_device_ = result.take_value().release();

    ASSERT_EQ(controller_device_->DdkAdd("test-camera-controller"), ZX_OK);
    ASSERT_EQ(root_->child_count(), 1u);
    controller_mock_ = root_->GetLatestChild();

    ASSERT_EQ(loop_.StartThread(), ZX_OK);
  }

  void TearDown() override {
    controller_protocol_ = nullptr;
    controller_device_ = nullptr;
    controller_mock_ = nullptr;
    loop_.Shutdown();
  }

  static void FailErrorHandler(zx_status_t status) {
    ADD_FAILURE() << "Channel Failure: " << status;
  }

  static void WaitForChannelClosure(const zx::channel& channel) {
    // TODO(https://fxbug.dev/42114317): allow unidirectional message processing
    // Currently, running a loop associated with fidl::InterfacePtr handles both inbound and
    // outbound messages. Depending on how quickly the server handles such requests, the
    // channel may or may not be closed by the time a single call to RunUntilIdle returns.
    zx_status_t status = channel.wait_one(ZX_CHANNEL_PEER_CLOSED, zx::time::infinite(), nullptr);
    if (status != ZX_OK) {
      EXPECT_EQ(status, ZX_ERR_BAD_HANDLE);
    }
  }

  template <class T>
  void WaitForInterfaceClosure(fidl::InterfacePtr<T>& ptr, zx_status_t expected_epitaph) {
    bool epitaph_received = false;
    zx_status_t epitaph_status = ZX_OK;
    ptr.set_error_handler([&](zx_status_t status) {
      ASSERT_FALSE(epitaph_received) << "We should only get one epitaph!";
      epitaph_received = true;
      epitaph_status = status;
    });
    WaitForChannelClosure(ptr.channel());
    RunLoopUntilIdle();
    if (epitaph_received) {  // Epitaphs are not guaranteed to be returned.
      EXPECT_EQ(epitaph_status, expected_epitaph);
    }
  }

  void BindControllerProtocol() {
    auto endpoints = fidl::Endpoints<fuchsia_hardware_camera::Device>::Create();
    fidl::BindServer(loop_.dispatcher(), std::move(endpoints.server), controller_device_);

    ASSERT_EQ(camera_protocol_.Bind(endpoints.client.TakeChannel()), ZX_OK);
    camera_protocol_.set_error_handler(FailErrorHandler);
    camera_protocol_->GetChannel2(controller_protocol_.NewRequest());
    controller_protocol_.set_error_handler(FailErrorHandler);
    RunLoopUntilIdle();
  }

  std::shared_ptr<MockDevice> root_;
  ControllerDevice* controller_device_ = nullptr;
  MockDevice* controller_mock_ = nullptr;
  fuchsia::hardware::camera::DevicePtr camera_protocol_;
  fuchsia::camera2::hal::ControllerPtr controller_protocol_;
  async::Loop incoming_loop_{&kAsyncLoopConfigNoAttachToCurrentThread};
  async_patterns::TestDispatcherBound<FakeSysmem> sysmem_{incoming_loop_.dispatcher(),
                                                          std::in_place};
  async_patterns::TestDispatcherBound<component::OutgoingDirectory> outgoing_{
      incoming_loop_.dispatcher(), std::in_place, async_patterns::PassDispatcher};
  async::Loop loop_{&kAsyncLoopConfigNeverAttachToThread};
};

// Verifies controller can start up and shut down.
TEST_F(ControllerDeviceTest, DdkLifecycle) {
  controller_device_->DdkUnbind(ddk::UnbindTxn{controller_mock_});
  controller_device_->DdkAsyncRemove();
  controller_mock_->WaitUntilUnbindReplyCalled();
  mock_ddk::ReleaseFlaggedDevices(root_.get());
  EXPECT_EQ(root_->child_count(), 0u);
}

// Verifies GetChannel is not supported.
TEST_F(ControllerDeviceTest, GetChannel) {
  auto endpoints = fidl::Endpoints<fuchsia_hardware_camera::Device>::Create();
  fidl::BindServer(loop_.dispatcher(), std::move(endpoints.server), controller_device_);

  ASSERT_EQ(camera_protocol_.Bind(endpoints.client.TakeChannel()), ZX_OK);
  camera_protocol_->GetChannel(controller_protocol_.NewRequest().TakeChannel());
  RunLoopUntilIdle();
  WaitForChannelClosure(controller_protocol_.channel());
  WaitForInterfaceClosure(camera_protocol_, ZX_ERR_NOT_SUPPORTED);
}

// Verifies that GetChannel2 works correctly.
TEST_F(ControllerDeviceTest, GetChannel2) {
  auto endpoints = fidl::Endpoints<fuchsia_hardware_camera::Device>::Create();
  fidl::BindServer(loop_.dispatcher(), std::move(endpoints.server), controller_device_);

  ASSERT_EQ(camera_protocol_.Bind(endpoints.client.TakeChannel()), ZX_OK);
  camera_protocol_->GetChannel2(controller_protocol_.NewRequest());
  camera_protocol_.set_error_handler(FailErrorHandler);
  RunLoopUntilIdle();
}

// Verifies that GetChannel2 can only have one binding.
TEST_F(ControllerDeviceTest, GetChannel2InvokeTwice) {
  auto endpoints = fidl::Endpoints<fuchsia_hardware_camera::Device>::Create();
  fidl::BindServer(loop_.dispatcher(), std::move(endpoints.server), controller_device_);

  ASSERT_EQ(camera_protocol_.Bind(endpoints.client.TakeChannel()), ZX_OK);
  camera_protocol_->GetChannel2(controller_protocol_.NewRequest());
  RunLoopUntilIdle();
  fuchsia::camera2::hal::ControllerPtr other_controller_protocol;
  camera_protocol_->GetChannel2(other_controller_protocol.NewRequest());
  RunLoopUntilIdle();
  WaitForChannelClosure(other_controller_protocol.channel());
}

// Verifies sanity of returned device info.
TEST_F(ControllerDeviceTest, GetDeviceInfo) {
  ASSERT_NO_FATAL_FAILURE(BindControllerProtocol());
  controller_protocol_->GetDeviceInfo([](fuchsia::camera2::DeviceInfo device_info) {
    ASSERT_TRUE(device_info.has_vendor_id());
    ASSERT_TRUE(device_info.has_product_id());
    EXPECT_NE(device_info.vendor_id(), 0u);
    EXPECT_NE(device_info.product_id(), 0u);
    EXPECT_EQ(fuchsia::camera2::DeviceType::BUILTIN, device_info.type());
  });
  RunLoopUntilIdle();
}

// Verifies sanity of returned configs.
TEST_F(ControllerDeviceTest, GetNextConfig) {
  ASSERT_NO_FATAL_FAILURE(BindControllerProtocol());
  uint32_t number_of_configs = 0;
  constexpr uint32_t kNumConfigs = 3;
  bool config_populated = false;

  while (number_of_configs != kNumConfigs) {
    controller_protocol_->GetNextConfig(
        [&](std::unique_ptr<fuchsia::camera2::hal::Config> config, zx_status_t status) {
          switch (number_of_configs) {
            case SherlockConfigs::MONITORING: {
              // Config 0 (monitoring)
              ASSERT_NE(config, nullptr);
              EXPECT_EQ(config->stream_configs.at(0).properties.stream_type(),
                        fuchsia::camera2::CameraStreamType::FULL_RESOLUTION |
                            fuchsia::camera2::CameraStreamType::MACHINE_LEARNING);
              EXPECT_EQ(config->stream_configs.at(1).properties.stream_type(),
                        fuchsia::camera2::CameraStreamType::DOWNSCALED_RESOLUTION |
                            fuchsia::camera2::CameraStreamType::MACHINE_LEARNING);
              EXPECT_EQ(config->stream_configs.at(2).properties.stream_type(),
                        fuchsia::camera2::CameraStreamType::MONITORING);
              break;
            }
            case SherlockConfigs::VIDEO: {
              // Config 1 (video conferencing)
              ASSERT_NE(config, nullptr);
              EXPECT_EQ(config->stream_configs.at(0).properties.stream_type(),
                        fuchsia::camera2::CameraStreamType::VIDEO_CONFERENCE |
                            fuchsia::camera2::CameraStreamType::MACHINE_LEARNING |
                            fuchsia::camera2::CameraStreamType::FULL_RESOLUTION);
              EXPECT_EQ(config->stream_configs.at(1).properties.stream_type(),
                        fuchsia::camera2::CameraStreamType::VIDEO_CONFERENCE);
              break;
            }
            case SherlockConfigs::VIDEO_EXTENDED_FOV: {
              // Config 2 (video conferencing with extended FOV)
              ASSERT_NE(config, nullptr);
              EXPECT_EQ(config->stream_configs.at(0).properties.stream_type(),
                        fuchsia::camera2::CameraStreamType::VIDEO_CONFERENCE |
                            fuchsia::camera2::CameraStreamType::MACHINE_LEARNING |
                            fuchsia::camera2::CameraStreamType::FULL_RESOLUTION |
                            fuchsia::camera2::CameraStreamType::EXTENDED_FOV);
              EXPECT_EQ(config->stream_configs.at(1).properties.stream_type(),
                        fuchsia::camera2::CameraStreamType::VIDEO_CONFERENCE |
                            fuchsia::camera2::CameraStreamType::EXTENDED_FOV);
              break;
            }
            default: {
              EXPECT_EQ(config, nullptr);
              EXPECT_EQ(status, ZX_ERR_STOP);
              break;
            }
          }

          config_populated = true;
          number_of_configs++;
        });

    while (!config_populated) {
      RunLoopUntilIdle();
    }
    config_populated = false;
  }
}

TEST_F(ControllerDeviceTest, CreateStreamInvalidArgs) {
  ASSERT_NO_FATAL_FAILURE(BindControllerProtocol());
  fuchsia::camera2::StreamPtr stream;
  std::unique_ptr<fuchsia::camera2::hal::Config> camera_config;
  bool configs_populated = false;
  controller_protocol_->GetNextConfig(
      [&](std::unique_ptr<fuchsia::camera2::hal::Config> config, zx_status_t status) {
        ASSERT_NE(config, nullptr);
        ASSERT_EQ(status, ZX_OK);
        configs_populated = true;
        camera_config = std::move(config);
      });
  while (!configs_populated) {
    RunLoopUntilIdle();
  }

  // Invalid config index.
  constexpr uint32_t kInvalidConfigIndex = 10;
  controller_protocol_->CreateStream(kInvalidConfigIndex, 0, 0, stream.NewRequest());
  WaitForInterfaceClosure(stream, ZX_ERR_INVALID_ARGS);

  // Invalid stream index.
  controller_protocol_->CreateStream(0, static_cast<uint32_t>(camera_config->stream_configs.size()),
                                     0, stream.NewRequest());
  WaitForInterfaceClosure(stream, ZX_ERR_INVALID_ARGS);

  // Invalid format index.
  controller_protocol_->CreateStream(
      0, 0, static_cast<uint32_t>(camera_config->stream_configs[0].image_formats.size()),
      stream.NewRequest());
  WaitForInterfaceClosure(stream, ZX_ERR_INVALID_ARGS);
}

}  // namespace
}  // namespace camera
