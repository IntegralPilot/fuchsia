// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_DRIVERS_INTEL_DISPLAY_DP_AUX_CHANNEL_H_
#define SRC_GRAPHICS_DISPLAY_DRIVERS_INTEL_DISPLAY_DP_AUX_CHANNEL_H_

#include <lib/zx/result.h>

#include <cstdint>
#include <span>

#include "src/graphics/display/lib/edid/edid.h"

namespace intel_display {

// Represents transactions performed over the DisplayPort Auxiliary (DP AUX)
// channel.
class DpAuxChannel {
 public:
  virtual ~DpAuxChannel() = default;

  // Reads an E-EDID block over the DP AUX channel.
  virtual zx::result<> ReadEdidBlock(int index,
                                     std::span<uint8_t, edid::kBlockSize> edid_block) = 0;

  // Reads from DisplayPort Configuration Data (DPCD) registers over the DP AUX
  // channel.
  virtual bool DpcdRead(uint32_t addr, uint8_t* buf, size_t size) = 0;

  // Writes to DisplayPort Configuration Data (DPCD) registers over the DP AUX
  // channel.
  virtual bool DpcdWrite(uint32_t addr, const uint8_t* buf, size_t size) = 0;
};

}  // namespace intel_display

#endif  // SRC_GRAPHICS_DISPLAY_DRIVERS_INTEL_DISPLAY_DP_AUX_CHANNEL_H_
