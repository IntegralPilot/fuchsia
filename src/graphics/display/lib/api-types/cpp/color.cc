// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/lib/api-types/cpp/color.h"

#include <fidl/fuchsia.hardware.display.engine/cpp/wire.h>

#include <type_traits>

namespace display {

static_assert(std::is_standard_layout_v<Color>);
static_assert(std::is_trivially_assignable_v<Color, Color>);
static_assert(std::is_trivially_copyable_v<Color>);
static_assert(std::is_trivially_copy_constructible_v<Color>);
static_assert(std::is_trivially_destructible_v<Color>);
static_assert(std::is_trivially_move_assignable_v<Color>);
static_assert(std::is_trivially_move_constructible_v<Color>);

// static
void Color::StaticAsserts() {
  static_assert(
      kBytesElements == decltype(fuchsia_hardware_display_types::wire::Color::bytes)::size(),
      "FIDL fuchsia.hardware.display.types/Color bytes size doesn't match Color");
}

}  // namespace display
