// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/coordinator/engine-driver-client-fidl.h"

#include <fidl/fuchsia.hardware.display.types/cpp/wire.h>
#include <lib/driver/testing/cpp/driver_runtime.h>
#include <lib/driver/testing/cpp/scoped_global_logger.h>
#include <lib/zx/result.h>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/graphics/display/drivers/coordinator/driver-display-config.h"
#include "src/graphics/display/drivers/coordinator/testing/mock-engine-fidl.h"
#include "src/graphics/display/lib/api-types/cpp/color-conversion.h"
#include "src/graphics/display/lib/api-types/cpp/display-id.h"
#include "src/graphics/display/lib/api-types/cpp/driver-layer.h"
#include "src/graphics/display/lib/api-types/cpp/mode-id.h"
#include "src/lib/testing/predicates/status.h"

namespace display_coordinator {

namespace {

class EngineDriverClientFidlTest : public ::testing::Test {
 public:
  void TearDown() override { mock_.CheckAllCallsReplayed(); }

  fdf::ClientEnd<fuchsia_hardware_display_engine::Engine> Connect() {
    auto [client, server] = fdf::Endpoints<fuchsia_hardware_display_engine::Engine>::Create();
    fdf::BindServer(dispatcher_->get(), std::move(server), &mock_);
    return std::move(client);
  }

 protected:
  fdf_testing::ScopedGlobalLogger logger_;
  fdf_testing::DriverRuntime driver_runtime_;
  fdf::UnownedSynchronizedDispatcher dispatcher_{driver_runtime_.StartBackgroundDispatcher()};

