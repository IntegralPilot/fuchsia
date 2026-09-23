// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/lib/api-protocols/cpp/display-engine-interface.h"

#include <fidl/fuchsia.sysmem2/cpp/wire.h>
#include <lib/fit/function.h>
#include <lib/zx/result.h>
#include <zircon/compiler.h>

#include <cstddef>
#include <cstdint>
#include <mutex>
#include <span>
#include <type_traits>
#include <vector>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/graphics/display/lib/api-protocols/cpp/display-engine-interface.h"
#include "src/graphics/display/lib/api-types/cpp/config-check-result.h"
#include "src/graphics/display/lib/api-types/cpp/display-id.h"
#include "src/graphics/display/lib/api-types/cpp/driver-buffer-collection-id.h"
#include "src/graphics/display/lib/api-types/cpp/driver-capture-image-id.h"
#include "src/graphics/display/lib/api-types/cpp/driver-config-stamp.h"
#include "src/graphics/display/lib/api-types/cpp/driver-image-id.h"
#include "src/graphics/display/lib/api-types/cpp/driver-layer.h"
#include "src/graphics/display/lib/api-types/cpp/engine-info.h"
#include "src/graphics/display/lib/api-types/cpp/image-buffer-usage.h"
#include "src/graphics/display/lib/api-types/cpp/image-metadata.h"
#include "src/graphics/display/lib/api-types/cpp/mode-id.h"

namespace display {

namespace {

static_assert(!std::is_copy_constructible_v<DisplayEngineInterface>);
static_assert(!std::is_move_constructible_v<DisplayEngineInterface>);

static_assert(!std::is_final_v<DisplayEngineInterface>);
static_assert(std::is_polymorphic_v<DisplayEngineInterface>);

// Minimally functional strict mock for testing the default method
// implementations in `DisplayEngineInterface`.
class DisplayEngineMinimalMock final : public display::DisplayEngineInterface {
 public:
  using CheckConfigurationChecker = fit::function<display::ConfigCheckResult(
      display::DisplayId display_id, display::ModeId display_mode_id,
      std::span<const display::DriverLayer> layers)>;
  using SubmitConfigurationChecker = fit::function<void(
      display::DisplayId display_id, display::ModeId display_mode_id,
      std::span<const display::DriverLayer> layers, display::DriverConfigStamp config_stamp)>;

  DisplayEngineMinimalMock();
  DisplayEngineMinimalMock(const DisplayEngineMinimalMock&) = delete;
  DisplayEngineMinimalMock& operator=(const DisplayEngineMinimalMock&) = delete;
  ~DisplayEngineMinimalMock();

  void ExpectCheckConfiguration(CheckConfigurationChecker checker);
  void ExpectSubmitConfiguration(SubmitConfigurationChecker checker);

  // Must be called at least once during an instance's lifetime.
  //
  // Tests are recommended to call this in a TearDown() method, or at the end of
  // the test case implementation.
  void CheckAllCallsReplayed();

  // `display::DisplayEngineInterface`:
  EngineInfo CompleteCoordinatorConnection() override;
  zx::result<> ImportBufferCollection(
      display::DriverBufferCollectionId buffer_collection_id,
      fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> buffer_collection_token) override;
  zx::result<> ReleaseBufferCollection(
      display::DriverBufferCollectionId buffer_collection_id) override;
  zx::result<display::DriverImageId> ImportImage(
      const display::ImageMetadata& image_metadata,
      display::DriverBufferCollectionId buffer_collection_id, uint32_t buffer_index) override;
  zx::result<display::DriverCaptureImageId> ImportImageForCapture(
      display::DriverBufferCollectionId buffer_collection_id, uint32_t buffer_index) override;
  void ReleaseImage(display::DriverImageId driver_image_id) override;
  display::ConfigCheckResult CheckConfiguration(
      display::DisplayId display_id, display::ModeId display_mode_id,
      std::span<const display::DriverLayer> layers) override;
  void SubmitConfiguration(display::DisplayId display_id, display::ModeId display_mode_id,
                           std::span<const display::DriverLayer> layers,
                           display::DriverConfigStamp driver_config_stamp) override;
  zx::result<> SetBufferCollectionConstraints(
      const display::ImageBufferUsage& image_buffer_usage,
      display::DriverBufferCollectionId buffer_collection_id) override;

