// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/lib/api-protocols/cpp/display-engine-fidl-adapter.h"

#include <fidl/fuchsia.hardware.display.engine/cpp/driver/wire.h>
#include <fidl/fuchsia.hardware.display.engine/cpp/wire.h>
#include <fidl/fuchsia.sysmem2/cpp/wire.h>
#include <lib/async/cpp/task.h>
#include <lib/driver/testing/cpp/driver_runtime.h>
#include <lib/driver/testing/cpp/scoped_global_logger.h>
#include <lib/fdf/dispatcher.h>
#include <lib/fit/result.h>
#include <lib/sync/completion.h>
#include <lib/zx/result.h>
#include <zircon/assert.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <cstddef>
#include <cstdint>
#include <initializer_list>
#include <optional>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/graphics/display/lib/api-protocols/cpp/mock-display-engine.h"
#include "src/graphics/display/lib/api-types/cpp/color-conversion.h"
#include "src/graphics/display/lib/api-types/cpp/config-check-result.h"
#include "src/graphics/display/lib/api-types/cpp/coordinate-transformation.h"
#include "src/graphics/display/lib/api-types/cpp/display-id.h"
#include "src/graphics/display/lib/api-types/cpp/driver-buffer-collection-id.h"
#include "src/graphics/display/lib/api-types/cpp/driver-capture-image-id.h"
#include "src/graphics/display/lib/api-types/cpp/driver-config-stamp.h"
#include "src/graphics/display/lib/api-types/cpp/driver-image-id.h"
#include "src/graphics/display/lib/api-types/cpp/driver-layer.h"
#include "src/graphics/display/lib/api-types/cpp/engine-info.h"
#include "src/graphics/display/lib/api-types/cpp/image-buffer-usage.h"
#include "src/graphics/display/lib/api-types/cpp/image-metadata.h"
#include "src/graphics/display/lib/api-types/cpp/image-tiling-type.h"
#include "src/graphics/display/lib/api-types/cpp/mode-id.h"
#include "src/graphics/display/lib/api-types/cpp/rectangle.h"
#include "src/lib/testing/predicates/status.h"

namespace display {

namespace {

class DisplayEngineFidlAdapterTest : public ::testing::Test {
 public:
  void SetUp() override {
    auto [fidl_client, fidl_server] =
        fdf::Endpoints<fuchsia_hardware_display_engine::Engine>::Create();
    fidl_client_ =
        fdf::WireSyncClient<fuchsia_hardware_display_engine::Engine>(std::move(fidl_client));

    ASSERT_OK(async::PostTask(
        dispatcher_->async_dispatcher(), [this, fidl_server = std::move(fidl_server)]() mutable {
          engine_events_fidl_.emplace();
          fidl_adapter_.emplace(&mock_, &engine_events_fidl_.value());
          fidl::ProtocolHandler<fuchsia_hardware_display_engine::Engine> fidl_handler =
              fidl_adapter_->CreateHandler(*(dispatcher_->get()));
          fidl_handler(std::move(fidl_server));
        }));
  }

  void TearDown() override {
    driver_runtime_.ShutdownBackgroundDispatcher(dispatcher_->get(), [this] {
      fidl_adapter_.reset();
      engine_events_fidl_.reset();
    });
    mock_.CheckAllCallsReplayed();
  }

 protected:
  fdf_testing::ScopedGlobalLogger logger_;
  fdf_testing::DriverRuntime driver_runtime_;
  fdf::UnownedSynchronizedDispatcher dispatcher_{driver_runtime_.StartBackgroundDispatcher()};

  display::testing::MockDisplayEngine mock_;
  fdf::WireSyncClient<fuchsia_hardware_display_engine::Engine> fidl_client_;