  testing::MockEngineFidl mock_;
  EngineDriverClientFidl fidl_client_{Connect()};
};

TEST_F(EngineDriverClientFidlTest, CompleteCoordinatorConnection) {
  constexpr display::EngineInfo kEngineInfo({
      .max_layer_count = 3,
      .max_connected_display_count = 1,
      .is_capture_supported = true,
  });

  mock_.ExpectCompleteCoordinatorConnection(
      [kEngineInfo](
          fuchsia_hardware_display_engine::wire::EngineCompleteCoordinatorConnectionRequest*
              request,
          fdf::Arena& arena,
          testing::MockEngineFidl::CompleteCoordinatorConnectionCompleter::Sync& completer) {
        completer.buffer(arena).Reply(kEngineInfo.ToFidl());
      });

  auto [listener_client, listener_server] =
      fdf::Endpoints<fuchsia_hardware_display_engine::EngineListener>::Create();
  display::EngineInfo result =
      fidl_client_.CompleteCoordinatorConnection(std::move(listener_client));
  EXPECT_EQ(result.max_layer_count(), kEngineInfo.max_layer_count());
  EXPECT_EQ(result.max_connected_display_count(), kEngineInfo.max_connected_display_count());
  EXPECT_EQ(result.is_capture_supported(), kEngineInfo.is_capture_supported());
}

TEST_F(EngineDriverClientFidlTest, UnsetListener) {
  mock_.ExpectUnsetListener(
      [](fdf::Arena& arena, testing::MockEngineFidl::UnsetListenerCompleter::Sync& completer) {});

  fidl_client_.UnsetListener();
}

TEST_F(EngineDriverClientFidlTest, ImportBufferCollection) {
  static constexpr display::DriverBufferCollectionId kCollectionId(1);
  auto [token_client, token_server] =
      fidl::Endpoints<fuchsia_sysmem2::BufferCollectionToken>::Create();

  mock_.ExpectImportBufferCollection(
      [](fuchsia_hardware_display_engine::wire::EngineImportBufferCollectionRequest* request,
         fdf::Arena& arena,
         testing::MockEngineFidl::ImportBufferCollectionCompleter::Sync& completer) {
        EXPECT_EQ(display::DriverBufferCollectionId(request->buffer_collection_id), kCollectionId);
        completer.buffer(arena).ReplySuccess();
      });

  EXPECT_OK(fidl_client_.ImportBufferCollection(kCollectionId, std::move(token_client)));
}

TEST_F(EngineDriverClientFidlTest, ReleaseBufferCollectionSuccess) {
  static constexpr display::DriverBufferCollectionId kCollectionId(1);

  mock_.ExpectReleaseBufferCollection(
      [](fuchsia_hardware_display_engine::wire::EngineReleaseBufferCollectionRequest* request,
         fdf::Arena& arena,
         testing::MockEngineFidl::ReleaseBufferCollectionCompleter::Sync& completer) {
        EXPECT_EQ(display::DriverBufferCollectionId(request->buffer_collection_id), kCollectionId);
        completer.buffer(arena).ReplySuccess();
      });

  EXPECT_OK(fidl_client_.ReleaseBufferCollection(kCollectionId));
}

TEST_F(EngineDriverClientFidlTest, ReleaseBufferCollectionFailure) {
  static constexpr display::DriverBufferCollectionId kCollectionId(1);

  mock_.ExpectReleaseBufferCollection(
      [](fuchsia_hardware_display_engine::wire::EngineReleaseBufferCollectionRequest* request,
         fdf::Arena& arena,
         testing::MockEngineFidl::ReleaseBufferCollectionCompleter::Sync& completer) {
        EXPECT_EQ(display::DriverBufferCollectionId(request->buffer_collection_id), kCollectionId);
        completer.buffer(arena).ReplyError(ZX_ERR_INVALID_ARGS);
      });

  zx::result<> result = fidl_client_.ReleaseBufferCollection(kCollectionId);
  EXPECT_STATUS(result, zx::error(ZX_ERR_INVALID_ARGS));
}

TEST_F(EngineDriverClientFidlTest, ImportImageSuccess) {
  static constexpr display::DriverImageId kImageId(1);
  static constexpr display::DriverBufferCollectionId kCollectionId(2);
  static constexpr uint32_t kIndex = 3;
  static constexpr display::ImageMetadata kMetadata = {{
      .width = 1024,
      .height = 768,
      .tiling_type = display::ImageTilingType::kLinear,
  }};

  mock_.ExpectImportImage(
      [](fuchsia_hardware_display_engine::wire::EngineImportImageRequest* request,
         fdf::Arena& arena, testing::MockEngineFidl::ImportImageCompleter::Sync& completer) {
        EXPECT_EQ(request->image_metadata.dimensions.width,
                  static_cast<uint32_t>(kMetadata.dimensions().width()));
        EXPECT_EQ(request->image_metadata.dimensions.height,
                  static_cast<uint32_t>(kMetadata.dimensions().height()));
        EXPECT_EQ(display::ImageTilingType(request->image_metadata.tiling_type),
                  kMetadata.tiling_type());
        EXPECT_EQ(display::DriverBufferCollectionId(request->buffer_collection_id), kCollectionId);
        EXPECT_EQ(request->buffer_collection_index, kIndex);
        completer.buffer(arena).ReplySuccess(kImageId.ToFidl());
      });

  zx::result<display::DriverImageId> result =
      fidl_client_.ImportImage(kMetadata, kCollectionId, kIndex);
  ASSERT_OK(result);
  EXPECT_EQ(result.value(), kImageId);
}

TEST_F(EngineDriverClientFidlTest, ImportImageFailure) {
  static constexpr display::DriverBufferCollectionId kCollectionId(2);
  static constexpr uint32_t kIndex = 3;
  static constexpr display::ImageMetadata kMetadata = {{
      .width = 1024,
      .height = 768,
      .tiling_type = display::ImageTilingType::kLinear,
  }};

  mock_.ExpectImportImage(
      [](fuchsia_hardware_display_engine::wire::EngineImportImageRequest* request,
         fdf::Arena& arena, testing::MockEngineFidl::ImportImageCompleter::Sync& completer) {
        EXPECT_EQ(request->image_metadata.dimensions.width,
                  static_cast<uint32_t>(kMetadata.dimensions().width()));
        EXPECT_EQ(request->image_metadata.dimensions.height,
                  static_cast<uint32_t>(kMetadata.dimensions().height()));
        EXPECT_EQ(display::ImageTilingType(request->image_metadata.tiling_type),
                  kMetadata.tiling_type());
        EXPECT_EQ(display::DriverBufferCollectionId(request->buffer_collection_id), kCollectionId);
        EXPECT_EQ(request->buffer_collection_index, kIndex);
        completer.buffer(arena).ReplyError(ZX_ERR_INVALID_ARGS);
      });

  zx::result<display::DriverImageId> result =
      fidl_client_.ImportImage(kMetadata, kCollectionId, kIndex);
  EXPECT_STATUS(result, zx::error(ZX_ERR_INVALID_ARGS));
}

TEST_F(EngineDriverClientFidlTest, ImportImageForCaptureSuccess) {
  static constexpr display::DriverCaptureImageId kImageId(1);
  static constexpr display::DriverBufferCollectionId kCollectionId(2);
  static constexpr uint32_t kIndex = 3;

  mock_.ExpectImportImageForCapture(
      [](fuchsia_hardware_display_engine::wire::EngineImportImageForCaptureRequest* request,
         fdf::Arena& arena,
         testing::MockEngineFidl::ImportImageForCaptureCompleter::Sync& completer) {
        EXPECT_EQ(display::DriverBufferCollectionId(request->buffer_collection_id), kCollectionId);
        EXPECT_EQ(request->buffer_collection_index, kIndex);
        completer.buffer(arena).ReplySuccess(kImageId.ToFidl());
      });

  zx::result<display::DriverCaptureImageId> result =
      fidl_client_.ImportImageForCapture(kCollectionId, kIndex);
  ASSERT_OK(result);
  EXPECT_EQ(result.value(), kImageId);
}

TEST_F(EngineDriverClientFidlTest, ImportImageForCaptureFailure) {
  static constexpr display::DriverBufferCollectionId kCollectionId(2);
  static constexpr uint32_t kIndex = 3;

  mock_.ExpectImportImageForCapture(
      [](fuchsia_hardware_display_engine::wire::EngineImportImageForCaptureRequest* request,
         fdf::Arena& arena,
         testing::MockEngineFidl::ImportImageForCaptureCompleter::Sync& completer) {
        EXPECT_EQ(display::DriverBufferCollectionId(request->buffer_collection_id), kCollectionId);
        EXPECT_EQ(request->buffer_collection_index, kIndex);
        completer.buffer(arena).ReplyError(ZX_ERR_INVALID_ARGS);
      });

  zx::result<display::DriverCaptureImageId> result =
      fidl_client_.ImportImageForCapture(kCollectionId, kIndex);
  EXPECT_STATUS(result, zx::error(ZX_ERR_INVALID_ARGS));
}

TEST_F(EngineDriverClientFidlTest, ReleaseImage) {
  static constexpr display::DriverImageId kImageId(1);

  mock_.ExpectReleaseImage(
      [](fuchsia_hardware_display_engine::wire::EngineReleaseImageRequest* request,
         fdf::Arena& arena) { EXPECT_EQ(display::DriverImageId(request->image_id), kImageId); });

  fidl_client_.ReleaseImage(kImageId);
}

TEST_F(EngineDriverClientFidlTest, CheckConfigurationOk) {
  const display::DriverLayer kLayer0({
      .display_destination = display::Rectangle({.x = 10, .y = 20, .width = 300, .height = 400}),
      .image_source = display::Rectangle({.x = 30, .y = 40, .width = 500, .height = 600}),
      .image_id = display::DriverImageId(4242),
      .image_metadata = display::ImageMetadata(
          {.width = 700, .height = 800, .tiling_type = display::ImageTilingType::kLinear}),
      .fallback_color = display::Color({.format = display::PixelFormat::kR8G8B8A8,
                                        .bytes = {{0xff, 0, 0xff, 0xff, 0, 0, 0, 0}}}),
      .alpha_mode = display::AlphaMode::kPremultiplied,
      .alpha_coefficient = 0.25f,
      .image_source_transformation = display::CoordinateTransformation::kReflectX,
  });
  const display::DriverLayer kLayers[] = {kLayer0};
  const DriverDisplayConfig kDisplayConfig = {
      .display_id = display::DisplayId(1),
      .mode_id = display::ModeId(2),
      .color_conversion = display::ColorConversion({
          .preoffsets = {0.1f, 0.2f, 0.3f},
          .coefficients =
              {
                  std::array<float, 3>{1.0f, 2.0f, 3.0f},
                  std::array<float, 3>{4.0f, 5.0f, 6.0f},
                  std::array<float, 3>{7.0f, 8.0f, 9.0f},
              },
          .postoffsets = {0.4f, 0.5f, 0.6f},
      }),
      .layer_count = 1,
  };

  mock_.ExpectCheckConfiguration(
      [&](fuchsia_hardware_display_engine::wire::EngineCheckConfigurationRequest* request,
          fdf::Arena& arena,
          testing::MockEngineFidl::CheckConfigurationCompleter::Sync& completer) {
        EXPECT_EQ(display::DisplayId(request->display_config.display_id),
                  kDisplayConfig.display_id);

        EXPECT_EQ(request->display_config.mode_id.value, kDisplayConfig.mode_id.value());

        EXPECT_THAT(request->display_config.color_conversion.preoffsets,
                    ::testing::ElementsAre(0.1f, 0.2f, 0.3f));

        ASSERT_EQ(request->display_config.color_conversion.coefficients.size(), 3u);
        EXPECT_THAT(request->display_config.color_conversion.coefficients[0],
                    ::testing::ElementsAre(1.0f, 2.0f, 3.0f));
        EXPECT_THAT(request->display_config.color_conversion.coefficients[1],
                    ::testing::ElementsAre(4.0f, 5.0f, 6.0f));
        EXPECT_THAT(request->display_config.color_conversion.coefficients[2],
                    ::testing::ElementsAre(7.0f, 8.0f, 9.0f));

        EXPECT_THAT(request->display_config.color_conversion.postoffsets,
                    ::testing::ElementsAre(0.4f, 0.5f, 0.6f));

        ASSERT_EQ(request->display_config.layers.size(), 1u);
        EXPECT_EQ(display::DriverLayer(request->display_config.layers[0]), kLayer0);

        completer.buffer(arena).ReplySuccess();
      });

  display::ConfigCheckResult result = fidl_client_.CheckConfiguration(kDisplayConfig, kLayers);
  EXPECT_EQ(result, display::ConfigCheckResult::kOk);
}

TEST_F(EngineDriverClientFidlTest, CheckConfigurationError) {
  const display::DriverLayer kLayer0({
      .display_destination = display::Rectangle({.x = 10, .y = 20, .width = 300, .height = 400}),
      .image_source = display::Rectangle({.x = 30, .y = 40, .width = 500, .height = 600}),
      .image_id = display::DriverImageId(4242),
      .image_metadata = display::ImageMetadata(
          {.width = 700, .height = 800, .tiling_type = display::ImageTilingType::kLinear}),
      .fallback_color = display::Color({.format = display::PixelFormat::kR8G8B8A8,
                                        .bytes = {{0xff, 0, 0xff, 0xff, 0, 0, 0, 0}}}),
      .alpha_mode = display::AlphaMode::kPremultiplied,
      .alpha_coefficient = 0.25f,
      .image_source_transformation = display::CoordinateTransformation::kReflectX,
  });
  const display::DriverLayer kLayers[] = {kLayer0};
  const DriverDisplayConfig kDisplayConfig = {
      .display_id = display::DisplayId(1),
      .mode_id = display::ModeId(2),
      .color_conversion = display::ColorConversion({
          .preoffsets = {0.1f, 0.2f, 0.3f},
          .coefficients =
              {
                  std::array<float, 3>{1.0f, 2.0f, 3.0f},
                  std::array<float, 3>{4.0f, 5.0f, 6.0f},
                  std::array<float, 3>{7.0f, 8.0f, 9.0f},
              },
          .postoffsets = {0.4f, 0.5f, 0.6f},
      }),
      .layer_count = 1,
  };

  mock_.ExpectCheckConfiguration(
      [&](fuchsia_hardware_display_engine::wire::EngineCheckConfigurationRequest* request,
          fdf::Arena& arena,
          testing::MockEngineFidl::CheckConfigurationCompleter::Sync& completer) {
        EXPECT_EQ(display::DisplayId(request->display_config.display_id),
                  kDisplayConfig.display_id);

        EXPECT_EQ(request->display_config.mode_id.value, kDisplayConfig.mode_id.value());
        EXPECT_THAT(request->display_config.color_conversion.preoffsets,
                    ::testing::ElementsAre(0.1f, 0.2f, 0.3f));

        ASSERT_EQ(request->display_config.color_conversion.coefficients.size(), 3u);
        EXPECT_THAT(request->display_config.color_conversion.coefficients[0],
                    ::testing::ElementsAre(1.0f, 2.0f, 3.0f));
        EXPECT_THAT(request->display_config.color_conversion.coefficients[1],
                    ::testing::ElementsAre(4.0f, 5.0f, 6.0f));
        EXPECT_THAT(request->display_config.color_conversion.coefficients[2],
                    ::testing::ElementsAre(7.0f, 8.0f, 9.0f));

        EXPECT_THAT(request->display_config.color_conversion.postoffsets,
                    ::testing::ElementsAre(0.4f, 0.5f, 0.6f));

        ASSERT_EQ(request->display_config.layers.size(), 1u);
        EXPECT_EQ(display::DriverLayer(request->display_config.layers[0]), kLayer0);

        completer.buffer(arena).ReplyError(
            fuchsia_hardware_display_types::ConfigResult::kUnsupportedConfig);
      });

  display::ConfigCheckResult result = fidl_client_.CheckConfiguration(kDisplayConfig, kLayers);
  EXPECT_EQ(result, display::ConfigCheckResult::kUnsupportedConfig);
}

TEST_F(EngineDriverClientFidlTest, SubmitConfiguration) {
  const display::DriverLayer kLayer0({
      .display_destination = display::Rectangle({.x = 10, .y = 20, .width = 300, .height = 400}),
      .image_source = display::Rectangle({.x = 30, .y = 40, .width = 500, .height = 600}),
      .image_id = display::DriverImageId(4242),
      .image_metadata = display::ImageMetadata(
          {.width = 700, .height = 800, .tiling_type = display::ImageTilingType::kLinear}),
      .fallback_color = display::Color({.format = display::PixelFormat::kR8G8B8A8,
                                        .bytes = {{0xff, 0, 0xff, 0xff, 0, 0, 0, 0}}}),
      .alpha_mode = display::AlphaMode::kPremultiplied,
      .alpha_coefficient = 0.25f,
      .image_source_transformation = display::CoordinateTransformation::kReflectX,
  });
  const display::DriverLayer kLayers[] = {kLayer0};
  const DriverDisplayConfig kDisplayConfig = {
      .display_id = display::DisplayId(1),
      .mode_id = display::ModeId(2),
      .color_conversion = display::ColorConversion({
          .preoffsets = {0.1f, 0.2f, 0.3f},
          .coefficients =
              {
                  std::array<float, 3>{1.0f, 2.0f, 3.0f},
                  std::array<float, 3>{4.0f, 5.0f, 6.0f},
                  std::array<float, 3>{7.0f, 8.0f, 9.0f},
              },
          .postoffsets = {0.4f, 0.5f, 0.6f},
      }),
      .layer_count = 1,
  };
  constexpr display::DriverConfigStamp kConfigStamp(42);

  mock_.ExpectSubmitConfiguration(
      [&](fuchsia_hardware_display_engine::wire::EngineSubmitConfigurationRequest* request,
          fdf::Arena& arena,
          testing::MockEngineFidl::SubmitConfigurationCompleter::Sync& completer) {
        EXPECT_EQ(display::DisplayId(request->display_config.display_id),
                  kDisplayConfig.display_id);

        EXPECT_EQ(request->display_config.mode_id.value, kDisplayConfig.mode_id.value());

        EXPECT_THAT(request->display_config.color_conversion.preoffsets,
                    ::testing::ElementsAre(0.1f, 0.2f, 0.3f));

        ASSERT_EQ(request->display_config.color_conversion.coefficients.size(), 3u);
        EXPECT_THAT(request->display_config.color_conversion.coefficients[0],
                    ::testing::ElementsAre(1.0f, 2.0f, 3.0f));
        EXPECT_THAT(request->display_config.color_conversion.coefficients[1],
                    ::testing::ElementsAre(4.0f, 5.0f, 6.0f));
        EXPECT_THAT(request->display_config.color_conversion.coefficients[2],
                    ::testing::ElementsAre(7.0f, 8.0f, 9.0f));

        EXPECT_THAT(request->display_config.color_conversion.postoffsets,
                    ::testing::ElementsAre(0.4f, 0.5f, 0.6f));

        ASSERT_EQ(request->display_config.layers.size(), 1u);
        EXPECT_EQ(display::DriverLayer(request->display_config.layers[0]), kLayer0);
        EXPECT_EQ(display::DriverConfigStamp(request->config_stamp), kConfigStamp);

        completer.buffer(arena).Reply();
      });

  fidl_client_.SubmitConfiguration(kDisplayConfig, kLayers, kConfigStamp);
}

TEST_F(EngineDriverClientFidlTest, ReleaseCapture) {
  static constexpr display::DriverCaptureImageId kImageId(1);

  mock_.ExpectReleaseCapture(
      [](fuchsia_hardware_display_engine::wire::EngineReleaseCaptureRequest* request,
         fdf::Arena& arena, testing::MockEngineFidl::ReleaseCaptureCompleter::Sync& completer) {
        EXPECT_EQ(display::DriverCaptureImageId(request->capture_image_id), kImageId);
        completer.buffer(arena).ReplySuccess();
      });

  EXPECT_OK(fidl_client_.ReleaseCapture(kImageId));
}

TEST_F(EngineDriverClientFidlTest, SetBufferCollectionConstraintsSuccess) {
  static constexpr display::DriverBufferCollectionId kCollectionId(1);
  static constexpr display::ImageBufferUsage kUsage = {{
      .tiling_type = display::ImageTilingType::kLinear,
  }};

  mock_.ExpectSetBufferCollectionConstraints(
      [](fuchsia_hardware_display_engine::wire::EngineSetBufferCollectionConstraintsRequest*
             request,
         fdf::Arena& arena,
         testing::MockEngineFidl::SetBufferCollectionConstraintsCompleter::Sync& completer) {
        EXPECT_EQ(display::DriverBufferCollectionId(request->buffer_collection_id), kCollectionId);
        EXPECT_EQ(display::ImageBufferUsage(request->usage), kUsage);
        completer.buffer(arena).ReplySuccess();
      });

  EXPECT_OK(fidl_client_.SetBufferCollectionConstraints(kUsage, kCollectionId));
}

TEST_F(EngineDriverClientFidlTest, SetBufferCollectionConstraintsFailure) {
  static constexpr display::DriverBufferCollectionId kCollectionId(1);
  static constexpr display::ImageBufferUsage kUsage = {{
      .tiling_type = display::ImageTilingType::kLinear,
  }};

  mock_.ExpectSetBufferCollectionConstraints(
      [](fuchsia_hardware_display_engine::wire::EngineSetBufferCollectionConstraintsRequest*
             request,
         fdf::Arena& arena,
         testing::MockEngineFidl::SetBufferCollectionConstraintsCompleter::Sync& completer) {
        EXPECT_EQ(display::DriverBufferCollectionId(request->buffer_collection_id), kCollectionId);
        EXPECT_EQ(display::ImageBufferUsage(request->usage), kUsage);
        completer.buffer(arena).ReplyError(ZX_ERR_INVALID_ARGS);
      });

  zx::result<> result = fidl_client_.SetBufferCollectionConstraints(kUsage, kCollectionId);
  EXPECT_STATUS(result, zx::error(ZX_ERR_INVALID_ARGS));
}

TEST_F(EngineDriverClientFidlTest, StartCaptureSuccess) {
  static constexpr display::DriverCaptureImageId kImageId(1);

  mock_.ExpectStartCapture(
      [](fuchsia_hardware_display_engine::wire::EngineStartCaptureRequest* request,
         fdf::Arena& arena, testing::MockEngineFidl::StartCaptureCompleter::Sync& completer) {
        EXPECT_EQ(display::DriverCaptureImageId(request->capture_image_id), kImageId);
        completer.buffer(arena).ReplySuccess();
      });

  EXPECT_OK(fidl_client_.StartCapture(kImageId));
}

TEST_F(EngineDriverClientFidlTest, StartCaptureFailure) {
  static constexpr display::DriverCaptureImageId kImageId(1);

  mock_.ExpectStartCapture(
      [](fuchsia_hardware_display_engine::wire::EngineStartCaptureRequest* request,
         fdf::Arena& arena, testing::MockEngineFidl::StartCaptureCompleter::Sync& completer) {
        EXPECT_EQ(display::DriverCaptureImageId(request->capture_image_id), kImageId);
        completer.buffer(arena).ReplyError(ZX_ERR_INVALID_ARGS);
      });

  zx::result<> result = fidl_client_.StartCapture(kImageId);
  EXPECT_STATUS(result, zx::error(ZX_ERR_INVALID_ARGS));
}

TEST_F(EngineDriverClientFidlTest, SetDisplayPowerModeSuccess) {
  static constexpr display::DisplayId kDisplayId(1);

  mock_.ExpectSetDisplayPowerMode(
      [](fuchsia_hardware_display_engine::wire::EngineSetDisplayPowerModeRequest* request,
         fdf::Arena& arena,
         testing::MockEngineFidl::SetDisplayPowerModeCompleter::Sync& completer) {
        EXPECT_EQ(display::DisplayId(request->display_id), kDisplayId);
        EXPECT_EQ(request->power_mode, fuchsia_hardware_display_types::wire::PowerMode::kOn);
        completer.buffer(arena).ReplySuccess();
      });

  zx::result<> result = fidl_client_.SetDisplayPowerMode(kDisplayId, display::PowerMode::kOn);
  EXPECT_OK(result);
}

TEST_F(EngineDriverClientFidlTest, SetDisplayPowerModeFailure) {
  static constexpr display::DisplayId kDisplayId(1);

  mock_.ExpectSetDisplayPowerMode(
      [](fuchsia_hardware_display_engine::wire::EngineSetDisplayPowerModeRequest* request,
         fdf::Arena& arena,
         testing::MockEngineFidl::SetDisplayPowerModeCompleter::Sync& completer) {
        EXPECT_EQ(display::DisplayId(request->display_id), kDisplayId);
        EXPECT_EQ(request->power_mode, fuchsia_hardware_display_types::wire::PowerMode::kOn);
        completer.buffer(arena).ReplyError(ZX_ERR_INVALID_ARGS);
      });

  zx::result<> result = fidl_client_.SetDisplayPowerMode(kDisplayId, display::PowerMode::kOn);
  EXPECT_STATUS(result, zx::error(ZX_ERR_INVALID_ARGS));
}

TEST_F(EngineDriverClientFidlTest, SetMinimumRgbSuccess) {
  static constexpr uint8_t kMinimumRgb = 128;

  mock_.ExpectSetMinimumRgb(
      [](fuchsia_hardware_display_engine::wire::EngineSetMinimumRgbRequest* request,
         fdf::Arena& arena, testing::MockEngineFidl::SetMinimumRgbCompleter::Sync& completer) {
        EXPECT_EQ(request->minimum_rgb, kMinimumRgb);
        completer.buffer(arena).ReplySuccess();
      });

  EXPECT_OK(fidl_client_.SetMinimumRgb(kMinimumRgb));
}

TEST_F(EngineDriverClientFidlTest, SetMinimumRgbFailure) {
  static constexpr uint8_t kMinimumRgb = 128;

  mock_.ExpectSetMinimumRgb(
      [](fuchsia_hardware_display_engine::wire::EngineSetMinimumRgbRequest* request,
         fdf::Arena& arena, testing::MockEngineFidl::SetMinimumRgbCompleter::Sync& completer) {
        EXPECT_EQ(request->minimum_rgb, kMinimumRgb);
        completer.buffer(arena).ReplyError(ZX_ERR_INVALID_ARGS);
      });

  zx::result<> result = fidl_client_.SetMinimumRgb(kMinimumRgb);
  EXPECT_STATUS(result, zx::error(ZX_ERR_INVALID_ARGS));
}

}  // namespace

}  // namespace display_coordinator
