// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/lib/designware-dsi/dsi-host-controller-config.h"

#include <lib/mipi-dsi/mipi-dsi.h>

#include <gtest/gtest.h>

#include "src/graphics/display/lib/designware-dsi/dphy-interface-config.h"
#include "src/graphics/display/lib/designware-dsi/dpi-interface-config.h"
#include "src/graphics/display/lib/designware-dsi/dsi-packet-handler-config.h"

namespace designware_dsi {

namespace {

TEST(DsiHostControllerConfigTest, Valid) {
  static constexpr DphyInterfaceConfig kDphyInterfaceConfig = {
      .data_lane_count = 4,
      .clock_lane_mode_automatic_control_enabled = true,

      .high_speed_mode_clock_lane_frequency_hz = 400'000'000,
      .escape_mode_clock_lane_frequency_hz = 10'000'000,

      .max_data_lane_hs_to_lp_transition_duration_lane_byte_clock_cycles = 10,
      .max_data_lane_lp_to_hs_transition_duration_lane_byte_clock_cycles = 20,
      .max_clock_lane_hs_to_lp_transition_duration_lane_byte_clock_cycles = 30,
      .max_clock_lane_lp_to_hs_transition_duration_lane_byte_clock_cycles = 40,
  };
  ASSERT_TRUE(kDphyInterfaceConfig.IsValid());

  static constexpr DpiColorComponentMapping kColorComponentMapping =
      DpiColorComponentMapping::k24BitR8G8B8;
  static constexpr display::DisplayTiming kTiming = {
      .horizontal_active_px = 0x444,
      .horizontal_front_porch_px = 0x033,
      .horizontal_sync_width_px = 0x022,
      .horizontal_back_porch_px = 0x011,
      .vertical_active_lines = 0x555,
      .vertical_front_porch_lines = 0x066,
      .vertical_sync_width_lines = 0x077,
      .vertical_back_porch_lines = 0x088,
      .pixel_clock_frequency_hz = 12'500'000,
      .fields_per_frame = display::FieldsPerFrame::kProgressive,
      .hsync_polarity = display::SyncPolarity::kNegative,
      .vsync_polarity = display::SyncPolarity::kNegative,
      .vblank_alternates = false,
      .pixel_repetition = 0,
  };
  static constexpr DpiLowPowerCommandTimerConfig kDpiLowPowerCommandTimerConfig = {
      .max_vertical_blank_escape_mode_command_size_bytes = 128,
      .max_vertical_active_escape_mode_command_size_bytes = 0,
  };
  static constexpr DpiInterfaceConfig kDpiInterfaceConfig = {
      .color_component_mapping = kColorComponentMapping,
      .video_timing = kTiming,
      .low_power_command_timer_config = kDpiLowPowerCommandTimerConfig,
  };
  static constexpr int64_t kDphyDataLaneBytesPerSecond =
      kDphyInterfaceConfig.high_speed_mode_data_lane_bits_per_second() / 8;
  ASSERT_TRUE(kDpiInterfaceConfig.IsValid(kDphyDataLaneBytesPerSecond));

  static constexpr DsiPacketHandlerConfig kDsiPacketHandlerConfig = {
      .packet_sequencing = mipi_dsi::DsiVideoModePacketSequencing::kBurst,
      .pixel_stream_packet_format = mipi_dsi::DsiPixelStreamPacketFormat::k24BitR8G8B8,
  };
  ASSERT_TRUE(kDsiPacketHandlerConfig.IsValid(kColorComponentMapping));

  static constexpr DsiHostControllerConfig kDsiHostControllerConfig = {
      .dphy_interface_config = kDphyInterfaceConfig,
      .dpi_interface_config = kDpiInterfaceConfig,
      .dsi_packet_handler_config = kDsiPacketHandlerConfig,
  };
  EXPECT_TRUE(kDsiHostControllerConfig.IsValid());
}

TEST(DsiHostControllerConfigTest, InvalidIfDphyInterfaceConfigIsInvalid) {
  static constexpr DphyInterfaceConfig kInvalidDphyInterfaceConfig = {
      .data_lane_count = 5,
      .clock_lane_mode_automatic_control_enabled = true,

      .high_speed_mode_clock_lane_frequency_hz = 400'000'000,
      .escape_mode_clock_lane_frequency_hz = 10'000'000,

      .max_data_lane_hs_to_lp_transition_duration_lane_byte_clock_cycles = 10,
      .max_data_lane_lp_to_hs_transition_duration_lane_byte_clock_cycles = 20,
      .max_clock_lane_hs_to_lp_transition_duration_lane_byte_clock_cycles = 30,
      .max_clock_lane_lp_to_hs_transition_duration_lane_byte_clock_cycles = 40,
  };
  ASSERT_FALSE(kInvalidDphyInterfaceConfig.IsValid());

  static constexpr DpiColorComponentMapping kColorComponentMapping =
      DpiColorComponentMapping::k24BitR8G8B8;
  static constexpr display::DisplayTiming kTiming = {
      .horizontal_active_px = 0x444,
      .horizontal_front_porch_px = 0x033,
      .horizontal_sync_width_px = 0x022,
      .horizontal_back_porch_px = 0x011,
      .vertical_active_lines = 0x555,
      .vertical_front_porch_lines = 0x066,
      .vertical_sync_width_lines = 0x077,
      .vertical_back_porch_lines = 0x088,
      .pixel_clock_frequency_hz = 12'500'000,
      .fields_per_frame = display::FieldsPerFrame::kProgressive,
      .hsync_polarity = display::SyncPolarity::kNegative,
      .vsync_polarity = display::SyncPolarity::kNegative,
      .vblank_alternates = false,
      .pixel_repetition = 0,
  };
  static constexpr DpiLowPowerCommandTimerConfig kDpiLowPowerCommandTimerConfig = {
      .max_vertical_blank_escape_mode_command_size_bytes = 128,
      .max_vertical_active_escape_mode_command_size_bytes = 0,
  };
  static constexpr DpiInterfaceConfig kDpiInterfaceConfig = {
      .color_component_mapping = kColorComponentMapping,
      .video_timing = kTiming,
      .low_power_command_timer_config = kDpiLowPowerCommandTimerConfig,
  };
  static constexpr int64_t kDphyDataLaneBytesPerSecond =
      kInvalidDphyInterfaceConfig.high_speed_mode_data_lane_bytes_per_second();
  ASSERT_TRUE(kDpiInterfaceConfig.IsValid(kDphyDataLaneBytesPerSecond));

  static constexpr DsiPacketHandlerConfig kDsiPacketHandlerConfig = {
      .packet_sequencing = mipi_dsi::DsiVideoModePacketSequencing::kBurst,
      .pixel_stream_packet_format = mipi_dsi::DsiPixelStreamPacketFormat::k24BitR8G8B8,
  };
  ASSERT_TRUE(kDsiPacketHandlerConfig.IsValid(kColorComponentMapping));

  static constexpr DsiHostControllerConfig kDsiHostControllerConfig = {
      .dphy_interface_config = kInvalidDphyInterfaceConfig,
      .dpi_interface_config = kDpiInterfaceConfig,
      .dsi_packet_handler_config = kDsiPacketHandlerConfig,
  };
  EXPECT_FALSE(kDsiHostControllerConfig.IsValid());
}

TEST(DsiHostControllerConfigTest, InvalidIfDpiInterfaceConfigIsIncompatible) {
  static constexpr DphyInterfaceConfig kDphyInterfaceConfig = {
      .data_lane_count = 4,
      .clock_lane_mode_automatic_control_enabled = true,

      .high_speed_mode_clock_lane_frequency_hz = 400'000'000,
      .escape_mode_clock_lane_frequency_hz = 10'000'000,

      .max_data_lane_hs_to_lp_transition_duration_lane_byte_clock_cycles = 10,
      .max_data_lane_lp_to_hs_transition_duration_lane_byte_clock_cycles = 20,
      .max_clock_lane_hs_to_lp_transition_duration_lane_byte_clock_cycles = 30,
      .max_clock_lane_lp_to_hs_transition_duration_lane_byte_clock_cycles = 40,
  };
  ASSERT_TRUE(kDphyInterfaceConfig.IsValid());

  static constexpr DpiColorComponentMapping kColorComponentMapping =
      DpiColorComponentMapping::k24BitR8G8B8;
  static constexpr display::DisplayTiming kIncompatibleTiming = {
      .horizontal_active_px = 0x444,
      .horizontal_front_porch_px = 0x033,
      .horizontal_sync_width_px = 0x022,
      .horizontal_back_porch_px = 0x011,
      .vertical_active_lines = 0x555,
      .vertical_front_porch_lines = 0x066,
      .vertical_sync_width_lines = 0x077,
      .vertical_back_porch_lines = 0x088,
      .pixel_clock_frequency_hz = 300'000'000,  // Incompatible with D-PHY config
      .fields_per_frame = display::FieldsPerFrame::kProgressive,
      .hsync_polarity = display::SyncPolarity::kNegative,
      .vsync_polarity = display::SyncPolarity::kNegative,
      .vblank_alternates = false,
      .pixel_repetition = 0,
  };
  static constexpr DpiLowPowerCommandTimerConfig kDpiLowPowerCommandTimerConfig = {
      .max_vertical_blank_escape_mode_command_size_bytes = 128,
      .max_vertical_active_escape_mode_command_size_bytes = 0,
  };
  static constexpr DpiInterfaceConfig kIncompatibleDpiInterfaceConfig = {
      .color_component_mapping = kColorComponentMapping,
      .video_timing = kIncompatibleTiming,
      .low_power_command_timer_config = kDpiLowPowerCommandTimerConfig,
  };
  static constexpr int64_t kDphyDataLaneBytesPerSecond =
      kDphyInterfaceConfig.high_speed_mode_data_lane_bits_per_second() / 8;
  ASSERT_FALSE(kIncompatibleDpiInterfaceConfig.IsValid(kDphyDataLaneBytesPerSecond));

  static constexpr DsiPacketHandlerConfig kDsiPacketHandlerConfig = {
      .packet_sequencing = mipi_dsi::DsiVideoModePacketSequencing::kBurst,
      .pixel_stream_packet_format = mipi_dsi::DsiPixelStreamPacketFormat::k24BitR8G8B8,
  };
  ASSERT_TRUE(kDsiPacketHandlerConfig.IsValid(kColorComponentMapping));

  static constexpr DsiHostControllerConfig kDsiHostControllerConfig = {
      .dphy_interface_config = kDphyInterfaceConfig,
      .dpi_interface_config = kIncompatibleDpiInterfaceConfig,
      .dsi_packet_handler_config = kDsiPacketHandlerConfig,
  };
  EXPECT_FALSE(kDsiHostControllerConfig.IsValid());
}

TEST(DsiHostControllerConfigTest, InvalidIfDsiPacketHandlerConfigIsIncompatible) {
  static constexpr DphyInterfaceConfig kDphyInterfaceConfig = {
      .data_lane_count = 4,
      .clock_lane_mode_automatic_control_enabled = true,

      .high_speed_mode_clock_lane_frequency_hz = 400'000'000,
      .escape_mode_clock_lane_frequency_hz = 10'000'000,

      .max_data_lane_hs_to_lp_transition_duration_lane_byte_clock_cycles = 10,
      .max_data_lane_lp_to_hs_transition_duration_lane_byte_clock_cycles = 20,
      .max_clock_lane_hs_to_lp_transition_duration_lane_byte_clock_cycles = 30,
      .max_clock_lane_lp_to_hs_transition_duration_lane_byte_clock_cycles = 40,
  };
  ASSERT_TRUE(kDphyInterfaceConfig.IsValid());

  static constexpr DpiColorComponentMapping kColorComponentMapping =
      DpiColorComponentMapping::k24BitR8G8B8;
  static constexpr display::DisplayTiming kTiming = {
      .horizontal_active_px = 0x444,
      .horizontal_front_porch_px = 0x033,
      .horizontal_sync_width_px = 0x022,
      .horizontal_back_porch_px = 0x011,
      .vertical_active_lines = 0x555,
      .vertical_front_porch_lines = 0x066,
      .vertical_sync_width_lines = 0x077,
      .vertical_back_porch_lines = 0x088,
      .pixel_clock_frequency_hz = 12'500'000,
      .fields_per_frame = display::FieldsPerFrame::kProgressive,
      .hsync_polarity = display::SyncPolarity::kNegative,
      .vsync_polarity = display::SyncPolarity::kNegative,
      .vblank_alternates = false,
      .pixel_repetition = 0,
  };
  static constexpr DpiLowPowerCommandTimerConfig kDpiLowPowerCommandTimerConfig = {
      .max_vertical_blank_escape_mode_command_size_bytes = 128,
      .max_vertical_active_escape_mode_command_size_bytes = 0,
  };
  static constexpr DpiInterfaceConfig kDpiInterfaceConfig = {
      .color_component_mapping = kColorComponentMapping,
      .video_timing = kTiming,
      .low_power_command_timer_config = kDpiLowPowerCommandTimerConfig,
  };
  static constexpr int64_t kDphyDataLaneBytesPerSecond =
      kDphyInterfaceConfig.high_speed_mode_data_lane_bits_per_second() / 8;
  ASSERT_TRUE(kDpiInterfaceConfig.IsValid(kDphyDataLaneBytesPerSecond));

  static constexpr DsiPacketHandlerConfig kIncompatibleDsiPacketHandlerConfig = {
      .packet_sequencing = mipi_dsi::DsiVideoModePacketSequencing::kBurst,
      // Incompatible with DPI color component mapping
      .pixel_stream_packet_format = mipi_dsi::DsiPixelStreamPacketFormat::k16BitR5G6B5,
  };
  ASSERT_FALSE(kIncompatibleDsiPacketHandlerConfig.IsValid(kColorComponentMapping));

  static constexpr DsiHostControllerConfig kDsiHostControllerConfig = {
      .dphy_interface_config = kDphyInterfaceConfig,
      .dpi_interface_config = kDpiInterfaceConfig,
      .dsi_packet_handler_config = kIncompatibleDsiPacketHandlerConfig,
  };
  EXPECT_FALSE(kDsiHostControllerConfig.IsValid());
}

}  // namespace

}  // namespace designware_dsi
