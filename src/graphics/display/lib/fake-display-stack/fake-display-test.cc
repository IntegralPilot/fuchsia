// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/lib/fake-display-stack/fake-display.h"

#include <fidl/fuchsia.sysmem2/cpp/wire.h>
#include <lib/driver/testing/cpp/driver_runtime.h>
#include <lib/driver/testing/cpp/scoped_global_logger.h>
#include <lib/fpromise/result.h>
#include <lib/fzl/vmo-mapper.h>
#include <lib/image-format/image_format.h>
#include <lib/inspect/cpp/hierarchy.h>
#include <lib/inspect/cpp/inspector.h>
#include <lib/inspect/cpp/reader.h>
#include <lib/inspect/cpp/vmo/types.h>
#include <lib/sync/cpp/completion.h>
#include <lib/zx/result.h>
#include <lib/zx/vmo.h>
#include <zircon/assert.h>
#include <zircon/errors.h>
#include <zircon/rights.h>
#include <zircon/status.h>
#include <zircon/syscalls.h>
#include <zircon/types.h>

#include <algorithm>
#include <array>
#include <cstddef>
#include <cstdint>
#include <memory>
#include <span>
#include <utility>
#include <vector>

#include <fbl/algorithm.h>
#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/devices/testing/mock-ddk/mock-device.h"
#include "src/graphics/display/lib/api-protocols/cpp/display-engine-events-interface.h"
#include "src/graphics/display/lib/api-types/cpp/alpha-mode.h"
#include "src/graphics/display/lib/api-types/cpp/color.h"
#include "src/graphics/display/lib/api-types/cpp/config-check-result.h"
#include "src/graphics/display/lib/api-types/cpp/coordinate-transformation.h"
#include "src/graphics/display/lib/api-types/cpp/dimensions.h"
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
#include "src/graphics/display/lib/api-types/cpp/mode-and-id.h"
#include "src/graphics/display/lib/api-types/cpp/mode-id.h"
#include "src/graphics/display/lib/api-types/cpp/pixel-format.h"
#include "src/graphics/display/lib/api-types/cpp/rectangle.h"
#include "src/graphics/display/lib/fake-display-stack/fake-sysmem-device-hierarchy.h"
#include "src/lib/testing/predicates/status.h"

namespace fake_display {

namespace {

// Used to populate FakeDisplayDeviceConfig.
constexpr display::DisplayId kDisplayId(1);
constexpr display::ModeId kModeId(1);
constexpr int32_t kDisplayWidth = 1280;
constexpr int32_t kDisplayHeight = 800;
constexpr int kRefreshRateHz = 60;

class TestDisplayEngineListener : public display::DisplayEngineEventsInterface {
 public:
  // display::DisplayEngineEventsInterface:
  void OnDisplayAdded(display::DisplayId display_id,
                      std::span<const display::ModeAndId> preferred_modes,
                      std::span<const display::PixelFormat> pixel_formats) override {
    display_added_.Signal();
  }
  void OnDisplayRemoved(display::DisplayId display_id) override {
    GTEST_FAIL() << "Unexpected call to OnDisplayRemoved";
  }
  void OnDisplayVsync(display::DisplayId display_id, zx::time_monotonic timestamp,
                      display::DriverConfigStamp config_stamp) override {
    // VSync signals are ignored for now.
  }
  void OnCaptureComplete() override { capture_completed_.Signal(); }

  // Signaled when the driver signals that a display was added.
  libsync::Completion& display_added() { return display_added_; }

  // Signaled when the driver signals that a capture was completed.
  libsync::Completion& capture_completed() { return capture_completed_; }

 private:
  libsync::Completion display_added_;
  libsync::Completion capture_completed_;
};

class FakeDisplayTest : public testing::Test {
 public:
  FakeDisplayTest() = default;

  void SetUp() override {
    zx::result<std::unique_ptr<FakeSysmemDeviceHierarchy>> create_sysmem_provider_result =
        FakeSysmemDeviceHierarchy::Create();
    ASSERT_OK(create_sysmem_provider_result);
    fake_sysmem_hierarchy_ = std::move(create_sysmem_provider_result).value();

    zx::result<fidl::ClientEnd<fuchsia_sysmem2::Allocator>> connect_allocator_result =
        fake_sysmem_hierarchy_->ConnectAllocator2();
    ASSERT_OK(connect_allocator_result);

    fake_display_ = std::make_unique<FakeDisplay>(
        &engine_listener_, std::move(connect_allocator_result).value(),
        GetFakeDisplayDeviceConfig(), inspect::Inspector());
  }

  virtual FakeDisplayDeviceConfig GetFakeDisplayDeviceConfig() const {
    return FakeDisplayDeviceConfig{
        .display_id = kDisplayId,
        .display_mode_id = kModeId,
        .display_mode = display::Mode({
            .active_width = kDisplayWidth,
            .active_height = kDisplayHeight,
            .refresh_rate_millihertz = kRefreshRateHz * 1'000,
        }),
        .engine_info = display::EngineInfo({
            .max_layer_count = 2,
            .max_connected_display_count = 1,
            .is_capture_supported = true,
        }),
        .periodic_vsync = false,
    };
  }

 protected:
  fdf_testing::ScopedGlobalLogger logger_;

  TestDisplayEngineListener engine_listener_;

  std::shared_ptr<fdf_testing::DriverRuntime> driver_runtime_ = mock_ddk::GetDriverRuntime();
  std::unique_ptr<FakeSysmemDeviceHierarchy> fake_sysmem_hierarchy_;

