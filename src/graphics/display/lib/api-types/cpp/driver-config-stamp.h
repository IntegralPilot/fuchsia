// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_LIB_API_TYPES_CPP_DRIVER_CONFIG_STAMP_H_
#define SRC_GRAPHICS_DISPLAY_LIB_API_TYPES_CPP_DRIVER_CONFIG_STAMP_H_

#include <fidl/fuchsia.hardware.display.engine/cpp/wire.h>

#include <cstdint>

#include "src/graphics/display/lib/api-types/cpp/id-type.h"

namespace display::internal {

using DriverConfigStampTraits =
    DefaultIdTypeTraits<uint64_t, fuchsia_hardware_display_engine::wire::ConfigStamp>;

}  // namespace display::internal

namespace display {

// More useful representation of `fuchsia.hardware.display.engine/ConfigStamp`.
using DriverConfigStamp = display::internal::IdType<display::internal::DriverConfigStampTraits>;

constexpr DriverConfigStamp kInvalidDriverConfigStamp(
    fuchsia_hardware_display_engine::wire::kInvalidConfigStampValue);

}  // namespace display

#endif  // SRC_GRAPHICS_DISPLAY_LIB_API_TYPES_CPP_DRIVER_CONFIG_STAMP_H_
