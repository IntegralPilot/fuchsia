// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/amlogic-display/logging.h"

#include <lib/driver/logging/cpp/logger.h>

#include <cinttypes>

#include "src/graphics/display/drivers/amlogic-display/panel-config.h"
#include "src/graphics/display/lib/api-types/cpp/display-timing.h"

namespace amlogic_display {

void LogDisplayTiming(const display::DisplayTiming& display_timing) {
  fdf::info("Display Timing: ");
  fdf::info("  Horizontal active (px): {}", display_timing.horizontal_active_px);
  fdf::info("  Horizontal front porch (px): {}", display_timing.horizontal_front_porch_px);
  fdf::info("  Horizontal sync width (px): {}", display_timing.horizontal_sync_width_px);
  fdf::info("  Horizontal back porch (px): {}", display_timing.horizontal_back_porch_px);
  fdf::info("  Horizontal blank (px): {}", display_timing.horizontal_blank_px());
  fdf::info("  Horizontal total (px): {}", display_timing.horizontal_total_px());
  fdf::info("");
  fdf::info("  Vertical active (lines): {}", display_timing.vertical_active_lines);
  fdf::info("  Vertical front porch (lines): {}", display_timing.vertical_front_porch_lines);
  fdf::info("  Vertical sync width (lines): {}", display_timing.vertical_sync_width_lines);
  fdf::info("  Vertical back porch (lines): {}", display_timing.vertical_back_porch_lines);
  fdf::info("  Vertical blank (lines): {}", display_timing.vertical_blank_lines());
  fdf::info("  Vertical total (lines): {}", display_timing.vertical_total_lines());
  fdf::info("");
  fdf::info("  Pixel clock frequency (Hz): {}", display_timing.pixel_clock_frequency_hz);
  fdf::info("  Fields per frame: {}",
            display_timing.fields_per_frame == display::FieldsPerFrame::kInterlaced
                ? "Interlaced"
                : "Progressive");
  fdf::info(
      "  Hsync polarity: {}",
      display_timing.hsync_polarity == display::SyncPolarity::kPositive ? "Positive" : "Negative");
  fdf::info(
      "  Vsync polarity: {}",
      display_timing.vsync_polarity == display::SyncPolarity::kPositive ? "Positive" : "Negative");
  fdf::info("  Vblank alternates: {}", display_timing.vblank_alternates ? "True" : "False");
  fdf::info("  Pixel repetition: {}", display_timing.pixel_repetition);
}

void LogPanelConfig(const PanelConfig& panel_config) {
  fdf::info("Panel Config for Panel \"{}\"", panel_config.name);
  fdf::info("  Power on DSI command sequence: size {}", panel_config.dsi_on.size());
  fdf::info("  Power off DSI command sequence: size {}", panel_config.dsi_off.size());
  fdf::info("  Power on PowerOp sequence: size {}", panel_config.power_on.size());
  fdf::info("  Power off PowerOp sequence: size {}", panel_config.power_off.size());
  fdf::info("");
  fdf::info("  D-PHY data lane count: {}", panel_config.dphy_data_lane_count);
  fdf::info("  Maximum D-PHY clock lane frequency (Hz): {}",
            panel_config.maximum_dphy_clock_lane_frequency_hz);
  fdf::info("  Maximum D-PHY data lane bitrate (bit/second): {}",
            panel_config.maximum_per_data_lane_bit_per_second());
  fdf::info("");
  LogDisplayTiming(panel_config.display_timing);
}

}  // namespace amlogic_display
