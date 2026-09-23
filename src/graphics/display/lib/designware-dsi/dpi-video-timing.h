// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_LIB_DESIGNWARE_DSI_DPI_VIDEO_TIMING_H_
#define SRC_GRAPHICS_DISPLAY_LIB_DESIGNWARE_DSI_DPI_VIDEO_TIMING_H_

#include <zircon/assert.h>

#include <cstdint>

#include "src/graphics/display/lib/api-types/cpp/display-timing.h"

namespace designware_dsi {

// The D-PHY lane byte clock is the TxByteClkHS in the PPI (PHY-Protocol
// Interface) in MIPI D-PHY 1.2. This is the same as the TxWordClkHS signal
// in D-PHY 2.0 and above, in instantiations where TxDataWidthHS is 0b00 (8-bit
// High-Speed Transmission data bus).

inline constexpr int32_t kMinDphyLaneByteRateToDpiPixelRateRatio = 1;
inline constexpr int32_t kMaxDphyLaneByteRateToDpiPixelRateRatio = 256;

// The ratio of MIPI D-PHY data lane byte transmission rate (bytes per
// second) to the MIPI DPI pixel rate (Hz).
//
// The DPI pixel rate must be divisble by the D-PHY data lane byte transmission
// rate, and the quotient must be
// >= kMinDphyLaneByteRateToDpiPixelClockFrequencyRatio and
// <= kMaxDphyLaneByteRateToDpiPixelClockFrequencyRatio.
constexpr int32_t DphyLaneByteRateToDpiPixelRateRatio(int64_t dphy_data_lane_bytes_per_second,
                                                      int64_t dpi_pixel_rate_hz) {
  ZX_DEBUG_ASSERT(dphy_data_lane_bytes_per_second % dpi_pixel_rate_hz == 0);

  const int64_t dphy_lane_byte_rate_to_dpi_pixel_rate_ratio_i64 =
      dphy_data_lane_bytes_per_second / dpi_pixel_rate_hz;
  ZX_DEBUG_ASSERT(dphy_lane_byte_rate_to_dpi_pixel_rate_ratio_i64 >=
                  kMinDphyLaneByteRateToDpiPixelRateRatio);
  ZX_DEBUG_ASSERT(dphy_lane_byte_rate_to_dpi_pixel_rate_ratio_i64 <=
                  kMaxDphyLaneByteRateToDpiPixelRateRatio);
  const int32_t dphy_lane_byte_rate_to_dpi_pixel_rate_ratio =
      static_cast<int32_t>(dphy_lane_byte_rate_to_dpi_pixel_rate_ratio_i64);

  return dphy_lane_byte_rate_to_dpi_pixel_rate_ratio;
}

// Amount of time it takes to transmit a number of pixels across the DPI
// interface between the display engine frontend and the DesignWare DSI
// host controller.
//
// The result is expressed in D-PHY lane byte clock cycles, which is the
// number of bytes that can be transmitted on a D-PHY data lane in High Speed
// mode.
//
// `pixels` must be >= 0 and <= 2^23 - 1.
//
// The DPI pixel rate (Hz) must be divisble by the D-PHY data lane byte
// transmission rate (bytes per second), and the quotient must be
// >= kMinDphyLaneByteRateToDpiPixelClockFrequencyRatio and
// <= kMaxDphyLaneByteRateToDpiPixelClockFrequencyRatio.
constexpr int32_t DpiPixelToDphyLaneByteClockCycle(int32_t pixels, int64_t dpi_pixel_rate_hz,
                                                   int64_t dphy_data_lane_bytes_per_second) {
  ZX_DEBUG_ASSERT(pixels >= 0);
  ZX_DEBUG_ASSERT(pixels <= (1 << 23) - 1);

  int32_t dphy_data_lane_byte_rate_to_dpi_pixel_rate_ratio =
      DphyLaneByteRateToDpiPixelRateRatio(dphy_data_lane_bytes_per_second, dpi_pixel_rate_hz);
  return pixels * dphy_data_lane_byte_rate_to_dpi_pixel_rate_ratio;
}

inline constexpr int32_t kMaxHorizontalSyncWidthPx = (1 << 12) - 1;
inline constexpr int32_t kMaxHorizontalBackPorchPx = (1 << 12) - 1;
inline constexpr int32_t kMaxHorizontalTotalPx = (1 << 15) - 1;

// Constrained by the VID_HSA_TIME (Horizontal sync active time) register of
// the DesignWare MIPI DSI Host Controller.
//
// MIPI DSI Host Controller Databook v1.51a, Section 5.1.19 "VID_HSA_TIME",
// page 214.
inline constexpr int32_t kMaxHorizontalSyncTimeLaneByteClockCycles = (1 << 12) - 1;

// Constrained by the VID_HBP_TIME (Horizontal back porch time) register of
// the DesignWare MIPI DSI Host Controller.
//
// MIPI DSI Host Controller Databook v1.51a, Section 5.1.20 "VID_HBP_TIME",
// page 215.
inline constexpr int32_t kMaxHorizontalBackPorchTimeLaneByteClockCycles = (1 << 12) - 1;

// Constrained by the VID_HLINE_TIME (Overall video line time) register of
// the DesignWare MIPI DSI Host Controller.
//
// MIPI DSI Host Controller Databook v1.51a, Section 5.1.21 "VID_HLINE_TIME",
// page 216.
inline constexpr int32_t kMaxHorizontalTotalTimeLaneByteClockCycles = (1 << 15) - 1;

// Constrained by the VID_VACTIVE_LINES (Vertical resolution) register of
// the DesignWare MIPI DSI Host Controller.
//
// MIPI DSI Host Controller Databook v1.51a, Section 5.1.25 "VID_VACTIVE_LINES",
// page 220.
inline constexpr int32_t kMaxVerticalActiveLines = (1 << 14) - 1;

// Constrained by the VID_VFP_LINES (Vertical front porch period) register of
// the DesignWare MIPI DSI Host Controller.
//
// MIPI DSI Host Controller Databook v1.51a, Section 5.1.24 "VID_VFP_LINES",
// page 219.
inline constexpr int32_t kMaxVerticalFrontPorchLines = (1 << 10) - 1;

// Constrained by the VID_VSA_LINES (Vertical sync active period) register of
// the DesignWare MIPI DSI Host Controller.
//
// MIPI DSI Host Controller Databook v1.51a, Section 5.1.22 "VID_VSA_LINES",
// page 217.
inline constexpr int32_t kMaxVerticalSyncWidthLines = (1 << 10) - 1;

// Constrained by the VID_VBP_LINES (Vertical back porch period) register of
// the DesignWare MIPI DSI Host Controller.
//
// MIPI DSI Host Controller Databook v1.51a, Section 5.1.23 "VID_VBP_LINES",
// page 218.
inline constexpr int32_t kMaxVerticalBackPorchLines = (1 << 10) - 1;

// Returns whether the given `timing` is valid for the DesignWare DSI Host
// Controller's DPI interface input when the D-PHY the Controller connects
// to is configured to transmit at `dphy_data_lane_bytes_per_second` in High
// Speed mode.
constexpr bool IsValidDpiVideoTiming(const display::DisplayTiming& timing,
                                     int64_t dphy_data_lane_bytes_per_second) {
  if (!timing.IsValid()) {
    return false;
  }
  if (timing.fields_per_frame != display::FieldsPerFrame::kProgressive) {
    return false;
  }
  if (timing.pixel_repetition != 0) {
    return false;
  }

  if (timing.pixel_clock_frequency_hz <= 0) {
    return false;
  }

  if (dphy_data_lane_bytes_per_second % timing.pixel_clock_frequency_hz != 0) {
    return false;
  }

  const int64_t dphy_data_lane_byte_rate_to_dpi_pixel_rate_ratio =
      dphy_data_lane_bytes_per_second / timing.pixel_clock_frequency_hz;
  if (dphy_data_lane_byte_rate_to_dpi_pixel_rate_ratio < kMinDphyLaneByteRateToDpiPixelRateRatio) {
    return false;
  }
  if (dphy_data_lane_byte_rate_to_dpi_pixel_rate_ratio > kMaxDphyLaneByteRateToDpiPixelRateRatio) {
    return false;
  }

  if (timing.horizontal_back_porch_px > kMaxHorizontalBackPorchPx) {
    return false;
  }

  if (timing.horizontal_sync_width_px > kMaxHorizontalSyncWidthPx) {
    return false;
  }

  if (timing.horizontal_total_px() > kMaxHorizontalTotalPx) {
    return false;
  }

  if (timing.vertical_active_lines > kMaxVerticalActiveLines) {
    return false;
  }

  if (timing.vertical_front_porch_lines > kMaxVerticalFrontPorchLines) {
    return false;
  }

  if (timing.vertical_sync_width_lines > kMaxVerticalSyncWidthLines) {
    return false;
  }

  if (timing.vertical_back_porch_lines > kMaxVerticalBackPorchLines) {
    return false;
  }

  const int32_t horizontal_sync_time_lane_byte_clock_cycles = DpiPixelToDphyLaneByteClockCycle(
      timing.horizontal_sync_width_px,
      /*dpi_pixel_rate_hz=*/timing.pixel_clock_frequency_hz, dphy_data_lane_bytes_per_second);
  if (horizontal_sync_time_lane_byte_clock_cycles < 0) {
    return false;
  }
  if (horizontal_sync_time_lane_byte_clock_cycles > kMaxHorizontalSyncTimeLaneByteClockCycles) {
    return false;
  }

  const int32_t horizontal_back_porch_time_lane_byte_clock_cycles =
      DpiPixelToDphyLaneByteClockCycle(timing.horizontal_back_porch_px,
                                       /*dpi_pixel_rate_hz=*/timing.pixel_clock_frequency_hz,
                                       dphy_data_lane_bytes_per_second);
  if (horizontal_back_porch_time_lane_byte_clock_cycles < 0) {
    return false;
  }
  if (horizontal_back_porch_time_lane_byte_clock_cycles >
      kMaxHorizontalBackPorchTimeLaneByteClockCycles) {
    return false;
  }

  const int32_t horizontal_total_time_lane_byte_clock_cycles = DpiPixelToDphyLaneByteClockCycle(
      timing.horizontal_total_px(),
      /*dpi_pixel_rate_hz=*/timing.pixel_clock_frequency_hz, dphy_data_lane_bytes_per_second);
  if (horizontal_total_time_lane_byte_clock_cycles < 0) {
    return false;
  }
  if (horizontal_total_time_lane_byte_clock_cycles > kMaxHorizontalTotalTimeLaneByteClockCycles) {
    return false;
  }

  return true;
}

}  // namespace designware_dsi

#endif  // SRC_GRAPHICS_DISPLAY_LIB_DESIGNWARE_DSI_DPI_VIDEO_TIMING_H_
