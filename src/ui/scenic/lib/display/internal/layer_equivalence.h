// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_DISPLAY_INTERNAL_LAYER_EQUIVALENCE_H_
#define SRC_UI_SCENIC_LIB_DISPLAY_INTERNAL_LAYER_EQUIVALENCE_H_

#include <fidl/fuchsia.hardware.display.types/cpp/fidl.h>
#include <fidl/fuchsia.math/cpp/fidl.h>

#include <variant>

#include "src/ui/scenic/lib/display/fidl_typedefs.h"
#include "src/ui/scenic/lib/display/typedefs.h"
#include "src/ui/scenic/lib/types/util/hash_combine.h"

namespace display::internal {

// Variant sub-type of `LayerEquivalence`.
//
// Represents the subset of an Image Layer's configuration that defines its equivalence class for
// the `fuchsia.hardware.display.Coordinator/CheckConfig()` method.  This includes properties like
// dimensions, format, transformation, and alpha, but excludes fields like the specific `ImageId`,
// which don't affect hardware compatibility for a given frame.
//
// TODO(https://fxbug.dev/446183922): it seems like the image's pixel format must be a meaningful
// datum to take into account, but I can't find any documentation/code in the display stack to
// support this theory.
struct ImageLayerEquivalence {
  // Represents potentially-meaningful ranges of alpha values.  If `CheckConfig()` succeeded with an
  // alpha value of 0.49, it doesn't need to be checked again if the alpha value changes to 0.50.
  // Zero and one are treated specially, since they might be optimized by the display driver.
  enum AlphaRange : uint8_t {
    kAlphaZero,
    kAlphaBetweenZeroAndOne,  // exclusive: doesn't include 0.0 nor 1.0
    kAlphaOne,
  };
  static AlphaRange MakeAlphaRange(float alpha) {
    if (alpha >= 1.0) {
      return kAlphaOne;
    }
    if (alpha <= 0.0) {
      return kAlphaZero;
    }
    return kAlphaBetweenZeroAndOne;
  }

  Rectangle display_destination = Rectangle({.x = 0, .y = 0, .width = 0, .height = 0});
  Rectangle image_source = Rectangle({.x = 0, .y = 0, .width = 0, .height = 0});
  RotateFlip image_source_transformation = RotateFlip::kIdentity();

  // Together, `image_dimensions` and `image_tiling_type` are equivalent to a
  // `fuchsia.hardware.display.types/ImageMetadata`.
  Extent2 image_dimensions = Extent2({.width = 0, .height = 0});
  uint32_t image_tiling_type = fuchsia_hardware_display_types::kImageTilingTypeLinear;

  BlendMode blend_mode = BlendMode::kReplace();
  AlphaRange alpha_range = kAlphaOne;

  constexpr bool operator==(const ImageLayerEquivalence& other) const {
    return display_destination == other.display_destination && image_source == other.image_source &&
           image_source_transformation == other.image_source_transformation &&
           image_dimensions == other.image_dimensions &&
           image_tiling_type == other.image_tiling_type && blend_mode == other.blend_mode &&
           alpha_range == other.alpha_range;
  }
};

// Variant sub-type of `LayerEquivalence`.
//
// Represents the subset of a Color Layer's configuration that defines its equivalence class for the
// `fuchsia.hardware.display.Coordinator/CheckConfig()` method.
struct ColorLayerEquivalence {
  WireColor color = {
      .format = fuchsia_images2::PixelFormat::kInvalid,
      .bytes = {},
  };
  Rectangle display_destination = Rectangle({.x = 0, .y = 0, .width = 0, .height = 0});

  constexpr bool operator==(const ColorLayerEquivalence& other) const {
    return color.format == other.color.format && color.bytes == other.color.bytes &&
           display_destination == other.display_destination;
  }
};

// Variant sub-type of `LayerEquivalence`.  This is the default value for a new `LayerEquivalence`.
struct UninitializedLayerEquivalence {
  constexpr bool operator==(const UninitializedLayerEquivalence& other) const { return true; }
};

// Represents the `CheckConfig()`-relevant properties of a single layer, defining its equivalence
// class.  This is a variant type which initially holds an `UninitializedLayerEquivalence`, and
// which can subsequently be set to either an `ImageLayerEquivalence` or a `ColorLayerEquivalence`.
struct LayerEquivalence {
  std::variant<UninitializedLayerEquivalence, ImageLayerEquivalence, ColorLayerEquivalence> config;