  std::unique_ptr<FakeDisplay> fake_display_;
};

TEST_F(FakeDisplayTest, Inspect) {
  fpromise::result<inspect::Hierarchy> read_result =
      inspect::ReadFromVmo(fake_display_->DuplicateInspectorVmoForTesting());
  ASSERT_TRUE(read_result.is_ok());

  const inspect::Hierarchy& hierarchy = read_result.value();
  const inspect::Hierarchy* config = hierarchy.GetByPath({"device_config"});
  ASSERT_NE(config, nullptr);

  const inspect::IntPropertyValue* width_px =
      config->node().get_property<inspect::IntPropertyValue>("width_px");
  ASSERT_NE(width_px, nullptr);
  EXPECT_EQ(width_px->value(), kDisplayWidth);

  const inspect::IntPropertyValue* height_px =
      config->node().get_property<inspect::IntPropertyValue>("height_px");
  ASSERT_NE(height_px, nullptr);
  EXPECT_EQ(height_px->value(), kDisplayHeight);

  const inspect::DoublePropertyValue* refresh_rate_hz =
      config->node().get_property<inspect::DoublePropertyValue>("refresh_rate_hz");
  ASSERT_NE(refresh_rate_hz, nullptr);
  EXPECT_DOUBLE_EQ(refresh_rate_hz->value(), double{kRefreshRateHz});

  const inspect::BoolPropertyValue* periodic_vsync =
      config->node().get_property<inspect::BoolPropertyValue>("periodic_vsync");
  ASSERT_NE(periodic_vsync, nullptr);
  EXPECT_EQ(periodic_vsync->value(), false);

  const inspect::BoolPropertyValue* is_capture_supported =
      config->node().get_property<inspect::BoolPropertyValue>("is_capture_supported");
  ASSERT_NE(is_capture_supported, nullptr);
  EXPECT_EQ(is_capture_supported->value(), true);
}

// A layer configuration that should pass all the layer tests in FakeDisplay::CheckConfiguration.
constexpr display::DriverLayer kAcceptableLayer({
    .display_destination =
        display::Rectangle({.x = 0, .y = 0, .width = kDisplayWidth, .height = kDisplayHeight}),
    .image_source =
        display::Rectangle({.x = 0, .y = 0, .width = kDisplayWidth, .height = kDisplayHeight}),
    .image_id = display::DriverImageId(4242),
    .image_metadata = display::ImageMetadata({.width = kDisplayWidth,
                                              .height = kDisplayHeight,
                                              .tiling_type = display::ImageTilingType::kLinear}),
    .fallback_color = display::Color({.format = display::PixelFormat::kB8G8R8A8,
                                      .bytes = std::initializer_list<uint8_t>{0x41, 0x42, 0x43,
                                                                              0x44, 0, 0, 0, 0}}),
    .image_source_transformation = display::CoordinateTransformation::kIdentity,
});

TEST_F(FakeDisplayTest, CheckConfigMultipleLayersSuccess) {
  display::DriverLayer layers[2] = {kAcceptableLayer, kAcceptableLayer};

  std::span<const display::DriverLayer> one_layer(layers, 1);
  // Check the display configuration, to make sure the layer configuration succeeds.
  display::ConfigCheckResult config_check_result =
      fake_display_->CheckConfiguration(kDisplayId, kModeId, one_layer);
  ASSERT_EQ(display::ConfigCheckResult::kOk, config_check_result)
      << "This test uses a DriverLayer that does not pass FakeDisplay::CheckConfiguration().";

  // Two layers should succeed, as FakeDisplay only supports up to two layers.
  std::span<const display::DriverLayer> two_layers(std::begin(layers), std::end(layers));
  config_check_result = fake_display_->CheckConfiguration(kDisplayId, kModeId, two_layers);
  EXPECT_EQ(display::ConfigCheckResult::kOk, config_check_result);
}

TEST_F(FakeDisplayTest, CheckConfigMultipleLayersFailureTooMany) {
  display::DriverLayer layers[3] = {kAcceptableLayer, kAcceptableLayer, kAcceptableLayer};

  std::span<const display::DriverLayer> one_layer(layers, 1);
  // Check the display configuration, to make sure the layer configuration succeeds.
  display::ConfigCheckResult config_check_result =
      fake_display_->CheckConfiguration(kDisplayId, kModeId, one_layer);
  ASSERT_EQ(display::ConfigCheckResult::kOk, config_check_result)
      << "This test uses a DriverLayer that does not pass FakeDisplay::CheckConfiguration().";

  // Three layers should fail, as FakeDisplay only supports up to two layers.
  std::span<const display::DriverLayer> three_layers(std::begin(layers), std::end(layers));
  config_check_result = fake_display_->CheckConfiguration(kDisplayId, kModeId, three_layers);
  EXPECT_EQ(display::ConfigCheckResult::kUnsupportedConfig, config_check_result);
}

class FakeDisplayRealSysmemTest : public FakeDisplayTest {
 public:
  struct BufferCollectionAndToken {
    fidl::WireSyncClient<fuchsia_sysmem2::BufferCollection> collection_client;
    fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> token;
  };

  FakeDisplayRealSysmemTest() = default;
  ~FakeDisplayRealSysmemTest() override = default;

  zx::result<BufferCollectionAndToken> CreateBufferCollection() {
    zx::result<fidl::Endpoints<fuchsia_sysmem2::BufferCollectionToken>> token_endpoints =
        fidl::CreateEndpoints<fuchsia_sysmem2::BufferCollectionToken>();

    EXPECT_OK(token_endpoints);
    if (!token_endpoints.is_ok()) {
      return token_endpoints.take_error();
    }

    auto& [token_client, token_server] = token_endpoints.value();
    fidl::Arena arena;
    auto allocate_shared_request =
        fuchsia_sysmem2::wire::AllocatorAllocateSharedCollectionRequest::Builder(arena);
    allocate_shared_request.token_request(std::move(token_server));
    fidl::Status allocate_token_status =
        sysmem_->AllocateSharedCollection(allocate_shared_request.Build());
    EXPECT_OK(allocate_token_status.status());
    if (!allocate_token_status.ok()) {
      return zx::error(allocate_token_status.status());
    }

    fidl::Status sync_status = fidl::WireCall(token_client)->Sync();
    EXPECT_OK(sync_status.status());
    if (!sync_status.ok()) {
      return zx::error(sync_status.status());
    }

    // At least one sysmem participant should specify buffer memory constraints.
    // The driver may not specify buffer memory constraints, so the test should
    // always provide one for sysmem through `buffer_collection_` client.
    //
    // Here we duplicate the token to set buffer collection constraints in
    // the test.
    std::vector<zx_rights_t> rights = {ZX_RIGHT_SAME_RIGHTS};
    auto duplicate_request =
        fuchsia_sysmem2::wire::BufferCollectionTokenDuplicateSyncRequest::Builder(arena);
    duplicate_request.rights_attenuation_masks(rights);
    fidl::WireResult duplicate_result =
        fidl::WireCall(token_client)->DuplicateSync(duplicate_request.Build());
    EXPECT_OK(duplicate_result.status());
    if (!duplicate_result.ok()) {
      return zx::error(duplicate_result.status());
    }

    auto& duplicate_value = duplicate_result.value();
    EXPECT_EQ(duplicate_value.tokens().size(), 1u);
    if (duplicate_value.tokens().size() != 1u) {
      return zx::error(ZX_ERR_BAD_STATE);
    }

    // Bind duplicated token to BufferCollection client.
    auto [collection_client, collection_server] =
        fidl::Endpoints<fuchsia_sysmem2::BufferCollection>::Create();
    auto bind_request = fuchsia_sysmem2::wire::AllocatorBindSharedCollectionRequest::Builder(arena);
    bind_request.token(std::move(duplicate_value.tokens()[0]));
    bind_request.buffer_collection_request(std::move(collection_server));
    fidl::Status bind_status = sysmem_->BindSharedCollection(bind_request.Build());
    EXPECT_OK(bind_status.status());
    if (!bind_status.ok()) {
      return zx::error(bind_status.status());
    }

    return zx::ok(BufferCollectionAndToken{
        .collection_client = fidl::WireSyncClient(std::move(collection_client)),
        .token = std::move(token_client),
    });
  }

  void SetUp() override {
    FakeDisplayTest::SetUp();

    zx::result<fidl::ClientEnd<fuchsia_sysmem2::Allocator>> connect_allocator_result =
        fake_sysmem_hierarchy_->ConnectAllocator2();
    ASSERT_OK(connect_allocator_result);

    sysmem_ = fidl::WireSyncClient<fuchsia_sysmem2::Allocator>(
        std::move(connect_allocator_result).value());
    EXPECT_TRUE(sysmem_.is_valid());
  }

  void TearDown() override {
    sysmem_ = {};
    FakeDisplayTest::TearDown();
  }

  const fidl::WireSyncClient<fuchsia_sysmem2::Allocator>& sysmem() const { return sysmem_; }

