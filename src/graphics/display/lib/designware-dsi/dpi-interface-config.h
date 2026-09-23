// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_LIB_DESIGNWARE_DSI_DPI_INTERFACE_CONFIG_H_
#define SRC_GRAPHICS_DISPLAY_LIB_DESIGNWARE_DSI_DPI_INTERFACE_CONFIG_H_

#include "src/graphics/display/lib/designware-dsi/dpi-video-timing.h"

namespace designware_dsi {

// The DesignWare MIPI DSI Host Controller uses an extended MIPI DPI2 (Display
// Pixel Interface) for signals of input pixels in Video mode.
//
// The following types configure the signal type, pin definitions and the
// timing of the DPI interface and the DPI FIFO.

// Defines the mapping between RGB / YCbCr components and the pins on the DPI
// interface.
//
// The mapping between color components and DPI interface pins is documented
// in DSI Host Databook, Section 2.3.2 "Enabling DPI", Figure 2-2 "Location of
// Color Components in an Extension of DPI-2 Pixel Interface", page 42.
//
// The enum values are used by the DPI_COLOR_CODING register fields to
// distinguish the DPI color formats:
// DSI Host Databook, Section 5.1.5 "DPI_COLOR_CODING", page 195.
enum class DpiColorComponentMapping : uint8_t {
  // Known as "16-Bit Config 1" in the databook.
  k16BitR5G6B5Config1 = 0b0000,

  // Known as "16-Bit Config 2" in the databook.
  k16BitR5G6B5Config2 = 0b0001,

  // Known as "16-Bit Config 3" in the databook.
  k16BitR5G6B5Config3 = 0b0010,

  // Known as "18-Bit Config 1" in the databook.
  k18BitR6G6B6Config1 = 0b0011,

  // Known as "18-Bit Config 2" in the databook.
  k18BitR6G6B6Config2 = 0b0100,

  // Known as "24-Bits" in the databook.
  k24BitR8G8B8 = 0b0101,

  // Known as "20-Bit YCbCr 4:2:2 Loosely Packed" in the databook.
  // It requires 2 clock cycles to transmit two contiguous pixels.
  k20BitYcbcr422 = 0b0110,

  // Known as "24-Bit YCbCr 4:2:2" in the databook.
  // It requires 2 clock cycles to transmit two contiguous pixels.
  k24BitYcbcr422 = 0b0111,

  // Known as "16-Bit YCbCr 4:2:2" in the databook.
  // It requires 2 clock cycles to transmit two contiguous pixels.
  k16BitYcbcr422 = 0b1000,

  // Known as "30-Bit" in the databook.
  k30BitR10G10B10 = 0b1001,

  // Known as "36-Bit" in the databook.
  // It requires 2 clock cycles to transmit one pixel.
  k36BitR12G12B12 = 0b1010,

  // Known as "12-Bit YCbCr 4:2:0" in the databook.
  k12BitYcbcr420 = 0b1011,

  // Known as "DSC24 compress data" in the databook.
  kDsc24Compressed = 0b1100,
};

// Constrained by the `outvact_lpcmd_time` field in the `DPI_LP_CMD_TIM` register.
// DSI Host Databook, Section 5.1.7 "DPI_LP_CMD_TIM", page 198.
inline constexpr int32_t kMaxPossibleMaxVerticalBlankEscapeModeCommandSizeBytes = 255;

// Constrained by the `invact_lpcmd_time` field in the `DPI_LP_CMD_TIM` register.
// DSI Host Databook, Section 5.1.7 "DPI_LP_CMD_TIM", page 198.
inline constexpr int32_t kMaxPossibleMaxVerticalActiveEscapeModeCommandSizeBytes = 255;

// Configures the Low-Power command timer of the DSI Host Controller so that
// it can send commands in the Video mode without a mode switch.
struct DpiLowPowerCommandTimerConfig {
  // The maximum size of a command that can be transmitted in Escape / Low
  // Power (LP) mode on a line during the vertical blank (vertical sync,
  // vertical back porch, and vertical front porch) regions.
  //
  // Must be >= 0 and <= kMaxPossibleMaxVerticalBlankEscapeModeCommandSizeBytes.
  //
  // This can be calculated based on the video mode and the D-PHY
  // configurations, using the formula in the DSI Host Databook, Section
  // 2.8.2.1 "Calculating the Time to Transmit Commands in LP Mode in the VSA,
  // VBP, and VFP Regions", page 78-79.
  int32_t max_vertical_blank_escape_mode_command_size_bytes;

  // The maximum size of a command that can be transmitted in Escape / Low
  // Power (LP) mode on a line during the vertical active region.
  //
  // Must be >= 0 and <= kMaxPossibleMaxVerticalActiveEscapeModeCommandSizeBytes.
  //
  // This can be calculated based on the video mode and the D-PHY
  // configurations, using the formula in the DSI Host Databook, Section
  // 2.8.2.2 "Calculating the Time to Transmit Commands in LP Mode in the HFP
  // region", page 79-81.
  int32_t max_vertical_active_escape_mode_command_size_bytes;

  constexpr bool IsValid() const;
};

// Configures the input MIPI DPI interface and the FIFO, so that they can
// interpret the input pins properly and latch the pixels on the correct time.
struct DpiInterfaceConfig {
  DpiColorComponentMapping color_component_mapping;

  // If IsValid() is true, the value is guaranteed to be valid and compatible
  // with the given D-PHY lane byte transmission rate.
  display::DisplayTiming video_timing;

  // If IsValid() is true, the value is guaranteed to be valid.
  DpiLowPowerCommandTimerConfig low_power_command_timer_config;

  // Returns true iff the config is valid and compatible with the given D-PHY
  // lane byte transmission rate.
  constexpr bool IsValid(int64_t dphy_data_lane_bytes_per_second) const;
};

constexpr bool DpiLowPowerCommandTimerConfig::IsValid() const {
  if (max_vertical_blank_escape_mode_command_size_bytes < 0) {
    return false;
  }
  if (max_vertical_blank_escape_mode_command_size_bytes >
      kMaxPossibleMaxVerticalBlankEscapeModeCommandSizeBytes) {
    return false;
  }
  if (max_vertical_active_escape_mode_command_size_bytes < 0) {
    return false;
  }
  if (max_vertical_active_escape_mode_command_size_bytes >
      kMaxPossibleMaxVerticalActiveEscapeModeCommandSizeBytes) {
    return false;
  }
  return true;
}

constexpr bool DpiInterfaceConfig::IsValid(int64_t dphy_data_lane_bytes_per_second) const {
  if (!IsValidDpiVideoTiming(video_timing, dphy_data_lane_bytes_per_second)) {
    return false;
  }
  if (!low_power_command_timer_config.IsValid()) {
    return false;
  }
  return true;
}

}  // namespace designware_dsi

#endif  // SRC_GRAPHICS_DISPLAY_LIB_DESIGNWARE_DSI_DPI_INTERFACE_CONFIG_H_
