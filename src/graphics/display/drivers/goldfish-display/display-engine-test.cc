// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/goldfish-display/display-engine.h"

#include <fidl/fuchsia.hardware.goldfish.pipe/cpp/wire.h>
#include <fidl/fuchsia.hardware.goldfish/cpp/wire.h>
#include <fidl/fuchsia.sysmem2/cpp/wire.h>
#include <fidl/fuchsia.sysmem2/cpp/wire_test_base.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/loop.h>
#include <lib/driver/testing/cpp/driver_runtime.h>
#include <lib/driver/testing/cpp/scoped_global_logger.h>
#include <lib/fdf/cpp/dispatcher.h>

#include <array>
#include <cstddef>
#include <cstdint>
#include <memory>

#include <gtest/gtest.h>

#include "src/graphics/display/lib/api-protocols/cpp/display-engine-events-fidl.h"
#include "src/graphics/display/lib/api-types/cpp/color-conversion.h"
#include "src/graphics/display/lib/api-types/cpp/config-check-result.h"
#include "src/graphics/display/lib/api-types/cpp/display-id.h"
#include "src/graphics/display/lib/api-types/cpp/driver-buffer-collection-id.h"
#include "src/graphics/display/lib/api-types/cpp/driver-layer.h"
#include "src/graphics/display/lib/api-types/cpp/mode-id.h"
#include "src/lib/testing/predicates/status.h"

namespace goldfish {

namespace {

constexpr int32_t kDisplayWidthPx = 1024;
constexpr int32_t kDisplayHeightPx = 768;
constexpr int32_t kDisplayRefreshRateHz = 60;

constexpr size_t kMaxLayerCount = 3;  // This is the max size of layer array.

}  // namespace

// TODO(https://fxbug.dev/42072949): Consider creating and using a unified set of sysmem
// testing doubles instead of writing mocks for each display driver test.
class FakeAllocator : public fidl::testing::WireTestBase<fuchsia_sysmem2::Allocator> {
 public:
  void NotImplemented_(const std::string& name, fidl::CompleterBase& completer) override {}
};

class GoldfishDisplayEngineTest : public testing::Test {
 public:
  GoldfishDisplayEngineTest() : loop_(&kAsyncLoopConfigNeverAttachToThread) {}

  void SetUp() override;
  void TearDown() override;

 protected:
  fdf_testing::ScopedGlobalLogger logger_;

  fdf_testing::DriverRuntime driver_runtime_;
  fdf::UnownedSynchronizedDispatcher display_event_dispatcher_ =
      driver_runtime_.StartBackgroundDispatcher();

  display::DisplayEngineEventsFidl engine_events_fidl_;
  std::unique_ptr<DisplayEngine> display_engine_;

