// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_DRIVERS_AMLOGIC_DISPLAY_PIXEL_GRID_SIZE2D_H_
#define SRC_GRAPHICS_DISPLAY_DRIVERS_AMLOGIC_DISPLAY_PIXEL_GRID_SIZE2D_H_

namespace amlogic_display {

// Represents the size of a non-empty regular 2D grid of pixels.
//
// The value is valid iff `width` and `height` are positive and do not exceed
// the maximum allowed values.
//
// The default-constructed value is invalid.
struct PixelGridSize2D {
  // Amlogic S905D2, S905D3, T931 and A311D SoCs support up to 4K x 2K
  // display output. The values below are the maximum sizes that can be
  // expressed in the OSD layer, encoder and transmitter register fields;
  // these are very loose upper bounds.
  static constexpr int kMaxWidth = 8191;
  static constexpr int kMaxHeight = 4095;

  // In pixels.
  int width = 0;

  // In pixels.
  int height = 0;

  constexpr bool IsValid() const {
    return width > 0 && height > 0 && width <= kMaxWidth && height <= kMaxHeight;
  }
};

constexpr bool operator==(const PixelGridSize2D& a, const PixelGridSize2D& b) noexcept {
  return a.width == b.width && a.height == b.height;
}

constexpr bool operator!=(const PixelGridSize2D& a, const PixelGridSize2D& b) noexcept {
  return !(a == b);
}

constexpr PixelGridSize2D kInvalidPixelGridSize2D = {.width = 0, .height = 0};

}  // namespace amlogic_display

#endif  // SRC_GRAPHICS_DISPLAY_DRIVERS_AMLOGIC_DISPLAY_PIXEL_GRID_SIZE2D_H_