 private:
  struct Expectation {
    CheckConfigurationChecker check_configuration_checker;
    SubmitConfigurationChecker submit_configuration_checker;
  };

  std::mutex mutex_;
  std::vector<Expectation> expectations_ __TA_GUARDED(mutex_);
  size_t call_index_ __TA_GUARDED(mutex_) = 0;
  bool check_all_calls_replayed_called_ __TA_GUARDED(mutex_) = false;
};

DisplayEngineMinimalMock::DisplayEngineMinimalMock() = default;

DisplayEngineMinimalMock::~DisplayEngineMinimalMock() {
  ZX_ASSERT_MSG(check_all_calls_replayed_called_, "CheckAllCallsReplayed() not called on a mock");
}

void DisplayEngineMinimalMock::ExpectCheckConfiguration(CheckConfigurationChecker checker) {
  std::lock_guard<std::mutex> lock(mutex_);
  expectations_.push_back({.check_configuration_checker = std::move(checker)});
}

void DisplayEngineMinimalMock::ExpectSubmitConfiguration(SubmitConfigurationChecker checker) {
  std::lock_guard<std::mutex> lock(mutex_);
  expectations_.push_back({.submit_configuration_checker = std::move(checker)});
}

void DisplayEngineMinimalMock::CheckAllCallsReplayed() {
  std::lock_guard<std::mutex> lock(mutex_);
  ZX_ASSERT_MSG(expectations_.size() == call_index_, "%zu expected calls were not received",
                expectations_.size() - call_index_);
  check_all_calls_replayed_called_ = true;
}

EngineInfo DisplayEngineMinimalMock::CompleteCoordinatorConnection() {
  ZX_PANIC("Received unexpected call type");
  return display::EngineInfo{{}};
}

zx::result<> DisplayEngineMinimalMock::ImportBufferCollection(
    display::DriverBufferCollectionId buffer_collection_id,
    fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> buffer_collection_token) {
  ZX_PANIC("Received unexpected call type");
  return zx::error(ZX_ERR_NOT_SUPPORTED);
}

zx::result<> DisplayEngineMinimalMock::ReleaseBufferCollection(
    display::DriverBufferCollectionId buffer_collection_id) {
  ZX_PANIC("Received unexpected call type");
  return zx::error(ZX_ERR_NOT_SUPPORTED);
}

zx::result<display::DriverImageId> DisplayEngineMinimalMock::ImportImage(
    const display::ImageMetadata& image_metadata,
    display::DriverBufferCollectionId buffer_collection_id, uint32_t buffer_index) {
  ZX_PANIC("Received unexpected call type");
  return zx::error(ZX_ERR_NOT_SUPPORTED);
}

zx::result<display::DriverCaptureImageId> DisplayEngineMinimalMock::ImportImageForCapture(
    display::DriverBufferCollectionId buffer_collection_id, uint32_t buffer_index) {
  ZX_PANIC("Received unexpected call type");
  return zx::error(ZX_ERR_NOT_SUPPORTED);
}

void DisplayEngineMinimalMock::ReleaseImage(display::DriverImageId driver_image_id) {
  ZX_PANIC("Received unexpected call type");
}

display::ConfigCheckResult DisplayEngineMinimalMock::CheckConfiguration(
    display::DisplayId display_id, display::ModeId display_mode_id,
    std::span<const display::DriverLayer> layers) {
  std::lock_guard<std::mutex> lock(mutex_);
  ZX_ASSERT_MSG(call_index_ < expectations_.size(), "All expected calls were already received");
  Expectation& call_expectation = expectations_[call_index_];
  ++call_index_;

  ZX_ASSERT_MSG(call_expectation.check_configuration_checker != nullptr,
                "Received call type does not match expected call type");
  return call_expectation.check_configuration_checker(display_id, display_mode_id, layers);
}

void DisplayEngineMinimalMock::SubmitConfiguration(display::DisplayId display_id,
                                                   display::ModeId display_mode_id,
                                                   std::span<const display::DriverLayer> layers,
                                                   display::DriverConfigStamp driver_config_stamp) {
  std::lock_guard<std::mutex> lock(mutex_);
  ZX_ASSERT_MSG(call_index_ < expectations_.size(), "All expected calls were already received");
  Expectation& call_expectation = expectations_[call_index_];
  ++call_index_;

  ZX_ASSERT_MSG(call_expectation.submit_configuration_checker != nullptr,
                "Received call type does not match expected call type");
  call_expectation.submit_configuration_checker(display_id, display_mode_id, layers,
                                                driver_config_stamp);
}

zx::result<> DisplayEngineMinimalMock::SetBufferCollectionConstraints(
    const display::ImageBufferUsage& image_buffer_usage,
    display::DriverBufferCollectionId buffer_collection_id) {
  ZX_PANIC("Received unexpected call type");
  return zx::error(ZX_ERR_NOT_SUPPORTED);
}

class DisplayEngineInterfaceTest : public ::testing::Test {
 public:
  void TearDown() override { mock_display_engine_.CheckAllCallsReplayed(); }

