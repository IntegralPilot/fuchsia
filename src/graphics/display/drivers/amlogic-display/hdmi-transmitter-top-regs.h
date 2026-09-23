// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_DRIVERS_AMLOGIC_DISPLAY_HDMI_TRANSMITTER_TOP_REGS_H_
#define SRC_GRAPHICS_DISPLAY_DRIVERS_AMLOGIC_DISPLAY_HDMI_TRANSMITTER_TOP_REGS_H_

#include <cstdint>

// Register addresses documented in A311D datasheet Section 10.2.3.44
// "10.2.3.44 HDCP2.2 IP Register Access" Table 10-2 "HDMITX Top-Level
// Registers"

#define HDMITX_TOP_SW_RESET (0x000 * sizeof(uint32_t))
#define HDMITX_TOP_CLK_CNTL (0x001 * sizeof(uint32_t))
#define HDMITX_TOP_INTR_MASKN (0x003 * sizeof(uint32_t))
#define HDMITX_TOP_INTR_STAT_CLR (0x005 * sizeof(uint32_t))
#define HDMITX_TOP_BIST_CNTL (0x006 * sizeof(uint32_t))
#define HDMITX_TOP_TMDS_CLK_PTTN_01 (0x00A * sizeof(uint32_t))
#define HDMITX_TOP_TMDS_CLK_PTTN_23 (0x00B * sizeof(uint32_t))
#define HDMITX_TOP_TMDS_CLK_PTTN_CNTL (0x00C * sizeof(uint32_t))

#endif  // SRC_GRAPHICS_DISPLAY_DRIVERS_AMLOGIC_DISPLAY_HDMI_TRANSMITTER_TOP_REGS_H_
