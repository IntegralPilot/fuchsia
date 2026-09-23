// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/coordinator/fidl-conversion.h"

#include <fidl/fuchsia.hardware.display.engine/cpp/wire.h>
#include <lib/fdf/cpp/arena.h>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/graphics/display/drivers/coordinator/driver-display-config.h"
#include "src/graphics/display/lib/api-types/cpp/color-conversion.h"
#include "src/graphics/display/lib/api-types/cpp/display-id.h"
#include "src/graphics/display/lib/api-types/cpp/driver-layer.h"
#include "src/graphics/display/lib/api-types/cpp/mode-id.h"

namespace display_coordinator {

namespace {

TEST(FidlConversionTest, ToFidlDisplayConfig) {
  const display::DriverLayer kLayer0({
      .display_destination = display::Rectangle({.x = 10, .y = 20, .width = 300, .height = 400}),
      .image_source = display::Rectangle({.x = 30, .y = 40, .width = 500, .height = 600}),
      .image_id = display::DriverImageId(4242),
      .image_metadata = display::ImageMetadata(
          {.width = 700, .height = 800, .tiling_type = display::ImageTilingType::kLinear}),
      .fallback_color = display::Color({.format = display::PixelFormat::kR8G8B8A8,
                                        .bytes = {{0xff, 0xdd, 0xcc, 0xbb, 0, 0, 0, 0}}}),
      .alpha_mode = display::AlphaMode::kPremultiplied,
      .alpha_coefficient = 0.25f,
      .image_source_transformation = display::CoordinateTransformation::kReflectX,
  });
  const display::DriverLayer kLayer1({
      .display_destination = display::Rectangle({.x = 11, .y = 22, .width = 303, .height = 404}),
      .image_source = display::Rectangle({.x = 33, .y = 44, .width = 505, .height = 606}),
      .image_id = display::DriverImageId(2424),
      .image_metadata = display::ImageMetadata(
          {.width = 707, .height = 808, .tiling_type = display::ImageTilingType::kLinear}),
      .fallback_color = display::Color({.format = display::PixelFormat::kR8G8B8A8,
                                        .bytes = {{0xaa, 0x99, 0x88, 0x77, 0, 0, 0, 0}}}),
      .alpha_mode = display::AlphaMode::kHwMultiply,
      .alpha_coefficient = 0.75f,
      .image_source_transformation = display::CoordinateTransformation::kReflectY,
  });
  const display::DriverLayer kLayers[] = {kLayer0, kLayer1};

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
      .layer_count = 2,
  };

  fdf::Arena arena('TEST');
  fuchsia_hardware_display_engine::wire::DisplayConfig fidl_config =
      ToFidlDisplayConfig(kDisplayConfig, kLayers, arena);

  EXPECT_EQ(fidl_config.display_id.value, 1u);
  EXPECT_EQ(fidl_config.mode_id.value, 2u);
  EXPECT_THAT(fidl_config.color_conversion.preoffsets, ::testing::ElementsAre(0.1f, 0.2f, 0.3f));
  ASSERT_EQ(fidl_config.color_conversion.coefficients.size(), 3u);
  EXPECT_THAT(fidl_config.color_conversion.coefficients[0],
              ::testing::ElementsAre(1.0f, 2.0f, 3.0f));
  EXPECT_THAT(fidl_config.color_conversion.coefficients[1],
              ::testing::ElementsAre(4.0f, 5.0f, 6.0f));
  EXPECT_THAT(fidl_config.color_conversion.coefficients[2],
              ::testing::ElementsAre(7.0f, 8.0f, 9.0f));
  EXPECT_THAT(fidl_config.color_conversion.postoffsets, ::testing::ElementsAre(0.4f, 0.5f, 0.6f));

  ASSERT_EQ(fidl_config.layers.size(), 2u);
  EXPECT_EQ(display::DriverLayer(fidl_config.layers[0]), kLayer0);
  EXPECT_EQ(display::DriverLayer(fidl_config.layers[1]), kLayer1);
}

}  // namespace

}  // namespace display_coordinator