  // Must be created and destroyed on the FIDL loop.
  std::optional<display::DisplayEngineEventsFidl> engine_events_fidl_;
  std::optional<display::DisplayEngineFidlAdapter> fidl_adapter_;
};

TEST_F(DisplayEngineFidlAdapterTest, CompleteCoordinatorConnection) {
  static constexpr display::EngineInfo kEngineInfo({
      .max_layer_count = 4,
      .max_connected_display_count = 1,
      .is_capture_supported = false,
  });

  mock_.ExpectCompleteCoordinatorConnection([&] { return kEngineInfo; });

  auto [listener_client, listener_server] =
      fdf::Endpoints<fuchsia_hardware_display_engine::EngineListener>::Create();

  fdf::Arena arena('TEST');
  fdf::WireUnownedResult<fuchsia_hardware_display_engine::Engine::CompleteCoordinatorConnection>
      fidl_transport_result =
          fidl_client_.buffer(arena)->CompleteCoordinatorConnection(std::move(listener_client));
  ASSERT_TRUE(fidl_transport_result.ok()) << fidl_transport_result.FormatDescription();

  fuchsia_hardware_display_engine::wire::EngineCompleteCoordinatorConnectionResponse&
      fidl_domain_result = fidl_transport_result.value();
  EXPECT_EQ(kEngineInfo, display::EngineInfo::From(fidl_domain_result.engine_info));
}

TEST_F(DisplayEngineFidlAdapterTest, ImportBufferCollectionSuccess) {
  auto [buffer_collection_token_client, buffer_collection_token_server] =
      fidl::Endpoints<fuchsia_sysmem2::BufferCollectionToken>::Create();
  static constexpr display::DriverBufferCollectionId kBufferCollectionId(42);

  const zx_handle_t buffer_collection_token_client_handle =
      buffer_collection_token_client.handle()->get();

  mock_.ExpectImportBufferCollection(
      [&](display::DriverBufferCollectionId buffer_collection_id,
          fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> buffer_collection_token) {
        EXPECT_EQ(kBufferCollectionId, buffer_collection_id);
        EXPECT_EQ(buffer_collection_token_client_handle, buffer_collection_token.handle()->get());
        return zx::ok();
      });

  fdf::Arena arena('TEST');
  fdf::WireUnownedResult<fuchsia_hardware_display_engine::Engine::ImportBufferCollection>
      fidl_transport_result = fidl_client_.buffer(arena)->ImportBufferCollection(
          kBufferCollectionId.ToFidl(), std::move(buffer_collection_token_client));
  ASSERT_TRUE(fidl_transport_result.ok()) << fidl_transport_result.FormatDescription();

  fit::result<zx_status_t>& fidl_domain_result = fidl_transport_result.value();
  EXPECT_OK(fidl_domain_result);
}

TEST_F(DisplayEngineFidlAdapterTest, ImportBufferCollectionEngineError) {
  auto [buffer_collection_token_client, buffer_collection_token_server] =
      fidl::Endpoints<fuchsia_sysmem2::BufferCollectionToken>::Create();
  static constexpr display::DriverBufferCollectionId kBufferCollectionId(42);

  mock_.ExpectImportBufferCollection(
      [&](display::DriverBufferCollectionId buffer_collection_id,
          fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> buffer_collection_token) {
        return zx::error(ZX_ERR_INTERNAL);
      });

  fdf::Arena arena('TEST');
  fdf::WireUnownedResult<fuchsia_hardware_display_engine::Engine::ImportBufferCollection>
      fidl_transport_result = fidl_client_.buffer(arena)->ImportBufferCollection(
          kBufferCollectionId.ToFidl(), std::move(buffer_collection_token_client));
  ASSERT_TRUE(fidl_transport_result.ok()) << fidl_transport_result.FormatDescription();

  fit::result<zx_status_t>& fidl_domain_result = fidl_transport_result.value();
  EXPECT_STATUS(zx::error(ZX_ERR_INTERNAL), fidl_domain_result);
}

TEST_F(DisplayEngineFidlAdapterTest, ReleaseBufferCollectionSuccess) {
  static constexpr display::DriverBufferCollectionId kBufferCollectionId(42);

  mock_.ExpectReleaseBufferCollection([&](display::DriverBufferCollectionId buffer_collection_id) {
    EXPECT_EQ(kBufferCollectionId, buffer_collection_id);
    return zx::ok();
  });

  fdf::Arena arena('TEST');
  fdf::WireUnownedResult<fuchsia_hardware_display_engine::Engine::ReleaseBufferCollection>
      fidl_transport_result =
          fidl_client_.buffer(arena)->ReleaseBufferCollection(kBufferCollectionId.ToFidl());
  ASSERT_TRUE(fidl_transport_result.ok()) << fidl_transport_result.FormatDescription();

  fit::result<zx_status_t>& fidl_domain_result = fidl_transport_result.value();
  EXPECT_OK(fidl_domain_result);
}

TEST_F(DisplayEngineFidlAdapterTest, ReleaseBufferCollectionEngineError) {
  static constexpr display::DriverBufferCollectionId kBufferCollectionId(42);

  mock_.ExpectReleaseBufferCollection([&](display::DriverBufferCollectionId buffer_collection_id) {
    return zx::error(ZX_ERR_INTERNAL);
  });

  fdf::Arena arena('TEST');
  fdf::WireUnownedResult<fuchsia_hardware_display_engine::Engine::ReleaseBufferCollection>
      fidl_transport_result =
          fidl_client_.buffer(arena)->ReleaseBufferCollection(kBufferCollectionId.ToFidl());
  ASSERT_TRUE(fidl_transport_result.ok()) << fidl_transport_result.FormatDescription();

  fit::result<zx_status_t>& fidl_domain_result = fidl_transport_result.value();
  EXPECT_STATUS(zx::error(ZX_ERR_INTERNAL), fidl_domain_result);
}

TEST_F(DisplayEngineFidlAdapterTest, ImportImageSuccess) {
  static constexpr display::ImageMetadata kImageMetadata(
      {.width = 640, .height = 480, .tiling_type = display::ImageTilingType::kLinear});
  static constexpr display::DriverBufferCollectionId kBufferCollectionId(42);
  static constexpr uint32_t kBufferIndex = 4242;
  static constexpr display::DriverImageId kImageId(84);

  mock_.ExpectImportImage([&](const display::ImageMetadata& image_metadata,
                              display::DriverBufferCollectionId buffer_collection_id,
                              uint32_t buffer_index) {
    EXPECT_EQ(kImageMetadata, image_metadata);
    EXPECT_EQ(kBufferCollectionId, buffer_collection_id);
    EXPECT_EQ(kBufferIndex, buffer_index);
    return zx::ok(kImageId);
  });

  fdf::Arena arena('TEST');
  fdf::WireUnownedResult<fuchsia_hardware_display_engine::Engine::ImportImage>
      fidl_transport_result = fidl_client_.buffer(arena)->ImportImage(
          kImageMetadata.ToFidl(), kBufferCollectionId.ToFidl(), kBufferIndex);
  ASSERT_TRUE(fidl_transport_result.ok()) << fidl_transport_result.FormatDescription();

  fit::result<zx_status_t, fuchsia_hardware_display_engine::wire::EngineImportImageResponse*>&
      fidl_domain_result = fidl_transport_result.value();
  ASSERT_TRUE(fidl_domain_result.is_ok()) << fidl_domain_result.error_value();
  EXPECT_EQ(kImageId, display::DriverImageId(fidl_domain_result.value()->image_id));
}

TEST_F(DisplayEngineFidlAdapterTest, ImportImageEngineError) {
  static constexpr display::ImageMetadata kImageMetadata(
      {.width = 640, .height = 480, .tiling_type = display::ImageTilingType::kLinear});
  static constexpr display::DriverBufferCollectionId kBufferCollectionId(42);
  static constexpr uint32_t kBufferIndex = 4242;

  mock_.ExpectImportImage([&](const display::ImageMetadata& image_metadata,
                              display::DriverBufferCollectionId buffer_collection_id,
                              uint32_t buffer_index) { return zx::error(ZX_ERR_INTERNAL); });

  fdf::Arena arena('TEST');
  fdf::WireUnownedResult<fuchsia_hardware_display_engine::Engine::ImportImage>
      fidl_transport_result = fidl_client_.buffer(arena)->ImportImage(
          kImageMetadata.ToFidl(), kBufferCollectionId.ToFidl(), kBufferIndex);
  ASSERT_TRUE(fidl_transport_result.ok()) << fidl_transport_result.FormatDescription();

  fit::result<zx_status_t, fuchsia_hardware_display_engine::wire::EngineImportImageResponse*>&
      fidl_domain_result = fidl_transport_result.value();
  ASSERT_TRUE(fidl_domain_result.is_error());
  EXPECT_EQ(fidl_domain_result.error_value(), ZX_ERR_INTERNAL);
}

TEST_F(DisplayEngineFidlAdapterTest, ImportImageForCaptureSuccess) {
  static constexpr display::DriverBufferCollectionId kBufferCollectionId(42);
  static constexpr uint32_t kBufferIndex = 4242;
  static constexpr display::DriverCaptureImageId kCaptureImageId(84);

  mock_.ExpectImportImageForCapture(
      [&](display::DriverBufferCollectionId buffer_collection_id, uint32_t buffer_index) {
        EXPECT_EQ(kBufferCollectionId, buffer_collection_id);
        EXPECT_EQ(kBufferIndex, buffer_index);
        return zx::ok(kCaptureImageId);
      });

  fdf::Arena arena('TEST');
  fdf::WireUnownedResult<fuchsia_hardware_display_engine::Engine::ImportImageForCapture>
      fidl_transport_result = fidl_client_.buffer(arena)->ImportImageForCapture(
          kBufferCollectionId.ToFidl(), kBufferIndex);
  ASSERT_TRUE(fidl_transport_result.ok()) << fidl_transport_result.FormatDescription();

  fit::result<zx_status_t,
              fuchsia_hardware_display_engine::wire::EngineImportImageForCaptureResponse*>&
      fidl_domain_result = fidl_transport_result.value();
  ASSERT_TRUE(fidl_domain_result.is_ok()) << fidl_domain_result.error_value();
  EXPECT_EQ(kCaptureImageId,
            display::DriverCaptureImageId(fidl_domain_result.value()->capture_image_id));
}

TEST_F(DisplayEngineFidlAdapterTest, ImportImageForCaptureEngineError) {
  static constexpr display::DriverBufferCollectionId kBufferCollectionId(42);
  static constexpr uint32_t kBufferIndex = 4242;

  mock_.ExpectImportImageForCapture(
      [&](display::DriverBufferCollectionId buffer_collection_id, uint32_t buffer_index) {
        return zx::error(ZX_ERR_INTERNAL);
      });

  fdf::Arena arena('TEST');
  fdf::WireUnownedResult<fuchsia_hardware_display_engine::Engine::ImportImageForCapture>
      fidl_transport_result = fidl_client_.buffer(arena)->ImportImageForCapture(
          kBufferCollectionId.ToFidl(), kBufferIndex);
  ASSERT_TRUE(fidl_transport_result.ok()) << fidl_transport_result.FormatDescription();

  fit::result<zx_status_t,
              fuchsia_hardware_display_engine::wire::EngineImportImageForCaptureResponse*>&
      fidl_domain_result = fidl_transport_result.value();
  ASSERT_TRUE(fidl_domain_result.is_error());
  EXPECT_EQ(fidl_domain_result.error_value(), ZX_ERR_INTERNAL);
}

TEST_F(DisplayEngineFidlAdapterTest, ReleaseImageSuccess) {
  static constexpr display::DriverImageId kImageId(42);

  libsync::Completion release_image_called;
  mock_.ExpectReleaseImage([&](display::DriverImageId image_id) {
    EXPECT_EQ(kImageId, image_id);
    release_image_called.Signal();
  });

  fdf::Arena arena('TEST');
  fidl::OneWayStatus fidl_transport_status =
      fidl_client_.buffer(arena)->ReleaseImage(kImageId.ToFidl());
  ASSERT_TRUE(fidl_transport_status.ok()) << fidl_transport_status.FormatDescription();

  // One-way calls complete before the FIDL call is processed on the other end.
  release_image_called.Wait();
}

// Valid layer with properties aimed at testing API translation layers.
//
// The returned layer's properties are all different numbers. This is intended to
// help catch logic errors in API translation layers, such as swapping dimensions
// (width vs height) or size and position.
//
// `seed` is a small integer that results in small variations in the layer
// properties. This is intended to help catch logic errors in accessing and
// converting layers stored in arrays.
constexpr display::DriverLayer CreateValidLayerWithSeed(int seed) {
  const uint8_t color_blue = 0x40 + static_cast<uint8_t>(seed % 16);
  const uint8_t color_green = 0x50 + static_cast<uint8_t>((seed / 16) % 16);
  const uint8_t color_red = 0x60 + static_cast<uint8_t>(seed % 16);
  const uint8_t color_alpha = 0x70 + static_cast<uint8_t>((seed / 16) % 16);

  return display::DriverLayer({
      .display_destination = display::Rectangle(
          {.x = 100 + seed, .y = 200 + seed, .width = 300 + seed, .height = 400 + seed}),
      .image_source = display::Rectangle(
          {.x = 500 + seed, .y = 600 + seed, .width = 700 + seed, .height = 800 + seed}),
      .image_id = display::DriverImageId(8000 + seed),
      .image_metadata = display::ImageMetadata({.width = 2000 + seed,
                                                .height = 1000 + seed,
                                                .tiling_type = display::ImageTilingType::kLinear}),
      .fallback_color = display::Color(
          {.format = display::PixelFormat::kB8G8R8A8,
           .bytes = std::initializer_list<uint8_t>{color_blue, color_green, color_red, color_alpha,
                                                   0, 0, 0, 0}}),
      .image_source_transformation = display::CoordinateTransformation::kIdentity,
  });
}

TEST_F(DisplayEngineFidlAdapterTest, CheckConfigurationSingleLayerSuccess) {
  static constexpr display::DisplayId kDisplayId(42);
  static constexpr display::ModeId kModeId(24);
  const display::ColorConversion kColorConversion = {
      {
          .preoffsets = {1.0f, 2.0f, 3.0f},
          .coefficients =
              {
                  std::array<float, 3>{4.0f, 5.0f, 6.0f},
                  std::array<float, 3>{7.0f, 8.0f, 9.0f},
                  std::array<float, 3>{10.0f, 11.0f, 12.0f},
              },
          .postoffsets = {13.0f, 14.0f, 15.0f},
      },
  };
  static constexpr std::array<display::DriverLayer, 1> kLayers = {CreateValidLayerWithSeed(0)};

  std::array<fuchsia_hardware_display_engine::wire::Layer, 1> fidl_layers = {kLayers[0].ToFidl()};
  const fuchsia_hardware_display_engine::wire::DisplayConfig fidl_display_config = {
      .display_id = kDisplayId.ToFidl(),
      .mode_id = kModeId.ToFidl(),
      .color_conversion = kColorConversion.ToFidl(),
      .layers =
          fidl::VectorView<fuchsia_hardware_display_engine::wire::Layer>::FromExternal(fidl_layers),
  };

  mock_.ExpectCheckConfiguration([&](display::DisplayId display_id, display::ModeId display_mode_id,
                                     display::ColorConversion color_conversion,
                                     std::span<const display::DriverLayer> layers) {
    EXPECT_EQ(kDisplayId, display_id);
    EXPECT_EQ(kModeId, display_mode_id);
    EXPECT_EQ(kColorConversion, color_conversion);
    EXPECT_THAT(layers, ::testing::ElementsAreArray(kLayers));
    return display::ConfigCheckResult::kOk;
  });

  fdf::Arena arena('TEST');
  fdf::WireUnownedResult<fuchsia_hardware_display_engine::Engine::CheckConfiguration>
      fidl_transport_result = fidl_client_.buffer(arena)->CheckConfiguration(fidl_display_config);
  ASSERT_TRUE(fidl_transport_result.ok()) << fidl_transport_result.FormatDescription();

  fit::result<fuchsia_hardware_display_types::wire::ConfigResult>& fidl_domain_result =
      fidl_transport_result.value();
  EXPECT_TRUE(fidl_domain_result.is_ok());
}

TEST_F(DisplayEngineFidlAdapterTest, CheckConfigurationMultiLayerSuccess) {
  static constexpr display::DisplayId kDisplayId(42);
  static constexpr display::ModeId kModeId(24);
  static constexpr std::array<display::DriverLayer, 4> kLayers = {
      CreateValidLayerWithSeed(0), CreateValidLayerWithSeed(1), CreateValidLayerWithSeed(2),
      CreateValidLayerWithSeed(3)};

  std::array<fuchsia_hardware_display_engine::wire::Layer, 4> fidl_layers = {
      kLayers[0].ToFidl(), kLayers[1].ToFidl(), kLayers[2].ToFidl(), kLayers[3].ToFidl()};
  const fuchsia_hardware_display_engine::wire::DisplayConfig fidl_display_config = {
      .display_id = kDisplayId.ToFidl(),
      .mode_id = kModeId.ToFidl(),
      .color_conversion = display::ColorConversion::kIdentity.ToFidl(),
      .layers =
          fidl::VectorView<fuchsia_hardware_display_engine::wire::Layer>::FromExternal(fidl_layers),
  };

  mock_.ExpectCheckConfiguration([&](display::DisplayId display_id, display::ModeId display_mode_id,
                                     display::ColorConversion color_conversion,
                                     std::span<const display::DriverLayer> layers) {
    EXPECT_EQ(kDisplayId, display_id);
    EXPECT_EQ(kModeId, display_mode_id);
    EXPECT_EQ(display::ColorConversion::kIdentity, color_conversion);
    EXPECT_THAT(layers, ::testing::ElementsAreArray(kLayers));
    return display::ConfigCheckResult::kOk;
  });

  fdf::Arena arena('TEST');
  fdf::WireUnownedResult<fuchsia_hardware_display_engine::Engine::CheckConfiguration>
      fidl_transport_result = fidl_client_.buffer(arena)->CheckConfiguration(fidl_display_config);
  ASSERT_TRUE(fidl_transport_result.ok()) << fidl_transport_result.FormatDescription();

  fit::result<fuchsia_hardware_display_types::wire::ConfigResult>& fidl_domain_result =
      fidl_transport_result.value();
  EXPECT_TRUE(fidl_domain_result.is_ok());
}

TEST_F(DisplayEngineFidlAdapterTest, CheckConfigurationAdapterErrorLayerCountPastLimit) {
  static constexpr display::DisplayId kDisplayId(42);
  static constexpr display::ModeId kModeId(24);
  static constexpr int kLayerCount = display::EngineInfo::kMaxAllowedMaxLayerCount + 1;

  std::vector<fuchsia_hardware_display_engine::wire::Layer> fidl_layers;
  fidl_layers.reserve(kLayerCount);
  for (int i = 0; i < kLayerCount; ++i) {
    fidl_layers.emplace_back(CreateValidLayerWithSeed(i).ToFidl());
  }
  const fuchsia_hardware_display_engine::wire::DisplayConfig fidl_display_config = {
      .display_id = kDisplayId.ToFidl(),
      .mode_id = kModeId.ToFidl(),
      .color_conversion = display::ColorConversion::kIdentity.ToFidl(),
      .layers =
          fidl::VectorView<fuchsia_hardware_display_engine::wire::Layer>::FromExternal(fidl_layers),
  };

  // The adapter is expected to reject the CheckConfiguration() call without
  // invoking the engine driver. So, the mock driver should not receive any
  // calls.

  fdf::Arena arena('TEST');
  fdf::WireUnownedResult<fuchsia_hardware_display_engine::Engine::CheckConfiguration>
      fidl_transport_result = fidl_client_.buffer(arena)->CheckConfiguration(fidl_display_config);
  ASSERT_TRUE(fidl_transport_result.ok()) << fidl_transport_result.FormatDescription();

  fit::result<fuchsia_hardware_display_types::wire::ConfigResult>& fidl_domain_result =
      fidl_transport_result.value();
  EXPECT_EQ(display::ConfigCheckResult::kUnsupportedConfig,
            display::ConfigCheckResult(fidl_domain_result.error_value()));
}

TEST_F(DisplayEngineFidlAdapterTest, CheckConfigurationEngineError) {
  static constexpr display::DisplayId kDisplayId(42);
  static constexpr display::ModeId kModeId(24);
  const display::ColorConversion kColorConversion = {
      {
          .preoffsets = {1.0f, 2.0f, 3.0f},
          .coefficients =
              {
                  std::array<float, 3>{4.0f, 5.0f, 6.0f},
                  std::array<float, 3>{7.0f, 8.0f, 9.0f},
                  std::array<float, 3>{10.0f, 11.0f, 12.0f},
              },
          .postoffsets = {13.0f, 14.0f, 15.0f},
      },
  };
  static constexpr std::array<display::DriverLayer, 1> kLayers = {CreateValidLayerWithSeed(0)};

  std::array<fuchsia_hardware_display_engine::wire::Layer, 1> fidl_layers = {kLayers[0].ToFidl()};
  const fuchsia_hardware_display_engine::wire::DisplayConfig fidl_display_config = {
      .display_id = kDisplayId.ToFidl(),
      .mode_id = kModeId.ToFidl(),
      .color_conversion = kColorConversion.ToFidl(),
      .layers =
          fidl::VectorView<fuchsia_hardware_display_engine::wire::Layer>::FromExternal(fidl_layers),
  };

  mock_.ExpectCheckConfiguration([&](display::DisplayId display_id, display::ModeId display_mode_id,
                                     display::ColorConversion color_conversion,
                                     std::span<const display::DriverLayer> layers) {
    return display::ConfigCheckResult::kUnsupportedDisplayModes;
  });

  fdf::Arena arena('TEST');
  fdf::WireUnownedResult<fuchsia_hardware_display_engine::Engine::CheckConfiguration>
      fidl_transport_result = fidl_client_.buffer(arena)->CheckConfiguration(fidl_display_config);
  ASSERT_TRUE(fidl_transport_result.ok()) << fidl_transport_result.FormatDescription();

  fit::result<fuchsia_hardware_display_types::wire::ConfigResult>& fidl_domain_result =
      fidl_transport_result.value();
  EXPECT_EQ(display::ConfigCheckResult::kUnsupportedDisplayModes,
            display::ConfigCheckResult(fidl_domain_result.error_value()));
}

TEST_F(DisplayEngineFidlAdapterTest, CheckConfigurationAdapterErrorModeIdInvalid) {
  static constexpr display::DisplayId kDisplayId(42);
  static constexpr std::array<display::DriverLayer, 1> kLayers = {CreateValidLayerWithSeed(0)};

  std::array<fuchsia_hardware_display_engine::wire::Layer, 1> fidl_layers = {kLayers[0].ToFidl()};
  const fuchsia_hardware_display_engine::wire::DisplayConfig fidl_display_config = {
      .display_id = kDisplayId.ToFidl(),
      .mode_id = kInvalidModeId.ToFidl(),
      .color_conversion = display::ColorConversion::kIdentity.ToFidl(),
      .layers =
          fidl::VectorView<fuchsia_hardware_display_engine::wire::Layer>::FromExternal(fidl_layers),
  };

  // The adapter is expected to reject the CheckConfiguration() call without
  // invoking the engine driver. So, the mock driver should not receive any
  // calls.

  fdf::Arena arena('TEST');
  fdf::WireUnownedResult<fuchsia_hardware_display_engine::Engine::CheckConfiguration>
      fidl_transport_result = fidl_client_.buffer(arena)->CheckConfiguration(fidl_display_config);
  ASSERT_TRUE(fidl_transport_result.ok()) << fidl_transport_result.FormatDescription();

  fit::result<fuchsia_hardware_display_types::wire::ConfigResult>& fidl_domain_result =
      fidl_transport_result.value();
  EXPECT_EQ(display::ConfigCheckResult::kInvalidConfig,
            display::ConfigCheckResult(fidl_domain_result.error_value()));
}

TEST_F(DisplayEngineFidlAdapterTest, CheckConfigurationAdapterErrorInvalidColorConversion) {
  static constexpr display::DisplayId kDisplayId(42);
  static constexpr display::ModeId kModeId(24);
  static constexpr std::array<display::DriverLayer, 1> kLayers = {CreateValidLayerWithSeed(0)};

  std::array<fuchsia_hardware_display_engine::wire::Layer, 1> fidl_layers = {kLayers[0].ToFidl()};
  const fuchsia_hardware_display_engine::wire::DisplayConfig fidl_display_config = {
      .display_id = kDisplayId.ToFidl(),
      .mode_id = kModeId.ToFidl(),
      .color_conversion =
          {
              .preoffsets = {std::nanf("1"), 2.0f, 3.0f},
              .coefficients =
                  {
                      fidl::Array<float, 3>{1.0f, 2.0f, 3.0f},
                      fidl::Array<float, 3>{4.0f, 5.0f, 6.0f},
                      fidl::Array<float, 3>{7.0f, 8.0f, 9.0f},
                  },
              .postoffsets = {1.0f, 2.0f, 3.0f},
          },
      .layers =
          fidl::VectorView<fuchsia_hardware_display_engine::wire::Layer>::FromExternal(fidl_layers),
  };

  // CheckConfiguration() is never called on the MockDisplayEngine, because the
  // color conversion check in `DisplayEngineFidlAdapter` has failed.

  fdf::Arena arena('TEST');
  fdf::WireUnownedResult<fuchsia_hardware_display_engine::Engine::CheckConfiguration>
      fidl_transport_result = fidl_client_.buffer(arena)->CheckConfiguration(fidl_display_config);
  ASSERT_TRUE(fidl_transport_result.ok()) << fidl_transport_result.FormatDescription();

  fit::result<fuchsia_hardware_display_types::wire::ConfigResult>& fidl_domain_result =
      fidl_transport_result.value();
  ASSERT_TRUE(fidl_domain_result.is_error());
  EXPECT_EQ(fidl_domain_result.error_value(),
            fuchsia_hardware_display_types::wire::ConfigResult::kInvalidConfig);
}

TEST_F(DisplayEngineFidlAdapterTest, SubmitConfigurationSingleLayer) {
  static constexpr display::DisplayId kDisplayId(42);
  static constexpr display::ModeId kModeId(24);
  const display::ColorConversion kColorConversion = {
      {
          .preoffsets = {1.0f, 2.0f, 3.0f},
          .coefficients =
              {
                  std::array<float, 3>{4.0f, 5.0f, 6.0f},
                  std::array<float, 3>{7.0f, 8.0f, 9.0f},
                  std::array<float, 3>{10.0f, 11.0f, 12.0f},
              },
          .postoffsets = {13.0f, 14.0f, 15.0f},
      },
  };
  static constexpr std::array<display::DriverLayer, 1> kLayers = {CreateValidLayerWithSeed(0)};
  static constexpr display::DriverConfigStamp kConfigStamp(4242);

  std::array<fuchsia_hardware_display_engine::wire::Layer, 1> fidl_layers = {kLayers[0].ToFidl()};
  const fuchsia_hardware_display_engine::wire::DisplayConfig fidl_display_config = {
      .display_id = kDisplayId.ToFidl(),
      .mode_id = kModeId.ToFidl(),
      .color_conversion = kColorConversion.ToFidl(),
      .layers =
          fidl::VectorView<fuchsia_hardware_display_engine::wire::Layer>::FromExternal(fidl_layers),
  };

  mock_.ExpectSubmitConfiguration(
      [&](display::DisplayId display_id, display::ModeId display_mode_id,
          display::ColorConversion color_conversion, std::span<const display::DriverLayer> layers,
          display::DriverConfigStamp config_stamp) {
        EXPECT_EQ(kDisplayId, display_id);
        EXPECT_EQ(kModeId, display_mode_id);
        EXPECT_EQ(kColorConversion, color_conversion);
        EXPECT_THAT(layers, ::testing::ElementsAreArray(kLayers));
        EXPECT_EQ(kConfigStamp, config_stamp);
      });

  fdf::Arena arena('TEST');
  fdf::WireUnownedResult<fuchsia_hardware_display_engine::Engine::SubmitConfiguration>
      fidl_transport_result = fidl_client_.buffer(arena)->SubmitConfiguration(
          fidl_display_config, kConfigStamp.ToFidl());
  ASSERT_TRUE(fidl_transport_result.ok()) << fidl_transport_result.FormatDescription();
}

TEST_F(DisplayEngineFidlAdapterTest, SubmitConfigurationMultiLayer) {
  static constexpr display::DisplayId kDisplayId(42);
  static constexpr display::ModeId kModeId(24);
  static constexpr std::array<display::DriverLayer, 4> kLayers = {
      CreateValidLayerWithSeed(0), CreateValidLayerWithSeed(1), CreateValidLayerWithSeed(2),
      CreateValidLayerWithSeed(3)};
  static constexpr display::DriverConfigStamp kConfigStamp(4242);

  std::array<fuchsia_hardware_display_engine::wire::Layer, 4> fidl_layers = {
      kLayers[0].ToFidl(), kLayers[1].ToFidl(), kLayers[2].ToFidl(), kLayers[3].ToFidl()};
  const fuchsia_hardware_display_engine::wire::DisplayConfig fidl_display_config = {
      .display_id = kDisplayId.ToFidl(),
      .mode_id = kModeId.ToFidl(),
      .color_conversion = display::ColorConversion::kIdentity.ToFidl(),
      .layers =
          fidl::VectorView<fuchsia_hardware_display_engine::wire::Layer>::FromExternal(fidl_layers),
  };

  mock_.ExpectSubmitConfiguration(
      [&](display::DisplayId display_id, display::ModeId display_mode_id,
          display::ColorConversion color_conversion, std::span<const display::DriverLayer> layers,
          display::DriverConfigStamp config_stamp) {
        EXPECT_EQ(kDisplayId, display_id);
        EXPECT_EQ(kModeId, display_mode_id);
        EXPECT_EQ(display::ColorConversion::kIdentity, color_conversion);
        EXPECT_THAT(layers, ::testing::ElementsAreArray(kLayers));
        EXPECT_EQ(kConfigStamp, config_stamp);
      });

  fdf::Arena arena('TEST');
  fdf::WireUnownedResult<fuchsia_hardware_display_engine::Engine::SubmitConfiguration>
      fidl_transport_result = fidl_client_.buffer(arena)->SubmitConfiguration(
          fidl_display_config, kConfigStamp.ToFidl());
  ASSERT_TRUE(fidl_transport_result.ok()) << fidl_transport_result.FormatDescription();
}

TEST_F(DisplayEngineFidlAdapterTest, SetBufferCollectionConstraintsSuccess) {
  static constexpr display::ImageBufferUsage kImageBufferUsage(
      {.tiling_type = display::ImageTilingType::kLinear});
  static constexpr display::DriverBufferCollectionId kBufferCollectionId(42);

  mock_.ExpectSetBufferCollectionConstraints(
      [&](const display::ImageBufferUsage& image_buffer_usage,
          display::DriverBufferCollectionId buffer_collection_id) {
        EXPECT_EQ(kImageBufferUsage, image_buffer_usage);
        EXPECT_EQ(kBufferCollectionId, buffer_collection_id);
        return zx::ok();
      });

  fdf::Arena arena('TEST');
  fdf::WireUnownedResult<fuchsia_hardware_display_engine::Engine::SetBufferCollectionConstraints>
      fidl_transport_result = fidl_client_.buffer(arena)->SetBufferCollectionConstraints(
          kImageBufferUsage.ToFidl(), kBufferCollectionId.ToFidl());
  ASSERT_TRUE(fidl_transport_result.ok()) << fidl_transport_result.FormatDescription();

  fit::result<zx_status_t>& fidl_domain_result = fidl_transport_result.value();
  EXPECT_OK(fidl_domain_result);
}

TEST_F(DisplayEngineFidlAdapterTest, SetBufferCollectionConstraintsEngineError) {
  static constexpr display::ImageBufferUsage kImageBufferUsage(
      {.tiling_type = display::ImageTilingType::kLinear});
  static constexpr display::DriverBufferCollectionId kBufferCollectionId(42);

  mock_.ExpectSetBufferCollectionConstraints(
      [&](const display::ImageBufferUsage& image_buffer_usage,
          display::DriverBufferCollectionId buffer_collection_id) {
        return zx::error(ZX_ERR_INTERNAL);
      });

  fdf::Arena arena('TEST');
  fdf::WireUnownedResult<fuchsia_hardware_display_engine::Engine::SetBufferCollectionConstraints>
      fidl_transport_result = fidl_client_.buffer(arena)->SetBufferCollectionConstraints(
          kImageBufferUsage.ToFidl(), kBufferCollectionId.ToFidl());
  ASSERT_TRUE(fidl_transport_result.ok()) << fidl_transport_result.FormatDescription();

  fit::result<zx_status_t>& fidl_domain_result = fidl_transport_result.value();
  EXPECT_STATUS(zx::error(ZX_ERR_INTERNAL), fidl_domain_result);
}

TEST_F(DisplayEngineFidlAdapterTest, SetDisplayPowerModeSuccess) {
  static constexpr display::DisplayId kDisplayId(42);
  static constexpr display::PowerMode kPowerMode = display::PowerMode::kOff;

  mock_.ExpectSetDisplayPowerMode(
      [&](display::DisplayId display_id, display::PowerMode power_mode) {
        EXPECT_EQ(kDisplayId, display_id);
        EXPECT_EQ(kPowerMode, power_mode);
        return zx::ok();
      });

  fdf::Arena arena('TEST');
  fdf::WireUnownedResult<fuchsia_hardware_display_engine::Engine::SetDisplayPowerMode>
      fidl_transport_result =
          fidl_client_.buffer(arena)->SetDisplayPowerMode(kDisplayId.ToFidl(), kPowerMode.ToFidl());
  ASSERT_TRUE(fidl_transport_result.ok()) << fidl_transport_result.FormatDescription();

  fit::result<zx_status_t>& fidl_domain_result = fidl_transport_result.value();
  EXPECT_OK(fidl_domain_result);
}

TEST_F(DisplayEngineFidlAdapterTest, SetDisplayPowerModeEngineError) {
  static constexpr display::DisplayId kDisplayId(42);
  static constexpr display::PowerMode kPowerMode = display::PowerMode::kOff;

  mock_.ExpectSetDisplayPowerMode(
      [&](display::DisplayId display_id, display::PowerMode power_mode) {
        return zx::error(ZX_ERR_INTERNAL);
      });

  fdf::Arena arena('TEST');
  fdf::WireUnownedResult<fuchsia_hardware_display_engine::Engine::SetDisplayPowerMode>
      fidl_transport_result =
          fidl_client_.buffer(arena)->SetDisplayPowerMode(kDisplayId.ToFidl(), kPowerMode.ToFidl());
  ASSERT_TRUE(fidl_transport_result.ok()) << fidl_transport_result.FormatDescription();

  fit::result<zx_status_t>& fidl_domain_result = fidl_transport_result.value();
  EXPECT_STATUS(zx::error(ZX_ERR_INTERNAL), fidl_domain_result);
}

TEST_F(DisplayEngineFidlAdapterTest, StartCaptureSuccess) {
  static constexpr display::DriverCaptureImageId kCaptureImageId(42);

  mock_.ExpectStartCapture([&](display::DriverCaptureImageId capture_image_id) {
    EXPECT_EQ(kCaptureImageId, capture_image_id);
    return zx::ok();
  });

  fdf::Arena arena('TEST');
  fdf::WireUnownedResult<fuchsia_hardware_display_engine::Engine::StartCapture>
      fidl_transport_result = fidl_client_.buffer(arena)->StartCapture(kCaptureImageId.ToFidl());
  ASSERT_TRUE(fidl_transport_result.ok()) << fidl_transport_result.FormatDescription();

  fit::result<zx_status_t>& fidl_domain_result = fidl_transport_result.value();
  EXPECT_OK(fidl_domain_result);
}

TEST_F(DisplayEngineFidlAdapterTest, StartCaptureEngineError) {
  static constexpr display::DriverCaptureImageId kCaptureImageId(42);

  mock_.ExpectStartCapture(
      [&](display::DriverCaptureImageId capture_image_id) { return zx::error(ZX_ERR_INTERNAL); });

  fdf::Arena arena('TEST');
  fdf::WireUnownedResult<fuchsia_hardware_display_engine::Engine::StartCapture>
      fidl_transport_result = fidl_client_.buffer(arena)->StartCapture(kCaptureImageId.ToFidl());
  ASSERT_TRUE(fidl_transport_result.ok()) << fidl_transport_result.FormatDescription();

  fit::result<zx_status_t>& fidl_domain_result = fidl_transport_result.value();
  EXPECT_STATUS(zx::error(ZX_ERR_INTERNAL), fidl_domain_result);
}

TEST_F(DisplayEngineFidlAdapterTest, ReleaseCaptureSuccess) {
  static constexpr display::DriverCaptureImageId kCaptureImageId(42);

  mock_.ExpectReleaseCapture([&](display::DriverCaptureImageId capture_image_id) {
    EXPECT_EQ(kCaptureImageId, capture_image_id);
    return zx::ok();
  });

  fdf::Arena arena('TEST');
  fdf::WireUnownedResult<fuchsia_hardware_display_engine::Engine::ReleaseCapture>
      fidl_transport_result = fidl_client_.buffer(arena)->ReleaseCapture(kCaptureImageId.ToFidl());
  ASSERT_TRUE(fidl_transport_result.ok()) << fidl_transport_result.FormatDescription();

  fit::result<zx_status_t>& fidl_domain_result = fidl_transport_result.value();
  EXPECT_OK(fidl_domain_result);
}

TEST_F(DisplayEngineFidlAdapterTest, ReleaseCaptureEngineError) {
  static constexpr display::DriverCaptureImageId kCaptureImageId(42);

  mock_.ExpectReleaseCapture(
      [&](display::DriverCaptureImageId capture_image_id) { return zx::error(ZX_ERR_INTERNAL); });

  fdf::Arena arena('TEST');
  fdf::WireUnownedResult<fuchsia_hardware_display_engine::Engine::ReleaseCapture>
      fidl_transport_result = fidl_client_.buffer(arena)->ReleaseCapture(kCaptureImageId.ToFidl());
  ASSERT_TRUE(fidl_transport_result.ok()) << fidl_transport_result.FormatDescription();

  fit::result<zx_status_t>& fidl_domain_result = fidl_transport_result.value();
  EXPECT_STATUS(zx::error(ZX_ERR_INTERNAL), fidl_domain_result);
}

TEST_F(DisplayEngineFidlAdapterTest, SetMinimumRgbSuccess) {
  static constexpr uint8_t kMinimumRgb = 42;

  mock_.ExpectSetMinimumRgb([&](uint8_t minimum_rgb) {
    EXPECT_EQ(kMinimumRgb, minimum_rgb);
    return zx::ok();
  });

  fdf::Arena arena('TEST');
  fdf::WireUnownedResult<fuchsia_hardware_display_engine::Engine::SetMinimumRgb>
      fidl_transport_result = fidl_client_.buffer(arena)->SetMinimumRgb(kMinimumRgb);
  ASSERT_TRUE(fidl_transport_result.ok()) << fidl_transport_result.FormatDescription();

  fit::result<zx_status_t>& fidl_domain_result = fidl_transport_result.value();
  EXPECT_OK(fidl_domain_result);
}

TEST_F(DisplayEngineFidlAdapterTest, SetMinimumRgbEngineError) {
  static constexpr uint8_t kMinimumRgb = 42;

  mock_.ExpectSetMinimumRgb([&](uint8_t minimum_rgb) { return zx::error(ZX_ERR_IO_REFUSED); });

  fdf::Arena arena('TEST');
  fdf::WireUnownedResult<fuchsia_hardware_display_engine::Engine::SetMinimumRgb>
      fidl_transport_result = fidl_client_.buffer(arena)->SetMinimumRgb(kMinimumRgb);
  ASSERT_TRUE(fidl_transport_result.ok()) << fidl_transport_result.FormatDescription();

  fit::result<zx_status_t>& fidl_domain_result = fidl_transport_result.value();
  EXPECT_STATUS(zx::error(ZX_ERR_IO_REFUSED), fidl_domain_result);
}

}  // namespace

}  // namespace display
