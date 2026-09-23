// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/testing/software-compositor/pixel.h"

#include <zircon/assert.h>

#include <cstdint>
#include <span>

namespace software_compositor {

// static
PixelData PixelData::FromRaw(std::span<const uint8_t> raw) {
  ZX_DEBUG_ASSERT(raw.size() == 4);
  PixelData color_raw;
  std::copy(raw.begin(), raw.end(), color_raw.data.begin());
  return color_raw;
}

PixelData PixelData::Convert(PixelFormat from, PixelFormat to) const {
  if (from == to) {
    return *this;
  }
  if (from == PixelFormat::kBgra8888) {
    ZX_DEBUG_ASSERT(to == PixelFormat::kRgba8888);
    return PixelData{
        .data = {data[2], data[1], data[0], data[3]},
    };
  }
  if (from == PixelFormat::kRgba8888) {
    ZX_DEBUG_ASSERT(to == PixelFormat::kBgra8888);
    return PixelData{
        .data = {data[2], data[1], data[0], data[3]},
    };
  }
  ZX_DEBUG_ASSERT_MSG(false, "Invalid PixelFormat pair: (%d -> %d)", static_cast<int>(from),
                      static_cast<int>(to));
  return *this;
}
}  // namespace software_compositor
