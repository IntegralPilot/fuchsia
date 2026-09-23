// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_TESTING_SOFTWARE_COMPOSITOR_PIXEL_H_
#define SRC_GRAPHICS_DISPLAY_TESTING_SOFTWARE_COMPOSITOR_PIXEL_H_

#include <fidl/fuchsia.images2/cpp/wire.h>
#include <zircon/assert.h>

#include <array>
#include <cinttypes>
#include <cstdint>
#include <span>

namespace software_compositor {

// Specifies the channel layout and internal channel representation within a
// pixel.
//
// Corresponds to `VkFormat` in Vulkan specification.
enum class PixelFormat {
  kRgba8888,
  kBgra8888,
};

constexpr PixelFormat ToPixelFormat(fuchsia_images2::PixelFormat fidl_pixel_format) {
  switch (fidl_pixel_format) {
    case fuchsia_images2::PixelFormat::kR8G8B8A8:
      return PixelFormat::kRgba8888;
    case fuchsia_images2::PixelFormat::kB8G8R8A8:
      return PixelFormat::kBgra8888;
    default:
      ZX_PANIC("Unsupported pixel format %" PRIu32, static_cast<uint32_t>(fidl_pixel_format));
  }
}

constexpr int GetBytesPerPixel(PixelFormat pixel_format) {
  switch (pixel_format) {
    case PixelFormat::kRgba8888:
    case PixelFormat::kBgra8888:
      return 4;
    default:
      ZX_DEBUG_ASSERT_MSG(false, "Invalid pixel format %d", static_cast<int>(pixel_format));
  }
}

// A Pixel (or texels for textures) represents a color on a specific point
// on the dot-matrix display. It may contain one or multiple channels (color
// components).
//
// PixelData stores the raw byte representation of channel values of a pixel
// (i.e. pixel data) without caring about the internal channel representation
// or the pixel format.
struct PixelData {
  std::array<uint8_t, 4> data;

  static PixelData FromRaw(std::span<const uint8_t> raw);
  PixelData Convert(PixelFormat from, PixelFormat to) const;
};

}  // namespace software_compositor

#endif  // SRC_GRAPHICS_DISPLAY_TESTING_SOFTWARE_COMPOSITOR_PIXEL_H_
