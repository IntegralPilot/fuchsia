// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/lib/api-types/cpp/pixel-format.h"

#include <zircon/assert.h>

#include <cinttypes>
#include <string_view>
#include <type_traits>

namespace display {

static_assert(std::is_standard_layout_v<PixelFormat>);
static_assert(std::is_trivially_assignable_v<PixelFormat, PixelFormat>);
static_assert(std::is_trivially_copyable_v<PixelFormat>);
static_assert(std::is_trivially_copy_constructible_v<PixelFormat>);
static_assert(std::is_trivially_destructible_v<PixelFormat>);
static_assert(std::is_trivially_move_assignable_v<PixelFormat>);
static_assert(std::is_trivially_move_constructible_v<PixelFormat>);

std::string_view PixelFormat::ToString() const {
  switch (format_) {
    case fuchsia_images2::wire::PixelFormat::kR8G8B8A8:
      return "R8G8B8A8";
    case fuchsia_images2::wire::PixelFormat::kB8G8R8A8:
      return "B8G8R8A8";
    case fuchsia_images2::wire::PixelFormat::kI420:
      return "I420";
    case fuchsia_images2::wire::PixelFormat::kNv12:
      return "NV12";
    case fuchsia_images2::wire::PixelFormat::kYuy2:
      return "YUY2";
    case fuchsia_images2::wire::PixelFormat::kYv12:
      return "YV12";
    case fuchsia_images2::wire::PixelFormat::kB8G8R8:
      return "B8G8R8";
    case fuchsia_images2::wire::PixelFormat::kR5G6B5:
      return "R5G6B5";
    case fuchsia_images2::wire::PixelFormat::kR3G3B2:
      return "R3G3B2";
    case fuchsia_images2::wire::PixelFormat::kL8:
      return "L8";
    case fuchsia_images2::wire::PixelFormat::kR8:
      return "R8";
    case fuchsia_images2::wire::PixelFormat::kA2R10G10B10:
      return "A2R10G10B10";
    case fuchsia_images2::wire::PixelFormat::kA2B10G10R10:
      return "A2B10G10R10";
    case fuchsia_images2::wire::PixelFormat::kP010:
      return "P010";
    case fuchsia_images2::wire::PixelFormat::kR8G8B8:
      return "R8G8B8";
    default:
      ZX_DEBUG_ASSERT_MSG(false, "Invalid PixelFormat value: %" PRIu32, ValueForLogging());
      return "(invalid value)";
  }
}

}  // namespace display
