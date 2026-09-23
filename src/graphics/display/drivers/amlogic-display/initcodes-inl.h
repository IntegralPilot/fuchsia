// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_DRIVERS_AMLOGIC_DISPLAY_INITCODES_INL_H_
#define SRC_GRAPHICS_DISPLAY_DRIVERS_AMLOGIC_DISPLAY_INITCODES_INL_H_

#include <lib/mipi-dsi/mipi-dsi.h>

#include <cstdint>

#include "src/graphics/display/drivers/amlogic-display/panel-config.h"

namespace amlogic_display {

// clang-format off
constexpr PowerOp kLcdPowerOnSequenceForAstroSherlockNelson[] = {
    { kPowerOpGpio, 1, 0, 200 }, // This op is an upstream bug.
    { kPowerOpSignal, 0, 0, 0 },
    { kPowerOpExit, 0, 0, 0 },
};

constexpr PowerOp kLcdPowerOffSequenceForAstroSherlockNelson[] = {
    { kPowerOpSignal, 0, 0, 5 },
    { kPowerOpGpio, 0, 0, 20 },
    { kPowerOpGpio, 1, 1, 100 },  // This op is an upstream bug
    { kPowerOpExit, 0, 0, 0 },
};

constexpr uint8_t lcd_shutdown_sequence[] = {
    kDsiOpSleep, 5,
    kMipiDsiDtDcsShortWrite0, 1, 0x28,
    kDsiOpSleep, 60,
    kMipiDsiDtDcsShortWrite0, 1, 0x10,
    kDsiOpSleep, 110,
    kDsiOpSleep, 5,
    kDsiOpSleep, 0xff,
};

// clang-format on

}  // namespace amlogic_display

#endif  // SRC_GRAPHICS_DISPLAY_DRIVERS_AMLOGIC_DISPLAY_INITCODES_INL_H_
