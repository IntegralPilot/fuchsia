// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/lib/api-types/cpp/image-tiling-type.h"

#include <zircon/assert.h>

#include <string_view>
#include <type_traits>

namespace display {

static_assert(std::is_standard_layout_v<ImageTilingType>);
static_assert(std::is_trivially_assignable_v<ImageTilingType, ImageTilingType>);
static_assert(std::is_trivially_copyable_v<ImageTilingType>);
static_assert(std::is_trivially_copy_constructible_v<ImageTilingType>);
static_assert(std::is_trivially_destructible_v<ImageTilingType>);
static_assert(std::is_trivially_move_assignable_v<ImageTilingType>);
static_assert(std::is_trivially_move_constructible_v<ImageTilingType>);

std::string_view ImageTilingType::ToString() const {
  switch (tiling_type_id_) {
    case fuchsia_hardware_display_types::wire::kImageTilingTypeLinear:
      return "Linear";
    case fuchsia_hardware_display_types::wire::kImageTilingTypeCapture:
      return "Capture";
  }
  return "(vendor-specific value)";
}

}  // namespace display
