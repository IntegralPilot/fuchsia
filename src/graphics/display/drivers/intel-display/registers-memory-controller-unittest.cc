// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/intel-display/registers-memory-controller.h"

#include <gtest/gtest.h>
#include <hwreg/bitfields.h>

namespace intel_display {

namespace {

TEST(MemoryChannelTimingsAlderLakeTest, GetForControllerAndChannel) {
  // The memory controller / channel base MMIO addresses come from the processor
  // datasheets, plus the 0x140'000 base for the GPU aperture.
  //
  // Raptor Lake: 743846-001 Section 3.2 pages 99-100
  // Alder Lake S: 655259-003 Section 3.2 pages 83-34

  auto controller0_channel0_timings =
      registers::MemoryChannelTimingsAlderLake::GetForControllerAndChannel(0, 0).FromValue(0);
  EXPECT_EQ(0x14e'000u, controller0_channel0_timings.reg_addr());

  auto controller0_channel1_timings =
      registers::MemoryChannelTimingsAlderLake::GetForControllerAndChannel(0, 1).FromValue(0);
  EXPECT_EQ(0x14e'800u, controller0_channel1_timings.reg_addr());

  auto controller1_channel0_timings =
      registers::MemoryChannelTimingsAlderLake::GetForControllerAndChannel(1, 0).FromValue(0);
  EXPECT_EQ(0x15e'000u, controller1_channel0_timings.reg_addr());

  auto controller1_channel1_timings =
      registers::MemoryChannelTimingsAlderLake::GetForControllerAndChannel(1, 1).FromValue(0);
  EXPECT_EQ(0x15e'800u, controller1_channel1_timings.reg_addr());
}

TEST(MemoryChannelTimingsTigerLakeTest, GetForUControllerAndChannel) {
  // The memory controller / channel base MMIO addresses come from the processor
  // datasheets, plus the 0x140'000 base for the GPU aperture.
  //
  // Tiger Lake U: 631122-003 Section 3.2 pages 106-107

  auto controller0_channel0_timings =
      registers::MemoryChannelTimingsTigerLake::GetForUControllerAndChannel(0, 0).FromValue(0);
  EXPECT_EQ(0x144'000u, controller0_channel0_timings.reg_addr());

  auto controller0_channel1_timings =
      registers::MemoryChannelTimingsTigerLake::GetForUControllerAndChannel(0, 1).FromValue(0);
  EXPECT_EQ(0x144'400u, controller0_channel1_timings.reg_addr());

  auto controller1_channel0_timings =
      registers::MemoryChannelTimingsTigerLake::GetForUControllerAndChannel(1, 0).FromValue(0);
  EXPECT_EQ(0x154'000u, controller1_channel0_timings.reg_addr());

  auto controller1_channel1_timings =
      registers::MemoryChannelTimingsTigerLake::GetForUControllerAndChannel(1, 1).FromValue(0);
  EXPECT_EQ(0x154'400u, controller1_channel1_timings.reg_addr());
}

TEST(MemoryChannelTimingsTigerLakeTest, GetForHControllerAndChannel) {
  // The memory controller / channel base MMIO addresses come from the processor
  // datasheets, plus the 0x140'000 base for the GPU aperture.
  //
  // Tiger Lake H: 643524-003 Section 3.2 pages 110-111

  auto controller0_channel0_timings =
      registers::MemoryChannelTimingsTigerLake::GetForHControllerAndChannel(0, 0).FromValue(0);
  EXPECT_EQ(0x14e'000u, controller0_channel0_timings.reg_addr());

  auto controller0_channel1_timings =
      registers::MemoryChannelTimingsTigerLake::GetForHControllerAndChannel(0, 1).FromValue(0);
  EXPECT_EQ(0x14e'800u, controller0_channel1_timings.reg_addr());

  auto controller1_channel0_timings =
      registers::MemoryChannelTimingsTigerLake::GetForHControllerAndChannel(1, 0).FromValue(0);
  EXPECT_EQ(0x15e'000u, controller1_channel0_timings.reg_addr());

  auto controller1_channel1_timings =
      registers::MemoryChannelTimingsTigerLake::GetForHControllerAndChannel(1, 1).FromValue(0);
  EXPECT_EQ(0x15e'800u, controller1_channel1_timings.reg_addr());
}

TEST(MemoryChannelTimingsIceLakeTest, GetForChannel) {
  // The memory controller / channel base MMIO addresses come from the processor
  // datasheets, plus the 0x140'000 base for the GPU aperture.
  //
  // Rocket Lake: 636761-004 Section 3.2 page 96
  // Ice Lake: 341078-004 Section 3.2 pages 99-100

  auto channel0_timings = registers::MemoryChannelTimingsIceLake::GetForChannel(0).FromValue(0);
  EXPECT_EQ(0x144'000u, channel0_timings.reg_addr());

  auto channel1_timings = registers::MemoryChannelTimingsIceLake::GetForChannel(1).FromValue(0);
  EXPECT_EQ(0x144'400u, channel1_timings.reg_addr());
}

TEST(MemoryChannelTimingsSkylakeTest, GetForChannel) {
  // The memory controller / channel base MMIO addresses come from the processor
  // datasheets, plus the 0x140'000 base for the GPU aperture.
  //
  // Comet Lake: 615212-003 Section 7 page 146
  // Whiskey Lake: 338024-001 Section 7 page 142

  auto channel0_timings = registers::MemoryChannelTimingsSkylake::GetForChannel(0).FromValue(0);
  EXPECT_EQ(0x144'000u, channel0_timings.reg_addr());

  auto channel1_timings = registers::MemoryChannelTimingsSkylake::GetForChannel(1).FromValue(0);
  EXPECT_EQ(0x144'400u, channel1_timings.reg_addr());
}

TEST(MemoryAddressDecoderInterChannelConfigIceLakeTest, GetForController) {
  // The memory controller / channel base MMIO addresses come from the processor
  // datasheets, plus the 0x140'000 base for the GPU aperture.
  //
  // Tiger Lake: Processor Datasheet Vol 2a (631122-003) page 139
  // Ice Lake: Processor Datasheet Vol 2 (341078-004) page 128

  auto controller0_inter_channel_config =
      registers::MemoryAddressDecoderInterChannelConfigIceLake::GetForController(0);
  EXPECT_EQ(0x145'000u, controller0_inter_channel_config.addr());

  auto controller1_inter_channel_config =
      registers::MemoryAddressDecoderInterChannelConfigIceLake::GetForController(1);
  EXPECT_EQ(0x155'000u, controller1_inter_channel_config.addr());
}

TEST(MemoryAddressDecoderInterChannelConfigIceLakeTest, GetForAlderLakeController) {
  // The memory controller / channel base MMIO addresses come from the processor
  // datasheets, plus the 0x140'000 base for the GPU aperture.
  //
  // Raptor Lake: 743846-001 Section 3.2 pages 99-100
  // Alder Lake S: 655259-003 Section 3.2 pages 83-34

  auto controller0_inter_channel_config =
      registers::MemoryAddressDecoderInterChannelConfigIceLake::GetForAlderLakeController(0);
  EXPECT_EQ(0x14d'800u, controller0_inter_channel_config.addr());

  auto controller1_inter_channel_config =
      registers::MemoryAddressDecoderInterChannelConfigIceLake::GetForAlderLakeController(1);
  EXPECT_EQ(0x15d'800u, controller1_inter_channel_config.addr());
}

TEST(MemoryAddressDecoderDimmParametersAlderLakeTest, DimmSDdrChipWidth) {
  auto channel0_dimm_parameters =
      registers::MemoryAddressDecoderDimmParametersAlderLake::GetForControllerAndChannel(0, 0)
          .FromValue(0);

  channel0_dimm_parameters.set_reg_value(0).set_dimm_s_ddr_chip_width_select(
      registers::MemoryAddressDecoderDimmParametersAlderLake::DdrChipWidthValue::kX8);
  EXPECT_EQ(8, channel0_dimm_parameters.dimm_s_ddr_chip_width());

  channel0_dimm_parameters.set_reg_value(0).set_dimm_s_ddr_chip_width_select(
      registers::MemoryAddressDecoderDimmParametersAlderLake::DdrChipWidthValue::kX16);
  EXPECT_EQ(16, channel0_dimm_parameters.dimm_s_ddr_chip_width());

  channel0_dimm_parameters.set_reg_value(0).set_dimm_s_ddr_chip_width_select(
      registers::MemoryAddressDecoderDimmParametersAlderLake::DdrChipWidthValue::kX32);
  EXPECT_EQ(32, channel0_dimm_parameters.dimm_s_ddr_chip_width());
}

TEST(MemoryAddressDecoderDimmParametersAlderLakeTest, DimmLDdrChipWidth) {
  auto channel0_dimm_parameters =
      registers::MemoryAddressDecoderDimmParametersAlderLake::GetForControllerAndChannel(0, 0)
          .FromValue(0);

  channel0_dimm_parameters.set_reg_value(0).set_dimm_l_ddr_chip_width_select(
      registers::MemoryAddressDecoderDimmParametersAlderLake::DdrChipWidthValue::kX8);
  EXPECT_EQ(8, channel0_dimm_parameters.dimm_l_ddr_chip_width());

  channel0_dimm_parameters.set_reg_value(0).set_dimm_l_ddr_chip_width_select(
      registers::MemoryAddressDecoderDimmParametersAlderLake::DdrChipWidthValue::kX16);
  EXPECT_EQ(16, channel0_dimm_parameters.dimm_l_ddr_chip_width());

  channel0_dimm_parameters.set_reg_value(0).set_dimm_l_ddr_chip_width_select(
      registers::MemoryAddressDecoderDimmParametersAlderLake::DdrChipWidthValue::kX32);
  EXPECT_EQ(32, channel0_dimm_parameters.dimm_l_ddr_chip_width());
}

TEST(MemoryAddressDecoderDimmParametersAlderLakeTest, DimmSRankCount) {
  auto channel0_dimm_parameters =
      registers::MemoryAddressDecoderDimmParametersAlderLake::GetForControllerAndChannel(0, 0)
          .FromValue(0);

  channel0_dimm_parameters.set_reg_value(0).set_dimm_s_rank_count_minus_1(0);
  EXPECT_EQ(1, channel0_dimm_parameters.dimm_s_rank_count());

  channel0_dimm_parameters.set_reg_value(0).set_dimm_s_rank_count_minus_1(1);
  EXPECT_EQ(2, channel0_dimm_parameters.dimm_s_rank_count());
}

TEST(MemoryAddressDecoderDimmParametersAlderLakeTest, DimmLRankCount) {
  auto channel0_dimm_parameters =
      registers::MemoryAddressDecoderDimmParametersAlderLake::GetForControllerAndChannel(0, 0)
          .FromValue(0);

  channel0_dimm_parameters.set_reg_value(0).set_dimm_l_rank_count_minus_1(0);
  EXPECT_EQ(1, channel0_dimm_parameters.dimm_l_rank_count());

  channel0_dimm_parameters.set_reg_value(0).set_dimm_l_rank_count_minus_1(1);
  EXPECT_EQ(2, channel0_dimm_parameters.dimm_l_rank_count());

  channel0_dimm_parameters.set_reg_value(0).set_dimm_l_rank_count_minus_1(2);
  EXPECT_EQ(3, channel0_dimm_parameters.dimm_l_rank_count());

  channel0_dimm_parameters.set_reg_value(0).set_dimm_l_rank_count_minus_1(3);
  EXPECT_EQ(4, channel0_dimm_parameters.dimm_l_rank_count());
}

TEST(MemoryAddressDecoderDimmParametersAlderLakeTest, DimmSSizeMb) {
  auto channel0_dimm_parameters =
      registers::MemoryAddressDecoderDimmParametersAlderLake::GetForControllerAndChannel(0, 0)
          .FromValue(0);

  channel0_dimm_parameters.set_reg_value(0).set_dimm_s_size_512mb(0);
  EXPECT_EQ(0, channel0_dimm_parameters.dimm_s_size_mb());

  channel0_dimm_parameters.set_reg_value(0).set_dimm_s_size_512mb(1);
  EXPECT_EQ(512, channel0_dimm_parameters.dimm_s_size_mb());

  channel0_dimm_parameters.set_reg_value(0).set_dimm_s_size_512mb(8);
  EXPECT_EQ(4'096, channel0_dimm_parameters.dimm_s_size_mb());

  channel0_dimm_parameters.set_reg_value(0).set_dimm_s_size_512mb(127);
  EXPECT_EQ(65'024, channel0_dimm_parameters.dimm_s_size_mb());
}

TEST(MemoryAddressDecoderDimmParametersAlderLakeTest, DimmLSizeMb) {
  auto channel0_dimm_parameters =
      registers::MemoryAddressDecoderDimmParametersAlderLake::GetForControllerAndChannel(0, 0)
          .FromValue(0);

  channel0_dimm_parameters.set_reg_value(0).set_dimm_l_size_512mb(0);
  EXPECT_EQ(0, channel0_dimm_parameters.dimm_l_size_mb());

  channel0_dimm_parameters.set_reg_value(0).set_dimm_l_size_512mb(1);
  EXPECT_EQ(512, channel0_dimm_parameters.dimm_l_size_mb());

  channel0_dimm_parameters.set_reg_value(0).set_dimm_l_size_512mb(8);
  EXPECT_EQ(4'096, channel0_dimm_parameters.dimm_l_size_mb());

  channel0_dimm_parameters.set_reg_value(0).set_dimm_l_size_512mb(127);
  EXPECT_EQ(65'024, channel0_dimm_parameters.dimm_l_size_mb());
}

TEST(MemoryAddressDecoderDimmParametersAlderLakeTest, GetForControllerAndChannel) {
  // The memory controller / channel base MMIO addresses come from the processor
  // datasheets, plus the 0x140'000 base for the GPU aperture.
  //
  // Raptor Lake: 743846-001 Section 3.2 pages 99-100
  // Alder Lake S: 655259-003 Section 3.2 pages 83-34

  auto controller0_channel0_dimm_parameters =
      registers::MemoryAddressDecoderDimmParametersAlderLake::GetForControllerAndChannel(0, 0)
          .FromValue(0);
  EXPECT_EQ(0x14d'80cu, controller0_channel0_dimm_parameters.reg_addr());

  auto controller0_channel1_dimm_parameters =
      registers::MemoryAddressDecoderDimmParametersAlderLake::GetForControllerAndChannel(0, 1)
          .FromValue(0);
  EXPECT_EQ(0x14d'810u, controller0_channel1_dimm_parameters.reg_addr());

  auto controller1_channel0_dimm_parameters =
      registers::MemoryAddressDecoderDimmParametersAlderLake::GetForControllerAndChannel(1, 0)
          .FromValue(0);
  EXPECT_EQ(0x15d'80cu, controller1_channel0_dimm_parameters.reg_addr());

  auto controller1_channel1_dimm_parameters =
      registers::MemoryAddressDecoderDimmParametersAlderLake::GetForControllerAndChannel(1, 1)
          .FromValue(0);
  EXPECT_EQ(0x15d'810u, controller1_channel1_dimm_parameters.reg_addr());
}

TEST(MemoryAddressDecoderDimmParametersCometLakeTest, DimmSDdrChipWidth) {
  auto channel0_dimm_parameters =
      registers::MemoryAddressDecoderDimmParametersCometLake::GetForControllerAndChannel(0, 0)
          .FromValue(0);

  channel0_dimm_parameters.set_reg_value(0).set_dimm_s_ddr_chip_width_select(
      registers::MemoryAddressDecoderDimmParametersCometLake::DdrChipWidthValue::kX8);
  EXPECT_EQ(8, channel0_dimm_parameters.dimm_s_ddr_chip_width());

  channel0_dimm_parameters.set_reg_value(0).set_dimm_s_ddr_chip_width_select(
      registers::MemoryAddressDecoderDimmParametersCometLake::DdrChipWidthValue::kX16);
  EXPECT_EQ(16, channel0_dimm_parameters.dimm_s_ddr_chip_width());

  channel0_dimm_parameters.set_reg_value(0).set_dimm_s_ddr_chip_width_select(
      registers::MemoryAddressDecoderDimmParametersCometLake::DdrChipWidthValue::kX32);
  EXPECT_EQ(32, channel0_dimm_parameters.dimm_s_ddr_chip_width());
}

TEST(MemoryAddressDecoderDimmParametersCometLakeTest, DimmLDdrChipWidth) {
  auto channel0_dimm_parameters =
      registers::MemoryAddressDecoderDimmParametersCometLake::GetForControllerAndChannel(0, 0)
          .FromValue(0);

  channel0_dimm_parameters.set_reg_value(0).set_dimm_l_ddr_chip_width_select(
      registers::MemoryAddressDecoderDimmParametersCometLake::DdrChipWidthValue::kX8);
  EXPECT_EQ(8, channel0_dimm_parameters.dimm_l_ddr_chip_width());

  channel0_dimm_parameters.set_reg_value(0).set_dimm_l_ddr_chip_width_select(
      registers::MemoryAddressDecoderDimmParametersCometLake::DdrChipWidthValue::kX16);
  EXPECT_EQ(16, channel0_dimm_parameters.dimm_l_ddr_chip_width());

  channel0_dimm_parameters.set_reg_value(0).set_dimm_l_ddr_chip_width_select(
      registers::MemoryAddressDecoderDimmParametersCometLake::DdrChipWidthValue::kX32);
  EXPECT_EQ(32, channel0_dimm_parameters.dimm_l_ddr_chip_width());
}

TEST(MemoryAddressDecoderDimmParametersCometLakeTest, DimmSRankCount) {
  auto channel0_dimm_parameters =
      registers::MemoryAddressDecoderDimmParametersCometLake::GetForControllerAndChannel(0, 0)
          .FromValue(0);

  channel0_dimm_parameters.set_reg_value(0).set_dimm_s_rank_count_minus_1(0);
  EXPECT_EQ(1, channel0_dimm_parameters.dimm_s_rank_count());

  channel0_dimm_parameters.set_reg_value(0).set_dimm_s_rank_count_minus_1(1);
  EXPECT_EQ(2, channel0_dimm_parameters.dimm_s_rank_count());
}

TEST(MemoryAddressDecoderDimmParametersCometLakeTest, DimmLRankCount) {
  auto channel0_dimm_parameters =
      registers::MemoryAddressDecoderDimmParametersCometLake::GetForControllerAndChannel(0, 0)
          .FromValue(0);

  channel0_dimm_parameters.set_reg_value(0).set_dimm_l_rank_count_minus_1(0);
  EXPECT_EQ(1, channel0_dimm_parameters.dimm_l_rank_count());

  channel0_dimm_parameters.set_reg_value(0).set_dimm_l_rank_count_minus_1(1);
  EXPECT_EQ(2, channel0_dimm_parameters.dimm_l_rank_count());

  channel0_dimm_parameters.set_reg_value(0).set_dimm_l_rank_count_minus_1(2);
  EXPECT_EQ(3, channel0_dimm_parameters.dimm_l_rank_count());

  channel0_dimm_parameters.set_reg_value(0).set_dimm_l_rank_count_minus_1(3);
  EXPECT_EQ(4, channel0_dimm_parameters.dimm_l_rank_count());
}

TEST(MemoryAddressDecoderDimmParametersCometLakeTest, DimmSSizeMb) {
  auto channel0_dimm_parameters =
      registers::MemoryAddressDecoderDimmParametersCometLake::GetForControllerAndChannel(0, 0)
          .FromValue(0);

  channel0_dimm_parameters.set_reg_value(0).set_dimm_s_size_512mb(0);
  EXPECT_EQ(0, channel0_dimm_parameters.dimm_s_size_mb());

  channel0_dimm_parameters.set_reg_value(0).set_dimm_s_size_512mb(1);
  EXPECT_EQ(512, channel0_dimm_parameters.dimm_s_size_mb());

  channel0_dimm_parameters.set_reg_value(0).set_dimm_s_size_512mb(8);
  EXPECT_EQ(4'096, channel0_dimm_parameters.dimm_s_size_mb());

  channel0_dimm_parameters.set_reg_value(0).set_dimm_s_size_512mb(127);
  EXPECT_EQ(65'024, channel0_dimm_parameters.dimm_s_size_mb());
}

TEST(MemoryAddressDecoderDimmParametersCometLakeTest, DimmLSizeMb) {
  auto channel0_dimm_parameters =
      registers::MemoryAddressDecoderDimmParametersCometLake::GetForControllerAndChannel(0, 0)
          .FromValue(0);

  channel0_dimm_parameters.set_reg_value(0).set_dimm_l_size_512mb(0);
  EXPECT_EQ(0, channel0_dimm_parameters.dimm_l_size_mb());

  channel0_dimm_parameters.set_reg_value(0).set_dimm_l_size_512mb(1);
  EXPECT_EQ(512, channel0_dimm_parameters.dimm_l_size_mb());

  channel0_dimm_parameters.set_reg_value(0).set_dimm_l_size_512mb(8);
  EXPECT_EQ(4'096, channel0_dimm_parameters.dimm_l_size_mb());

  channel0_dimm_parameters.set_reg_value(0).set_dimm_l_size_512mb(127);
  EXPECT_EQ(65'024, channel0_dimm_parameters.dimm_l_size_mb());
}

TEST(MemoryAddressDecoderDimmParametersCometLakeTest, GetForControllerAndChannel) {
  // The memory controller / channel base MMIO addresses come from the processor
  // datasheets, plus the 0x140'000 base for the GPU aperture.
  //
  // Tiger Lake: Processor Datasheet Vol 2a (631122-003) pages 141-143
  // Ice Lake: Processor Datasheet Vol 2 (341078-004) pages 131-132

  auto controller0_channel0_dimm_parameters =
      registers::MemoryAddressDecoderDimmParametersCometLake::GetForControllerAndChannel(0, 0)
          .FromValue(0);
  EXPECT_EQ(0x145'00cu, controller0_channel0_dimm_parameters.reg_addr());

  auto controller0_channel1_dimm_parameters =
      registers::MemoryAddressDecoderDimmParametersCometLake::GetForControllerAndChannel(0, 1)
          .FromValue(0);
  EXPECT_EQ(0x145'010u, controller0_channel1_dimm_parameters.reg_addr());

  auto controller1_channel0_dimm_parameters =
      registers::MemoryAddressDecoderDimmParametersCometLake::GetForControllerAndChannel(1, 0)
          .FromValue(0);
  EXPECT_EQ(0x155'00cu, controller1_channel0_dimm_parameters.reg_addr());

  auto controller1_channel1_dimm_parameters =
      registers::MemoryAddressDecoderDimmParametersCometLake::GetForControllerAndChannel(1, 1)
          .FromValue(0);
  EXPECT_EQ(0x155'010u, controller1_channel1_dimm_parameters.reg_addr());
}

TEST(MemoryAddressDecoderDimmParametersCometLakeTest, GetForTigerLakeHControllerAndChannel) {
  // The memory controller / channel base MMIO addresses come from the processor
  // datasheets, plus the 0x140'000 base for the GPU aperture.
  //

  auto controller0_channel0_dimm_parameters =
      registers::MemoryAddressDecoderDimmParametersCometLake::GetForTigerLakeHControllerAndChannel(
          0, 0)
          .FromValue(0);
  EXPECT_EQ(0x14d'80cu, controller0_channel0_dimm_parameters.reg_addr());

  auto controller0_channel1_dimm_parameters =
      registers::MemoryAddressDecoderDimmParametersCometLake::GetForTigerLakeHControllerAndChannel(
          0, 1)
          .FromValue(0);
  EXPECT_EQ(0x14d'810u, controller0_channel1_dimm_parameters.reg_addr());

  auto controller1_channel0_dimm_parameters =
      registers::MemoryAddressDecoderDimmParametersCometLake::GetForTigerLakeHControllerAndChannel(
          1, 0)
          .FromValue(0);
  EXPECT_EQ(0x15d'80cu, controller1_channel0_dimm_parameters.reg_addr());

  auto controller1_channel1_dimm_parameters =
      registers::MemoryAddressDecoderDimmParametersCometLake::GetForTigerLakeHControllerAndChannel(
          1, 1)
          .FromValue(0);
  EXPECT_EQ(0x15d'810u, controller1_channel1_dimm_parameters.reg_addr());
}

TEST(MemoryAddressDecoderDimmParametersSkylakeTest, DimmSDdrChipWidth) {
  auto channel0_dimm_parameters =
      registers::MemoryAddressDecoderDimmParametersSkylake::GetForChannel(0).FromValue(0);

  channel0_dimm_parameters.set_reg_value(0).set_dimm_s_ddr_chip_width_select(
      registers::MemoryAddressDecoderDimmParametersSkylake::DdrChipWidthValue::kX8);
  EXPECT_EQ(8, channel0_dimm_parameters.dimm_s_ddr_chip_width());

  channel0_dimm_parameters.set_reg_value(0).set_dimm_s_ddr_chip_width_select(
      registers::MemoryAddressDecoderDimmParametersSkylake::DdrChipWidthValue::kX16);
  EXPECT_EQ(16, channel0_dimm_parameters.dimm_s_ddr_chip_width());

  channel0_dimm_parameters.set_reg_value(0).set_dimm_s_ddr_chip_width_select(
      registers::MemoryAddressDecoderDimmParametersSkylake::DdrChipWidthValue::kX32);
  EXPECT_EQ(32, channel0_dimm_parameters.dimm_s_ddr_chip_width());
}

TEST(MemoryAddressDecoderDimmParametersSkylakeTest, DimmLDdrChipWidth) {
  auto channel0_dimm_parameters =
      registers::MemoryAddressDecoderDimmParametersSkylake::GetForChannel(0).FromValue(0);

  channel0_dimm_parameters.set_reg_value(0).set_dimm_l_ddr_chip_width_select(
      registers::MemoryAddressDecoderDimmParametersSkylake::DdrChipWidthValue::kX8);
  EXPECT_EQ(8, channel0_dimm_parameters.dimm_l_ddr_chip_width());

  channel0_dimm_parameters.set_reg_value(0).set_dimm_l_ddr_chip_width_select(
      registers::MemoryAddressDecoderDimmParametersSkylake::DdrChipWidthValue::kX16);
  EXPECT_EQ(16, channel0_dimm_parameters.dimm_l_ddr_chip_width());

  channel0_dimm_parameters.set_reg_value(0).set_dimm_l_ddr_chip_width_select(
      registers::MemoryAddressDecoderDimmParametersSkylake::DdrChipWidthValue::kX32);
  EXPECT_EQ(32, channel0_dimm_parameters.dimm_l_ddr_chip_width());
}

TEST(MemoryAddressDecoderDimmParametersSkylakeTest, DimmSRankCount) {
  auto channel0_dimm_parameters =
      registers::MemoryAddressDecoderDimmParametersSkylake::GetForChannel(0).FromValue(0);

  channel0_dimm_parameters.set_reg_value(0).set_dimm_s_rank_count_minus_1(0);
  EXPECT_EQ(1, channel0_dimm_parameters.dimm_s_rank_count());

  channel0_dimm_parameters.set_reg_value(0).set_dimm_s_rank_count_minus_1(1);
  EXPECT_EQ(2, channel0_dimm_parameters.dimm_s_rank_count());
}

TEST(MemoryAddressDecoderDimmParametersSkylakeTest, DimmLRankCount) {
  auto channel0_dimm_parameters =
      registers::MemoryAddressDecoderDimmParametersSkylake::GetForChannel(0).FromValue(0);

  channel0_dimm_parameters.set_reg_value(0).set_dimm_l_rank_count_minus_1(0);
  EXPECT_EQ(1, channel0_dimm_parameters.dimm_l_rank_count());

  channel0_dimm_parameters.set_reg_value(0).set_dimm_l_rank_count_minus_1(1);
  EXPECT_EQ(2, channel0_dimm_parameters.dimm_l_rank_count());
}

TEST(MemoryAddressDecoderDimmParametersSkylakeTest, DimmSSizeMb) {
  auto channel0_dimm_parameters =
      registers::MemoryAddressDecoderDimmParametersSkylake::GetForChannel(0).FromValue(0);

  channel0_dimm_parameters.set_reg_value(0).set_dimm_s_size_1gb(0);
  EXPECT_EQ(0, channel0_dimm_parameters.dimm_s_size_mb());

  channel0_dimm_parameters.set_reg_value(0).set_dimm_s_size_1gb(1);
  EXPECT_EQ(1'024, channel0_dimm_parameters.dimm_s_size_mb());

  channel0_dimm_parameters.set_reg_value(0).set_dimm_s_size_1gb(8);
  EXPECT_EQ(8'192, channel0_dimm_parameters.dimm_s_size_mb());

  channel0_dimm_parameters.set_reg_value(0).set_dimm_s_size_1gb(63);
  EXPECT_EQ(64'512, channel0_dimm_parameters.dimm_s_size_mb());
}

TEST(MemoryAddressDecoderDimmParametersSkylakeTest, DimmLSizeMb) {
  auto channel0_dimm_parameters =
      registers::MemoryAddressDecoderDimmParametersSkylake::GetForChannel(0).FromValue(0);

  channel0_dimm_parameters.set_reg_value(0).set_dimm_l_size_1gb(0);
  EXPECT_EQ(0, channel0_dimm_parameters.dimm_l_size_mb());

  channel0_dimm_parameters.set_reg_value(0).set_dimm_l_size_1gb(1);
  EXPECT_EQ(1'024, channel0_dimm_parameters.dimm_l_size_mb());

  channel0_dimm_parameters.set_reg_value(0).set_dimm_l_size_1gb(8);
  EXPECT_EQ(8'192, channel0_dimm_parameters.dimm_l_size_mb());

  channel0_dimm_parameters.set_reg_value(0).set_dimm_l_size_1gb(63);
  EXPECT_EQ(64'512, channel0_dimm_parameters.dimm_l_size_mb());
}

TEST(MemoryAddressDecoderDimmParametersSkylakeTest, GetForChannel) {
  // The register MMIO addresses come from the processor datasheets, plus the
  // 0x140'000 base for the GPU aperture.
  //

  auto channel0_dimm_parameters =
      registers::MemoryAddressDecoderDimmParametersSkylake::GetForChannel(0).FromValue(0);
  EXPECT_EQ(0x145'00cu, channel0_dimm_parameters.reg_addr());

  auto channel1_dimm_parameters =
      registers::MemoryAddressDecoderDimmParametersSkylake::GetForChannel(1).FromValue(0);
  EXPECT_EQ(0x145'010u, channel1_dimm_parameters.reg_addr());
}

TEST(MemoryControllerBiosDataIceLakeTest, DdrPhyBusFrequencyHzForTigerLake) {
  auto bios_data = registers::MemoryControllerBiosDataIceLake::Get().FromValue(0);

  bios_data.set_reg_value(0)
      .set_controller_frequency_base_select(
          registers::MemoryControllerBiosDataIceLake::ControllerFrequencyBaseValue::k100Mhz)
      .set_controller_frequency_multiplier(0)
      .set_ddr_phy_bus_clock_multiplier_shift_tiger_lake(0);
  EXPECT_EQ(0, bios_data.ddr_phy_bus_frequency_hz());

  bios_data.set_reg_value(0)
      .set_controller_frequency_base_select(
          registers::MemoryControllerBiosDataIceLake::ControllerFrequencyBaseValue::k133Mhz)
      .set_controller_frequency_multiplier(0)
      .set_ddr_phy_bus_clock_multiplier_shift_tiger_lake(0);
  EXPECT_EQ(0, bios_data.ddr_phy_bus_frequency_hz());

  bios_data.set_reg_value(0)
      .set_controller_frequency_base_select(
          registers::MemoryControllerBiosDataIceLake::ControllerFrequencyBaseValue::k100Mhz)
      .set_controller_frequency_multiplier(0)
      .set_ddr_phy_bus_clock_multiplier_shift_tiger_lake(1);
  EXPECT_EQ(0, bios_data.ddr_phy_bus_frequency_hz());

  bios_data.set_reg_value(0)
      .set_controller_frequency_base_select(
          registers::MemoryControllerBiosDataIceLake::ControllerFrequencyBaseValue::k133Mhz)
      .set_controller_frequency_multiplier(0)
      .set_ddr_phy_bus_clock_multiplier_shift_tiger_lake(1);
  EXPECT_EQ(0, bios_data.ddr_phy_bus_frequency_hz());

  bios_data.set_reg_value(0)
      .set_controller_frequency_base_select(
          registers::MemoryControllerBiosDataIceLake::ControllerFrequencyBaseValue::k100Mhz)
      .set_controller_frequency_multiplier(3)
      .set_ddr_phy_bus_clock_multiplier_shift_tiger_lake(0);
  EXPECT_EQ(300'000'000, bios_data.ddr_phy_bus_frequency_hz());

  bios_data.set_reg_value(0)
      .set_controller_frequency_base_select(
          registers::MemoryControllerBiosDataIceLake::ControllerFrequencyBaseValue::k133Mhz)
      .set_controller_frequency_multiplier(3)
      .set_ddr_phy_bus_clock_multiplier_shift_tiger_lake(0);
  EXPECT_EQ(400'000'000, bios_data.ddr_phy_bus_frequency_hz());

  bios_data.set_reg_value(0)
      .set_controller_frequency_base_select(
          registers::MemoryControllerBiosDataIceLake::ControllerFrequencyBaseValue::k100Mhz)
      .set_controller_frequency_multiplier(3)
      .set_ddr_phy_bus_clock_multiplier_shift_tiger_lake(1);
  EXPECT_EQ(600'000'000, bios_data.ddr_phy_bus_frequency_hz());

  bios_data.set_reg_value(0)
      .set_controller_frequency_base_select(
          registers::MemoryControllerBiosDataIceLake::ControllerFrequencyBaseValue::k133Mhz)
      .set_controller_frequency_multiplier(3)
      .set_ddr_phy_bus_clock_multiplier_shift_tiger_lake(1);
  EXPECT_EQ(800'000'000, bios_data.ddr_phy_bus_frequency_hz());

  bios_data.set_reg_value(0)
      .set_controller_frequency_base_select(
          registers::MemoryControllerBiosDataIceLake::ControllerFrequencyBaseValue::k133Mhz)
      .set_controller_frequency_multiplier(4)
      .set_ddr_phy_bus_clock_multiplier_shift_tiger_lake(0);
  EXPECT_EQ(533'333'333, bios_data.ddr_phy_bus_frequency_hz());

  bios_data.set_reg_value(0)
      .set_controller_frequency_base_select(
          registers::MemoryControllerBiosDataIceLake::ControllerFrequencyBaseValue::k133Mhz)
      .set_controller_frequency_multiplier(4)
      .set_ddr_phy_bus_clock_multiplier_shift_tiger_lake(1);
  EXPECT_EQ(1'066'666'667, bios_data.ddr_phy_bus_frequency_hz());

  bios_data.set_reg_value(0)
      .set_controller_frequency_base_select(
          registers::MemoryControllerBiosDataIceLake::ControllerFrequencyBaseValue::k133Mhz)
      .set_controller_frequency_multiplier(5)
      .set_ddr_phy_bus_clock_multiplier_shift_tiger_lake(0);
  EXPECT_EQ(666'666'667, bios_data.ddr_phy_bus_frequency_hz());

  bios_data.set_reg_value(0)
      .set_controller_frequency_base_select(
          registers::MemoryControllerBiosDataIceLake::ControllerFrequencyBaseValue::k133Mhz)
      .set_controller_frequency_multiplier(5)
      .set_ddr_phy_bus_clock_multiplier_shift_tiger_lake(1);
  EXPECT_EQ(1'333'333'333, bios_data.ddr_phy_bus_frequency_hz());

  bios_data.set_reg_value(0)
      .set_controller_frequency_base_select(
          registers::MemoryControllerBiosDataIceLake::ControllerFrequencyBaseValue::k100Mhz)
      .set_controller_frequency_multiplier(255)
      .set_ddr_phy_bus_clock_multiplier_shift_tiger_lake(1);
  EXPECT_EQ(51'000'000'000, bios_data.ddr_phy_bus_frequency_hz());

  bios_data.set_reg_value(0)
      .set_controller_frequency_base_select(
          registers::MemoryControllerBiosDataIceLake::ControllerFrequencyBaseValue::k133Mhz)
      .set_controller_frequency_multiplier(255)
      .set_ddr_phy_bus_clock_multiplier_shift_tiger_lake(1);
  EXPECT_EQ(68'000'000'000, bios_data.ddr_phy_bus_frequency_hz());
}

TEST(MemoryControllerBiosDataIceLakeTest, ControllerQuadClockFrequencyHzTigerLake) {
  auto bios_data = registers::MemoryControllerBiosDataIceLake::Get().FromValue(0);

  bios_data.set_reg_value(0)
      .set_controller_frequency_base_select(
          registers::MemoryControllerBiosDataIceLake::ControllerFrequencyBaseValue::k100Mhz)
      .set_controller_frequency_multiplier(0);
  EXPECT_EQ(0, bios_data.controller_quad_clock_frequency_hz());

  bios_data.set_reg_value(0)
      .set_controller_frequency_base_select(
          registers::MemoryControllerBiosDataIceLake::ControllerFrequencyBaseValue::k133Mhz)
      .set_controller_frequency_multiplier(0);
  EXPECT_EQ(0, bios_data.controller_quad_clock_frequency_hz());

  bios_data.set_reg_value(0)
      .set_controller_frequency_base_select(
          registers::MemoryControllerBiosDataIceLake::ControllerFrequencyBaseValue::k100Mhz)
      .set_controller_frequency_multiplier(3);
  EXPECT_EQ(300'000'000, bios_data.controller_quad_clock_frequency_hz());

  bios_data.set_reg_value(0)
      .set_controller_frequency_base_select(
          registers::MemoryControllerBiosDataIceLake::ControllerFrequencyBaseValue::k133Mhz)
      .set_controller_frequency_multiplier(3);
  EXPECT_EQ(400'000'000, bios_data.controller_quad_clock_frequency_hz());

  bios_data.set_reg_value(0)
      .set_controller_frequency_base_select(
          registers::MemoryControllerBiosDataIceLake::ControllerFrequencyBaseValue::k133Mhz)
      .set_controller_frequency_multiplier(4);
  EXPECT_EQ(533'333'333, bios_data.controller_quad_clock_frequency_hz());

  bios_data.set_reg_value(0)
      .set_controller_frequency_base_select(
          registers::MemoryControllerBiosDataIceLake::ControllerFrequencyBaseValue::k133Mhz)
      .set_controller_frequency_multiplier(5);
  EXPECT_EQ(666'666'667, bios_data.controller_quad_clock_frequency_hz());

  bios_data.set_reg_value(0)
      .set_controller_frequency_base_select(
          registers::MemoryControllerBiosDataIceLake::ControllerFrequencyBaseValue::k100Mhz)
      .set_controller_frequency_multiplier(255);
  EXPECT_EQ(25'500'000'000, bios_data.controller_quad_clock_frequency_hz());

  bios_data.set_reg_value(0)
      .set_controller_frequency_base_select(
          registers::MemoryControllerBiosDataIceLake::ControllerFrequencyBaseValue::k133Mhz)
      .set_controller_frequency_multiplier(255);
  EXPECT_EQ(34'000'000'000, bios_data.controller_quad_clock_frequency_hz());
}

TEST(MemoryControllerBiosDataIceLakeTest, ControllerFrequencyBaseHzX3) {
  auto bios_data = registers::MemoryControllerBiosDataIceLake::Get().FromValue(0);

  bios_data.set_reg_value(0).set_controller_frequency_base_select(
      registers::MemoryControllerBiosDataIceLake::ControllerFrequencyBaseValue::k100Mhz);
  EXPECT_EQ(300'000'000, bios_data.controller_frequency_base_hz_x3());

  bios_data.set_reg_value(0).set_controller_frequency_base_select(
      registers::MemoryControllerBiosDataIceLake::ControllerFrequencyBaseValue::k133Mhz);
  EXPECT_EQ(400'000'000, bios_data.controller_frequency_base_hz_x3());
}

TEST(MemoryControllerBiosDataIceLakeTest, ControllerFrequencyBaseHzX3Invalid) {
  auto bios_data = registers::MemoryControllerBiosDataIceLake::Get().FromValue(0);

  bios_data.set_reg_value(0).set_controller_frequency_base_select(
      static_cast<registers::MemoryControllerBiosDataIceLake::ControllerFrequencyBaseValue>(7));
  EXPECT_EQ(0, bios_data.controller_frequency_base_hz_x3());

  bios_data.set_reg_value(0).set_controller_frequency_base_select(
      static_cast<registers::MemoryControllerBiosDataIceLake::ControllerFrequencyBaseValue>(6));
  EXPECT_EQ(0, bios_data.controller_frequency_base_hz_x3());
}

TEST(MemoryControllerBiosDataIceLakeTest, DataTransmitRailMaxCurrentMilliamps) {
  auto bios_data = registers::MemoryControllerBiosDataIceLake::Get().FromValue(0);

  bios_data.set_reg_value(0).set_data_transmit_rail_max_current_multiplier(0);
  EXPECT_EQ(0, bios_data.data_transmit_rail_max_current_milliamps());

  bios_data.set_reg_value(0).set_data_transmit_rail_max_current_multiplier(1);
  EXPECT_EQ(250, bios_data.data_transmit_rail_max_current_milliamps());

  bios_data.set_reg_value(0).set_data_transmit_rail_max_current_multiplier(2);
  EXPECT_EQ(500, bios_data.data_transmit_rail_max_current_milliamps());

  bios_data.set_reg_value(0).set_data_transmit_rail_max_current_multiplier(15);
  EXPECT_EQ(3'750, bios_data.data_transmit_rail_max_current_milliamps());
}

TEST(MemoryControllerBiosDataIceLakeTest, DataTransmitRailVoltageMillivolts) {
  auto bios_data = registers::MemoryControllerBiosDataIceLake::Get().FromValue(0);

  bios_data.set_reg_value(0).set_data_transmit_rail_voltage_multiplier(0);
  EXPECT_EQ(0, bios_data.data_transmit_rail_voltage_millivolts());

  bios_data.set_reg_value(0).set_data_transmit_rail_voltage_multiplier(1);
  EXPECT_EQ(5, bios_data.data_transmit_rail_voltage_millivolts());

  bios_data.set_reg_value(0).set_data_transmit_rail_voltage_multiplier(2);
  EXPECT_EQ(10, bios_data.data_transmit_rail_voltage_millivolts());

  bios_data.set_reg_value(0).set_data_transmit_rail_voltage_multiplier(1023);
  EXPECT_EQ(5'115, bios_data.data_transmit_rail_voltage_millivolts());
}

TEST(MemoryControllerBiosDataIceLakeTest, DdrPhyBusFrequencyHzIceLake) {
  auto bios_data = registers::MemoryControllerBiosDataIceLake::Get().FromValue(0);

  bios_data.set_reg_value(0)
      .set_controller_frequency_base_select(
          registers::MemoryControllerBiosDataIceLake::ControllerFrequencyBaseValue::k100Mhz)
      .set_controller_frequency_multiplier(0)
      .set_ddr_phy_bus_clock_multiplier_shift_ice_lake(0);
  EXPECT_EQ(0, bios_data.ddr_phy_bus_frequency_hz());

  bios_data.set_reg_value(0)
      .set_controller_frequency_base_select(
          registers::MemoryControllerBiosDataIceLake::ControllerFrequencyBaseValue::k133Mhz)
      .set_controller_frequency_multiplier(0)
      .set_ddr_phy_bus_clock_multiplier_shift_ice_lake(0);
  EXPECT_EQ(0, bios_data.ddr_phy_bus_frequency_hz());

  bios_data.set_reg_value(0)
      .set_controller_frequency_base_select(
          registers::MemoryControllerBiosDataIceLake::ControllerFrequencyBaseValue::k100Mhz)
      .set_controller_frequency_multiplier(0)
      .set_ddr_phy_bus_clock_multiplier_shift_ice_lake(1);
  EXPECT_EQ(0, bios_data.ddr_phy_bus_frequency_hz());

  bios_data.set_reg_value(0)
      .set_controller_frequency_base_select(
          registers::MemoryControllerBiosDataIceLake::ControllerFrequencyBaseValue::k133Mhz)
      .set_controller_frequency_multiplier(0)
      .set_ddr_phy_bus_clock_multiplier_shift_ice_lake(1);
  EXPECT_EQ(0, bios_data.ddr_phy_bus_frequency_hz());

  bios_data.set_reg_value(0)
      .set_controller_frequency_base_select(
          registers::MemoryControllerBiosDataIceLake::ControllerFrequencyBaseValue::k100Mhz)
      .set_controller_frequency_multiplier(3)
      .set_ddr_phy_bus_clock_multiplier_shift_ice_lake(0);
  EXPECT_EQ(300'000'000, bios_data.ddr_phy_bus_frequency_hz());

  bios_data.set_reg_value(0)
      .set_controller_frequency_base_select(
          registers::MemoryControllerBiosDataIceLake::ControllerFrequencyBaseValue::k133Mhz)
      .set_controller_frequency_multiplier(3)
      .set_ddr_phy_bus_clock_multiplier_shift_ice_lake(0);
  EXPECT_EQ(400'000'000, bios_data.ddr_phy_bus_frequency_hz());

  bios_data.set_reg_value(0)
      .set_controller_frequency_base_select(
          registers::MemoryControllerBiosDataIceLake::ControllerFrequencyBaseValue::k100Mhz)
      .set_controller_frequency_multiplier(3)
      .set_ddr_phy_bus_clock_multiplier_shift_ice_lake(1);
  EXPECT_EQ(600'000'000, bios_data.ddr_phy_bus_frequency_hz());

  bios_data.set_reg_value(0)
      .set_controller_frequency_base_select(
          registers::MemoryControllerBiosDataIceLake::ControllerFrequencyBaseValue::k133Mhz)
      .set_controller_frequency_multiplier(3)
      .set_ddr_phy_bus_clock_multiplier_shift_ice_lake(1);
  EXPECT_EQ(800'000'000, bios_data.ddr_phy_bus_frequency_hz());

  bios_data.set_reg_value(0)
      .set_controller_frequency_base_select(
          registers::MemoryControllerBiosDataIceLake::ControllerFrequencyBaseValue::k133Mhz)
      .set_controller_frequency_multiplier(4)
      .set_ddr_phy_bus_clock_multiplier_shift_ice_lake(0);
  EXPECT_EQ(533'333'333, bios_data.ddr_phy_bus_frequency_hz());

  bios_data.set_reg_value(0)
      .set_controller_frequency_base_select(
          registers::MemoryControllerBiosDataIceLake::ControllerFrequencyBaseValue::k133Mhz)
      .set_controller_frequency_multiplier(4)
      .set_ddr_phy_bus_clock_multiplier_shift_ice_lake(1);
  EXPECT_EQ(1'066'666'667, bios_data.ddr_phy_bus_frequency_hz());

  bios_data.set_reg_value(0)
      .set_controller_frequency_base_select(
          registers::MemoryControllerBiosDataIceLake::ControllerFrequencyBaseValue::k133Mhz)
      .set_controller_frequency_multiplier(5)
      .set_ddr_phy_bus_clock_multiplier_shift_ice_lake(0);
  EXPECT_EQ(666'666'667, bios_data.ddr_phy_bus_frequency_hz());

  bios_data.set_reg_value(0)
      .set_controller_frequency_base_select(
          registers::MemoryControllerBiosDataIceLake::ControllerFrequencyBaseValue::k133Mhz)
      .set_controller_frequency_multiplier(5)
      .set_ddr_phy_bus_clock_multiplier_shift_ice_lake(1);
  EXPECT_EQ(1'333'333'333, bios_data.ddr_phy_bus_frequency_hz());

  bios_data.set_reg_value(0)
      .set_controller_frequency_base_select(
          registers::MemoryControllerBiosDataIceLake::ControllerFrequencyBaseValue::k100Mhz)
      .set_controller_frequency_multiplier(255)
      .set_ddr_phy_bus_clock_multiplier_shift_ice_lake(1);
  EXPECT_EQ(51'000'000'000, bios_data.ddr_phy_bus_frequency_hz());

  bios_data.set_reg_value(0)
      .set_controller_frequency_base_select(
          registers::MemoryControllerBiosDataIceLake::ControllerFrequencyBaseValue::k133Mhz)
      .set_controller_frequency_multiplier(255)
      .set_ddr_phy_bus_clock_multiplier_shift_ice_lake(1);
  EXPECT_EQ(68'000'000'000, bios_data.ddr_phy_bus_frequency_hz());
}

TEST(MemoryControllerBiosDataSkylakeTest, ControllerQuadClockFrequencyHz) {
  auto bios_data = registers::MemoryControllerBiosDataSkylake::Get().FromValue(0);

  // Test cases from the register-level reference.
  //
  // Kaby Lake: Processor Datasheet Vol 2 (631122-003) Section 6.83 page 198

  bios_data.set_reg_value(0).set_controller_frequency_multiplier(0);
  EXPECT_EQ(0, bios_data.controller_quad_clock_frequency_hz());

  bios_data.set_reg_value(0).set_controller_frequency_multiplier(3);
  EXPECT_EQ(800'000'000, bios_data.controller_quad_clock_frequency_hz());

  bios_data.set_reg_value(0).set_controller_frequency_multiplier(4);
  EXPECT_EQ(1'066'666'667, bios_data.controller_quad_clock_frequency_hz());

  bios_data.set_reg_value(0).set_controller_frequency_multiplier(5);
  EXPECT_EQ(1'333'333'333, bios_data.controller_quad_clock_frequency_hz());

  bios_data.set_reg_value(0).set_controller_frequency_multiplier(6);
  EXPECT_EQ(1'600'000'000, bios_data.controller_quad_clock_frequency_hz());

  bios_data.set_reg_value(0).set_controller_frequency_multiplier(7);
  EXPECT_EQ(1'866'666'667, bios_data.controller_quad_clock_frequency_hz());

  bios_data.set_reg_value(0).set_controller_frequency_multiplier(8);
  EXPECT_EQ(2'133'333'333, bios_data.controller_quad_clock_frequency_hz());
}

}  // namespace

}  // namespace intel_display
