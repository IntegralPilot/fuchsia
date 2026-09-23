// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_DRIVERS_INTEL_DISPLAY_DDI_H_
#define SRC_GRAPHICS_DISPLAY_DRIVERS_INTEL_DISPLAY_DDI_H_

#include <array>
#include <span>

#include "src/graphics/display/drivers/intel-display/registers-ddi.h"

namespace intel_display {

// Get the list of DDIs supported by the device of |device_id|.
std::span<const DdiId> GetDdiIds(uint16_t device_id);

}  // namespace intel_display

#endif  // SRC_GRAPHICS_DISPLAY_DRIVERS_INTEL_DISPLAY_DDI_H_