 protected:
  DisplayEngineMinimalMock mock_display_engine_;
};

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

TEST_F(DisplayEngineInterfaceTest, CheckConfigurationIdentityColorConversionSuccess) {
  static constexpr display::DisplayId kDisplayId(1);
  static constexpr display::ModeId kModeId(2);
  static constexpr std::array<display::DriverLayer, 1> kLayers = {CreateValidLayerWithSeed(0)};

  mock_display_engine_.ExpectCheckConfiguration([&](display::DisplayId display_id,
                                                    display::ModeId mode_id,
                                                    std::span<const display::DriverLayer> layers) {
    EXPECT_EQ(kDisplayId, display_id);
    EXPECT_EQ(kModeId, mode_id);
    EXPECT_THAT(layers, ::testing::ElementsAreArray(kLayers));
    return display::ConfigCheckResult::kOk;
  });

  DisplayEngineInterface& display_engine = mock_display_engine_;
  EXPECT_EQ(display::ConfigCheckResult::kOk,
            display_engine.CheckConfiguration(kDisplayId, kModeId,
                                              display::ColorConversion::kIdentity, kLayers));
}

TEST_F(DisplayEngineInterfaceTest, CheckConfigurationNonIdentityColorConversionError) {
  static constexpr display::DisplayId kDisplayId(1);
  static constexpr display::ModeId kModeId(2);
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
  //
  // The mock is not expected to be called because the default
  // `CheckConfiguration()` implementation in `DisplayEngineInterface` will
  // return `kUnsupportedConfig` when the color conversion is not identity.

  DisplayEngineInterface& display_engine = mock_display_engine_;
  EXPECT_EQ(display::ConfigCheckResult::kUnsupportedConfig,
            display_engine.CheckConfiguration(kDisplayId, kModeId, kColorConversion, kLayers));
}

TEST_F(DisplayEngineInterfaceTest, SubmitConfigurationIdentityColorConversion) {
  static constexpr display::DisplayId kDisplayId(1);
  static constexpr display::ModeId kModeId(2);
  static constexpr std::array<display::DriverLayer, 1> kLayers = {CreateValidLayerWithSeed(0)};
  static constexpr display::DriverConfigStamp kConfigStamp(42);

  mock_display_engine_.ExpectSubmitConfiguration(
      [&](display::DisplayId display_id, display::ModeId mode_id,
          std::span<const display::DriverLayer> layers, display::DriverConfigStamp config_stamp) {
        EXPECT_EQ(kDisplayId, display_id);
        EXPECT_EQ(kModeId, mode_id);
        EXPECT_THAT(layers, ::testing::ElementsAreArray(kLayers));
        EXPECT_EQ(kConfigStamp, config_stamp);
      });

  DisplayEngineInterface& display_engine = mock_display_engine_;
  display_engine.SubmitConfiguration(kDisplayId, kModeId, display::ColorConversion::kIdentity,
                                     kLayers, kConfigStamp);
}

}  // namespace

}  // namespace display
