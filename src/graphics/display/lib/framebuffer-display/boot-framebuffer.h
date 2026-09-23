// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_LIB_FRAMEBUFFER_DISPLAY_BOOT_FRAMEBUFFER_H_
#define SRC_GRAPHICS_DISPLAY_LIB_FRAMEBUFFER_DISPLAY_BOOT_FRAMEBUFFER_H_

#include <lib/zbi-format/graphics.h>
#include <cstdint>
#include <limits>
#include <optional>

namespace framebuffer_display {

inline std::optional<uint64_t> BootFramebufferSize(const zbi_swfb_t& info) {
  switch (info.format) {
    case ZBI_PIXEL_FORMAT_RGB_X888:
    case ZBI_PIXEL_FORMAT_ARGB_8888:
    case ZBI_PIXEL_FORMAT_BGR_888_X:
    case ZBI_PIXEL_FORMAT_ABGR_8888:
    case ZBI_PIXEL_FORMAT_ARGB_2_10_10_10:
    case ZBI_PIXEL_FORMAT_ABGR_2_10_10_10:
      break;
    default:
      return std::nullopt;
  }
  if (info.base == 0 || info.width == 0 || info.height == 0 || info.stride < info.width ||
      info.width > INT32_MAX || info.height > INT32_MAX || info.stride > INT32_MAX) {
    return std::nullopt;
  }
  const uint64_t size = uint64_t{info.stride} * info.height * 4;
  // The display backend and sysmem constraints use 32-bit byte counts.
  if (size > UINT32_MAX || info.base > UINT64_MAX - size - 4095) {
    return std::nullopt;
  }
  return size;
}

inline uint32_t PackBgra8888(uint32_t pixel, zbi_pixel_format_t format) {
  uint32_t b = pixel & 0xff;
  uint32_t g = (pixel >> 8) & 0xff;
  uint32_t r = (pixel >> 16) & 0xff;
  if (format == ZBI_PIXEL_FORMAT_ABGR_8888 || format == ZBI_PIXEL_FORMAT_BGR_888_X ||
      format == ZBI_PIXEL_FORMAT_ABGR_2_10_10_10) {
    const uint32_t tmp = r;
    r = b;
    b = tmp;
  }
  if (format == ZBI_PIXEL_FORMAT_ARGB_2_10_10_10 ||
      format == ZBI_PIXEL_FORMAT_ABGR_2_10_10_10) {
    // Replicate high bits so both black and full-intensity white are exact.
    r = (r << 2) | (r >> 6);
    g = (g << 2) | (g >> 6);
    b = (b << 2) | (b >> 6);
    return 0xc0000000 | (r << 20) | (g << 10) | b;
  }
  return 0xff000000 | (r << 16) | (g << 8) | b;
}

}  // namespace framebuffer_display
#endif  // SRC_GRAPHICS_DISPLAY_LIB_FRAMEBUFFER_DISPLAY_BOOT_FRAMEBUFFER_H_
