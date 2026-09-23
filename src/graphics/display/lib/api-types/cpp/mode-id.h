// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_LIB_API_TYPES_CPP_MODE_ID_H_
#define SRC_GRAPHICS_DISPLAY_LIB_API_TYPES_CPP_MODE_ID_H_

#include <fidl/fuchsia.hardware.display.types/cpp/wire.h>
#include <fidl/fuchsia.hardware.display/cpp/wire.h>

#include <cstdint>

#include "src/graphics/display/lib/api-types/cpp/id-type.h"

namespace display::internal {

using ModeIdTraits = DefaultIdTypeTraits<uint16_t, fuchsia_hardware_display_types::wire::ModeId>;

}  // namespace display::internal

namespace display {

// More useful representation of `fuchsia.hardware.display.types/ModeId`.
using ModeId = display::internal::IdType<display::internal::ModeIdTraits>;

constexpr ModeId kInvalidModeId(fuchsia_hardware_display_types::wire::kInvalidModeId);

}  // namespace display

#endif  // SRC_GRAPHICS_DISPLAY_LIB_API_TYPES_CPP_MODE_ID_H_
