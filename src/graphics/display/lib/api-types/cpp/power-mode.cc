// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/lib/api-types/cpp/power-mode.h"

#include <zircon/assert.h>

#include <cinttypes>
#include <string_view>
#include <type_traits>

namespace display {

static_assert(std::is_standard_layout_v<PowerMode>);
static_assert(std::is_trivially_assignable_v<PowerMode, PowerMode>);
static_assert(std::is_trivially_copyable_v<PowerMode>);
static_assert(std::is_trivially_copy_constructible_v<PowerMode>);
static_assert(std::is_trivially_destructible_v<PowerMode>);
static_assert(std::is_trivially_move_assignable_v<PowerMode>);
static_assert(std::is_trivially_move_constructible_v<PowerMode>);

std::string_view PowerMode::ToString() const {
  switch (power_mode_) {
    case fuchsia_hardware_display_types::wire::PowerMode::kOff:
      return "Off";
    case fuchsia_hardware_display_types::wire::PowerMode::kOn:
      return "On";
    case fuchsia_hardware_display_types::wire::PowerMode::kDoze:
      return "Doze";
    case fuchsia_hardware_display_types::wire::PowerMode::kDozeSuspend:
      return "DozeSuspend";
    default:
      return "(unknown)";
  }
}

}  // namespace display
