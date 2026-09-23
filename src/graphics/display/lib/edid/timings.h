// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_LIB_EDID_TIMINGS_H_
#define SRC_GRAPHICS_DISPLAY_LIB_EDID_TIMINGS_H_

#include <cinttypes>
#include <span>

#include "src/graphics/display/lib/api-types/cpp/display-timing.h"

namespace edid::internal {

extern const std::span<const display::DisplayTiming> kDmtDisplayTimings;
extern const std::span<const display::DisplayTiming> kCtaDisplayTimings;

}  // namespace edid::internal

#endif  // SRC_GRAPHICS_DISPLAY_LIB_EDID_TIMINGS_H_
