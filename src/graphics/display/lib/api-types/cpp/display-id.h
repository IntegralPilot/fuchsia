// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_LIB_API_TYPES_CPP_DISPLAY_ID_H_
#define SRC_GRAPHICS_DISPLAY_LIB_API_TYPES_CPP_DISPLAY_ID_H_

#include <fidl/fuchsia.hardware.display.types/cpp/wire.h>

#include <cstdint>

#include "src/graphics/display/lib/api-types/cpp/id-type.h"

namespace display::internal {

using DisplayIdTraits =
    DefaultIdTypeTraits<uint64_t, fuchsia_hardware_display_types::wire::DisplayId>;

}  // namespace display::internal

namespace display {

// More useful representation of `fuchsia.hardware.display.types/DisplayId`.
using DisplayId = display::internal::IdType<display::internal::DisplayIdTraits>;

constexpr DisplayId kInvalidDisplayId(fuchsia_hardware_display_types::wire::kInvalidDispId);

}  // namespace display

#endif  // SRC_GRAPHICS_DISPLAY_LIB_API_TYPES_CPP_DISPLAY_ID_H_
