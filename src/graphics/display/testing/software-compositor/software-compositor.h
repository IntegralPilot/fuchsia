// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_TESTING_SOFTWARE_COMPOSITOR_SOFTWARE_COMPOSITOR_H_
#define SRC_GRAPHICS_DISPLAY_TESTING_SOFTWARE_COMPOSITOR_SOFTWARE_COMPOSITOR_H_

#include <cstdint>
#include <span>

#include "src/graphics/display/lib/api-types/cpp/alpha-mode.h"
#include "src/graphics/display/lib/api-types/cpp/color.h"
#include "src/graphics/display/lib/api-types/cpp/coordinate-transformation.h"
#include "src/graphics/display/lib/api-types/cpp/rectangle.h"
#include "src/graphics/display/testing/software-compositor/pixel.h"

namespace software_compositor {

struct Offset2D {
  int x;
  int y;
};

inline bool operator==(const Offset2D& lhs, const Offset2D& rhs) {
  return lhs.x == rhs.x && lhs.y == rhs.y;
}

inline bool operator!=(const Offset2D& lhs, const Offset2D& rhs) { return !(lhs == rhs); }

struct ImageProperties {
  int width;
  int height;
  int stride_bytes;
  PixelFormat pixel_format;
};

struct OutputImage {
 public:
  inline PixelData At(const Offset2D& offset) const;
  inline void SetPixelData(const Offset2D& offset, const PixelData& color) const;

  std::span<uint8_t> buffer;
  ImageProperties properties;
};

struct InputImage {
 public:
  inline PixelData At(const Offset2D& offset) const;

  static const InputImage kNoInputImage;

  // Empty iff the input has no image.
  std::span<const uint8_t> buffer;
  ImageProperties properties;
};

// static
constexpr InputImage InputImage::kNoInputImage = {
    .buffer = {},
    .properties = {},
};

// A compositor using software rendering to fill solid colors and draw images
// onto an output image (canvas).
class SoftwareCompositor {
 public:
  // Properties of composition of a layer onto the canvas.
  struct CompositionProperties {
    // Only the `image_source` part of the input image will be clipped and used.
    //
    // Equivalent to the `image_source` field in FIDL
    // [`fuchsia.hardware.display.engine/Layer`] struct.
    ::display::Rectangle image_source;

    // The destination (canvas) frame. The clipped input image will be scaled
    // and painted onto the `canvas_destination` region of the canvas.
    // `canvas_destination` will be cropped to the extent of the canvas.
    //
    // Equivalent to the `display_destination` field in FIDL
    // [`fuchsia.hardware.display.engine/Layer`] struct.
    ::display::Rectangle canvas_destination;

    // Indicates how image will be transformed (rotated / flipped).
    //
    // Equivalent to the `image_source_transformation` field in FIDL
    // [`fuchsia.hardware.display.engine/Layer`] struct.
    ::display::CoordinateTransformation transform;

    // Indicates whether alpha blending will be performed and the type of alpha
    // blending.
    //
    // Equivalent to the `alpha_mode` field in FIDL
    // [`fuchsia.hardware.display.engine/Layer`] struct.
    ::display::AlphaMode alpha_mode;

    // The color to use if the layer has no image.
    //
    // Equivalent to the `fallback_color` field in FIDL
    // [`fuchsia.hardware.display.engine/Layer`] struct.
    ::display::Color fallback_color;
  };

  // Represents a layer to be composited on the canvas.
  struct LayerForComposition {
    InputImage image;
    CompositionProperties properties;
  };

  // The buffer backing `canvas` must outlive the newly created instance.
  explicit SoftwareCompositor(const OutputImage& canvas);

  // Clears the canvas by filling a solid `color` of format `pixel_format` on
  // the whole canvas.
  //
  // This produces the same result as Vulkan command `vkCmdClearColorImage`.
  void ClearCanvas(const PixelData& color, PixelFormat pixel_format) const;

  // Composites all the layers in `image_layers` onto the the canvas using
  // composition properties specified in each image layer.
  //
  // Canvas pixels not covered by any image layer in `image_layers` will not
  // be modified.
  //
  // Image layers are sorted by z-index in ascending order, i.e. image layer
  // in the front of the `image_layers` list will be composited first and on the
  // bottom, the layer in the end will be composited the last and on the top.
  //
  // For each ImageLayerForComposition, its `composition_properties` must
  // fulfill the following constraints:
  // - `alpha_mode` must be kDisable.
  // - `transform` must be kIdentity.
  // - the source frame `source_frame` must start at (0, 0) and have the same
  //   size as `input_image`.
  // - the destination frame `canvas_frame` must fall completely within the
  //   canvas and have the same size as `input_image`.
  // TODO(https://fxbug.dev/42075534): Supports more composition properties.
  // TODO(https://fxbug.dev/42080652): Instead of providing a separate ClearCanvas()
  // command, we should integrate background filling into
  // CompositeImageLayers().
  void CompositeLayers(std::span<const LayerForComposition> image_layers) const;

 private:
  // Composites the `input_image` onto the top of the canvas using given
  // `composition_properties`.
  //
  // For composition without alpha composition nor transformation, this produces
  // the same result as the Vulkan command `vkCmdBlitImage`.
  //
  // Currently only the following `composition_properties` is supported:
  // - `alpha_mode` must be kDisable.
  // - `transform` must be kIdentity.
  // - the source frame `source_frame` must start at (0, 0) and have the same
  //   size as `input_image`.
  // - the destination frame `canvas_frame` must fall completely within the
  //   canvas and have the same size as `input_image`.
  // TODO(https://fxbug.dev/42075534): Supports more composition properties.
  void CompositeImage(const InputImage& input_image,
                      const CompositionProperties& composition_properties) const;

  // Composites the `fallback_color` onto the top of the canvas using given
  // `composition_properties`.
  //
  // Currently only the following `composition_properties` is supported:
  // - `alpha_mode` must be kDisable.
  // - `transform` must be kIdentity.
  // - the destination frame `canvas_frame` must fall completely within the
  //   canvas and have the same size as `input_image`.
  // TODO(https://fxbug.dev/42075534): Supports more composition properties.
  void CompositeFallbackColor(const CompositionProperties& composition_properties) const;

  const OutputImage canvas_;
};

}  // namespace software_compositor

#endif  // SRC_GRAPHICS_DISPLAY_TESTING_SOFTWARE_COMPOSITOR_SOFTWARE_COMPOSITOR_H_
