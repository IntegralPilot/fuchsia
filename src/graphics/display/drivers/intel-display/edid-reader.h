// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_DRIVERS_INTEL_DISPLAY_EDID_READER_H_
#define SRC_GRAPHICS_DISPLAY_DRIVERS_INTEL_DISPLAY_EDID_READER_H_

#include <lib/fit/function.h>
#include <lib/zx/result.h>

#include <array>
#include <cstdint>
#include <span>

#include <fbl/vector.h>

#include "src/graphics/display/lib/edid/edid.h"

namespace intel_display {

// `index` must be non-negative and less than `edid::kMaxEdidBlockCount`.
using ReadEdidBlockFunction =
    fit::inline_function<zx::result<>(int index, std::span<uint8_t, edid::kBlockSize> edid_block),
                         /*inline_target_size=*/16>;

zx::result<fbl::Vector<uint8_t>> ReadExtendedEdid(ReadEdidBlockFunction read_edid_block);

}  // namespace intel_display

#endif  // SRC_GRAPHICS_DISPLAY_DRIVERS_INTEL_DISPLAY_EDID_READER_H_
