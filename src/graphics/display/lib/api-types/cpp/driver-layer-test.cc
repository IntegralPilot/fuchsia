// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/lib/api-types/cpp/driver-layer.h"

#include <fidl/fuchsia.hardware.display.engine/cpp/wire.h>
#include <fidl/fuchsia.hardware.display.types/cpp/wire.h>
#include <fidl/fuchsia.images2/cpp/wire.h>

#include <cstdint>
#include <initializer_list>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/graphics/display/lib/api-types/cpp/alpha-mode.h"
#include "src/graphics/display/lib/api-types/cpp/color.h"
#include "src/graphics/display/lib/api-types/cpp/coordinate-transformation.h"
#include "src/graphics/display/lib/api-types/cpp/driver-image-id.h"
#include "src/graphics/display/lib/api-types/cpp/image-metadata.h"
#include "src/graphics/display/lib/api-types/cpp/image-tiling-type.h"
#include "src/graphics/display/lib/api-types/cpp/pixel-format.h"
#include "src/graphics/display/lib/api-types/cpp/rectangle.h"

namespace display {

namespace {

constexpr DriverImageId kImageId(4242);
constexpr DriverImageId kImageId2(4242);

constexpr Color kFuchsiaRgba({.format = PixelFormat::kR8G8B8A8,
                              .bytes = std::initializer_list<uint8_t>{0xff, 0, 0xff, 0xff, 0, 0, 0,
                                                                      0}});
constexpr Color kRedRgba({.format = PixelFormat::kR8G8B8A8,
                          .bytes = std::initializer_list<uint8_t>{0xff, 0, 0, 0xff, 0, 0, 0, 0}});

constexpr DriverLayer kUpscaledLayer({
    .display_destination = Rectangle({.x = 10, .y = 20, .width = 300, .height = 400}),
    .image_source = Rectangle({.x = 30, .y = 40, .width = 500, .height = 600}),
    .image_id = kImageId,
    .image_metadata =
        ImageMetadata({.width = 700, .height = 800, .tiling_type = ImageTilingType::kLinear}),
    .fallback_color = kFuchsiaRgba,
    .alpha_mode = AlphaMode::kPremultiplied,
    .alpha_coefficient = 0.25f,
    .image_source_transformation = CoordinateTransformation::kReflectX,
});

constexpr DriverLayer kUpscaledLayer2({
    .display_destination = Rectangle({.x = 10, .y = 20, .width = 300, .height = 400}),
    .image_source = Rectangle({.x = 30, .y = 40, .width = 500, .height = 600}),
    .image_id = kImageId2,
    .image_metadata =
        ImageMetadata({.width = 700, .height = 800, .tiling_type = ImageTilingType::kLinear}),
    .fallback_color = kFuchsiaRgba,
    .alpha_mode = AlphaMode::kPremultiplied,
    .alpha_coefficient = 0.25f,
    .image_source_transformation = CoordinateTransformation::kReflectX,
});

constexpr DriverLayer kDownscaledLayer({
    .display_destination = Rectangle({.x = 10, .y = 20, .width = 500, .height = 600}),
    .image_source = Rectangle({.x = 30, .y = 40, .width = 300, .height = 400}),
    .image_id = kImageId,
    .image_metadata =
        ImageMetadata({.width = 700, .height = 800, .tiling_type = ImageTilingType::kLinear}),
    .fallback_color = kFuchsiaRgba,
    .alpha_mode = AlphaMode::kPremultiplied,
    .alpha_coefficient = 0.25f,
    .image_source_transformation = CoordinateTransformation::kReflectX,
});

TEST(DriverLayerTest, EqualityIsReflexive) {
  EXPECT_EQ(kUpscaledLayer, kUpscaledLayer);
  EXPECT_EQ(kUpscaledLayer2, kUpscaledLayer2);
  EXPECT_EQ(kDownscaledLayer, kDownscaledLayer);
}

TEST(DriverLayerTest, EqualityIsSymmetric) {
  EXPECT_EQ(kUpscaledLayer, kUpscaledLayer2);
  EXPECT_EQ(kUpscaledLayer2, kUpscaledLayer);
}

TEST(DriverLayerTest, EqualityForDifferentDisplayDestinations) {
  constexpr DriverLayer kOtherLayer({
      .display_destination = Rectangle({.x = 1, .y = 2, .width = 3, .height = 4}),
      .image_source = Rectangle({.x = 30, .y = 40, .width = 500, .height = 600}),
      .image_id = kImageId,
      .image_metadata =
          ImageMetadata({.width = 700, .height = 800, .tiling_type = ImageTilingType::kLinear}),
      .fallback_color = kFuchsiaRgba,
      .alpha_mode = AlphaMode::kPremultiplied,
      .alpha_coefficient = 0.25f,
      .image_source_transformation = CoordinateTransformation::kReflectX,
  });
  EXPECT_NE(kUpscaledLayer, kOtherLayer);
  EXPECT_NE(kOtherLayer, kUpscaledLayer);
}

TEST(DriverLayerTest, EqualityForDifferentImageSources) {
  constexpr DriverLayer kOtherLayer({
      .display_destination = Rectangle({.x = 10, .y = 20, .width = 300, .height = 400}),
      .image_source = Rectangle({.x = 3, .y = 4, .width = 5, .height = 6}),
      .image_id = kImageId,
      .image_metadata =
          ImageMetadata({.width = 700, .height = 800, .tiling_type = ImageTilingType::kLinear}),
      .fallback_color = kFuchsiaRgba,
      .alpha_mode = AlphaMode::kPremultiplied,
      .alpha_coefficient = 0.25f,
      .image_source_transformation = CoordinateTransformation::kReflectX,
  });
  EXPECT_NE(kUpscaledLayer, kOtherLayer);
  EXPECT_NE(kOtherLayer, kUpscaledLayer);
}

TEST(DriverLayerTest, EqualityForDifferentImageIds) {
  constexpr DriverLayer kOtherLayer({
      .display_destination = Rectangle({.x = 10, .y = 20, .width = 300, .height = 400}),
      .image_source = Rectangle({.x = 30, .y = 40, .width = 500, .height = 600}),
      .image_id = DriverImageId(4343),
      .image_metadata =
          ImageMetadata({.width = 700, .height = 800, .tiling_type = ImageTilingType::kLinear}),
      .fallback_color = kFuchsiaRgba,
      .alpha_mode = AlphaMode::kPremultiplied,
      .alpha_coefficient = 0.25f,
      .image_source_transformation = CoordinateTransformation::kReflectX,
  });
  EXPECT_NE(kUpscaledLayer, kOtherLayer);
  EXPECT_NE(kOtherLayer, kUpscaledLayer);
}

TEST(DriverLayerTest, EqualityForDifferentImageMetadata) {
  constexpr DriverLayer kOtherLayer({
      .display_destination = Rectangle({.x = 10, .y = 20, .width = 300, .height = 400}),
      .image_source = Rectangle({.x = 30, .y = 40, .width = 500, .height = 600}),
      .image_id = kImageId,
      .image_metadata =
          ImageMetadata({.width = 7, .height = 8, .tiling_type = ImageTilingType::kLinear}),
      .fallback_color = kFuchsiaRgba,
      .alpha_mode = AlphaMode::kPremultiplied,
      .alpha_coefficient = 0.25f,
      .image_source_transformation = CoordinateTransformation::kReflectX,
  });
  EXPECT_NE(kUpscaledLayer, kOtherLayer);
  EXPECT_NE(kOtherLayer, kUpscaledLayer);
}

TEST(DriverLayerTest, EqualityForDifferentFallbackColor) {
  constexpr DriverLayer kOtherLayer({
      .display_destination = Rectangle({.x = 10, .y = 20, .width = 300, .height = 400}),
      .image_source = Rectangle({.x = 30, .y = 40, .width = 500, .height = 600}),
      .image_id = kImageId,
      .image_metadata =
          ImageMetadata({.width = 700, .height = 800, .tiling_type = ImageTilingType::kLinear}),
      .fallback_color = kRedRgba,
      .alpha_mode = AlphaMode::kPremultiplied,
      .alpha_coefficient = 0.25f,
      .image_source_transformation = CoordinateTransformation::kReflectX,
  });
  EXPECT_NE(kUpscaledLayer, kOtherLayer);
  EXPECT_NE(kOtherLayer, kUpscaledLayer);
}

TEST(DriverLayerTest, EqualityForDifferentAlphaMode) {
  constexpr DriverLayer kOtherLayer({
      .display_destination = Rectangle({.x = 10, .y = 20, .width = 300, .height = 400}),
      .image_source = Rectangle({.x = 30, .y = 40, .width = 500, .height = 600}),
      .image_id = kImageId,
      .image_metadata =
          ImageMetadata({.width = 700, .height = 800, .tiling_type = ImageTilingType::kLinear}),
      .fallback_color = kFuchsiaRgba,
      .alpha_mode = AlphaMode::kDisable,
      .alpha_coefficient = 0.25f,
      .image_source_transformation = CoordinateTransformation::kReflectX,
  });
  EXPECT_NE(kUpscaledLayer, kOtherLayer);
  EXPECT_NE(kOtherLayer, kUpscaledLayer);
}

TEST(DriverLayerTest, EqualityForDifferentAlphaCoefficient) {
  constexpr DriverLayer kOtherLayer({
      .display_destination = Rectangle({.x = 10, .y = 20, .width = 300, .height = 400}),
      .image_source = Rectangle({.x = 30, .y = 40, .width = 500, .height = 600}),
      .image_id = kImageId,
      .image_metadata =
          ImageMetadata({.width = 700, .height = 800, .tiling_type = ImageTilingType::kLinear}),
      .fallback_color = kFuchsiaRgba,
      .alpha_mode = AlphaMode::kPremultiplied,
      .alpha_coefficient = 0.5f,
      .image_source_transformation = CoordinateTransformation::kReflectX,
  });
  EXPECT_NE(kUpscaledLayer, kOtherLayer);
  EXPECT_NE(kOtherLayer, kUpscaledLayer);
}

TEST(DriverLayerTest, EqualityForDifferentImageSourceTransformation) {
  constexpr DriverLayer kOtherLayer({
      .display_destination = Rectangle({.x = 10, .y = 20, .width = 300, .height = 400}),
      .image_source = Rectangle({.x = 30, .y = 40, .width = 500, .height = 600}),
      .image_id = kImageId,
      .image_metadata =
          ImageMetadata({.width = 7, .height = 8, .tiling_type = ImageTilingType::kLinear}),
      .fallback_color = kFuchsiaRgba,
      .alpha_mode = AlphaMode::kPremultiplied,
      .alpha_coefficient = 0.25f,
      .image_source_transformation = CoordinateTransformation::kRotateCcw90ReflectX,
  });
  EXPECT_NE(kUpscaledLayer, kOtherLayer);
  EXPECT_NE(kOtherLayer, kUpscaledLayer);
}

TEST(DriverLayerTest, FromDesignatedInitializer) {
  static constexpr DriverLayer layer({
      .display_destination = Rectangle({.x = 10, .y = 20, .width = 300, .height = 400}),
      .image_source = Rectangle({.x = 30, .y = 40, .width = 500, .height = 600}),
      .image_id = kImageId,
      .image_metadata =
          ImageMetadata({.width = 700, .height = 800, .tiling_type = ImageTilingType::kLinear}),
      .fallback_color =
          Color({.format = PixelFormat::kR8G8B8A8,
                 .bytes = std::initializer_list<uint8_t>{0xff, 0, 0xff, 0xff, 0, 0, 0, 0}}),
      .alpha_mode = AlphaMode::kPremultiplied,
      .alpha_coefficient = 0.25f,
      .image_source_transformation = CoordinateTransformation::kReflectX,
  });

  EXPECT_EQ(10, layer.display_destination().x());
  EXPECT_EQ(20, layer.display_destination().y());
  EXPECT_EQ(300, layer.display_destination().width());
  EXPECT_EQ(400, layer.display_destination().height());

  EXPECT_EQ(30, layer.image_source().x());
  EXPECT_EQ(40, layer.image_source().y());
  EXPECT_EQ(500, layer.image_source().width());
  EXPECT_EQ(600, layer.image_source().height());

  EXPECT_EQ(kImageId, layer.image_id());

  EXPECT_EQ(700, layer.image_metadata().width());
  EXPECT_EQ(800, layer.image_metadata().height());
  EXPECT_EQ(ImageTilingType::kLinear, layer.image_metadata().tiling_type());

  EXPECT_EQ(PixelFormat::kR8G8B8A8, layer.fallback_color().format());
  EXPECT_THAT(
      layer.fallback_color().bytes(),
      testing::ElementsAreArray(std::initializer_list<uint8_t>{0xff, 0, 0xff, 0xff, 0, 0, 0, 0}));

  EXPECT_EQ(AlphaMode::kPremultiplied, layer.alpha_mode());
  EXPECT_EQ(0.25f, layer.alpha_coefficient());
  EXPECT_EQ(CoordinateTransformation::kReflectX, layer.image_source_transformation());
}

TEST(DriverLayerTest, FromFidlLayer) {
  static constexpr fuchsia_hardware_display_engine::wire::Layer fidl_layer = {
      .display_destination = {.x = 10, .y = 20, .width = 300, .height = 400},
      .image_source = {.x = 30, .y = 40, .width = 500, .height = 600},
      .image_id = fuchsia_hardware_display_engine::wire::ImageId({.value = 4242}),
      .image_metadata = {.dimensions = {.width = 700, .height = 800},
                         .tiling_type =
                             fuchsia_hardware_display_types::wire::kImageTilingTypeLinear},
      .fallback_color = {.format = fuchsia_images2::wire::PixelFormat::kR8G8B8A8,
                         .bytes = {0xff, 0, 0xff, 0xff, 0, 0, 0, 0}},
      .alpha_mode = fuchsia_hardware_display_types::AlphaMode::kPremultiplied,
      .alpha_layer_val = 0.25f,
      .image_source_transformation =
          fuchsia_hardware_display_types::CoordinateTransformation::kReflectX,
  };
  static constexpr DriverLayer layer(fidl_layer);

  EXPECT_EQ(10, layer.display_destination().x());
  EXPECT_EQ(20, layer.display_destination().y());
  EXPECT_EQ(300, layer.display_destination().width());
  EXPECT_EQ(400, layer.display_destination().height());

  EXPECT_EQ(30, layer.image_source().x());
  EXPECT_EQ(40, layer.image_source().y());
  EXPECT_EQ(500, layer.image_source().width());
  EXPECT_EQ(600, layer.image_source().height());

  EXPECT_EQ(kImageId, layer.image_id());

  EXPECT_EQ(700, layer.image_metadata().width());
  EXPECT_EQ(800, layer.image_metadata().height());
  EXPECT_EQ(ImageTilingType::kLinear, layer.image_metadata().tiling_type());

  EXPECT_EQ(PixelFormat::kR8G8B8A8, layer.fallback_color().format());
  EXPECT_THAT(
      layer.fallback_color().bytes(),
      testing::ElementsAreArray(std::initializer_list<uint8_t>{0xff, 0, 0xff, 0xff, 0, 0, 0, 0}));

  EXPECT_EQ(AlphaMode::kPremultiplied, layer.alpha_mode());
  EXPECT_EQ(0.25f, layer.alpha_coefficient());
  EXPECT_EQ(CoordinateTransformation::kReflectX, layer.image_source_transformation());
}

TEST(DriverLayerTest, ToFidlLayer) {
  static constexpr DriverLayer layer({
      .display_destination = Rectangle({.x = 10, .y = 20, .width = 300, .height = 400}),
      .image_source = Rectangle({.x = 30, .y = 40, .width = 500, .height = 600}),
      .image_id = kImageId,
      .image_metadata =
          ImageMetadata({.width = 700, .height = 800, .tiling_type = ImageTilingType::kLinear}),
      .fallback_color =
          Color({.format = PixelFormat::kR8G8B8A8,
                 .bytes = std::initializer_list<uint8_t>{0xff, 0, 0xff, 0xff, 0, 0, 0, 0}}),
      .alpha_mode = AlphaMode::kPremultiplied,
      .alpha_coefficient = 0.25f,
      .image_source_transformation = CoordinateTransformation::kReflectX,
  });
  static constexpr fuchsia_hardware_display_engine::wire::Layer fidl_layer = layer.ToFidl();

  EXPECT_EQ(10u, fidl_layer.display_destination.x);
  EXPECT_EQ(20u, fidl_layer.display_destination.y);
  EXPECT_EQ(300u, fidl_layer.display_destination.width);
  EXPECT_EQ(400u, fidl_layer.display_destination.height);

  EXPECT_EQ(30u, fidl_layer.image_source.x);
  EXPECT_EQ(40u, fidl_layer.image_source.y);
  EXPECT_EQ(500u, fidl_layer.image_source.width);
  EXPECT_EQ(600u, fidl_layer.image_source.height);

  EXPECT_EQ(4242u, fidl_layer.image_id.value);

  EXPECT_EQ(700u, fidl_layer.image_metadata.dimensions.width);
  EXPECT_EQ(800u, fidl_layer.image_metadata.dimensions.height);
  EXPECT_EQ(fuchsia_hardware_display_types::wire::kImageTilingTypeLinear,
            fidl_layer.image_metadata.tiling_type);

  EXPECT_EQ(fuchsia_images2::wire::PixelFormat::kR8G8B8A8, fidl_layer.fallback_color.format);
  EXPECT_THAT(
      fidl_layer.fallback_color.bytes,
      testing::ElementsAreArray(std::initializer_list<uint8_t>{0xff, 0, 0xff, 0xff, 0, 0, 0, 0}));

  EXPECT_EQ(fuchsia_hardware_display_types::wire::AlphaMode::kPremultiplied, fidl_layer.alpha_mode);
  EXPECT_EQ(0.25f, fidl_layer.alpha_layer_val);
  EXPECT_EQ(fuchsia_hardware_display_types::wire::CoordinateTransformation::kReflectX,
            fidl_layer.image_source_transformation);
}

TEST(DriverLayerTest, IsValidFidlScaledLayer) {
  EXPECT_TRUE(DriverLayer::IsValid(fuchsia_hardware_display_engine::wire::Layer{
      .display_destination = {.x = 10, .y = 20, .width = 300, .height = 400},
      .image_source = {.x = 30, .y = 40, .width = 500, .height = 600},
      .image_id = fuchsia_hardware_display_engine::wire::ImageId({.value = 4242}),
      .image_metadata = {.dimensions = {.width = 700, .height = 800},
                         .tiling_type =
                             fuchsia_hardware_display_types::wire::kImageTilingTypeLinear},
      .fallback_color = {.format = fuchsia_images2::wire::PixelFormat::kR8G8B8A8,
                         .bytes = {0xff, 0, 0xff, 0xff, 0, 0, 0, 0}},
      .alpha_mode = fuchsia_hardware_display_types::AlphaMode::kDisable,
      .alpha_layer_val = 0.25f,
      .image_source_transformation =
          fuchsia_hardware_display_types::CoordinateTransformation::kReflectX,
  }));
}

TEST(DriverLayerTest, IsValidFidlScaledLayerWithoutImageId) {
  EXPECT_TRUE(DriverLayer::IsValid(fuchsia_hardware_display_engine::wire::Layer{
      .display_destination = {.x = 10, .y = 20, .width = 300, .height = 400},
      .image_source = {.x = 30, .y = 40, .width = 500, .height = 600},
      .image_id = fuchsia_hardware_display_engine::wire::ImageId(
          {.value = fuchsia_hardware_display_types::wire::kInvalidDispId}),
      .image_metadata = {.dimensions = {.width = 700, .height = 800},
                         .tiling_type =
                             fuchsia_hardware_display_types::wire::kImageTilingTypeLinear},
      .fallback_color = {.format = fuchsia_images2::wire::PixelFormat::kR8G8B8A8,
                         .bytes = {0xff, 0, 0xff, 0xff, 0, 0, 0, 0}},
      .alpha_mode = fuchsia_hardware_display_types::AlphaMode::kDisable,
      .alpha_layer_val = 0.25f,
      .image_source_transformation =
          fuchsia_hardware_display_types::CoordinateTransformation::kReflectX,
  }));
}

TEST(DriverLayerTest, IsValidFidlSolidFillLayer) {
  EXPECT_TRUE(DriverLayer::IsValid(fuchsia_hardware_display_engine::wire::Layer{
      .display_destination = {.x = 10, .y = 20, .width = 300, .height = 400},
      .image_source = {.x = 0, .y = 0, .width = 0, .height = 0},
      .image_id = fuchsia_hardware_display_engine::wire::ImageId(
          {.value = fuchsia_hardware_display_types::wire::kInvalidDispId}),
      .image_metadata = {.dimensions = {.width = 0, .height = 0},
                         .tiling_type =
                             fuchsia_hardware_display_types::wire::kImageTilingTypeLinear},
      .fallback_color = {.format = fuchsia_images2::wire::PixelFormat::kR8G8B8A8,
                         .bytes = {0xff, 0, 0xff, 0xff, 0, 0, 0, 0}},
      .alpha_mode = fuchsia_hardware_display_types::AlphaMode::kDisable,
      .alpha_layer_val = 0.25f,
      .image_source_transformation =
          fuchsia_hardware_display_types::CoordinateTransformation::kIdentity,
  }));
}

TEST(DriverLayerTest, IsValidFidlSolidFillLayerWithImageId) {
  EXPECT_FALSE(DriverLayer::IsValid(fuchsia_hardware_display_engine::wire::Layer{
      .display_destination = {.x = 10, .y = 20, .width = 300, .height = 400},
      .image_source = {.x = 0, .y = 0, .width = 0, .height = 0},
      .image_id = fuchsia_hardware_display_engine::wire::ImageId({.value = 4242}),
      .image_metadata = {.dimensions = {.width = 0, .height = 0},
                         .tiling_type =
                             fuchsia_hardware_display_types::wire::kImageTilingTypeLinear},
      .fallback_color = {.format = fuchsia_images2::wire::PixelFormat::kR8G8B8A8,
                         .bytes = {0xff, 0, 0xff, 0xff, 0, 0, 0, 0}},
      .alpha_mode = fuchsia_hardware_display_types::AlphaMode::kDisable,
      .alpha_layer_val = 0.25f,
      .image_source_transformation =
          fuchsia_hardware_display_types::CoordinateTransformation::kIdentity,
  }));
}

TEST(DriverLayerTest, IsValidFidlSolidFillLayerWithMetadataDimensions) {
  EXPECT_FALSE(DriverLayer::IsValid(fuchsia_hardware_display_engine::wire::Layer{
      .display_destination = {.x = 10, .y = 20, .width = 300, .height = 400},
      .image_source = {.x = 0, .y = 0, .width = 0, .height = 0},
      .image_id = fuchsia_hardware_display_engine::wire::ImageId(
          {.value = fuchsia_hardware_display_types::wire::kInvalidDispId}),
      .image_metadata = {.dimensions = {.width = 700, .height = 800},
                         .tiling_type =
                             fuchsia_hardware_display_types::wire::kImageTilingTypeLinear},
      .fallback_color = {.format = fuchsia_images2::wire::PixelFormat::kR8G8B8A8,
                         .bytes = {0xff, 0, 0xff, 0xff, 0, 0, 0, 0}},
      .alpha_mode = fuchsia_hardware_display_types::AlphaMode::kDisable,
      .alpha_layer_val = 0.25f,
      .image_source_transformation =
          fuchsia_hardware_display_types::CoordinateTransformation::kIdentity,
  }));
}

TEST(DriverLayerTest, IsValidFidlSolidFillLayerWithMetadataTilingType) {
  EXPECT_FALSE(DriverLayer::IsValid(fuchsia_hardware_display_engine::wire::Layer{
      .display_destination = {.x = 10, .y = 20, .width = 300, .height = 400},
      .image_source = {.x = 0, .y = 0, .width = 0, .height = 0},
      .image_id = fuchsia_hardware_display_engine::wire::ImageId(
          {.value = fuchsia_hardware_display_types::wire::kInvalidDispId}),
      .image_metadata = {.dimensions = {.width = 0, .height = 0},
                         .tiling_type =
                             fuchsia_hardware_display_types::wire::kImageTilingTypeCapture},
      .fallback_color = {.format = fuchsia_images2::wire::PixelFormat::kR8G8B8A8,
                         .bytes = {0xff, 0, 0xff, 0xff, 0, 0, 0, 0}},
      .alpha_mode = fuchsia_hardware_display_types::AlphaMode::kDisable,
      .alpha_layer_val = 0.25f,
      .image_source_transformation =
          fuchsia_hardware_display_types::CoordinateTransformation::kIdentity,
  }));
}

TEST(DriverLayerTest, IsValidFidlSolidFillLayerWithTransformation) {
  EXPECT_FALSE(DriverLayer::IsValid(fuchsia_hardware_display_engine::wire::Layer{
      .display_destination = {.x = 10, .y = 20, .width = 300, .height = 400},
      .image_source = {.x = 0, .y = 0, .width = 0, .height = 0},
      .image_id = fuchsia_hardware_display_engine::wire::ImageId(
          {.value = fuchsia_hardware_display_types::wire::kInvalidDispId}),
      .image_metadata = {.dimensions = {.width = 0, .height = 0},
                         .tiling_type =
                             fuchsia_hardware_display_types::wire::kImageTilingTypeLinear},
      .fallback_color = {.format = fuchsia_images2::wire::PixelFormat::kR8G8B8A8,
                         .bytes = {0xff, 0, 0xff, 0xff, 0, 0, 0, 0}},
      .alpha_mode = fuchsia_hardware_display_types::AlphaMode::kDisable,
      .alpha_layer_val = 0.25f,
      .image_source_transformation =
          fuchsia_hardware_display_types::CoordinateTransformation::kReflectX,
  }));
}

TEST(DriverLayerTest, IsValidFidlEmptyDestination) {
  EXPECT_FALSE(DriverLayer::IsValid(fuchsia_hardware_display_engine::wire::Layer{
      .display_destination = {.x = 0, .y = 0, .width = 0, .height = 0},
      .image_source = {.x = 30, .y = 40, .width = 500, .height = 600},
      .image_id = fuchsia_hardware_display_engine::wire::ImageId({.value = 4242}),
      .image_metadata = {.dimensions = {.width = 700, .height = 800},
                         .tiling_type =
                             fuchsia_hardware_display_types::wire::kImageTilingTypeLinear},
      .fallback_color = {.format = fuchsia_images2::wire::PixelFormat::kR8G8B8A8,
                         .bytes = {0xff, 0, 0xff, 0xff, 0, 0, 0, 0}},
      .alpha_mode = fuchsia_hardware_display_types::AlphaMode::kDisable,
      .alpha_layer_val = 0.25f,
      .image_source_transformation =
          fuchsia_hardware_display_types::CoordinateTransformation::kReflectX,
  }));
}

TEST(DriverLayerTest, IsValidFidlScaledLayerWithEmptyMetadataDimensions) {
  EXPECT_FALSE(DriverLayer::IsValid(fuchsia_hardware_display_engine::wire::Layer{
      .display_destination = {.x = 10, .y = 20, .width = 300, .height = 400},
      .image_source = {.x = 30, .y = 40, .width = 500, .height = 600},
      .image_id = fuchsia_hardware_display_engine::wire::ImageId({.value = 4242}),
      .image_metadata = {.dimensions = {.width = 0, .height = 0},
                         .tiling_type =
                             fuchsia_hardware_display_types::wire::kImageTilingTypeLinear},
      .fallback_color = {.format = fuchsia_images2::wire::PixelFormat::kR8G8B8A8,
                         .bytes = {0xff, 0, 0xff, 0xff, 0, 0, 0, 0}},
      .alpha_mode = fuchsia_hardware_display_types::AlphaMode::kDisable,
      .alpha_layer_val = 0.25f,
      .image_source_transformation =
          fuchsia_hardware_display_types::CoordinateTransformation::kReflectX,
  }));
}

}  // namespace

}  // namespace display
