// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/camera/lib/fake_camera/fake_camera.h"

#include <fuchsia/sysmem/cpp/fidl.h>
#include <lib/sys/cpp/component_context.h>
#include <zircon/status.h>
#include <zircon/types.h>

#include "src/lib/fsl/handles/object_info.h"
#include "src/lib/testing/loop_fixture/real_loop_fixture.h"

constexpr auto kCameraIdentifier = "FakeCameraTest";

class FakeCameraTest : public gtest::RealLoopFixture {
 public:
  FakeCameraTest() { context_ = sys::ComponentContext::CreateAndServeOutgoingDirectory(); }

 protected:
  virtual void SetUp() override {
    context_->svc()->Connect(allocator_.NewRequest());

    fuchsia::sysmem2::AllocatorSetDebugClientInfoRequest set_debug_request;
    set_debug_request.set_name(fsl::GetCurrentProcessName());
    set_debug_request.set_id(fsl::GetCurrentProcessKoid());
    allocator_->SetDebugClientInfo(std::move(set_debug_request));

    allocator_.set_error_handler(MakeErrorHandler("Sysmem Allocator"));
  }

  virtual void TearDown() override {
    allocator_ = nullptr;
    RunLoopUntilIdle();
  }

  static fit::function<void(zx_status_t status)> MakeErrorHandler(std::string server) {
    return [server](zx_status_t status) {
      ADD_FAILURE() << server << " server disconnected - " << status;
    };
  }

  template <class T>
  static void SetFailOnError(fidl::InterfacePtr<T>& ptr, std::string name = T::Name_) {
    ptr.set_error_handler([=](zx_status_t status) {
      ADD_FAILURE() << name << " server disconnected: " << zx_status_get_string(status);
    });
  }

  std::unique_ptr<sys::ComponentContext> context_;
  fuchsia::sysmem2::AllocatorPtr allocator_;
};

TEST_F(FakeCameraTest, InvalidArgs) {
  {  // No configurations.
    auto result = camera::FakeCamera::Create(kCameraIdentifier, {});
    ASSERT_TRUE(result.is_error());
    EXPECT_EQ(result.error(), ZX_ERR_INVALID_ARGS);
  }

  {  // No streams.
    std::vector<camera::FakeConfiguration> configs;
    configs.push_back({});
    auto result = camera::FakeCamera::Create(kCameraIdentifier, std::move(configs));
    ASSERT_TRUE(result.is_error());
    EXPECT_EQ(result.error(), ZX_ERR_INVALID_ARGS);
  }

  {  // Null stream.
    camera::FakeConfiguration config;
    config.push_back(nullptr);
    std::vector<camera::FakeConfiguration> configs;
    configs.push_back(std::move(config));
    auto result = camera::FakeCamera::Create(kCameraIdentifier, std::move(configs));
    ASSERT_TRUE(result.is_error());
    EXPECT_EQ(result.error(), ZX_ERR_INVALID_ARGS);
  }
}

TEST_F(FakeCameraTest, SetBufferCollectionInvokesCallback) {
  fuchsia::camera3::StreamProperties properties{
      .image_format{.pixel_format{.type = fuchsia::sysmem::PixelFormatType::NV12},
                    .coded_width = 256,
                    .coded_height = 128,
                    .bytes_per_row = 256,
                    .color_space{.type = fuchsia::sysmem::ColorSpaceType::REC601_NTSC}},
      .frame_rate{
          .numerator = 30,
          .denominator = 1,
      }};
  bool callback_invoked = false;
  auto stream_result = camera::FakeStream::Create(
      std::move(properties),
      [&](fidl::InterfaceHandle<fuchsia::sysmem2::BufferCollectionToken> token) {
        callback_invoked = true;
      });
  ASSERT_TRUE(stream_result.is_ok());
  std::shared_ptr<camera::FakeStream> stream = stream_result.take_value();

  camera::FakeConfiguration config;
  config.push_back(stream);
  std::vector<camera::FakeConfiguration> configs;
  configs.push_back(std::move(config));
  auto camera_result = camera::FakeCamera::Create(kCameraIdentifier, std::move(configs));
  ASSERT_TRUE(camera_result.is_ok());
  auto camera = camera_result.take_value();

  fuchsia::camera3::DevicePtr device_protocol;
  SetFailOnError(device_protocol, "Device");
  fuchsia::camera3::StreamPtr stream_protocol;
  SetFailOnError(stream_protocol, "Stream");
  camera->GetHandler()(device_protocol.NewRequest());
  fuchsia::sysmem2::BufferCollectionTokenPtr token;
  SetFailOnError(token, "Token");
  device_protocol->GetConfigurations(
      [&](std::vector<fuchsia::camera3::Configuration> configurations) {
        ASSERT_FALSE(configurations.empty());
        ASSERT_FALSE(configurations[0].streams.empty());
        device_protocol->ConnectToStream(0, stream_protocol.NewRequest());

        fuchsia::sysmem2::AllocatorAllocateSharedCollectionRequest allocate_shared_request;
        allocate_shared_request.set_token_request(token.NewRequest());
        allocator_->AllocateSharedCollection(std::move(allocate_shared_request));

        stream_protocol->SetBufferCollection(
            fuchsia::sysmem::BufferCollectionTokenHandle(token.Unbind().TakeChannel()));
      });
  RunLoopUntil([&]() { return HasFailure() || callback_invoked; });
}
