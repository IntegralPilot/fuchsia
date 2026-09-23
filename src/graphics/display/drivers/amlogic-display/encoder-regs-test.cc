// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/amlogic-display/encoder-regs.h"

#include <gtest/gtest.h>

#include "src/graphics/display/lib/api-types/cpp/display-timing.h"

namespace amlogic_display {

namespace {

TEST(HdmiEncoderModeConfigTest, SetFor1080i) {
  HdmiEncoderModeConfig hdmi_encoder_mode_config = HdmiEncoderModeConfig::Get().FromValue(0);
  hdmi_encoder_mode_config.ConfigureFor1080i();

  // The register should be set to 0x1ffc for 1080i per S912 Datasheet.
  // S912 Datasheet, "ENCP_VIDEO_MODE", Section 27.4 "CVBS and LCD", page 381.
  constexpr uint32_t kExpectedEncoderModeConfigReg = 0x1ffc;
  EXPECT_EQ(hdmi_encoder_mode_config.reg_value(), kExpectedEncoderModeConfigReg);
}

TEST(HdmiEncoderModeConfigTest, ConfigureForStandardDefinitionProgressive) {
  HdmiEncoderModeConfig hdmi_encoder_mode_config = HdmiEncoderModeConfig::Get().FromValue(0);
  hdmi_encoder_mode_config.ConfigureForStandardDefinitionProgressive();

  // The register should be set to 0x0 for 480p / 576p per S912 Datasheet.
  // S912 Datasheet, "ENCP_VIDEO_MODE", Section 27.5 "CVBS and LCD", page 381.
  constexpr uint32_t kExpectedEncoderModeConfigReg = 0x0;
  EXPECT_EQ(hdmi_encoder_mode_config.reg_value(), kExpectedEncoderModeConfigReg);
}

TEST(HdmiEncoderModeConfigTest, SetForHighDefinitionProgressive) {
  HdmiEncoderModeConfig hdmi_encoder_mode_config = HdmiEncoderModeConfig::Get().FromValue(0);
  hdmi_encoder_mode_config.ConfigureForHighDefinitionProgressive();

  // The register should be set to 0x40 for other progressive timings per
  // Amlogic-provided code. Note that the Amlogic-provided code also sets the
  // DE signal polarity, which in this driver should set by themselves for
  // consistency with the transmitter configuration.
  constexpr uint32_t kExpectedEncoderModeConfigReg = 0x40;
  EXPECT_EQ(hdmi_encoder_mode_config.reg_value(), kExpectedEncoderModeConfigReg);
}

TEST(HdmiEncoderAdvancedModeConfigTest, GetViuFifoDownsamplingMultiplier) {
  HdmiEncoderAdvancedModeConfig mode_config = HdmiEncoderAdvancedModeConfig::Get().FromValue(0);

  mode_config.set_reg_value(0).set_viu_fifo_downsampling_multiplier_selection(
      HdmiEncoderAdvancedModeConfig::ViuFifoDownsamplingMultiplierSelection::k1);
  EXPECT_EQ(mode_config.viu_fifo_downsampling_multiplier(), 1);

  mode_config.set_reg_value(0).set_viu_fifo_downsampling_multiplier_selection(
      HdmiEncoderAdvancedModeConfig::ViuFifoDownsamplingMultiplierSelection::k2);
  EXPECT_EQ(mode_config.viu_fifo_downsampling_multiplier(), 2);

  mode_config.set_reg_value(0).set_viu_fifo_downsampling_multiplier_selection(
      HdmiEncoderAdvancedModeConfig::ViuFifoDownsamplingMultiplierSelection::k4);
  EXPECT_EQ(mode_config.viu_fifo_downsampling_multiplier(), 4);

  mode_config.set_reg_value(0).set_viu_fifo_downsampling_multiplier_selection(
      HdmiEncoderAdvancedModeConfig::ViuFifoDownsamplingMultiplierSelection::k8);
  EXPECT_EQ(mode_config.viu_fifo_downsampling_multiplier(), 8);
}

TEST(HdmiEncoderAdvancedModeConfigTest, SetViuFifoDownsamplingMultiplier) {
  HdmiEncoderAdvancedModeConfig mode_config = HdmiEncoderAdvancedModeConfig::Get().FromValue(0);

  mode_config.set_reg_value(0).set_viu_fifo_downsampling_multiplier(1);
  EXPECT_EQ(mode_config.viu_fifo_downsampling_multiplier(), 1);

  mode_config.set_reg_value(0).set_viu_fifo_downsampling_multiplier(2);
  EXPECT_EQ(mode_config.viu_fifo_downsampling_multiplier(), 2);

  mode_config.set_reg_value(0).set_viu_fifo_downsampling_multiplier(4);
  EXPECT_EQ(mode_config.viu_fifo_downsampling_multiplier(), 4);

  mode_config.set_reg_value(0).set_viu_fifo_downsampling_multiplier(8);
  EXPECT_EQ(mode_config.viu_fifo_downsampling_multiplier(), 8);
}

TEST(HdmiEncoderHorizontalTotalTest, HorizontalPeriodPx) {
  HdmiEncoderHorizontalTotal horizontal_total = HdmiEncoderHorizontalTotal::Get().FromValue(4095);
  EXPECT_EQ(horizontal_total.pixels_minus_one(), 4095u);
  EXPECT_EQ(horizontal_total.pixels(), 4096);

  horizontal_total.set_reg_value(0);
  horizontal_total.set_pixels(4096);
  EXPECT_EQ(horizontal_total.pixels(), 4096);
}

TEST(HdmiEncoderHorizontalSyncEndTest, HorizontalSyncEndPx) {
  HdmiEncoderHorizontalSyncEnd horizontal_sync_end =
      HdmiEncoderHorizontalSyncEnd::Get().FromValue(4096);
  EXPECT_EQ(horizontal_sync_end.pixels_plus_one(), 4096u);
  EXPECT_EQ(horizontal_sync_end.pixels(), 4095);

  horizontal_sync_end.set_reg_value(0);
  horizontal_sync_end.set_pixels(4095);
  EXPECT_EQ(horizontal_sync_end.pixels(), 4095);
}

TEST(HdmiEncoderVerticalTotalTest, VerticalPeriodLine) {
  HdmiEncoderVerticalTotal vertical_total = HdmiEncoderVerticalTotal::Get().FromValue(2047);
  EXPECT_EQ(vertical_total.lines_minus_one(), 2047u);
  EXPECT_EQ(vertical_total.pixels(), 2048);

  vertical_total.set_reg_value(0);
  vertical_total.set_lines(2048);
  EXPECT_EQ(vertical_total.pixels(), 2048);
}

TEST(HdmiEncoderVideoVerticalSyncEndTest, VerticalSyncEndLine) {
  HdmiEncoderVideoVerticalSyncEnd vertical_sync_end =
      HdmiEncoderVideoVerticalSyncEnd::Get().FromValue(2047);
  EXPECT_EQ(vertical_sync_end.lines_plus_one(), 2047u);
  EXPECT_EQ(vertical_sync_end.lines(), 2046);

  vertical_sync_end.set_reg_value(0);
  vertical_sync_end.set_lines(2046);
  EXPECT_EQ(vertical_sync_end.lines(), 2046);
}

TEST(HdmiEncoderDataEnableHorizontalActiveEndTest, HorizontalActiveEndPx) {
  HdmiEncoderDataEnableHorizontalActiveEnd horizontal_active_end =
      HdmiEncoderDataEnableHorizontalActiveEnd::Get().FromValue(4096);
  EXPECT_EQ(horizontal_active_end.pixels_plus_one(), 4096u);
  EXPECT_EQ(horizontal_active_end.pixels(), 4095);

  horizontal_active_end.set_reg_value(0);
  horizontal_active_end.set_pixels(4095);
  EXPECT_EQ(horizontal_active_end.pixels(), 4095);
}

TEST(HdmiEncoderDataEnableVerticalActiveEndTest, VerticalActiveEndLine) {
  HdmiEncoderDataEnableVerticalActiveEnd vertical_active_end =
      HdmiEncoderDataEnableVerticalActiveEnd::Get().FromValue(2047);
  EXPECT_EQ(vertical_active_end.lines_plus_one(), 2047u);
  EXPECT_EQ(vertical_active_end.lines(), 2046);

  vertical_active_end.set_reg_value(0);
  vertical_active_end.set_lines(2046);
  EXPECT_EQ(vertical_active_end.lines(), 2046);
}

TEST(HdmiEncoderDataEnableVerticalActiveEndOddFieldsTest, VerticalActiveEndLine) {
  HdmiEncoderDataEnableVerticalActiveEndOddFields vertical_active_end =
      HdmiEncoderDataEnableVerticalActiveEndOddFields::Get().FromValue(2047);
  EXPECT_EQ(vertical_active_end.lines_plus_one(), 2047u);
  EXPECT_EQ(vertical_active_end.lines(), 2046);

  vertical_active_end.set_reg_value(0);
  vertical_active_end.set_lines(2046);
  EXPECT_EQ(vertical_active_end.lines(), 2046);
}

TEST(HdmiEncoderVerticalSyncEndTest, VerticalSyncEndLine) {
  HdmiEncoderVerticalSyncEnd vertical_sync_end = HdmiEncoderVerticalSyncEnd::Get().FromValue(2047);
  EXPECT_EQ(vertical_sync_end.lines_plus_one(), 2047u);
  EXPECT_EQ(vertical_sync_end.lines(), 2046);

  vertical_sync_end.set_reg_value(0);
  vertical_sync_end.set_lines(2046);
  EXPECT_EQ(vertical_sync_end.lines(), 2046);
}

TEST(HdmiEncoderVerticalSyncEndOddFieldsTest, VerticalSyncEndLine) {
  HdmiEncoderVerticalSyncEndOddFields vertical_sync_end =
      HdmiEncoderVerticalSyncEndOddFields::Get().FromValue(2047);
  EXPECT_EQ(vertical_sync_end.lines_plus_one(), 2047u);
  EXPECT_EQ(vertical_sync_end.lines(), 2046);

  vertical_sync_end.set_reg_value(0);
  vertical_sync_end.set_lines(2046);
  EXPECT_EQ(vertical_sync_end.lines(), 2046);
}

TEST(HdmiEncoderTransmitterBridgeSettingTest, FifoReadDownsamplingMultiplier) {
  HdmiEncoderTransmitterBridgeSetting hdmi_encoder_transmitter_bridge_setting =
      HdmiEncoderTransmitterBridgeSetting::Get().FromValue(0xf000);
  EXPECT_EQ(hdmi_encoder_transmitter_bridge_setting.fifo_read_downsampling_multiplier(), 16);
  EXPECT_EQ(hdmi_encoder_transmitter_bridge_setting.fifo_read_downsampling_multiplier_minus_one(),
            15u);

  hdmi_encoder_transmitter_bridge_setting.set_reg_value(0);
  hdmi_encoder_transmitter_bridge_setting.set_fifo_read_downsampling_multiplier(16);
  EXPECT_EQ(hdmi_encoder_transmitter_bridge_setting.fifo_read_downsampling_multiplier(), 16);
}

TEST(HdmiEncoderTransmitterBridgeSettingTest, FifoWriteDownsamplingMultiplier) {
  HdmiEncoderTransmitterBridgeSetting hdmi_encoder_transmitter_bridge_setting =
      HdmiEncoderTransmitterBridgeSetting::Get().FromValue(0x0f00);
  EXPECT_EQ(hdmi_encoder_transmitter_bridge_setting.fifo_write_downsampling_multiplier_minus_one(),
            15u);
  EXPECT_EQ(hdmi_encoder_transmitter_bridge_setting.fifo_write_downsampling_multiplier(), 16);

  hdmi_encoder_transmitter_bridge_setting.set_reg_value(0);
  hdmi_encoder_transmitter_bridge_setting.set_fifo_write_downsampling_multiplier(16);
  EXPECT_EQ(hdmi_encoder_transmitter_bridge_setting.fifo_write_downsampling_multiplier(), 16);
}

TEST(HdmiEncoderTransmitterBridgeSettingTest, DviClockPolarity) {
  // Test the getter.
  HdmiEncoderTransmitterBridgeSetting hdmi_encoder_transmitter_bridge_setting =
      HdmiEncoderTransmitterBridgeSetting::Get().FromValue(0x0010);
  EXPECT_TRUE(hdmi_encoder_transmitter_bridge_setting.is_dvi_clock_polarity_positive());
  EXPECT_EQ(hdmi_encoder_transmitter_bridge_setting.dvi_clock_polarity(),
            display::SyncPolarity::kPositive);

  hdmi_encoder_transmitter_bridge_setting.set_reg_value(0);
  EXPECT_FALSE(hdmi_encoder_transmitter_bridge_setting.is_dvi_clock_polarity_positive());
  EXPECT_EQ(hdmi_encoder_transmitter_bridge_setting.dvi_clock_polarity(),
            display::SyncPolarity::kNegative);

  // Test the setter.
  hdmi_encoder_transmitter_bridge_setting.set_reg_value(0);
  hdmi_encoder_transmitter_bridge_setting.set_dvi_clock_polarity(display::SyncPolarity::kPositive);
  EXPECT_EQ(hdmi_encoder_transmitter_bridge_setting.dvi_clock_polarity(),
            display::SyncPolarity::kPositive);

  hdmi_encoder_transmitter_bridge_setting.set_reg_value(0);
  hdmi_encoder_transmitter_bridge_setting.set_dvi_clock_polarity(display::SyncPolarity::kNegative);
  EXPECT_EQ(hdmi_encoder_transmitter_bridge_setting.dvi_clock_polarity(),
            display::SyncPolarity::kNegative);
}

TEST(HdmiEncoderTransmitterBridgeSettingTest, VsyncPolarity) {
  // Test the getter.
  HdmiEncoderTransmitterBridgeSetting hdmi_encoder_transmitter_bridge_setting =
      HdmiEncoderTransmitterBridgeSetting::Get().FromValue(0x0008);
  EXPECT_TRUE(hdmi_encoder_transmitter_bridge_setting.is_vsync_polarity_positive());
  EXPECT_EQ(hdmi_encoder_transmitter_bridge_setting.vsync_polarity(),
            display::SyncPolarity::kPositive);

  hdmi_encoder_transmitter_bridge_setting.set_reg_value(0);
  EXPECT_FALSE(hdmi_encoder_transmitter_bridge_setting.is_vsync_polarity_positive());
  EXPECT_EQ(hdmi_encoder_transmitter_bridge_setting.vsync_polarity(),
            display::SyncPolarity::kNegative);

  // Test the setter.
  hdmi_encoder_transmitter_bridge_setting.set_reg_value(0);
  hdmi_encoder_transmitter_bridge_setting.set_vsync_polarity(display::SyncPolarity::kPositive);
  EXPECT_EQ(hdmi_encoder_transmitter_bridge_setting.vsync_polarity(),
            display::SyncPolarity::kPositive);

  hdmi_encoder_transmitter_bridge_setting.set_reg_value(0);
  hdmi_encoder_transmitter_bridge_setting.set_vsync_polarity(display::SyncPolarity::kNegative);
  EXPECT_EQ(hdmi_encoder_transmitter_bridge_setting.vsync_polarity(),
            display::SyncPolarity::kNegative);
}

TEST(HdmiEncoderTransmitterBridgeSettingTest, HsyncPolarity) {
  // Test the getter.
  HdmiEncoderTransmitterBridgeSetting hdmi_encoder_transmitter_bridge_setting =
      HdmiEncoderTransmitterBridgeSetting::Get().FromValue(0x0004);
  EXPECT_TRUE(hdmi_encoder_transmitter_bridge_setting.is_hsync_polarity_positive());
  EXPECT_EQ(hdmi_encoder_transmitter_bridge_setting.hsync_polarity(),
            display::SyncPolarity::kPositive);

  hdmi_encoder_transmitter_bridge_setting.set_reg_value(0);
  EXPECT_FALSE(hdmi_encoder_transmitter_bridge_setting.is_hsync_polarity_positive());
  EXPECT_EQ(hdmi_encoder_transmitter_bridge_setting.hsync_polarity(),
            display::SyncPolarity::kNegative);

  // Test the setter.
  hdmi_encoder_transmitter_bridge_setting.set_reg_value(0);
  hdmi_encoder_transmitter_bridge_setting.set_hsync_polarity(display::SyncPolarity::kPositive);
  EXPECT_EQ(hdmi_encoder_transmitter_bridge_setting.hsync_polarity(),
            display::SyncPolarity::kPositive);

  hdmi_encoder_transmitter_bridge_setting.set_reg_value(0);
  hdmi_encoder_transmitter_bridge_setting.set_hsync_polarity(display::SyncPolarity::kNegative);
  EXPECT_EQ(hdmi_encoder_transmitter_bridge_setting.hsync_polarity(),
            display::SyncPolarity::kNegative);
}

}  // namespace

}  // namespace amlogic_display