  std::optional<fidl::ServerBindingRef<fuchsia_hardware_goldfish_pipe::Bus>> binding_;
  std::optional<fidl::ServerBindingRef<fuchsia_sysmem2::Allocator>> allocator_binding_;
  async::Loop loop_;
  FakeAllocator mock_allocator_;
};

void GoldfishDisplayEngineTest::SetUp() {
  auto [control_client, control_server] =
      fidl::Endpoints<fuchsia_hardware_goldfish::ControlDevice>::Create();
  auto [pipe_bus_client, pipe_bus_server] =
      fidl::Endpoints<fuchsia_hardware_goldfish_pipe::Bus>::Create();
  auto [sysmem_client, sysmem_server] = fidl::Endpoints<fuchsia_sysmem2::Allocator>::Create();
  allocator_binding_ =
      fidl::BindServer(loop_.dispatcher(), std::move(sysmem_server), &mock_allocator_);

  display_engine_ = std::make_unique<DisplayEngine>(
      std::move(control_client), std::move(pipe_bus_client), std::move(sysmem_client),
      std::make_unique<RenderControl>(), display_event_dispatcher_->async_dispatcher(),
      &engine_events_fidl_);

  // Call SetupPrimaryDisplayForTesting() so that we can set up the display
  // devices without any dependency on proper driver binding.
  display_engine_->SetupPrimaryDisplayForTesting(kDisplayWidthPx, kDisplayHeightPx,
                                                 kDisplayRefreshRateHz);
}

void GoldfishDisplayEngineTest::TearDown() { allocator_binding_->Unbind(); }

TEST_F(GoldfishDisplayEngineTest, CheckConfigMultiLayer) {
  static constexpr display::Rectangle kDisplayArea = {
      {.x = 0, .y = 0, .width = kDisplayWidthPx, .height = kDisplayHeightPx}};
  static constexpr display::DriverLayer kLayers[] = {
      {{
          .display_destination = kDisplayArea,
          .image_source = kDisplayArea,
          .image_id = display::kInvalidDriverImageId,
          .image_metadata =
              display::ImageMetadata({.width = kDisplayWidthPx,
                                      .height = kDisplayHeightPx,
                                      .tiling_type = display::ImageTilingType::kLinear}),
          .fallback_color = display::Color({.format = display::PixelFormat::kB8G8R8A8,
                                            .bytes = {{0x41, 0x42, 0x43, 0x44, 0, 0, 0, 0}}}),
          .alpha_mode = display::AlphaMode::kDisable,
          .image_source_transformation = display::CoordinateTransformation::kIdentity,
      }},
      {{
          .display_destination = kDisplayArea,
          .image_source = kDisplayArea,
          .image_id = display::kInvalidDriverImageId,
          .image_metadata =
              display::ImageMetadata({.width = kDisplayWidthPx,
                                      .height = kDisplayHeightPx,
                                      .tiling_type = display::ImageTilingType::kLinear}),
          .fallback_color = display::Color({.format = display::PixelFormat::kB8G8R8A8,
                                            .bytes = {{0x41, 0x42, 0x43, 0x44, 0, 0, 0, 0}}}),
          .alpha_mode = display::AlphaMode::kDisable,
          .image_source_transformation = display::CoordinateTransformation::kIdentity,
      }},
      {{
          .display_destination = kDisplayArea,
          .image_source = kDisplayArea,
          .image_id = display::kInvalidDriverImageId,
          .image_metadata =
              display::ImageMetadata({.width = kDisplayWidthPx,
                                      .height = kDisplayHeightPx,
                                      .tiling_type = display::ImageTilingType::kLinear}),
          .fallback_color = display::Color({.format = display::PixelFormat::kB8G8R8A8,
                                            .bytes = {{0x41, 0x42, 0x43, 0x44, 0, 0, 0, 0}}}),
          .alpha_mode = display::AlphaMode::kDisable,
          .image_source_transformation = display::CoordinateTransformation::kIdentity,
      }},
  };
  static_assert(std::size(kLayers) == kMaxLayerCount);

  display::ConfigCheckResult res = display_engine_->CheckConfiguration(
      display::DisplayId(1), display::ModeId(1), display::ColorConversion::kIdentity, kLayers);
  EXPECT_EQ(display::ConfigCheckResult::kUnsupportedConfig, res);
}

TEST_F(GoldfishDisplayEngineTest, CheckConfigLayerColor) {
  static constexpr display::Rectangle kDisplayArea = {
      {.x = 0, .y = 0, .width = kDisplayWidthPx, .height = kDisplayHeightPx}};
  static constexpr display::DriverLayer kLayers[] = {
      {{
          .display_destination = kDisplayArea,
          .image_source = display::Rectangle({.x = 0, .y = 0, .width = 0, .height = 0}),
          .image_id = display::kInvalidDriverImageId,
          .image_metadata = display::ImageMetadata(
              {.width = 0, .height = 0, .tiling_type = display::ImageTilingType::kLinear}),
          .fallback_color = display::Color({.format = display::PixelFormat::kB8G8R8A8,
                                            .bytes = {{0x41, 0x42, 0x43, 0x44, 0, 0, 0, 0}}}),
          .alpha_mode = display::AlphaMode::kDisable,
          .image_source_transformation = display::CoordinateTransformation::kIdentity,
      }},
  };
  display::ConfigCheckResult res = display_engine_->CheckConfiguration(
      display::DisplayId(1), display::ModeId(1), display::ColorConversion::kIdentity, kLayers);
  EXPECT_EQ(display::ConfigCheckResult::kUnsupportedConfig, res);
}

TEST_F(GoldfishDisplayEngineTest, CheckConfigLayerPrimary) {
  static constexpr display::Rectangle kDisplayArea = {
      {.x = 0, .y = 0, .width = kDisplayWidthPx, .height = kDisplayHeightPx}};
  static constexpr display::DriverLayer kLayers[] = {
      {{
          .display_destination = kDisplayArea,
          .image_source = kDisplayArea,
          .image_id = display::kInvalidDriverImageId,
          .image_metadata =
              display::ImageMetadata({.width = kDisplayWidthPx,
                                      .height = kDisplayHeightPx,
                                      .tiling_type = display::ImageTilingType::kLinear}),
          .fallback_color = display::Color({.format = display::PixelFormat::kB8G8R8A8,
                                            .bytes = {{0x41, 0x42, 0x43, 0x44, 0, 0, 0, 0}}}),
          .alpha_mode = display::AlphaMode::kDisable,
          .image_source_transformation = display::CoordinateTransformation::kIdentity,
      }},
  };

  display::ConfigCheckResult res = display_engine_->CheckConfiguration(
      display::DisplayId(1), display::ModeId(1), display::ColorConversion::kIdentity, kLayers);
  EXPECT_EQ(display::ConfigCheckResult::kOk, res);
}

TEST_F(GoldfishDisplayEngineTest, CheckConfigLayerDestFrame) {
  static constexpr display::DriverLayer kLayers[] = {
      {{
          .display_destination = display::Rectangle(
              {.x = 0, .y = 0, .width = kDisplayHeightPx, .height = kDisplayHeightPx}),
          .image_source = display::Rectangle(
              {.x = 0, .y = 0, .width = kDisplayWidthPx, .height = kDisplayHeightPx}),
          .image_id = display::kInvalidDriverImageId,
          .image_metadata =
              display::ImageMetadata({.width = kDisplayWidthPx,
                                      .height = kDisplayHeightPx,
                                      .tiling_type = display::ImageTilingType::kLinear}),
          .fallback_color = display::Color({.format = display::PixelFormat::kB8G8R8A8,
                                            .bytes = {{0x41, 0x42, 0x43, 0x44, 0, 0, 0, 0}}}),
          .alpha_mode = display::AlphaMode::kDisable,
          .image_source_transformation = display::CoordinateTransformation::kIdentity,
      }},
  };
  display::ConfigCheckResult res = display_engine_->CheckConfiguration(
      display::DisplayId(1), display::ModeId(1), display::ColorConversion::kIdentity, kLayers);
  EXPECT_EQ(display::ConfigCheckResult::kUnsupportedConfig, res);
}

TEST_F(GoldfishDisplayEngineTest, CheckConfigLayerSrcFrame) {
  static constexpr display::DriverLayer kLayers[] = {
      {{
          .display_destination = display::Rectangle(
              {.x = 0, .y = 0, .width = kDisplayWidthPx, .height = kDisplayHeightPx}),
          .image_source = display::Rectangle(
              {.x = 0, .y = 0, .width = kDisplayHeightPx, .height = kDisplayHeightPx}),
          .image_id = display::kInvalidDriverImageId,
          .image_metadata =
              display::ImageMetadata({.width = kDisplayWidthPx,
                                      .height = kDisplayHeightPx,
                                      .tiling_type = display::ImageTilingType::kLinear}),
          .fallback_color = display::Color({.format = display::PixelFormat::kB8G8R8A8,
                                            .bytes = {{0x41, 0x42, 0x43, 0x44, 0, 0, 0, 0}}}),
          .alpha_mode = display::AlphaMode::kDisable,
          .image_source_transformation = display::CoordinateTransformation::kIdentity,
      }},
  };
  display::ConfigCheckResult res = display_engine_->CheckConfiguration(
      display::DisplayId(1), display::ModeId(1), display::ColorConversion::kIdentity, kLayers);
  EXPECT_EQ(display::ConfigCheckResult::kUnsupportedConfig, res);
}

TEST_F(GoldfishDisplayEngineTest, CheckConfigLayerAlpha) {
  static constexpr display::Rectangle kDisplayArea = {
      {.x = 0, .y = 0, .width = kDisplayWidthPx, .height = kDisplayHeightPx}};
  static constexpr display::DriverLayer kLayers[] = {
      {{
          .display_destination = kDisplayArea,
          .image_source = kDisplayArea,
          .image_id = display::kInvalidDriverImageId,
          .image_metadata =
              display::ImageMetadata({.width = kDisplayWidthPx,
                                      .height = kDisplayHeightPx,
                                      .tiling_type = display::ImageTilingType::kLinear}),
          .fallback_color = display::Color({.format = display::PixelFormat::kB8G8R8A8,
                                            .bytes = {{0x41, 0x42, 0x43, 0x44, 0, 0, 0, 0}}}),
          .alpha_mode = display::AlphaMode::kHwMultiply,
          .image_source_transformation = display::CoordinateTransformation::kIdentity,
      }},
  };

  display::ConfigCheckResult res = display_engine_->CheckConfiguration(
      display::DisplayId(1), display::ModeId(1), display::ColorConversion::kIdentity, kLayers);
  EXPECT_EQ(display::ConfigCheckResult::kUnsupportedConfig, res);
}

TEST_F(GoldfishDisplayEngineTest, CheckConfigLayerTransform) {
  static constexpr display::Rectangle kDisplayArea = {
      {.x = 0, .y = 0, .width = kDisplayWidthPx, .height = kDisplayHeightPx}};
  static constexpr display::DriverLayer kLayers[] = {
      {{
          .display_destination = kDisplayArea,
          .image_source = kDisplayArea,
          .image_id = display::kInvalidDriverImageId,
          .image_metadata =
              display::ImageMetadata({.width = kDisplayWidthPx,
                                      .height = kDisplayHeightPx,
                                      .tiling_type = display::ImageTilingType::kLinear}),
          .fallback_color = display::Color({.format = display::PixelFormat::kB8G8R8A8,
                                            .bytes = {{0x41, 0x42, 0x43, 0x44, 0, 0, 0, 0}}}),
          .alpha_mode = display::AlphaMode::kDisable,
          .image_source_transformation = display::CoordinateTransformation::kReflectX,
      }},
  };

  display::ConfigCheckResult res = display_engine_->CheckConfiguration(
      display::DisplayId(1), display::ModeId(1), display::ColorConversion::kIdentity, kLayers);
  EXPECT_EQ(display::ConfigCheckResult::kUnsupportedConfig, res);
}

TEST_F(GoldfishDisplayEngineTest, CheckConfigLayerColorConversionSupported) {
  static constexpr display::Rectangle kDisplayArea = {
      {.x = 0, .y = 0, .width = kDisplayWidthPx, .height = kDisplayHeightPx}};
  static constexpr display::DriverLayer kLayers[] = {
      {{
          .display_destination = kDisplayArea,
          .image_source = kDisplayArea,
          .image_id = display::kInvalidDriverImageId,
          .image_metadata =
              display::ImageMetadata({.width = kDisplayWidthPx,
                                      .height = kDisplayHeightPx,
                                      .tiling_type = display::ImageTilingType::kLinear}),
          .fallback_color = display::Color({.format = display::PixelFormat::kB8G8R8A8,
                                            .bytes = {{0x41, 0x42, 0x43, 0x44, 0, 0, 0, 0}}}),
          .alpha_mode = display::AlphaMode::kDisable,
          .image_source_transformation = display::CoordinateTransformation::kReflectX,
      }},
  };
  const display::ColorConversion kColorConversion = {{
      .preoffsets = {0.1f, 0.2f, 0.3f},
      .coefficients =
          {
              std::array<float, 3>{1.0f, 2.0f, 3.0f},
              std::array<float, 3>{4.0f, 5.0f, 6.0f},
              std::array<float, 3>{7.0f, 8.0f, 9.0f},
          },
      .postoffsets = {0.4f, 0.5f, 0.6f},
  }};

  display::ConfigCheckResult res = display_engine_->CheckConfiguration(
      display::DisplayId(1), display::ModeId(1), kColorConversion, kLayers);
  EXPECT_EQ(display::ConfigCheckResult::kUnsupportedConfig, res);
  // TODO(https://fxbug.dev/435550805): For now, driver will pretend it
  // supports color conversion.
}

TEST_F(GoldfishDisplayEngineTest, ImportBufferCollection) {
  zx::result token1_endpoints = fidl::CreateEndpoints<fuchsia_sysmem2::BufferCollectionToken>();
  ASSERT_TRUE(token1_endpoints.is_ok());
  zx::result token2_endpoints = fidl::CreateEndpoints<fuchsia_sysmem2::BufferCollectionToken>();
  ASSERT_TRUE(token2_endpoints.is_ok());

  // Test ImportBufferCollection().
  static constexpr display::DriverBufferCollectionId kValidCollectionId(1);
  EXPECT_OK(display_engine_->ImportBufferCollection(kValidCollectionId,
                                                    std::move(token1_endpoints->client)));

  // `collection_id` must be unused.
  EXPECT_EQ(display_engine_
                ->ImportBufferCollection(kValidCollectionId, std::move(token2_endpoints->client))
                .status_value(),
            ZX_ERR_ALREADY_EXISTS);

  // Test ReleaseBufferCollection().
  static constexpr display::DriverBufferCollectionId kInvalidCollectionId(2);
  EXPECT_EQ(display_engine_->ReleaseBufferCollection(kInvalidCollectionId).status_value(),
            ZX_ERR_NOT_FOUND);
  EXPECT_OK(display_engine_->ReleaseBufferCollection(kValidCollectionId));

  loop_.Shutdown();
}

// TODO(https://fxbug.dev/42073664): Implement a fake sysmem and a fake goldfish-pipe
// driver to test importing images using ImportImage().

}  // namespace goldfish