  constexpr bool operator==(const LayerEquivalence& other) const { return config == other.config; }

  LayerEquivalence& operator=(const ImageLayerEquivalence& image_le) {
    config = image_le;
    return *this;
  }

  LayerEquivalence& operator=(const ColorLayerEquivalence& color_le) {
    config = color_le;
    return *this;
  }
};

std::ostream& operator<<(std::ostream& str, const LayerEquivalence& le);
std::ostream& operator<<(std::ostream& str, const ImageLayerEquivalence& le);
std::ostream& operator<<(std::ostream& str, const ColorLayerEquivalence& le);
std::ostream& operator<<(std::ostream& str, const UninitializedLayerEquivalence& le);

}  // namespace display::internal

namespace std {

template <>
struct hash<display::internal::ImageLayerEquivalence> {
  std::size_t operator()(const display::internal::ImageLayerEquivalence& image_le) {
    // Random seed (`openssl rand -hex 8`) avoids collisions with types with the same memory layout.
    std::size_t seed = 0x4240a6155ac4cdfa;
    types::hash_combine(seed, image_le.display_destination);
    types::hash_combine(seed, image_le.image_source);
    types::hash_combine(seed, image_le.image_source_transformation);
    types::hash_combine(seed, image_le.image_dimensions);
    types::hash_combine(seed, image_le.image_tiling_type);
    types::hash_combine(seed, image_le.blend_mode);
    types::hash_combine(seed, image_le.alpha_range);
    return seed;
  }
};

template <>
struct hash<display::internal::ColorLayerEquivalence> {
  std::size_t operator()(const display::internal::ColorLayerEquivalence& color_le) {
    // Random seed (`openssl rand -hex 8`) avoids collisions with types with the same memory layout.
    std::size_t seed = 0xed5f7145816d6d96;
    types::hash_combine(seed, static_cast<uint32_t>(color_le.color.format));
    {
      // There's no std::hash for std:array.
      const uint64_t bytes = std::bit_cast<uint64_t>(color_le.color.bytes);
      types::hash_combine(seed, bytes);
    }
    types::hash_combine(seed, color_le.display_destination);

    return seed;
  }
};

template <>
struct hash<display::internal::UninitializedLayerEquivalence> {
  std::size_t operator()(const display::internal::UninitializedLayerEquivalence& uninitialized_le) {
    // Random seed (`openssl rand -hex 8`) avoids collisions with types with the same memory layout.
    return 0x493a4982c7cb5c28;
  }
};

template <>
struct hash<display::internal::LayerEquivalence> {
  std::size_t operator()(const display::internal::LayerEquivalence& le) {
    // Random seed (`openssl rand -hex 8`) avoids collisions with types with the same memory layout.
    std::size_t seed = 0x06cc66fb9d5ca0c5;
    if (const display::internal::ImageLayerEquivalence* image_le =
            std::get_if<display::internal::ImageLayerEquivalence>(&le.config)) {
      types::hash_combine(seed, *image_le);
      return seed;
    }
    if (const display::internal::ColorLayerEquivalence* color_le =
            std::get_if<display::internal::ColorLayerEquivalence>(&le.config)) {
      types::hash_combine(seed, *color_le);
      return seed;
    }
    if (const display::internal::UninitializedLayerEquivalence* uninitialized_le =
            std::get_if<display::internal::UninitializedLayerEquivalence>(&le.config)) {
      types::hash_combine(seed, *uninitialized_le);
      return seed;
    }
    __UNREACHABLE;
    return seed;
  }
};

}  // namespace std

#endif  // SRC_UI_SCENIC_LIB_DISPLAY_INTERNAL_LAYER_EQUIVALENCE_H_
