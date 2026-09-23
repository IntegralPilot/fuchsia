// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/display/internal/display_equivalence.h"

#include <gtest/gtest.h>

namespace display::internal::test {
namespace {

TEST(LayerEquivalence, HashingAndEquality) {
  // Test ImageLayerEquivalence fields individually.
  ImageLayerEquivalence image_layer1, image_layer2;
  std::hash<ImageLayerEquivalence> image_hasher;

  EXPECT_EQ(image_layer1, image_layer2);
  EXPECT_EQ(image_hasher(image_layer1), image_hasher(image_layer2));

  image_layer1.display_destination = Rectangle({.x = 0, .y = 0, .width = 10, .height = 10});
  EXPECT_NE(image_layer1, image_layer2);
  EXPECT_NE(image_hasher(image_layer1), image_hasher(image_layer2));
  image_layer2.display_destination = image_layer1.display_destination;
  EXPECT_EQ(image_layer1, image_layer2);
  EXPECT_EQ(image_hasher(image_layer1), image_hasher(image_layer2));

  image_layer1.image_source = Rectangle({.x = 0, .y = 0, .width = 10, .height = 10});
  EXPECT_NE(image_layer1, image_layer2);
  EXPECT_NE(image_hasher(image_layer1), image_hasher(image_layer2));
  image_layer2.image_source = image_layer1.image_source;
  EXPECT_EQ(image_layer1, image_layer2);
  EXPECT_EQ(image_hasher(image_layer1), image_hasher(image_layer2));

  image_layer1.image_source_transformation = RotateFlip::kRotateCcw90();
  EXPECT_NE(image_layer1, image_layer2);
  EXPECT_NE(image_hasher(image_layer1), image_hasher(image_layer2));
  image_layer2.image_source_transformation = image_layer1.image_source_transformation;
  EXPECT_EQ(image_layer1, image_layer2);
  EXPECT_EQ(image_hasher(image_layer1), image_hasher(image_layer2));

  image_layer1.image_dimensions = Extent2({.width = 100, .height = 200});
  EXPECT_NE(image_layer1, image_layer2);
  EXPECT_NE(image_hasher(image_layer1), image_hasher(image_layer2));
  image_layer2.image_dimensions = image_layer1.image_dimensions;
  EXPECT_EQ(image_layer1, image_layer2);
  EXPECT_EQ(image_hasher(image_layer1), image_hasher(image_layer2));

  image_layer1.image_tiling_type = 1;
  EXPECT_NE(image_layer1, image_layer2);
  EXPECT_NE(image_hasher(image_layer1), image_hasher(image_layer2));
  image_layer2.image_tiling_type = image_layer1.image_tiling_type;
  EXPECT_EQ(image_layer1, image_layer2);
  EXPECT_EQ(image_hasher(image_layer1), image_hasher(image_layer2));

  image_layer1.blend_mode = BlendMode::kPremultipliedAlpha();
  EXPECT_NE(image_layer1, image_layer2);
  EXPECT_NE(image_hasher(image_layer1), image_hasher(image_layer2));
  image_layer2.blend_mode = image_layer1.blend_mode;
  EXPECT_EQ(image_layer1, image_layer2);
  EXPECT_EQ(image_hasher(image_layer1), image_hasher(image_layer2));

  image_layer1.alpha_range = ImageLayerEquivalence::MakeAlphaRange(0.51f);
  image_layer2.alpha_range = ImageLayerEquivalence::MakeAlphaRange(0.49f);
  EXPECT_EQ(image_layer1, image_layer2);
  EXPECT_EQ(image_hasher(image_layer1), image_hasher(image_layer2));
  image_layer2.alpha_range = ImageLayerEquivalence::MakeAlphaRange(0.0f);
  EXPECT_NE(image_layer1, image_layer2);
  EXPECT_NE(image_hasher(image_layer1), image_hasher(image_layer2));
  image_layer2.alpha_range = ImageLayerEquivalence::MakeAlphaRange(0.0f);
  EXPECT_NE(image_layer1, image_layer2);
  EXPECT_NE(image_hasher(image_layer1), image_hasher(image_layer2));

  // Test ColorLayerEquivalence fields individually.
  ColorLayerEquivalence color_layer1, color_layer2;
  std::hash<ColorLayerEquivalence> color_hasher;

  EXPECT_EQ(color_layer1, color_layer2);
  EXPECT_EQ(color_hasher(color_layer1), color_hasher(color_layer2));

  color_layer1.color = {.format = fuchsia_images2::PixelFormat::kA2B10G10R10, .bytes = {}};
  EXPECT_NE(color_layer1, color_layer2);
  EXPECT_NE(color_hasher(color_layer1), color_hasher(color_layer2));
  color_layer2.color = color_layer1.color;
  EXPECT_EQ(color_layer1, color_layer2);
  EXPECT_EQ(color_hasher(color_layer1), color_hasher(color_layer2));

  color_layer1.color = {.format = fuchsia_images2::PixelFormat::kR2G2B2X2,
                        .bytes = {10, 20, 30, 255}};
  EXPECT_NE(color_layer1, color_layer2);
  EXPECT_NE(color_hasher(color_layer1), color_hasher(color_layer2));
  color_layer2.color = color_layer1.color;
  EXPECT_EQ(color_layer1, color_layer2);
  EXPECT_EQ(color_hasher(color_layer1), color_hasher(color_layer2));

  color_layer1.display_destination = Rectangle({.x = 10, .y = 10, .width = 20, .height = 20});
  EXPECT_NE(color_layer1, color_layer2);
  EXPECT_NE(color_hasher(color_layer1), color_hasher(color_layer2));
  color_layer2.display_destination = color_layer1.display_destination;
  EXPECT_EQ(color_layer1, color_layer2);
  EXPECT_EQ(color_hasher(color_layer1), color_hasher(color_layer2));

  LayerEquivalence layer1, layer2;
  std::hash<LayerEquivalence> layer_hasher;

  // Test UninitializedLayerEquivalence
  LayerEquivalence uninitialized_layer1, uninitialized_layer2;
  EXPECT_EQ(uninitialized_layer1, uninitialized_layer2);
  EXPECT_EQ(layer_hasher(uninitialized_layer1), layer_hasher(uninitialized_layer2));

  layer1 = image_layer1;
  layer2 = image_layer1;
  EXPECT_EQ(layer1, layer2);
  EXPECT_EQ(layer_hasher(layer1), layer_hasher(layer2));
  EXPECT_NE(layer1, uninitialized_layer1);
  EXPECT_NE(layer_hasher(layer1), layer_hasher(uninitialized_layer1));

  layer2 = color_layer2;
  EXPECT_NE(layer1, layer2);
  EXPECT_NE(layer_hasher(layer1), layer_hasher(layer2));
  EXPECT_NE(layer2, uninitialized_layer1);
  EXPECT_NE(layer_hasher(layer2), layer_hasher(uninitialized_layer1));

  layer1 = color_layer1;
  EXPECT_EQ(layer1, layer2);
  EXPECT_EQ(layer_hasher(layer1), layer_hasher(layer2));
}

TEST(DisplayEquivalence, HashingAndEquality) {
  DisplayEquivalence spec1, spec2;
  std::hash<DisplayEquivalence> hasher;

  EXPECT_EQ(spec1.layers, spec2.layers);
  EXPECT_EQ(spec1.display_mode, spec2.display_mode);
  EXPECT_EQ(spec1.color_conversion_preoffsets, spec2.color_conversion_preoffsets);
  EXPECT_EQ(spec1.color_conversion_coefficients, spec2.color_conversion_coefficients);
  EXPECT_EQ(spec1.color_conversion_postoffsets, spec2.color_conversion_postoffsets);

  EXPECT_EQ(spec1, spec2);
  EXPECT_EQ(hasher(spec1), hasher(spec2));

  // Test layers
  ImageLayerEquivalence image_layer;
  image_layer.alpha_range = ImageLayerEquivalence::MakeAlphaRange(0.5f);
  ColorLayerEquivalence color_layer;
  color_layer.display_destination = Rectangle({.x = 10, .y = 10, .width = 20, .height = 20});

  spec1.layers.push_back(LayerEquivalence{image_layer});
  EXPECT_NE(spec1, spec2);
  EXPECT_NE(hasher(spec1), hasher(spec2));
  spec2.layers.push_back(LayerEquivalence{image_layer});
  EXPECT_EQ(spec1, spec2);
  EXPECT_EQ(hasher(spec1), hasher(spec2));

  spec1.layers.push_back(LayerEquivalence{color_layer});
  EXPECT_NE(spec1, spec2);
  EXPECT_NE(hasher(spec1), hasher(spec2));
  spec2.layers.push_back(LayerEquivalence{color_layer});
  EXPECT_EQ(spec1, spec2);
  EXPECT_EQ(hasher(spec1), hasher(spec2));

  // Order matters
  spec1.layers = {LayerEquivalence{image_layer}, LayerEquivalence{color_layer}};
  spec2.layers = {LayerEquivalence{color_layer}, LayerEquivalence{image_layer}};
  EXPECT_NE(spec1, spec2);
  EXPECT_NE(hasher(spec1), hasher(spec2));
  spec2.layers = spec1.layers;
  EXPECT_EQ(spec1, spec2);
  EXPECT_EQ(hasher(spec1), hasher(spec2));

  // Test display_mode
  spec1.display_mode =
      types::DisplayMode({.active_area = types::Extent2({.width = 1024, .height = 768}),
                          .refresh_rate_millihertz = 60000,
                          .mode_flags = 0});
  EXPECT_NE(spec1, spec2);
  EXPECT_NE(hasher(spec1), hasher(spec2));
  spec2.display_mode = spec1.display_mode;
  EXPECT_EQ(spec1, spec2);
  EXPECT_EQ(hasher(spec1), hasher(spec2));

  // Test color_conversion_preoffsets
  spec1.color_conversion_preoffsets = {0.1f, 0.2f, 0.3f};
  EXPECT_NE(spec1, spec2);
  EXPECT_NE(hasher(spec1), hasher(spec2));
  spec2.color_conversion_preoffsets = spec1.color_conversion_preoffsets;
  EXPECT_EQ(spec1, spec2);
  EXPECT_EQ(hasher(spec1), hasher(spec2));

  // Test color_conversion_coefficients
  spec1.color_conversion_coefficients = {1.f, 0.f, 0.f, 0.f, 1.f, 0.f, 0.f, 0.f, 1.f};
  EXPECT_NE(spec1, spec2);
  EXPECT_NE(hasher(spec1), hasher(spec2));
  spec2.color_conversion_coefficients = spec1.color_conversion_coefficients;
  EXPECT_EQ(spec1, spec2);
  EXPECT_EQ(hasher(spec1), hasher(spec2));

  // Test color_conversion_postoffsets
  spec1.color_conversion_postoffsets = {0.4f, 0.5f, 0.6f};
  EXPECT_NE(spec1, spec2);
  EXPECT_NE(hasher(spec1), hasher(spec2));
  spec2.color_conversion_postoffsets = spec1.color_conversion_postoffsets;
  EXPECT_EQ(spec1, spec2);
  EXPECT_EQ(hasher(spec1), hasher(spec2));
}

}  // namespace
}  // namespace display::internal::test
