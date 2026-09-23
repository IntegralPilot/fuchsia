// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/lib/api-types/cpp/coordinate-transformation.h"

#include <zircon/assert.h>

#include <cinttypes>
#include <string_view>
#include <type_traits>

namespace display {

static_assert(std::is_standard_layout_v<CoordinateTransformation>);
static_assert(std::is_trivially_assignable_v<CoordinateTransformation, CoordinateTransformation>);
static_assert(std::is_trivially_copyable_v<CoordinateTransformation>);
static_assert(std::is_trivially_copy_constructible_v<CoordinateTransformation>);
static_assert(std::is_trivially_destructible_v<CoordinateTransformation>);
static_assert(std::is_trivially_move_assignable_v<CoordinateTransformation>);
static_assert(std::is_trivially_move_constructible_v<CoordinateTransformation>);

std::string_view CoordinateTransformation::ToString() const {
  switch (transformation_) {
    case fuchsia_hardware_display_types::wire::CoordinateTransformation::kIdentity:
      return "Identity";
    case fuchsia_hardware_display_types::wire::CoordinateTransformation::kReflectX:
      return "ReflectX";
    case fuchsia_hardware_display_types::wire::CoordinateTransformation::kReflectY:
      return "ReflectY";
    case fuchsia_hardware_display_types::wire::CoordinateTransformation::kRotateCcw90:
      return "RotateCcw90";
    case fuchsia_hardware_display_types::wire::CoordinateTransformation::kRotateCcw180:
      return "RotateCcw180";
    case fuchsia_hardware_display_types::wire::CoordinateTransformation::kRotateCcw270:
      return "RotateCcw270";
    case fuchsia_hardware_display_types::wire::CoordinateTransformation::kRotateCcw90ReflectX:
      return "RotateCcw90ReflectX";
    case fuchsia_hardware_display_types::wire::CoordinateTransformation::kRotateCcw90ReflectY:
      return "RotateCcw90ReflectY";
  }

  ZX_DEBUG_ASSERT_MSG(false, "Invalid CoordinateTransformation value: %" PRIu32, ValueForLogging());
  return "(invalid value)";
}

}  // namespace display
