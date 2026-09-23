// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_LIB_API_TYPES_CPP_CONFIG_STAMP_H_
#define SRC_GRAPHICS_DISPLAY_LIB_API_TYPES_CPP_CONFIG_STAMP_H_

#include <fidl/fuchsia.hardware.display/cpp/wire.h>

#include <cstdint>

#include "src/graphics/display/lib/api-types/cpp/id-type.h"

namespace display::internal {

using ConfigStampTraits =
    DefaultIdTypeTraits<uint64_t, fuchsia_hardware_display::wire::ConfigStamp>;

}  // namespace display::internal

namespace display {

// More useful representation of `fuchsia.hardware.display/ConfigStamp`.
using ConfigStamp = display::internal::IdType<display::internal::ConfigStampTraits>;

constexpr ConfigStamp kInvalidConfigStamp(fuchsia_hardware_display::wire::kInvalidConfigStampValue);

}  // namespace display

#endif  // SRC_GRAPHICS_DISPLAY_LIB_API_TYPES_CPP_CONFIG_STAMP_H_
