// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_DRIVERS_AMLOGIC_DISPLAY_GPIO_MUX_REGS_H_
#define SRC_GRAPHICS_DISPLAY_DRIVERS_AMLOGIC_DISPLAY_GPIO_MUX_REGS_H_

// Register offsets from the A311D datasheet section 8.9.2 "GPIO Multiplex
// Function", Table 8-23 "Pin Mux Registers".
//
// Section 8.9.2 states that all the registers below are on CBUS (regular
// power-gated domain), and their offsets are relative to 0xff63'4400. Section
// 8.1 "System" > "Memory Map" lists this range as PERIPHS_REGS.
//
// The following datasheets have matching information.
// * S905D2, Section 6.8.2 "GPIO Multiplex Function", Section 6.1 "Memory Map"
// * S905D3, Section 6.9.2 "GPIO Multiplex Function", Section 6.1 "Memory Map"
// * T931, Section 6.9.2 "GPIO Multiplex Function", Section 6.1 "Memory Map"
#define PAD_PULL_UP_EN_REG3 (0x4b << 2)
#define PAD_PULL_UP_REG3 (0x3d << 2)
#define P_PREG_PAD_GPIO3_EN_N (0x19 << 2)
#define PERIPHS_PIN_MUX_B (0xbb << 2)

#endif  // SRC_GRAPHICS_DISPLAY_DRIVERS_AMLOGIC_DISPLAY_GPIO_MUX_REGS_H_