 private:
  fidl::WireSyncClient<fuchsia_sysmem2::Allocator> sysmem_;
};

// Creates a BufferCollectionConstraints that tests can use to configure their
// own BufferCollections to allocate buffers.
//
// It provides sysmem Constraints that request exactly 1 CPU-readable and
// writable buffer with size of at least `min_size_bytes` bytes and can hold
// an image of pixel format `pixel_format`.
//
// As we require the image to be both readable and writable, the constraints
// will work for both simple (scanout) images and capture images.
fuchsia_sysmem2::wire::BufferCollectionConstraints CreateImageConstraints(
    fidl::AnyArena& arena, uint32_t min_size_bytes,
    fuchsia_images2::wire::PixelFormat pixel_format) {
  // fake-display driver doesn't add extra image format constraints when
  // SetBufferCollectionConstraints() is called. To make sure we allocate an
  // image buffer, we add constraints here.
  auto constraints = fuchsia_sysmem2::wire::BufferCollectionConstraints::Builder(arena);
  constraints.usage(
      fuchsia_sysmem2::wire::BufferUsage::Builder(arena)
          .cpu(fuchsia_sysmem2::wire::kCpuUsageRead | fuchsia_sysmem2::wire::kCpuUsageWrite)
          .Build());
  constraints.min_buffer_count(1);
  constraints.max_buffer_count(1);
  auto bmc = fuchsia_sysmem2::wire::BufferMemoryConstraints::Builder(arena);
  bmc.min_size_bytes(min_size_bytes);
  bmc.ram_domain_supported(false);
  // The test cases need direct CPU access to the buffers and we
  // don't enforce cache flushing, so we should narrow down the
  // allowed sysmem heaps to the heaps supporting CPU domain and
  // reject all the other heaps.
  bmc.cpu_domain_supported(true);
  bmc.inaccessible_domain_supported(false);
  constraints.buffer_memory_constraints(bmc.Build());
  auto ifc = fuchsia_sysmem2::wire::ImageFormatConstraints::Builder(arena);
  ifc.pixel_format(pixel_format);
  ifc.color_spaces(std::array{fuchsia_images2::ColorSpace::kSrgb});
  constraints.image_format_constraints(std::array{ifc.Build()});
  return constraints.Build();
}

constexpr fuchsia_math::wire::Point kZeroPoint = {.x = 0, .y = 0};

// Creates a layer containing an on the top-left corner.
constexpr display::DriverLayer CreateImageLayerConfig(
    display::DriverImageId image_id, const display::Color& fallback_color,
    const display::ImageMetadata& image_metadata,
    const fuchsia_math::wire::Point& top_left = kZeroPoint) {
  return display::DriverLayer({
      .display_destination = display::Rectangle({.x = top_left.x,
                                                 .y = top_left.y,
                                                 .width = image_metadata.width(),
                                                 .height = image_metadata.height()}),
      .image_source = display::Rectangle(
          {.x = 0, .y = 0, .width = image_metadata.width(), .height = image_metadata.height()}),
      .image_id = image_id,
      .image_metadata = image_metadata,
      .fallback_color = fallback_color,
      .alpha_mode = display::AlphaMode::kDisable,
      .alpha_coefficient = 1.0,
      .image_source_transformation = display::CoordinateTransformation::kIdentity,
  });
}

constexpr display::DriverLayer CreateColorFillLayerConfig(
    const display::Color& fill_color, const display::Dimensions& layer_dimensions) {
  return display::DriverLayer({
      .display_destination = display::Rectangle(
          {.x = 0, .y = 0, .width = layer_dimensions.width(), .height = layer_dimensions.height()}),
      .image_source = display::Rectangle({.x = 0, .y = 0, .width = 0, .height = 0}),
      .image_id = display::kInvalidDriverImageId,
      .image_metadata = display::ImageMetadata(
          {.width = 0, .height = 0, .tiling_type = display::ImageTilingType::kLinear}),
      .fallback_color = fill_color,
      .alpha_mode = display::AlphaMode::kDisable,
      .alpha_coefficient = 1.0,
      .image_source_transformation = display::CoordinateTransformation::kIdentity,
  });
}

std::pair<zx::vmo, fuchsia_sysmem2::wire::SingleBufferSettings> GetAllocatedBufferAndSettings(
    fidl::AnyArena& arena, const fidl::WireSyncClient<fuchsia_sysmem2::BufferCollection>& client) {
  auto wait_result = client->WaitForAllBuffersAllocated();
  ZX_ASSERT_MSG(wait_result.ok(), "WaitForBuffersAllocated() FIDL call failed: %s (%u)",
                wait_result.status_string(), fidl::ToUnderlying(wait_result->error_value()));
  auto& buffer_collection_info = wait_result.value()->buffer_collection_info();
  ZX_ASSERT_MSG(buffer_collection_info.buffers().size() == 1u,
                "Incorrect number of buffers allocated: actual %zu, expected 1",
                buffer_collection_info.buffers().size());
  // wire types don't provide a way to clone into a different arena short of converting to natural
  // type and back; we could consider returning the natural type instead, or converting this whole
  // file to natural types, but for now this allows the caller to use wire types everyhwere (not
  // necessarily a goal; just how the client code currently works)
  auto settings_clone = fidl::ToWire(arena, fidl::ToNatural(buffer_collection_info.settings()));
  return {std::move(buffer_collection_info.buffers()[0].vmo()), std::move(settings_clone)};
}

void FillImageWithColor(std::span<uint8_t> image_buffer, const std::vector<uint8_t>& color_raw,
                        int width, int height, uint32_t bytes_per_row_divisor) {
  size_t bytes_per_pixel = color_raw.size();
  size_t row_stride_bytes = fbl::round_up(width * bytes_per_pixel, bytes_per_row_divisor);

  for (int row = 0; row < height; ++row) {
    auto row_buffer = image_buffer.subspan(row * row_stride_bytes, row_stride_bytes);
    auto it = row_buffer.begin();
    for (int col = 0; col < width; ++col) {
      it = std::copy(color_raw.begin(), color_raw.end(), it);
    }
  }
}

TEST_F(FakeDisplayRealSysmemTest, ImportBufferCollection) {
  zx::result<BufferCollectionAndToken> new_buffer_collection_result = CreateBufferCollection();
  ASSERT_OK(new_buffer_collection_result);
  auto [collection_client, token] = std::move(new_buffer_collection_result.value());

  // Test ImportBufferCollection().
  constexpr display::DriverBufferCollectionId kValidBufferCollectionId(1);
  EXPECT_OK(fake_display_->ImportBufferCollection(kValidBufferCollectionId, std::move(token)));

  // `driver_buffer_collection_id` must be unused.
  auto another_token_endpoints = fidl::Endpoints<fuchsia_sysmem2::BufferCollectionToken>::Create();
  EXPECT_STATUS(fake_display_->ImportBufferCollection(kValidBufferCollectionId,
                                                      std::move(another_token_endpoints.client)),
                zx::error(ZX_ERR_ALREADY_EXISTS));

  // Driver sets BufferCollection buffer memory constraints.
  static constexpr display::ImageBufferUsage kDisplayUsage({
      .tiling_type = display::ImageTilingType::kLinear,
  });
  EXPECT_OK(fake_display_->SetBufferCollectionConstraints(kDisplayUsage, kValidBufferCollectionId));

  // Set BufferCollection buffer memory constraints.
  fidl::Arena arena;
  auto set_constraints_request =
      fuchsia_sysmem2::wire::BufferCollectionSetConstraintsRequest::Builder(arena);
  set_constraints_request.constraints(CreateImageConstraints(
      arena, /*min_size_bytes=*/4096, fuchsia_images2::PixelFormat::kR8G8B8A8));
  fidl::Status set_constraints_status =
      collection_client->SetConstraints(set_constraints_request.Build());
  EXPECT_TRUE(set_constraints_status.ok());

  // Both the test-side client and the driver have  set the constraints.
  // The buffer should be allocated correctly in sysmem.
  EXPECT_TRUE(collection_client->WaitForAllBuffersAllocated().ok());

  // Test ReleaseBufferCollection().
  // TODO(https://fxbug.dev/42079040): Consider adding RAII handles to release the
  // imported buffer collections.
  constexpr display::DriverBufferCollectionId kInvalidBufferCollectionId(2);
  EXPECT_STATUS(fake_display_->ReleaseBufferCollection(kInvalidBufferCollectionId),
                zx::error(ZX_ERR_NOT_FOUND));
  EXPECT_OK(fake_display_->ReleaseBufferCollection(kValidBufferCollectionId));
}

TEST_F(FakeDisplayRealSysmemTest, ImportImage) {
  zx::result<BufferCollectionAndToken> new_buffer_collection_result = CreateBufferCollection();
  ASSERT_OK(new_buffer_collection_result);
  auto [collection_client, token] = std::move(new_buffer_collection_result.value());

  constexpr display::DriverBufferCollectionId kBufferCollectionId(1);
  ASSERT_OK(fake_display_->ImportBufferCollection(kBufferCollectionId, std::move(token)));

  // Driver sets BufferCollection buffer memory constraints.
  static constexpr display::ImageBufferUsage kDisplayUsage({
      .tiling_type = display::ImageTilingType::kLinear,
  });
  ASSERT_OK(fake_display_->SetBufferCollectionConstraints(kDisplayUsage, kBufferCollectionId));

  // Set BufferCollection buffer memory constraints.
  static constexpr const display::ImageMetadata kDisplayImageMetadata({
      .width = 1024,
      .height = 768,
      .tiling_type = display::ImageTilingType::kLinear,
  });

  const PixelFormatAndModifier kPixelFormat(fuchsia_images2::PixelFormat::kB8G8R8A8,
                                            fuchsia_images2::PixelFormatModifier::kLinear);

  const uint32_t bytes_per_pixel = ImageFormatStrideBytesPerWidthPixel(kPixelFormat);
  fidl::Arena arena;
  auto set_constraints_request =
      fuchsia_sysmem2::wire::BufferCollectionSetConstraintsRequest::Builder(arena);
  set_constraints_request.constraints(
      CreateImageConstraints(arena,
                             /*min_size_bytes=*/kDisplayImageMetadata.width() *
                                 kDisplayImageMetadata.height() * bytes_per_pixel,
                             kPixelFormat.pixel_format));
  fidl::Status set_constraints_status =
      collection_client->SetConstraints(set_constraints_request.Build());
  ASSERT_TRUE(set_constraints_status.ok());

  // Both the test-side client and the driver have set the constraints.
  // The buffer should be allocated correctly in sysmem.
  EXPECT_TRUE(collection_client->WaitForAllBuffersAllocated().ok());

  // TODO(https://fxbug.dev/42079037): Split all valid / invalid imports into separate
  // test cases.
  // Invalid import: Bad image type.
  static constexpr const display::ImageMetadata kInvalidTilingTypeMetadata({
      .width = 1024,
      .height = 768,
      .tiling_type = display::ImageTilingType::kCapture,
  });
  EXPECT_STATUS(fake_display_->ImportImage(kInvalidTilingTypeMetadata, kBufferCollectionId,
                                           /*buffer_index=*/0),
                zx::error(ZX_ERR_INVALID_ARGS));

  // Invalid import: Invalid collection ID.
  constexpr display::DriverBufferCollectionId kInvalidBufferCollectionId(100);
  EXPECT_STATUS(fake_display_->ImportImage(kDisplayImageMetadata, kInvalidBufferCollectionId,
                                           /*buffer_index=*/0),
                zx::error(ZX_ERR_NOT_FOUND));

  // Invalid import: Invalid buffer collection index.
  constexpr uint32_t kInvalidBufferCollectionIndex = 100u;
  EXPECT_STATUS(fake_display_->ImportImage(kDisplayImageMetadata, kBufferCollectionId,
                                           kInvalidBufferCollectionIndex),
                zx::error(ZX_ERR_OUT_OF_RANGE));

  // Valid import.
  zx::result<display::DriverImageId> valid_import_result =
      fake_display_->ImportImage(kDisplayImageMetadata, kBufferCollectionId,
                                 /*index=*/0);
  ASSERT_OK(valid_import_result);
  EXPECT_NE(display::kInvalidDriverImageId, valid_import_result.value());

  // Release the image.
  fake_display_->ReleaseImage(valid_import_result.value());

  EXPECT_OK(fake_display_->ReleaseBufferCollection(kBufferCollectionId));
}

TEST_F(FakeDisplayRealSysmemTest, ImportImageForCapture) {
  zx::result<BufferCollectionAndToken> new_buffer_collection_result = CreateBufferCollection();
  ASSERT_OK(new_buffer_collection_result);
  auto [collection_client, token] = std::move(new_buffer_collection_result.value());

  constexpr display::DriverBufferCollectionId kBufferCollectionId(1);
  EXPECT_OK(fake_display_->ImportBufferCollection(kBufferCollectionId, std::move(token)));

  const PixelFormatAndModifier kPixelFormat(fuchsia_images2::PixelFormat::kB8G8R8A8,
                                            fuchsia_images2::PixelFormatModifier::kLinear);

  static constexpr display::ImageBufferUsage kCaptureUsage({
      .tiling_type = display::ImageTilingType::kCapture,
  });
  ASSERT_OK(fake_display_->SetBufferCollectionConstraints(kCaptureUsage, kBufferCollectionId));

  const uint32_t bytes_per_pixel = ImageFormatStrideBytesPerWidthPixel(kPixelFormat);
  const uint32_t size_bytes = kDisplayWidth * kDisplayHeight * bytes_per_pixel;
  // Set BufferCollection buffer memory constraints.
  fidl::Arena arena;
  auto set_constraints_request =
      fuchsia_sysmem2::wire::BufferCollectionSetConstraintsRequest::Builder(arena);
  set_constraints_request.constraints(
      CreateImageConstraints(arena, size_bytes, kPixelFormat.pixel_format));
  fidl::Status set_constraints_status =
      collection_client->SetConstraints(set_constraints_request.Build());
  ASSERT_TRUE(set_constraints_status.ok());

  // Both the test-side client and the driver have set the constraints.
  // The buffer should be allocated correctly in sysmem.
  ASSERT_TRUE(collection_client->WaitForAllBuffersAllocated().ok());

  // TODO(https://fxbug.dev/42079037): Split all valid / invalid imports into separate
  // test cases.
  // Invalid import: Invalid collection ID.
  constexpr display::DriverBufferCollectionId kInvalidBufferCollectionId(100);
  EXPECT_STATUS(
      fake_display_->ImportImageForCapture(kInvalidBufferCollectionId, /*buffer_index=*/0),
      zx::error(ZX_ERR_NOT_FOUND));

  // Invalid import: Invalid buffer collection index.
  constexpr uint64_t kInvalidBufferCollectionIndex = 100u;
  EXPECT_STATUS(
      fake_display_->ImportImageForCapture(kBufferCollectionId, kInvalidBufferCollectionIndex),
      zx::error(ZX_ERR_OUT_OF_RANGE));

  // Valid import.
  zx::result<display::DriverCaptureImageId> valid_import_result =
      fake_display_->ImportImageForCapture(kBufferCollectionId,
                                           /*buffer_index=*/0);
  ASSERT_OK(valid_import_result);
  EXPECT_NE(display::kInvalidDriverCaptureImageId, valid_import_result.value());

  // Release the image.
  // TODO(https://fxbug.dev/42079040): Consider adding RAII handles to release the
  // imported images and buffer collections.
  EXPECT_OK(fake_display_->ReleaseCapture(valid_import_result.value()));

  EXPECT_OK(fake_display_->ReleaseBufferCollection(kBufferCollectionId));
}

TEST_F(FakeDisplayRealSysmemTest, CaptureImage) {
  zx::result<BufferCollectionAndToken> new_capture_buffer_collection_result =
      CreateBufferCollection();
  ASSERT_OK(new_capture_buffer_collection_result);
  auto [capture_collection_client, capture_token] =
      std::move(new_capture_buffer_collection_result.value());

  zx::result<BufferCollectionAndToken> new_framebuffer_buffer_collection_result =
      CreateBufferCollection();
  ASSERT_OK(new_framebuffer_buffer_collection_result);
  auto [framebuffer_collection_client, framebuffer_token] =
      std::move(new_framebuffer_buffer_collection_result.value());

  display::EngineInfo engine_info = fake_display_->CompleteCoordinatorConnection();
  ASSERT_EQ(true, engine_info.is_capture_supported());

  constexpr display::DriverBufferCollectionId kCaptureBufferCollectionId(1);
  constexpr display::DriverBufferCollectionId kFramebufferBufferCollectionId(2);
  ASSERT_OK(
      fake_display_->ImportBufferCollection(kCaptureBufferCollectionId, std::move(capture_token)));
  ASSERT_OK(fake_display_->ImportBufferCollection(kFramebufferBufferCollectionId,
                                                  std::move(framebuffer_token)));

  const auto kPixelFormat = PixelFormatAndModifier(fuchsia_images2::PixelFormat::kB8G8R8A8,
                                                   fuchsia_images2::PixelFormatModifier::kLinear);

  // Set BufferCollection buffer memory constraints from the display driver's
  // end.
  static constexpr display::ImageBufferUsage kDisplayUsage({
      .tiling_type = display::ImageTilingType::kLinear,
  });
  ASSERT_OK(
      fake_display_->SetBufferCollectionConstraints(kDisplayUsage, kFramebufferBufferCollectionId));
  static constexpr display::ImageBufferUsage kCaptureUsage({
      .tiling_type = display::ImageTilingType::kCapture,
  });
  ASSERT_OK(
      fake_display_->SetBufferCollectionConstraints(kCaptureUsage, kCaptureBufferCollectionId));

  // Set BufferCollection buffer memory constraints from the test's end.
  const uint32_t bytes_per_pixel = ImageFormatStrideBytesPerWidthPixel(kPixelFormat);
  const uint32_t size_bytes = kDisplayWidth * kDisplayHeight * bytes_per_pixel;

  fidl::Arena arena;
  auto framebuffer_set_constraints_request =
      fuchsia_sysmem2::wire::BufferCollectionSetConstraintsRequest::Builder(arena);
  framebuffer_set_constraints_request.constraints(
      CreateImageConstraints(arena, size_bytes, kPixelFormat.pixel_format));
  fidl::Status set_framebuffer_constraints_status =
      framebuffer_collection_client->SetConstraints(framebuffer_set_constraints_request.Build());
  ASSERT_TRUE(set_framebuffer_constraints_status.ok());
  arena.Reset();

  auto capture_set_constraints_request =
      fuchsia_sysmem2::wire::BufferCollectionSetConstraintsRequest::Builder(arena);
  capture_set_constraints_request.constraints(
      CreateImageConstraints(arena, size_bytes, kPixelFormat.pixel_format));
  fidl::Status set_capture_constraints_status =
      capture_collection_client->SetConstraints(capture_set_constraints_request.Build());
  ASSERT_TRUE(set_capture_constraints_status.ok());
  arena.Reset();

  // Both the test-side client and the driver have set the constraints.
  // The buffers should be allocated correctly in sysmem.
  auto [framebuffer_vmo, framebuffer_settings] =
      GetAllocatedBufferAndSettings(arena, framebuffer_collection_client);
  auto [capture_vmo, capture_settings] =
      GetAllocatedBufferAndSettings(arena, capture_collection_client);

  // Fill the framebuffer.
  fzl::VmoMapper framebuffer_mapper;
  ASSERT_OK(framebuffer_mapper.Map(framebuffer_vmo));
  std::span<uint8_t> framebuffer_bytes(reinterpret_cast<uint8_t*>(framebuffer_mapper.start()),
                                       framebuffer_mapper.size());
  const std::vector<uint8_t> kBlueBgraBytes = {0xff, 0, 0, 0xff};
  FillImageWithColor(framebuffer_bytes, kBlueBgraBytes, kDisplayWidth, kDisplayHeight,
                     framebuffer_settings.image_format_constraints().bytes_per_row_divisor());
  zx_cache_flush(framebuffer_bytes.data(), framebuffer_bytes.size(),
                 ZX_CACHE_FLUSH_DATA | ZX_CACHE_FLUSH_INVALIDATE);
  framebuffer_mapper.Unmap();

  // Import capture image.
  zx::result<display::DriverCaptureImageId> capture_import_result =
      fake_display_->ImportImageForCapture(kCaptureBufferCollectionId,
                                           /*buffer_index=*/0);
  ASSERT_OK(capture_import_result);
  ASSERT_NE(display::kInvalidDriverCaptureImageId, capture_import_result.value());

  // Import framebuffer image.
  static constexpr display::ImageMetadata kFramebufferImageMetadata({
      .width = kDisplayWidth,
      .height = kDisplayHeight,
      .tiling_type = display::ImageTilingType::kLinear,
  });

  zx::result<display::DriverImageId> framebuffer_import_result =
      fake_display_->ImportImage(kFramebufferImageMetadata, kFramebufferBufferCollectionId,
                                 /*buffer_index=*/0);
  ASSERT_OK(framebuffer_import_result);
  ASSERT_NE(display::kInvalidDriverImageId, framebuffer_import_result.value());

  // Create display configuration.
  static constexpr display::Color kBlackBgra({
      .format = display::PixelFormat::kB8G8R8A8,
      .bytes = {{0x00, 0x00, 0x00, 0xff, 0x00, 0x00, 0x00, 0x00}},
  });
  constexpr size_t kLayerCount = 1;
  std::array<const display::DriverLayer, kLayerCount> kLayers = {
      CreateImageLayerConfig(framebuffer_import_result.value(), kBlackBgra,
                             kFramebufferImageMetadata),
  };

  // Check and apply the display configuration.
  display::ConfigCheckResult config_check_result =
      fake_display_->CheckConfiguration(kDisplayId, kModeId, kLayers);
  ASSERT_EQ(display::ConfigCheckResult::kOk, config_check_result);

  static constexpr display::DriverConfigStamp kConfigStamp(1);
  fake_display_->SubmitConfiguration(kDisplayId, kModeId, kLayers, kConfigStamp);

  // Start capture; wait until the capture ends.
  ASSERT_FALSE(engine_listener_.capture_completed().signaled());
  ASSERT_OK(fake_display_->StartCapture(capture_import_result.value()));
  engine_listener_.capture_completed().Wait();
  EXPECT_TRUE(engine_listener_.capture_completed().signaled());

  // Verify the captured image has the same content as the original image.
  constexpr int kCaptureBytesPerPixel = 4;
  uint32_t capture_bytes_per_row_divisor =
      capture_settings.image_format_constraints().bytes_per_row_divisor();
  uint32_t capture_row_stride_bytes =
      fbl::round_up(uint32_t{kDisplayWidth} * kCaptureBytesPerPixel, capture_bytes_per_row_divisor);

  {
    fzl::VmoMapper capture_mapper;
    ASSERT_OK(capture_mapper.Map(capture_vmo));
    std::span<const uint8_t> capture_bytes(reinterpret_cast<const uint8_t*>(capture_mapper.start()),
                                           /*count=*/capture_mapper.size());
    zx_cache_flush(capture_bytes.data(), capture_bytes.size(), ZX_CACHE_FLUSH_DATA);

    for (int row = 0; row < kDisplayHeight; ++row) {
      std::span<const uint8_t> capture_row =
          capture_bytes.subspan(row * capture_row_stride_bytes, capture_row_stride_bytes);
      auto it = capture_row.begin();
      for (int col = 0; col < kDisplayWidth; ++col) {
        std::vector<uint8_t> curr_color(it, it + kCaptureBytesPerPixel);

        // EXPECT_THAT() would be correct here. However, it generates a lot of
        // logging output when the capture functionality is completely broken,
        // greatly reducing the test's helpfulness.
        ASSERT_THAT(curr_color, testing::ElementsAreArray(kBlueBgraBytes))
            << "Color mismatch at row " << row << " column " << col;
        it += kCaptureBytesPerPixel;
      }
    }
  }

  // Apply a solid color fill configuration so the image can be released.
  static constexpr display::Dimensions kDisplayDimensions(
      {.width = kDisplayWidth, .height = kDisplayHeight});
  std::array<const display::DriverLayer, kLayerCount> kColorFillLayers = {
      CreateColorFillLayerConfig(kBlackBgra, kDisplayDimensions),
  };
  static constexpr display::DriverConfigStamp kEmptyConfigStamp(2);
  fake_display_->SubmitConfiguration(kDisplayId, kModeId, kColorFillLayers, kEmptyConfigStamp);

  // Release the image.
  // TODO(https://fxbug.dev/42079040): Consider adding RAII handles to release the
  // imported images and buffer collections.
  fake_display_->ReleaseImage(framebuffer_import_result.value());
  EXPECT_OK(fake_display_->ReleaseCapture(capture_import_result.value()));

  EXPECT_OK(fake_display_->ReleaseBufferCollection(kFramebufferBufferCollectionId));
  EXPECT_OK(fake_display_->ReleaseBufferCollection(kCaptureBufferCollectionId));
}

TEST_F(FakeDisplayRealSysmemTest, CaptureSolidColorFill) {
  zx::result<BufferCollectionAndToken> new_capture_buffer_collection_result =
      CreateBufferCollection();
  ASSERT_OK(new_capture_buffer_collection_result);
  auto [capture_collection_client, capture_token] =
      std::move(new_capture_buffer_collection_result.value());

  display::EngineInfo engine_info = fake_display_->CompleteCoordinatorConnection();
  ASSERT_EQ(true, engine_info.is_capture_supported());

  constexpr display::DriverBufferCollectionId kCaptureBufferCollectionId(1);
  ASSERT_OK(
      fake_display_->ImportBufferCollection(kCaptureBufferCollectionId, std::move(capture_token)));

  const auto kPixelFormat = PixelFormatAndModifier(fuchsia_images2::PixelFormat::kB8G8R8A8,
                                                   fuchsia_images2::PixelFormatModifier::kLinear);

  constexpr display::Dimensions kDisplayDimensions(
      {.width = kDisplayWidth, .height = kDisplayHeight});

  // Set BufferCollection buffer memory constraints from the display driver's
  // end.
  static constexpr display::ImageBufferUsage kCaptureUsage({
      .tiling_type = display::ImageTilingType::kCapture,
  });
  ASSERT_OK(
      fake_display_->SetBufferCollectionConstraints(kCaptureUsage, kCaptureBufferCollectionId));

  // Set BufferCollection buffer memory constraints from the test's end.
  const uint32_t bytes_per_pixel = ImageFormatStrideBytesPerWidthPixel(kPixelFormat);
  const uint32_t size_bytes = kDisplayWidth * kDisplayHeight * bytes_per_pixel;

  fidl::Arena arena;
  auto capture_set_constraints_request =
      fuchsia_sysmem2::wire::BufferCollectionSetConstraintsRequest::Builder(arena);
  capture_set_constraints_request.constraints(
      CreateImageConstraints(arena, size_bytes, kPixelFormat.pixel_format));
  fidl::Status set_capture_constraints_status =
      capture_collection_client->SetConstraints(capture_set_constraints_request.Build());
  ASSERT_TRUE(set_capture_constraints_status.ok());
  arena.Reset();

  // Both the test-side client and the driver have set the constraints.
  // The buffers should be allocated correctly in sysmem.
  auto [capture_vmo, capture_settings] =
      GetAllocatedBufferAndSettings(arena, capture_collection_client);

  // Import capture image.
  zx::result<display::DriverCaptureImageId> capture_import_result =
      fake_display_->ImportImageForCapture(kCaptureBufferCollectionId,
                                           /*buffer_index=*/0);
  ASSERT_OK(capture_import_result);
  ASSERT_NE(display::kInvalidDriverCaptureImageId, capture_import_result.value());

  // Create display configuration.
  static constexpr display::Color kBlueBgra({
      .format = display::PixelFormat::kB8G8R8A8,
      .bytes = {{0xff, 0x00, 0x00, 0xff, 0x00, 0x00, 0x00, 0x00}},
  });
  constexpr size_t kLayerCount = 1;
  std::array<const display::DriverLayer, kLayerCount> kLayers = {
      CreateColorFillLayerConfig(kBlueBgra, kDisplayDimensions),
  };

  // Check and apply the display configuration.
  display::ConfigCheckResult config_check_result =
      fake_display_->CheckConfiguration(kDisplayId, kModeId, kLayers);
  ASSERT_EQ(display::ConfigCheckResult::kOk, config_check_result);

  static constexpr display::DriverConfigStamp kConfigStamp(1);
  fake_display_->SubmitConfiguration(kDisplayId, kModeId, kLayers, kConfigStamp);
  // Start capture; wait until the capture ends.
  ASSERT_FALSE(engine_listener_.capture_completed().signaled());
  ASSERT_OK(fake_display_->StartCapture(capture_import_result.value()));
  engine_listener_.capture_completed().Wait();
  EXPECT_TRUE(engine_listener_.capture_completed().signaled());

  // Verify the captured image has the same content as the original image.
  constexpr int kCaptureBytesPerPixel = 4;
  uint32_t capture_bytes_per_row_divisor =
      capture_settings.image_format_constraints().bytes_per_row_divisor();
  uint32_t capture_row_stride_bytes =
      fbl::round_up(uint32_t{kDisplayWidth} * kCaptureBytesPerPixel, capture_bytes_per_row_divisor);

  const std::vector<uint8_t> kBlueBgraBytes = {0xff, 0, 0, 0xff};
  {
    fzl::VmoMapper capture_mapper;
    ASSERT_OK(capture_mapper.Map(capture_vmo));
    std::span<const uint8_t> capture_bytes(reinterpret_cast<const uint8_t*>(capture_mapper.start()),
                                           /*count=*/capture_mapper.size());
    zx_cache_flush(capture_bytes.data(), capture_bytes.size(), ZX_CACHE_FLUSH_DATA);

    for (int row = 0; row < kDisplayHeight; ++row) {
      std::span<const uint8_t> capture_row =
          capture_bytes.subspan(row * capture_row_stride_bytes, capture_row_stride_bytes);
      auto it = capture_row.begin();
      for (int col = 0; col < kDisplayWidth; ++col) {
        std::vector<uint8_t> curr_color(it, it + kCaptureBytesPerPixel);

        // EXPECT_THAT() would be correct here. However, it generates a lot of
        // logging output when the capture functionality is completely broken,
        // greatly reducing the test's helpfulness.
        ASSERT_THAT(curr_color, testing::ElementsAreArray(kBlueBgraBytes))
            << "Color mismatch at row " << row << " column " << col;
        it += kCaptureBytesPerPixel;
      }
    }
  }

  // Release the capture image.
  // TODO(https://fxbug.dev/42079040): Consider adding RAII handles to release the
  // imported images and buffer collections.
  EXPECT_OK(fake_display_->ReleaseCapture(capture_import_result.value()));
  EXPECT_OK(fake_display_->ReleaseBufferCollection(kCaptureBufferCollectionId));
}

TEST_F(FakeDisplayRealSysmemTest, CaptureMultipleImageLayers) {
  zx::result<BufferCollectionAndToken> new_capture_buffer_collection_result =
      CreateBufferCollection();
  ASSERT_OK(new_capture_buffer_collection_result);
  auto [capture_collection_client, capture_token] =
      std::move(new_capture_buffer_collection_result.value());

  zx::result<BufferCollectionAndToken> new_framebuffer_buffer_collection_result =
      CreateBufferCollection();
  ASSERT_OK(new_framebuffer_buffer_collection_result);
  auto [framebuffer1_collection_client, framebuffer1_token] =
      std::move(new_framebuffer_buffer_collection_result.value());

  new_framebuffer_buffer_collection_result = CreateBufferCollection();
  ASSERT_OK(new_framebuffer_buffer_collection_result);
  auto [framebuffer2_collection_client, framebuffer2_token] =
      std::move(new_framebuffer_buffer_collection_result.value());

  display::EngineInfo engine_info = fake_display_->CompleteCoordinatorConnection();
  ASSERT_EQ(true, engine_info.is_capture_supported());

  constexpr display::DriverBufferCollectionId kCaptureBufferCollectionId(1);
  constexpr display::DriverBufferCollectionId kFramebuffer1BufferCollectionId(2);
  constexpr display::DriverBufferCollectionId kFramebuffer2BufferCollectionId(3);
  ASSERT_OK(
      fake_display_->ImportBufferCollection(kCaptureBufferCollectionId, std::move(capture_token)));
  ASSERT_OK(fake_display_->ImportBufferCollection(kFramebuffer1BufferCollectionId,
                                                  std::move(framebuffer1_token)));
  ASSERT_OK(fake_display_->ImportBufferCollection(kFramebuffer2BufferCollectionId,
                                                  std::move(framebuffer2_token)));

  const auto kPixelFormat = PixelFormatAndModifier(fuchsia_images2::PixelFormat::kB8G8R8A8,
                                                   fuchsia_images2::PixelFormatModifier::kLinear);

  // Set BufferCollection buffer memory constraints from the display driver's
  // end.
  static constexpr display::ImageBufferUsage kDisplayUsage({
      .tiling_type = display::ImageTilingType::kLinear,
  });
  ASSERT_OK(fake_display_->SetBufferCollectionConstraints(kDisplayUsage,
                                                          kFramebuffer1BufferCollectionId));
  ASSERT_OK(fake_display_->SetBufferCollectionConstraints(kDisplayUsage,
                                                          kFramebuffer2BufferCollectionId));
  static constexpr display::ImageBufferUsage kCaptureUsage({
      .tiling_type = display::ImageTilingType::kCapture,
  });
  ASSERT_OK(
      fake_display_->SetBufferCollectionConstraints(kCaptureUsage, kCaptureBufferCollectionId));

  // Set BufferCollection buffer memory constraints from the test's end.
  const uint32_t bytes_per_pixel = ImageFormatStrideBytesPerWidthPixel(kPixelFormat);
  const uint32_t framebuffer1_size_bytes = kDisplayWidth * kDisplayHeight * bytes_per_pixel;

  fidl::Arena arena;
  auto framebuffer1_set_constraints_request =
      fuchsia_sysmem2::wire::BufferCollectionSetConstraintsRequest::Builder(arena);
  framebuffer1_set_constraints_request.constraints(
      CreateImageConstraints(arena, framebuffer1_size_bytes, kPixelFormat.pixel_format));
  fidl::Status set_framebuffer1_constraints_status =
      framebuffer1_collection_client->SetConstraints(framebuffer1_set_constraints_request.Build());
  ASSERT_TRUE(set_framebuffer1_constraints_status.ok());
  arena.Reset();

  const uint32_t framebuffer2_size_bytes =
      (kDisplayWidth / 2) * (kDisplayHeight / 2) * bytes_per_pixel;

  auto framebuffer2_set_constraints_request =
      fuchsia_sysmem2::wire::BufferCollectionSetConstraintsRequest::Builder(arena);
  framebuffer2_set_constraints_request.constraints(
      CreateImageConstraints(arena, framebuffer2_size_bytes, kPixelFormat.pixel_format));
  fidl::Status set_framebuffer2_constraints_status =
      framebuffer2_collection_client->SetConstraints(framebuffer2_set_constraints_request.Build());
  ASSERT_TRUE(set_framebuffer2_constraints_status.ok());
  arena.Reset();

  auto capture_set_constraints_request =
      fuchsia_sysmem2::wire::BufferCollectionSetConstraintsRequest::Builder(arena);
  capture_set_constraints_request.constraints(
      CreateImageConstraints(arena, framebuffer1_size_bytes, kPixelFormat.pixel_format));
  fidl::Status set_capture_constraints_status =
      capture_collection_client->SetConstraints(capture_set_constraints_request.Build());
  ASSERT_TRUE(set_capture_constraints_status.ok());
  arena.Reset();

  // Both the test-side client and the driver have set the constraints.
  // The buffers should be allocated correctly in sysmem.
  auto [framebuffer1_vmo, framebuffer1_settings] =
      GetAllocatedBufferAndSettings(arena, framebuffer1_collection_client);
  auto [framebuffer2_vmo, framebuffer2_settings] =
      GetAllocatedBufferAndSettings(arena, framebuffer2_collection_client);
  auto [capture_vmo, capture_settings] =
      GetAllocatedBufferAndSettings(arena, capture_collection_client);

  // Fill the framebuffers.
  fzl::VmoMapper framebuffer1_mapper;
  ASSERT_OK(framebuffer1_mapper.Map(framebuffer1_vmo));
  std::span<uint8_t> framebuffer1_bytes(reinterpret_cast<uint8_t*>(framebuffer1_mapper.start()),
                                        framebuffer1_mapper.size());
  const std::vector<uint8_t> kBlueBgraBytes = {0xff, 0, 0, 0xff};
  FillImageWithColor(framebuffer1_bytes, kBlueBgraBytes, kDisplayWidth, kDisplayHeight,
                     framebuffer1_settings.image_format_constraints().bytes_per_row_divisor());
  zx_cache_flush(framebuffer1_bytes.data(), framebuffer1_bytes.size(),
                 ZX_CACHE_FLUSH_DATA | ZX_CACHE_FLUSH_INVALIDATE);
  framebuffer1_mapper.Unmap();

  fzl::VmoMapper framebuffer2_mapper;
  ASSERT_OK(framebuffer2_mapper.Map(framebuffer2_vmo));
  std::span<uint8_t> framebuffer2_bytes(reinterpret_cast<uint8_t*>(framebuffer2_mapper.start()),
                                        framebuffer2_mapper.size());
  const std::vector<uint8_t> kPurpleBgraBytes = {0x88, 0, 0x88, 0xff};
  FillImageWithColor(framebuffer2_bytes, kPurpleBgraBytes, kDisplayWidth / 2, kDisplayHeight / 2,
                     framebuffer2_settings.image_format_constraints().bytes_per_row_divisor());
  zx_cache_flush(framebuffer2_bytes.data(), framebuffer2_bytes.size(),
                 ZX_CACHE_FLUSH_DATA | ZX_CACHE_FLUSH_INVALIDATE);
  framebuffer2_mapper.Unmap();

  // Import capture image.
  zx::result<display::DriverCaptureImageId> capture_import_result =
      fake_display_->ImportImageForCapture(kCaptureBufferCollectionId,
                                           /*buffer_index=*/0);
  ASSERT_OK(capture_import_result);
  ASSERT_NE(display::kInvalidDriverCaptureImageId, capture_import_result.value());

  // Import framebuffer images.
  static constexpr display::ImageMetadata kFramebuffer1ImageMetadata({
      .width = kDisplayWidth,
      .height = kDisplayHeight,
      .tiling_type = display::ImageTilingType::kLinear,
  });

  zx::result<display::DriverImageId> framebuffer1_import_result =
      fake_display_->ImportImage(kFramebuffer1ImageMetadata, kFramebuffer1BufferCollectionId,
                                 /*buffer_index=*/0);
  ASSERT_OK(framebuffer1_import_result);
  ASSERT_NE(display::kInvalidDriverImageId, framebuffer1_import_result.value());

  static constexpr display::ImageMetadata kFramebuffer2ImageMetadata({
      .width = kDisplayWidth / 2,
      .height = kDisplayHeight / 2,
      .tiling_type = display::ImageTilingType::kLinear,
  });
  zx::result<display::DriverImageId> framebuffer2_import_result =
      fake_display_->ImportImage(kFramebuffer2ImageMetadata, kFramebuffer2BufferCollectionId,
                                 /*buffer_index=*/0);
  ASSERT_OK(framebuffer2_import_result);
  ASSERT_NE(display::kInvalidDriverImageId, framebuffer2_import_result.value());

  // Create display configuration.
  static constexpr display::Color kBlackBgra({
      .format = display::PixelFormat::kB8G8R8A8,
      .bytes = {{0x00, 0x00, 0x00, 0xff, 0x00, 0x00, 0x00, 0x00}},
  });
  constexpr fuchsia_math::wire::Point kLayer1TopLeft = {.x = 0, .y = 0};
  constexpr fuchsia_math::wire::Point kLayer2TopLeft = {.x = 10, .y = 20};
  constexpr size_t kLayerCount = 2;
  std::array<const display::DriverLayer, kLayerCount> kLayers = {
      CreateImageLayerConfig(framebuffer1_import_result.value(), kBlackBgra,
                             kFramebuffer1ImageMetadata, kLayer1TopLeft),
      CreateImageLayerConfig(framebuffer2_import_result.value(), kBlackBgra,
                             kFramebuffer2ImageMetadata, kLayer2TopLeft),
  };

  // Check and apply the display configuration.
  display::ConfigCheckResult config_check_result =
      fake_display_->CheckConfiguration(kDisplayId, kModeId, kLayers);
  ASSERT_EQ(display::ConfigCheckResult::kOk, config_check_result);

  static constexpr display::DriverConfigStamp kConfigStamp(1);
  fake_display_->SubmitConfiguration(kDisplayId, kModeId, kLayers, kConfigStamp);

  // Start capture; wait until the capture ends.
  ASSERT_FALSE(engine_listener_.capture_completed().signaled());
  ASSERT_OK(fake_display_->StartCapture(capture_import_result.value()));
  engine_listener_.capture_completed().Wait();
  EXPECT_TRUE(engine_listener_.capture_completed().signaled());

  // Verify the captured image has the same content as the original image.
  constexpr int kCaptureBytesPerPixel = 4;
  uint32_t capture_bytes_per_row_divisor =
      capture_settings.image_format_constraints().bytes_per_row_divisor();
  uint32_t capture_row_stride_bytes =
      fbl::round_up(uint32_t{kDisplayWidth} * kCaptureBytesPerPixel, capture_bytes_per_row_divisor);

  {
    fzl::VmoMapper capture_mapper;
    ASSERT_OK(capture_mapper.Map(capture_vmo));
    std::span<const uint8_t> capture_bytes(reinterpret_cast<const uint8_t*>(capture_mapper.start()),
                                           /*count=*/capture_mapper.size());
    zx_cache_flush(capture_bytes.data(), capture_bytes.size(), ZX_CACHE_FLUSH_DATA);

    for (int row = 0; row < kDisplayHeight; ++row) {
      std::span<const uint8_t> capture_row =
          capture_bytes.subspan(row * capture_row_stride_bytes, capture_row_stride_bytes);
      auto it = capture_row.begin();
      for (int col = 0; col < kDisplayWidth; ++col) {
        std::vector<uint8_t> curr_color(it, it + kCaptureBytesPerPixel);

        bool is_layer2 = [&] {
          if (row < kLayer2TopLeft.y)
            return false;
          if (row >= (kLayer2TopLeft.y + kFramebuffer2ImageMetadata.height()))
            return false;
          if (col < kLayer2TopLeft.x)
            return false;
          if (col >= (kLayer2TopLeft.x + kFramebuffer2ImageMetadata.width()))
            return false;
          return true;
        }();

        // EXPECT_THAT() would be correct here. However, it generates a lot of
        // logging output when the capture functionality is completely broken,
        // greatly reducing the test's helpfulness.
        if (is_layer2) {
          ASSERT_THAT(curr_color, testing::ElementsAreArray(kPurpleBgraBytes))
              << "Color mismatch at row " << row << " column " << col;
        } else {
          ASSERT_THAT(curr_color, testing::ElementsAreArray(kBlueBgraBytes))
              << "Color mismatch at row " << row << " column " << col;
        }
        it += kCaptureBytesPerPixel;
      }
    }
  }

  // Apply a solid color fill configuration so the image can be released.
  static constexpr display::Dimensions kDisplayDimensions(
      {.width = kDisplayWidth, .height = kDisplayHeight});
  std::array<const display::DriverLayer, 1> kColorFillLayers = {
      CreateColorFillLayerConfig(kBlackBgra, kDisplayDimensions),
  };
  static constexpr display::DriverConfigStamp kEmptyConfigStamp(2);
  fake_display_->SubmitConfiguration(kDisplayId, kModeId, kColorFillLayers, kEmptyConfigStamp);

  // Release the image.
  // TODO(https://fxbug.dev/42079040): Consider adding RAII handles to release the
  // imported images and buffer collections.
  fake_display_->ReleaseImage(framebuffer1_import_result.value());
  EXPECT_OK(fake_display_->ReleaseCapture(capture_import_result.value()));

  EXPECT_OK(fake_display_->ReleaseBufferCollection(kFramebuffer2BufferCollectionId));
  EXPECT_OK(fake_display_->ReleaseBufferCollection(kFramebuffer1BufferCollectionId));
  EXPECT_OK(fake_display_->ReleaseBufferCollection(kCaptureBufferCollectionId));
}

class FakeDisplayWithoutCaptureRealSysmemTest : public FakeDisplayRealSysmemTest {
 public:
  FakeDisplayDeviceConfig GetFakeDisplayDeviceConfig() const override {
    return {
        .display_id = kDisplayId,
        .display_mode_id = kModeId,
        .display_mode = display::Mode({
            .active_width = kDisplayWidth,
            .active_height = kDisplayHeight,
            .refresh_rate_millihertz = kRefreshRateHz * 1'000,
        }),
        .engine_info = display::EngineInfo({
            .max_layer_count = 1,
            .max_connected_display_count = 1,
            .is_capture_supported = false,
        }),
        .periodic_vsync = false,
    };
  }
};

TEST_F(FakeDisplayWithoutCaptureRealSysmemTest, ImportImageForCapture) {
  static constexpr display::DriverBufferCollectionId kFakeCollectionId(1);
  static constexpr uint32_t kFakeCollectionIndex = 0;
  EXPECT_STATUS(fake_display_->ImportImageForCapture(kFakeCollectionId, kFakeCollectionIndex),
                zx::error(ZX_ERR_NOT_SUPPORTED));
}

TEST_F(FakeDisplayWithoutCaptureRealSysmemTest, StartCapture) {
  static constexpr display::DriverCaptureImageId kFakeCaptureId(1);
  EXPECT_STATUS(fake_display_->StartCapture(kFakeCaptureId), zx::error(ZX_ERR_NOT_SUPPORTED));
}

TEST_F(FakeDisplayWithoutCaptureRealSysmemTest, ReleaseCapture) {
  static constexpr display::DriverCaptureImageId kFakeCaptureId(1);
  EXPECT_STATUS(fake_display_->ReleaseCapture(kFakeCaptureId), zx::error(ZX_ERR_NOT_SUPPORTED));
}

}  // namespace
}  // namespace fake_display
