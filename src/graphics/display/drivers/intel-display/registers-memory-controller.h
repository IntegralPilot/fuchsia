// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_DRIVERS_INTEL_DISPLAY_REGISTERS_MEMORY_CONTROLLER_H_
#define SRC_GRAPHICS_DISPLAY_DRIVERS_INTEL_DISPLAY_REGISTERS_MEMORY_CONTROLLER_H_

#include <zircon/assert.h>

#include <algorithm>
#include <cstdint>

#include <hwreg/bitfields.h>

#include "src/graphics/display/drivers/intel-display/hardware-common.h"

namespace registers {

// The registers of the processor's memory controller are exposed via MMIO in
// the MCHBAR (Memory Controller Hub Base Address Range). This header only
// covers the registers that are relevant to the display engine's operation.
//
// The memory controller registers are typically documented in Volume 2 of the
// respective processor's datasheet. This header references the following
// documents.
//
// Raptor Lake: 13th Generation Intel Core Processor Datasheet, Volume 2 of 2,
//              document 743846-001
// Alder Lake S: 12th Generation Intel Core Processors Datasheet, Volume 2 of 2,
//               document 655259-003
// Alder Lake H: 12th Generation Intel Core Processors Datasheet, Volume 2 of 2,
//               document 710723-003
// Rocket Lake: 11th Generation Intel Core Processor Datasheet, Volume 2 of 2,
//              document 636761-004
// Tiger Lake U: 11th Generation Intel Core Processors Datasheet,
//               Volume 2a of 2, document 631122-003
// Tiger Lake H: 11th Generation Intel Core Processors Datasheet,
//               Volume 2b of 2, document 643524-003
// Ice Lake: 10th Generation Intel Processor Families Datasheet, Volume 2 of 2,
//           document 341078-004
// Comet Lake: 10th Generation Intel Processor Families Datasheet,
//             Volume 2 of 2, document 615212-003
// Coffee Lake: 8th and 9th Generation Intel Core Processor Families and Intel
//              Xeon E Processor Family Datasheet, Volume 2 of 2,
//              document 337345-003
// Whiskey Lake: 8th Generation Intel Core Processor Families Datasheet,
//               Volume 2 of 2, document 338024-001
// Amber Lake: 7th Generation Intel Processor Families for U/Y Platforms and 8th
//             Generation Intel Processor Family for U Quad Core Datasheet,
//             Volume 2 of 2, document 334662-005
// Kaby Lake S: 7th Generation Intel Processor Families for S Platforms and
//              Intel Core X-Series Processor Family Datasheet, Volume 2 of 2,
//              document 335196-002
// Kaby Lake H: 7th Generation Intel Processor Families for H Platforms
//              Datasheet, Volume 2 of 2, document 335191-003
// Skylake U: 6th Generation Intel Processor Families for U/Y Platforms
//            Datasheet, Volume 2 of 2, document 332991-003
// Skylake S: 6th Generation Intel Processor Datasheet for S-Platforms
//            Datasheet – Volume 2 of 2, document 332688-003
// Skylake H: 6th Generation Intel® Processor Families for H Platforms
//            Datasheet, Volume 2 of 2, document 332987-003
//
// Many processor datasheets are listed at
// https://www.intel.com/content/www/us/en/products/docs/processors/core/core-technical-resources.html

// MCHBAR (Memory Controller Hub Base Address Range) Aperture
//
// A subset of the memory controller's MMIO address space is mirrored in the
// graphics MMIO space, so the graphics drivers can conveniently read DRAM
// configuration information.
//
// This aperture is documented in a "GFX MMIO – MCHBAR Aperture" section in most
// recent PRMs. While the section is missing from the Tiger Lake PRMs, we have
// experimentally confirmed that the aperture still works.
//
// Lakefield: IHD-OS-LKF-Vol 13-4.21, page 9
// Ice Lake: IHD-OS-ICLLP-Vol 13-1.20, page 9
// Kaby Lake: IHD-OS-KBL-Vol 5-1.17, page 2
// Skylake: IHD-OS-SKL-Vol 5-05.16, page 2
constexpr uint32_t kMemoryControllerBarMirrorOffset = 0x140'000;

// TC_PRE_0_0_0_MCHBAR (PRE Command Timing)
//
// Raptor Lake: 743846-001 Section 3.2.70 pages 143-144
// Alder Lake S: 655259-003 Section 3.2.66 pages 126-127
// Alder Lake H: 710723-003 Section 3.2.66 pages 155-156
class MemoryChannelTimingsAlderLake
    : public hwreg::RegisterBase<MemoryChannelTimingsAlderLake, uint64_t> {
 public:
  DEF_RSVDZ_BIT(63);

  // Value added to DRAM timings when the LPDDR dimm is hot.
  //
  // This field must be zero for non-LP (Low-Power) DDR.
  DEF_FIELD(62, 59, derating_extensions);

  // tRCD DDR timing parameter.
  //
  // The minimum delay between ACT and CAS in the same bank.
  //
  // On LPDDR5x, this delay only applies to RD (read) CAS, and the minimum delay
  // between ACT and WR (write) CAS is specified separately the `t_rcdw` field.
  // On other technologies, this delay applies to both RD and WR.
  DEF_FIELD(58, 51, t_rcd);

  // tRAS DDR timing parameter.
  //
  // The minimum delay between ACT and PRE in the same bank.
  DEF_FIELD(50, 42, t_ras);

  // tWRPRE DDR timing parameter.
  //
  // The minimum delay between WR and PRE in the same bank.
  DEF_FIELD(41, 32, t_wrpre);

  // t_RCDW DDR timing parameter.
  //
  // The minimum delay between ACT and CAS WR in the same bank. This delay is
  // only relevant in LPDDR5x configurations.
  DEF_FIELD(31, 24, t_rcdw);

  // tPPD DDR timing parameter.
  //
  // The minimum delay between PRE/PREALL commands in the same rank.
  //
  // This field is not used in DDR5 configurations.
  DEF_FIELD(23, 20, t_ppd);

  // tRDPRE DDR timing parameter.
  //
  // The minimum delay between RD and PRE commands in the same bank.
  DEF_FIELD(19, 13, t_rdpre);

  // tRPab - tRBpb LPDDR timing parameter value.
  //
  // The difference between the minimum delay between PREALL and ACT and the
  // minimum delay between PRE and ACT. Must be zero for DDR, because only LPDDR
  // allows this difference.
  DEF_FIELD(12, 8, t_rpab_ext);

  // tRP DDR timing parameter.
  //
  // The minimum delay between PRE and ACT in the same bank.
  DEF_FIELD(7, 0, t_rp);

  static auto GetForControllerAndChannel(int memory_controller_index, int channel_index) {
    // The MMIO offsets for the 2nd memory channel and controller are documented
    // in the "[Processor] Memory Controller (MCHBAR) Registers" section of the
    // Volume 2 in the processor datasheets.
    //
    // Raptor Lake: 743846-001 Section 3.2 pages 99-100
    // Alder Lake S: 655259-003 Section 3.2 pages 83-34
    // Alder Lake H: 710723-003 Section 3.2 pages 112-113

    ZX_ASSERT(memory_controller_index >= 0);
    ZX_ASSERT(memory_controller_index <= 1);
    ZX_ASSERT(channel_index >= 0);
    ZX_ASSERT(channel_index <= 1);
    return hwreg::RegisterAddr<MemoryChannelTimingsAlderLake>(
        kMemoryControllerBarMirrorOffset + 0xe000 + 0x10000 * memory_controller_index +
        0x800 * channel_index);
  }
};

// TC_PRE_0_0_0_MCHBAR (PRE Command Timing)
//
// See `MemoryChannelTimingsAlderLake` for field semantics.
//
// Tiger Lake U: 631122-003 Section 3.2.2 pages 110-111
// Tiger Lake H: 643524-003 Section 3.2.24 pages 128-129
class MemoryChannelTimingsTigerLake
    : public hwreg::RegisterBase<MemoryChannelTimingsTigerLake, uint64_t> {
 public:
  DEF_RSVDZ_FIELD(63, 48);

  DEF_FIELD(47, 41, t_rcd);
  DEF_FIELD(40, 33, t_ras);

  DEF_RSVDZ_FIELD(32, 30);

  DEF_FIELD(29, 21, t_wrpre);
  DEF_FIELD(20, 17, t_ppd);
  DEF_FIELD(16, 11, t_rdpre);
  DEF_FIELD(10, 7, t_rpab_ext);
  DEF_FIELD(6, 0, t_rp);

  // For Tiger Lake U and H35 SKUs.
  //
  // `memory_controller_index` and `channel_index` are 0-based.
  static auto GetForUControllerAndChannel(int memory_controller_index, int channel_index) {
    // The MMIO offsets for the 2nd memory channel and controller are documented
    // in the "[Processor] Memory Controller (MCHBAR) Registers" section of the
    // Volume 2 in the processor datasheets.
    //
    // Tiger Lake U: 631122-003 Section 3.2 pages 106-107
    ZX_ASSERT(memory_controller_index >= 0);
    ZX_ASSERT(memory_controller_index <= 1);
    ZX_ASSERT(channel_index >= 0);
    ZX_ASSERT(channel_index <= 1);
    return hwreg::RegisterAddr<MemoryChannelTimingsTigerLake>(
        kMemoryControllerBarMirrorOffset + 0x4000 + 0x10000 * memory_controller_index +
        0x400 * channel_index);
  }

  // For Tiger Lake H SKUs.
  //
  // `memory_controller_index` and `channel_index` are 0-based.
  static auto GetForHControllerAndChannel(int memory_controller_index, int channel_index) {
    // The MMIO offsets for the 2nd memory channel and controller are documented
    // in the "[Processor] Memory Controller (MCHBAR) Registers" section of the
    // Volume 2 in the processor datasheets.
    //
    // Tiger Lake H: 643524-003 Section 3.2 pages 110-111
    ZX_ASSERT(memory_controller_index >= 0);
    ZX_ASSERT(memory_controller_index <= 1);
    ZX_ASSERT(channel_index >= 0);
    ZX_ASSERT(channel_index <= 1);
    return hwreg::RegisterAddr<MemoryChannelTimingsTigerLake>(
        kMemoryControllerBarMirrorOffset + 0xe000 + 0x10000 * memory_controller_index +
        0x800 * channel_index);
  }
};

// TC_PRE_0_0_0_MCHBAR (PRE Command Timing)
//
// See `MemoryChannelTimingsAlderLake` for field semantics.
//
// Rocket Lake: 636761-004 Section 3.2.2 pages 98-99
// Ice Lake: 341078-004 Section 3.2.2 pages 101-102
class MemoryChannelTimingsIceLake
    : public hwreg::RegisterBase<MemoryChannelTimingsIceLake, uint32_t> {
 public:
  DEF_FIELD(31, 24, t_wrpre);
  DEF_FIELD(23, 21, t_ppd);
  DEF_FIELD(20, 16, t_rdpre);
  DEF_FIELD(15, 9, t_ras);
  DEF_FIELD(8, 6, t_rpab_ext);

  // tRCD and tRP DDR timing parameters.
  DEF_FIELD(5, 0, t_rp_rcd);

  // `channel_index` is 0-based.
  static auto GetForChannel(int channel_index) {
    // The MMIO offsets for the 2nd memory channel are documented in the
    // "[Processor] Memory Controller (MCHBAR) Registers" section of the Volume
    // 2 in the processor datasheets.
    //
    // Rocket Lake: 636761-004 Section 3.2 page 96
    // Ice Lake: 341078-004 Section 3.2 pages 99-100

    ZX_ASSERT(channel_index >= 0);
    ZX_ASSERT(channel_index <= 1);
    return hwreg::RegisterAddr<MemoryChannelTimingsIceLake>(kMemoryControllerBarMirrorOffset +
                                                            0x4000 + 0x400 * channel_index);
  }
};

// TC_PRE_0_0_0_MCHBAR (PRE Command Timing)
//
// See `MemoryChannelTimingsAlderLake` for field semantics.
//
// In older documentation, this register's instances are documented as
// MCHBAR_CH0_CR_TC_PRE_0_0_0_MCHBAR and MCHBAR_CH1_CR_TC_PRE_0_0_0_MCHBAR.
//
// Comet Lake: 615212-003 Section 7.1 pages 147-148
// Coffee Lake: 337345-003 Section 7.1 page 158
// Whiskey Lake: 338024-001 Section 7.1 pages 144-145
// Amber Lake: 334662-005 Section 6.1 pages 123-124
// Kaby Lake S: 335196-002 Section 7.1 pages 155-156
// Kaby Lake H: 335191-003 Section 7.1 pages 160-161
// Skylake U: 332991-003 Section 7.1 pages 157-158
// Skylake S: 332688-003 Section 7.1 page 146
// Skylake H: 332987-003 Section 7.1 pages 157-158
class MemoryChannelTimingsSkylake
    : public hwreg::RegisterBase<MemoryChannelTimingsSkylake, uint32_t> {
 public:
  DEF_FIELD(30, 24, t_wrpre);
  DEF_FIELD(19, 16, t_rdpre);
  DEF_FIELD(14, 8, t_ras);
  DEF_FIELD(7, 6, t_rpab_ext);

  // tRCD and tRP DDR timing parameters.
  DEF_FIELD(5, 0, t_rp_rcd);

  // `channel_index` is 0-based.
  static auto GetForChannel(int channel_index) {
    // The 2nd memory channel MMIO offset is documented in the "[Processor]
    // Memory Controller (MCHBAR) Registers" section of the Volume 2 in more
    // recent processor datasheets.
    //
    // Comet Lake: 615212-003 Section 7 page 146
    // Whiskey Lake: 338024-001 Section 7 page 142
    //
    // In older processor datasheets, the 2nd memory channel offsets are only
    // documented implicitly, by the presence of registers such as
    // MCHBAR_CH1_CR_TC_PRE_0_0_0_MCHBAR.
    //
    // Coffee Lake: 337345-003 Section 7.13 page 170
    // Amber Lake: 334662-005 Section 6.11 page 134
    // Kaby Lake S: 335196-002 Section 7.11 page 166
    // Kaby Lake H: Section 7.13 page 172
    // Skylake U: 332991-003 Section 7.11 pages 167-168
    // Skylake S: 332688-003 Section 7.11 page 154
    // Skylake H: 332987-003 Section 7.11 pages 167-168

    ZX_ASSERT(channel_index >= 0);
    ZX_ASSERT(channel_index <= 1);
    return hwreg::RegisterAddr<MemoryChannelTimingsSkylake>(kMemoryControllerBarMirrorOffset +
                                                            0x4000 + 0x400 * channel_index);
  }
};

// MAD_INTER_CHANNEL_0_0_0_MCHBAR (Inter-Channel Decode Parameters).
//
// Raptor Lake: 743846-001 Section 3.2.46 pages 126-127
// Alder Lake S: 655259-003 Section 3.2.42 pages 107-108
// Alder Lake H: 710723-003 Section 3.2.42 pages 136-137
// Rocket Lake: 636761-004 Section 3.2.36 pages 125-126
// Tiger Lake U: 631122-003 Section 3.2.38 pages 139-140
// Tiger Lake H: 643524-003 Section 3.2.6 pages 115-116
// Ice Lake: 341078-004 Section 3.2.35 pages 128-129
class MemoryAddressDecoderInterChannelConfigIceLake
    : public hwreg::RegisterBase<MemoryAddressDecoderInterChannelConfigIceLake, uint32_t> {
 public:
  // Documented values for `channel_width_select`.
  enum class ChannelWidthTigerLakeValue {
    kX16 = 0b00,
    kX32 = 0b01,
    kX64 = 0b10,
    kReserved = 0b11,
  };

  // Documented values for `ddr_type_select`.
  enum class DdrTypeValue {
    kDoubleDataRam4 = 0,          // DDRAM 4
    kDoubleDataRam5 = 1,          // DDRAM 5
    kLowPowerDoubleDataRam5 = 2,  // LPDDRAM5
    kLowPowerDoubleDataRam4 = 3,  // LPDDRAM4
  };

  // If true, the memory controller operates on 32-byte requests.
  //
  // By default, the memory controller operates on 64-byte cache lines.
  //
  // This field is not defined on some platforms (Rocket Lake, Ice Lake, Comet
  // Lake). The underlying bit is reserved MBZ (must be zero). This results in
  // correct read semantics, as the feature appears disabled on these platforms.
  DEF_BIT(31, half_cacheline_mode_enabled);

  DEF_RSVDZ_FIELD(30, 29);

  // This field is not documented on some platforms (Ice Lake, Comet Lake). The
  // underlying bits are MBZ (must be zero).
  DEF_ENUM_FIELD(ChannelWidthTigerLakeValue, 28, 27, channel_width_select_tiger_lake);

  DEF_RSVDZ_FIELD(26, 20);

  // The size of Channel S in 512MB units.
  //
  // Valid values are 0-128.
  DEF_FIELD(19, 12, channel_s_size_512);

  DEF_RSVDZ_FIELD(11, 5);

  // If false, Channel L is the physical channel 0.
  DEF_BIT(4, channel_l_is_physical_channel1);

  // If true, the channel operates as two 32-bit channels.
  //
  // This field should only be set to true in LPDDR4 configurations. By default,
  // LPDDR4 has a single 64-bit channel.
  //
  // This bit is reserved MBZ (must be zero) on some platforms (Raptor Lake,
  // Alder Lake, Tiger Lake). This gives us the right read semantics, as the
  // feature appears disabled on these platforms.
  DEF_BIT(3, enhanced_channel_mode);

  // The type of DDR memory installed in the system.
  DEF_ENUM_FIELD(DdrTypeValue, 2, 0, ddr_type_select);

  // For Rocket Lake, Tiger Lake U/H35, and Ice Lake.
  //
  // `memory_controller_index` is 0-based. Rocket Lake and Ice Lake processors
  // have a single memory controller.
  static auto GetForController(int memory_controller_index) {
    // The MMIO offsets for 2nd memory controller are documented in the
    // "[Processor] Memory Controller (MCHBAR) Registers" section of the Volume
    // 2 in the processor datasheets.
    //
    // Rocket Lake: 636761-004 Section 3.2 page 96 (no 2nd memory controller)
    // Tiger Lake U: 631122-003 Section 3.2 pages 106-107
    // Ice Lake: 341078-004 Section 3.2 pages 99-100 (no 2nd memory controller)

    ZX_ASSERT(memory_controller_index >= 0);
    ZX_ASSERT(memory_controller_index <= 1);
    return hwreg::RegisterAddr<MemoryAddressDecoderInterChannelConfigIceLake>(
        kMemoryControllerBarMirrorOffset + 0x5000 + 0x10000 * memory_controller_index);
  }

  // For Raptor Lake, Alder Lake, and Tiger Lake H.
  //
  // `memory_controller_index` is 0-based
  static auto GetForAlderLakeController(int memory_controller_index) {
    // The MMIO offsets for 2nd memory controller are documented in the
    // "[Processor] Memory Controller (MCHBAR) Registers" section of the Volume
    // 2 in the processor datasheets.
    //
    // Raptor Lake: 743846-001 Section 3.2 pages 99-100
    // Alder Lake S: 655259-003 Section 3.2 pages 83-34
    // Alder Lake H: 710723-003 Section 3.2 pages 112-113
    // Tiger Lake H: 643524-003 Section 3.2 pages 110-111

    ZX_ASSERT(memory_controller_index >= 0);
    ZX_ASSERT(memory_controller_index <= 1);
    return hwreg::RegisterAddr<MemoryAddressDecoderInterChannelConfigIceLake>(
        kMemoryControllerBarMirrorOffset + 0xd800 + 0x10000 * memory_controller_index);
  }
};

// MAD_INTER_CHANNEL_0_0_0_MCHBAR (Address decoder inter channel configuration).
//
// Comet Lake: 615212-003 Section 8.33 pages 191-192
// Coffee Lake: 337345-003 Section 7.37 page 193
// Whiskey Lake: 338024-001 Section 7.10 page 153
// Amber Lake: 334662-005 Section 6.31 page 154
// Kaby Lake S: 335196-002 Section 7.31 page 186
// Kaby Lake H: 335191-003 Section 7.37 page 195
// Skylake U: 332991-003 Section 7.31 page 187
// Skylake S: 332688-003 Section 7.31 page 169
// Skylake H: 332987-003 Section 7.31 page 187
class MemoryAddressDecoderInterChannelConfigSkylake
    : public hwreg::RegisterBase<MemoryAddressDecoderInterChannelConfigSkylake, uint32_t> {
 public:
  // Documented values for `ddr_type_select`.
  enum class DdrTypeValue {
    kDoubleDataRam4 = 0b000,          // DDR4
    kDoubleDataRam3 = 0b011,          // DDR3
    kLowPowerDoubleDataRam3 = 0b010,  // LPDDR3
    kLowPowerDoubleDataRam4 = 0b011,  // LPDDR4, reserved before Comet Lake
  };

  DEF_RSVDZ_FIELD(31, 29);

  // True if the attached memory is DDR4-E.
  //
  // This bit should be zero if `ddr_type_select` does not indicate DDR4.
  //
  // This bit is only defined on Comet Lake, and is reserved MBZ (must be zero)
  // on all other platforms. This gives us the right read semantics, as the
  // feature appears disabled on these platforms.
  DEF_BIT(28, uses_ddr4e_memory);

  DEF_RSVDZ_FIELD(27, 19);

  // The size of Channel S in 1GB units.
  //
  // Valid values are 0-64.
  //
  // This field value is does not match the Comet Lake datasheet. Comet Lake
  // memory controllers use the field layout in the
  // `MemoryAddressDecoderInterChannelConfigIceLake` class. `channel_s_size`
  // uses bits 19:12 and expresses the channel size in multiples of 512 MB. See
  // `ddr_type_select` for why we use this class for Comet Lake.
  DEF_FIELD(18, 12, channel_s_size_1gb);

  DEF_RSVDZ_FIELD(11, 5);

  // If false, Channel L is the physical channel 0.
  DEF_BIT(4, channel_l_is_physical_channel1);

  // If true, the channel operates as two 32-bit channels.
  //
  // This bit must be true in LPDDR4 configurations, and can be set to
  // true for LPDDR3. In all other configurations, this bit must be false.
  //
  // This bit is only defined on Comet Lake, and is reserved MBZ (must be zero)
  // on all other platforms. This gives us the right read semantics, as the
  // feature appears disabled on these platforms.
  DEF_BIT(3, enhanced_channel_mode);

  // The type of DDR memory installed in the system.
  //
  // Bit 2 only belongs to this field on Comet Lake. On other platforms, bit 2
  // is reserved MBZ (must be zero). This gives us the right read semantics,
  // because all documented DDR type values have bit 2 set to zero.
  //
  // Comet Lake memory controllers use the DDR type values documented here, in
  // `DdrTypeValue`. This is the main reason we use this class for Comet Lake,
  // instead of `MemoryAddressDecoderInterChannelConfigIceLake`.
  DEF_ENUM_FIELD(DdrTypeValue, 2, 0, ddr_type_select);

  static auto Get() {
    return hwreg::RegisterAddr<MemoryAddressDecoderInterChannelConfigSkylake>(
        kMemoryControllerBarMirrorOffset + 0x5000);
  }
};

// MAD_DIMM_CH0_0_0_0_MCHBAR (Channel 0 DIMM Characteristics)
// MAD_DIMM_CH1_0_0_0_MCHBAR (Channel 1 DIMM Characteristics)
//
// Raptor Lake: 743846-001 Sections 3.2.49-3.2.50 pages 128-130
// Alder Lake S: 655259-003 Sections 3.2.45-3.2.46 pages 110-112
// Alder Lake H: 710723-003 Sections 3.2.45-3.2.46 pages 136-137
class MemoryAddressDecoderDimmParametersAlderLake
    : public hwreg::RegisterBase<MemoryAddressDecoderDimmParametersAlderLake, uint32_t> {
 public:
  enum class DdrChipWidthValue {
    kX8 = 0b00,
    kX16 = 0b01,
    kX32 = 0b10,
    kReserved = 0b11,
  };

  // If true, XaB (extended bank hashing B) is enabled in the address decoder.
  DEF_BIT(31, enable_extended_bank_hashing_b);

  // If true, XbB (extended bank hashing A) is enabled in the address decoder.
  DEF_BIT(30, enable_extended_bank_hashing_a);

  // Selects how zone address bits feed into BG and CAS bits.
  //
  // This field's value influences the computation of BG0/1 (Bank Group bits 0
  // and 1) and CAS5/6 (column address bits 5 and 6). The address computation
  // depends on the DDR type and the value in this field.
  DEF_FIELD(29, 28, bank_group_bit_options);

  // The number of ranks in DIMM S. 0 = 1 rank, ... 3 = 4 ranks.
  //
  // Values above 1 (2 ranks) are not valid for for DIMM S.
  DEF_FIELD(27, 26, dimm_s_rank_count_minus_1);

  // The DDR chip width for DIMM S.
  DEF_ENUM_FIELD(DdrChipWidthValue, 25, 24, dimm_s_ddr_chip_width_select);

  DEF_RSVDZ_BIT(23);

  // Size of DIMM S in multiples of 0.5 GB (512 MB).
  DEF_FIELD(22, 16, dimm_s_size_512mb);

  DEF_RSVDZ_FIELD(15, 13);

  // If false, DIMM L capacity exceeds 8GB.
  //
  // This bit must be false for non-DDR5 configurations.
  DEF_BIT(12, ddr5_dimm_l_capacity_is_8gb);

  // If false, DIMM S capacity exceeds 8GB.
  //
  // This bit must be false for non-DDR5 configurations.
  DEF_BIT(11, ddr5_dimm_s_capacity_is_8gb);

  // The number of ranks in DIMM L. 0 = 1 rank, 1 = 2 ranks ... 3 = 4 ranks.
  //
  // Values above 1 (2 ranks) are only valid in ERM (Enhanced Rank Mode).
  DEF_FIELD(10, 9, dimm_l_rank_count_minus_1);

  DEF_ENUM_FIELD(DdrChipWidthValue, 8, 7, dimm_l_ddr_chip_width_select);

  // Size of DIMM L in multiples of 0.5 GB (512 MB).
  DEF_FIELD(6, 0, dimm_l_size_512mb);

  // The width of the DDR chips in DIMM S.
  int dimm_s_ddr_chip_width() const {
    return DdrChipWidthValueToInteger(dimm_s_ddr_chip_width_select());
  }

  // The width of the DDR chips in DIMM L.
  int dimm_l_ddr_chip_width() const {
    return DdrChipWidthValueToInteger(dimm_l_ddr_chip_width_select());
  }

  // The number of ranks in DIMM S.
  int dimm_s_rank_count() const {
    // The cast and the addition will not overflow (causing UB) because the
    // underlying field is a 2-bit integer.
    return static_cast<int>(dimm_s_rank_count_minus_1()) + 1;
  }

  // The number of ranks in DIMM L.
  int dimm_l_rank_count() const {
    // The cast and the addition will not overflow (causing UB) because the
    // underlying field is a 2-bit integer.
    return static_cast<int>(dimm_l_rank_count_minus_1()) + 1;
  }

  // The size of DIMM S, in MB.
  int dimm_s_size_mb() const {
    // The cast and the multiplication will not overflow (causing UB) because
    // the underlying field is a 7-bit integer.
    return static_cast<int>(dimm_s_size_512mb()) * 512;
  }

  // The size of DIMM L, in MB.
  int dimm_l_size_mb() const {
    // The cast and the multiplication will not overflow (causing UB) because
    // the underlying field is a 7-bit integer.
    return static_cast<int>(dimm_l_size_512mb()) * 512;
  }

  static auto GetForControllerAndChannel(int memory_controller_index, int channel_index) {
    ZX_ASSERT(memory_controller_index >= 0);
    ZX_ASSERT(memory_controller_index <= 1);
    ZX_ASSERT(channel_index >= 0);
    ZX_ASSERT(channel_index <= 1);
    return hwreg::RegisterAddr<MemoryAddressDecoderDimmParametersAlderLake>(
        kMemoryControllerBarMirrorOffset + 0xd80c + 0x10000 * memory_controller_index +
        4 * channel_index);
  }

 private:
  static int DdrChipWidthValueToInteger(DdrChipWidthValue value) {
    return 8 << static_cast<int>(value);
  }
};

// MAD_DIMM_CH0_0_0_0_MCHBAR (Channel 0 DIMM Characteristics)
// MAD_DIMM_CH1_0_0_0_MCHBAR (Channel 1 DIMM Characteristics)
//
// Rocket Lake: 636761-004 Sections 3.2.39-3.2.40 pages 128-129
// Tiger Lake U: 631122-003 Sections 3.2.41-3.2.42 pages 141-143
// Tiger Lake H: 643524-003 Sections 3.2.9-3.2.10 pages 117-119
// Ice Lake: 341078-004 Sections 3.2.38-3.2.39 pages 131-132
// Comet Lake: 615212-003 Sections 8.36-8.37 pages 194-196
class MemoryAddressDecoderDimmParametersCometLake
    : public hwreg::RegisterBase<MemoryAddressDecoderDimmParametersCometLake, uint32_t> {
 public:
  enum class DdrChipWidthValue {
    kX8 = 0b00,
    kX16 = 0b01,
    kX32 = 0b10,
    kReserved = 0b11,
  };

  DEF_RSVDZ_BIT(31);

  // If false, DDR5 capacity exceeds 8GB.
  //
  // This bit must be false for non-DDR5 configurations.
  //
  // This bit is only defined on Tiger Lake H, and is reserved MBZ (must be
  // zero) on all other platforms. This results in correct read semantics, as
  // the feature appears disabled on these platforms.
  DEF_BIT(30, ddr5_capacity_is_8gb);

  // Selects how zone address bits feed into BG and CAS bits.
  //
  // If this bit is true, channel address bit 6 becomes BG (bank group) bit 0,
  // and channel address bit 11 becomes CAS (column address) bit 7.
  DEF_BIT(29, swap_channel_address_bits_6_11);

  // If true, DIMM S is built from 8 GB DRAM modules.
  //
  // This bit must be false on non-DDR3 configurations.
  //
  // This bit is only defined on Comet Lake. It is reserved MBZ (must be zero)
  // on later memory controllers. This results in correct read semantics, as the
  // feature appears disabled on these platforms.
  DEF_BIT(28, dimm_s_built_from_8gb_modules);

  // The number of ranks in DIMM S. 0 = 1 rank, ... 3 = 4 ranks.
  //
  // Values above 1 (2 ranks) are not valid for for DIMM S.
  DEF_FIELD(27, 26, dimm_s_rank_count_minus_1);

  // The DDR chip width for DIMM S.
  DEF_ENUM_FIELD(DdrChipWidthValue, 25, 24, dimm_s_ddr_chip_width_select);

  DEF_RSVDZ_BIT(23);

  // Size of DIMM S in multiples of 0.5 GB (512 MB).
  DEF_FIELD(22, 16, dimm_s_size_512mb);

  DEF_RSVDZ_FIELD(15, 12);

  // If true, DIMM L is built from 8 GB DRAM modules.
  //
  // This bit must be false on non-DDR3 configurations.
  //
  // This bit is only defined on Comet Lake. It is reserved MBZ (must be zero)
  // on later memory controllers. This results in correct read semantics, as the
  // feature appears disabled on these platforms.
  DEF_BIT(11, dimm_l_built_from_8gb_modules);

  // The number of ranks in DIMM L. 0 = 1 rank, 1 = 2 ranks ... 3 = 4 ranks.
  //
  // Values above 1 (2 ranks) are only valid in ERM (Enhanced Rank Mode).
  DEF_FIELD(10, 9, dimm_l_rank_count_minus_1);

  DEF_ENUM_FIELD(DdrChipWidthValue, 8, 7, dimm_l_ddr_chip_width_select);

  // Size of DIMM L in multiples of 0.5 GB (512 MB).
  DEF_FIELD(6, 0, dimm_l_size_512mb);

  // The width of the DDR chips in DIMM S.
  int dimm_s_ddr_chip_width() const {
    return DdrChipWidthValueToInteger(dimm_s_ddr_chip_width_select());
  }

  // The width of the DDR chips in DIMM L.
  int dimm_l_ddr_chip_width() const {
    return DdrChipWidthValueToInteger(dimm_l_ddr_chip_width_select());
  }

  // The number of ranks in DIMM S.
  int dimm_s_rank_count() const {
    // The cast and the addition will not overflow (causing UB) because the
    // underlying field is a 2-bit integer.
    return static_cast<int>(dimm_s_rank_count_minus_1()) + 1;
  }

  // The number of ranks in DIMM L.
  int dimm_l_rank_count() const {
    // The cast and the addition will not overflow (causing UB) because the
    // underlying field is a 2-bit integer.
    return static_cast<int>(dimm_l_rank_count_minus_1()) + 1;
  }

  // The size of DIMM S, in MB.
  int dimm_s_size_mb() const {
    // The cast and the multiplication will not overflow (causing UB) because
    // the underlying field is a 7-bit integer.
    return static_cast<int>(dimm_s_size_512mb()) * 512;
  }

  // The size of DIMM L, in MB.
  int dimm_l_size_mb() const {
    // The cast and the multiplication will not overflow (causing UB) because
    // the underlying field is a 7-bit integer.
    return static_cast<int>(dimm_l_size_512mb()) * 512;
  }

  static auto GetForControllerAndChannel(int memory_controller_index, int channel_index) {
    ZX_ASSERT(memory_controller_index >= 0);
    ZX_ASSERT(memory_controller_index <= 1);
    ZX_ASSERT(channel_index >= 0);
    ZX_ASSERT(channel_index <= 1);
    return hwreg::RegisterAddr<MemoryAddressDecoderDimmParametersCometLake>(
        kMemoryControllerBarMirrorOffset + 0x500c + 0x10000 * memory_controller_index +
        4 * channel_index);
  }

  // For Tiger Lake H SKUs.
  //
  // `memory_controller_index` and `channel_index` are 0-based.
  static auto GetForTigerLakeHControllerAndChannel(int memory_controller_index, int channel_index) {
    ZX_ASSERT(memory_controller_index >= 0);
    ZX_ASSERT(memory_controller_index <= 1);
    ZX_ASSERT(channel_index >= 0);
    ZX_ASSERT(channel_index <= 1);
    return hwreg::RegisterAddr<MemoryChannelTimingsTigerLake>(
        kMemoryControllerBarMirrorOffset + 0xd80c + 0x10000 * memory_controller_index +
        4 * channel_index);
  }

 private:
  static int DdrChipWidthValueToInteger(DdrChipWidthValue value) {
    return 8 << static_cast<int>(value);
  }
};

// MAD_DIMM_CH0_0_0_0_MCHBAR (Channel 0 DIMM Characteristics)
// MAD_DIMM_CH1_0_0_0_MCHBAR (Channel 1 DIMM Characteristics)
//
// Coffee Lake: 337345-003 Sections 7.40-7.41 pages 196-199
// Whiskey Lake: 338024-001 Sections 7.11-7.12 pages 154-156
// Amber Lake: 334662-005 Sections 6.34-6.35 pages 157-160
// Kaby Lake S: 335196-002 Sections 7.34-7.35 pages 189-192
// Kaby Lake H: 335191-003 Sections 7.40-7.41 pages 198-201
// Skylake U: 332991-003 Sections 7.34-7.35 pages 190-193
// Skylake S: 332688-003 Sections 7.34-7.35 pages 172-174
// Skylake H: 332987-003 Sections 7.34-7.35 pages 190-193
class MemoryAddressDecoderDimmParametersSkylake
    : public hwreg::RegisterBase<MemoryAddressDecoderDimmParametersSkylake, uint32_t> {
 public:
  enum class DdrChipWidthValue {
    kX8 = 0b00,
    kX16 = 0b01,
    kX32 = 0b10,
    kReserved = 0b11,
  };

  DEF_RSVDZ_FIELD(31, 28);

  // If true, DIMM S is built from 8 GB DRAM modules.
  //
  // This bit is expected to be false on non-DDR3 configurations.
  DEF_BIT(27, dimm_s_built_from_8gb_modules);

  // The number of ranks in DIMM S. 0 = 1 rank, 1 = 2 ranks.
  DEF_BIT(26, dimm_s_rank_count_minus_1);

  // The width of the DDR chips in DIMM S.
  DEF_ENUM_FIELD(DdrChipWidthValue, 25, 24, dimm_s_ddr_chip_width_select);

  DEF_RSVDZ_FIELD(23, 22);

  // Size of DIMM S in multiples of 1GB.
  DEF_FIELD(21, 16, dimm_s_size_1gb);

  DEF_RSVDZ_FIELD(15, 12);

  // If true, DIMM L is built from 8 GB DRAM modules.
  //
  // This bit is expected to be false on non-DDR3 configurations.
  DEF_BIT(11, dimm_l_built_from_8gb_modules);

  // The number of ranks in DIMM L. 0 = 1 rank, 1 = 2 ranks.
  DEF_BIT(10, dimm_l_rank_count_minus_1);

  // The width of the DDR chips in DIMM L.
  DEF_ENUM_FIELD(DdrChipWidthValue, 9, 8, dimm_l_ddr_chip_width_select);

  DEF_RSVDZ_FIELD(7, 6);

  // Size of DIMM L in multiples of 1GB.
  DEF_FIELD(5, 0, dimm_l_size_1gb);

  // The width of the DDR chips in DIMM S.
  int dimm_s_ddr_chip_width() const {
    return DdrChipWidthValueToInteger(dimm_s_ddr_chip_width_select());
  }

  // The width of the DDR chips in DIMM L.
  int dimm_l_ddr_chip_width() const {
    return DdrChipWidthValueToInteger(dimm_l_ddr_chip_width_select());
  }

  // The number of ranks in DIMM S.
  int dimm_s_rank_count() const {
    // The cast and the addition will not overflow (causing UB) because the
    // underlying field is a 1-bit integer.
    return static_cast<int>(dimm_s_rank_count_minus_1()) + 1;
  }

  // The number of ranks in DIMM L.
  int dimm_l_rank_count() const {
    // The cast and the addition will not overflow (causing UB) because the
    // underlying field is a 1-bit integer.
    return static_cast<int>(dimm_l_rank_count_minus_1()) + 1;
  }

  // The size of DIMM S, in MB.
  int dimm_s_size_mb() const {
    // The cast and the multiplication will not overflow (causing UB) because
    // the underlying field is a 6-bit integer.
    return static_cast<int>(dimm_s_size_1gb()) * 1024;
  }

  // The size of DIMM L, in MB.
  int dimm_l_size_mb() const {
    // The cast and the multiplication will not overflow (causing UB) because
    // the underlying field is a 6-bit integer.
    return static_cast<int>(dimm_l_size_1gb()) * 1024;
  }

  static auto GetForChannel(int channel_index) {
    ZX_ASSERT(channel_index >= 0);
    ZX_ASSERT(channel_index <= 1);
    return hwreg::RegisterAddr<MemoryAddressDecoderDimmParametersSkylake>(
        kMemoryControllerBarMirrorOffset + 0x500c + 4 * channel_index);
  }

 private:
  static int DdrChipWidthValueToInteger(DdrChipWidthValue value) {
    return 8 << static_cast<int>(value);
  }
};

// MC_BIOS_REQ_0_0_0_MCHBAR_PCU (Memory Controller BIOS Request)
// MC_BIOS_DATA_0_0_0_MCHBAR_PCU (Memory Controller BIOS Data)
//
// Raptor Lake: 743846-001 Sections 3.3.42-3.3.43 pages 202-204
// Alder Lake S: 655259-003 Section 3.3.41-3.3.42 pages 184-187
// Alder Lake H: 710723-003 Section 3.3.41-3.3.42 pages 213-216
// Rocket Lake: 636761-004 Sections 3.3.45-3.3.46 pages 172-174
// Tiger Lake U: 631122-003 Sections 3.3.44-3.3.45 pages 197-199
// Tiger Lake H: 643524-003 Sections 3.3.45-3.3.46 pages 190-192
// Ice Lake: 341078-004 Section 3.3.44-3.3.45 pages 177-179
class MemoryControllerBiosDataIceLake
    : public hwreg::RegisterBase<MemoryControllerBiosDataIceLake, uint32_t> {
 public:
  // Documented values for `memory_controller_frequency_base_select`.
  enum class ControllerFrequencyBaseValue {
    k133Mhz = 0b000,
    k100Mhz = 0b001,
  };

  // If true, the PCU (power controller) has not yet applied this configuration.
  //
  // The system firmware sets this bit to true when it writes a new value to
  // this register. The PCU firmware clears this bit after it applies the
  // requested configuration changes.
  //
  // This bit is only meaningful for the MC_BIOS_REQ_0_0_0_MCHBAR_PCU (Memory
  // Controller BIOS Request) register. It must be zero in the
  // MC_BIOS_DATA_0_0_0_MCHBAR_PCU (Memory Controller BIOS Data) register.
  DEF_BIT(31, request_pending);

  // Sets IccMax (the maximum current) on the VDDQ_TX (DDR data transmit) rail.
  //
  // The value is a multiplier with the base 250 mA.
  //
  // This field is reserved MBZ (must be zero) on Ice Lake and Rocket Lake.
  DEF_FIELD(30, 27, data_transmit_rail_max_current_multiplier);

  // Sets the voltage on the VDDQ_TX (DDR data transmit) rail.
  //
  // The value is a multiplier with the base 5mV.
  //
  // This field is reserved MBZ (must be zero) on Ice Lake and Rocket Lake.
  DEF_FIELD(26, 17, data_transmit_rail_voltage_multiplier);

  // Sets the DDR PHY bus clock relatively to the memory controller Qclk.
  //
  // This field is reserved MBZ (must be zero) on Alder Lake and Tiger Lake.
  // Those platforms use the `ddr_phy_bus_clock_multiplier_shift_tiger_lake`
  // field with the same semantics.
  DEF_BIT(16, ddr_phy_bus_clock_multiplier_shift_ice_lake);

  DEF_RSVDZ_FIELD(15, 14);

  // 2-bit equivalent of `ddr_phy_bus_clock_multiplier_shift_ice_lake`.
  //
  // 0 = 1X multiplier, so the DDR bus matches Qclk. 1 = 2X multiplier, so the
  // DDR bus operates at 2x Qclk. 2 = 4x multiplier.
  //
  // This field is reserved MBZ (must be zero) on Ice Lake and Rocket Lake.
  DEF_FIELD(13, 12, ddr_phy_bus_clock_multiplier_shift_tiger_lake);

  // The base for the memory controller Qclk (quad clock) frequency.
  DEF_ENUM_FIELD(ControllerFrequencyBaseValue, 11, 8, controller_frequency_base_select);

  // The multiplier for the memory controller Qclk (quad clock) frequency.
  //
  // After the MRC (Memory Reference initialization Code) runs, the multiplier
  // should be greater than or equal to 3. A multiplier of 0 indicates that the
  // memory controller PLL will be shut down. Multipliers 1 and 2 are reserved.
  DEF_FIELD(7, 0, controller_frequency_multiplier);

  // The maximum Icc (current) on the VDD_TX (DDR data transmit) rail.
  //
  // The return value is expressed in mA (milliamperes). A value of zero means
  // that the field is not populated.
  int32_t data_transmit_rail_max_current_milliamps() const {
    static constexpr int32_t kCurrentBaseMilliamps = 250;

    // The cast and the multiplication will not overflow (causing UB) because
    // the underlying field is a 4-bit integer. The multiplication result fits
    // in 12 bits, because the base fits in 8 bits.
    return static_cast<int32_t>(data_transmit_rail_max_current_multiplier()) *
           kCurrentBaseMilliamps;
  }

  // The voltage on the VDD_TX (DDR data transmit) rail.
  //
  // The return value is expressed in mV (millivolts). A value of zero means
  // that the field is not populated.
  int32_t data_transmit_rail_voltage_millivolts() const {
    static constexpr int32_t kVoltageBaseMillivolts = 5;

    // The cast and the multiplication will not overflow (causing UB) because
    // the underlying field is a 10-bit integer. The multiplication result fits
    // in 13 bits, because the base fits in 3 bits.
    return static_cast<int32_t>(data_transmit_rail_voltage_multiplier()) * kVoltageBaseMillivolts;
  }

  // Sets the DDR PHY bus clock relatively to the memory controller Qclk.
  //
  // This field is a multiplier is relative to the memory controller Qclk (quad
  // clock) frequency. The field value is represented in log2, so the multiplier
  // is 1 << field_value.
  int ddr_phy_bus_clock_multiplier_shift() const {
    // The casts will not underflow (causing UB) because the underlying fields
    // are 1 and 2-bit integers.
    return std::max<int32_t>(static_cast<int32_t>(ddr_phy_bus_clock_multiplier_shift_ice_lake()),
                             static_cast<int32_t>(ddr_phy_bus_clock_multiplier_shift_tiger_lake()));
  }

  // The DDR PHY bus frequency, in Hz.
  //
  // Returns 0 if the memory controller PLL is disabled, or if the register has
  // an invalid configuration.
  int64_t ddr_phy_bus_frequency_hz() const {
    // The cast and the multiplication will not overflow (causing UB) because
    // the base is a 31-bit integer, and the multiplier is an 8-bit field. So,
    // the multiplication result will fit in 39 bits.
    const int64_t quad_clock_frequency_hz_x3 =
        int64_t{controller_frequency_base_hz_x3()} *
        static_cast<int32_t>(controller_frequency_multiplier());

    // The shift will not overflow (causing UB) because
    // `quad_clock_frequency_hz_x3` is a 39-bit integer, and the shift value is
    // a 1-bit field.
    const int64_t ddr_frequency_hz_x3 = quad_clock_frequency_hz_x3
                                        << ddr_phy_bus_clock_multiplier_shift();

    return (ddr_frequency_hz_x3 + 1) / 3;
  }

  // The memory controller's Qclk (quad-clock) frequency, in Hz.
  //
  // Returns 0 if the memory controller PLL is disabled, or if the register has
  // an invalid configuration.
  int64_t controller_quad_clock_frequency_hz() const {
    // The cast and the multiplication will not overflow (causing UB) because
    // the base is a 31-bit integer, and the multiplier is an 8-bit field. So,
    // the multiplication result will fit in 39 bits.
    const int64_t frequency_hz_x3 = int64_t{controller_frequency_base_hz_x3()} *
                                    static_cast<int32_t>(controller_frequency_multiplier());

    // The addition will not overflow (causing UB) because `frequency_hz_x3`
    // fits in 39 bits, so the result fits in 40 bits.
    return (frequency_hz_x3 + 1) / 3;
  }

  // The memory controller's base frequency in Hz, multiplied by 3.
  //
  // Returns zero if the field is set to an undocumented value.
  //
  // The unusual return convention follows the datasheet recommendation for
  // representing the memory controller's base frequency.
  int32_t controller_frequency_base_hz_x3() const {
    switch (controller_frequency_base_select()) {
      case ControllerFrequencyBaseValue::k133Mhz:
        return 400'000'000;
      case ControllerFrequencyBaseValue::k100Mhz:
        return 300'000'000;
    }
    return 0;
  }

  static auto Get() {
    return hwreg::RegisterAddr<MemoryControllerBiosDataIceLake>(kMemoryControllerBarMirrorOffset +
                                                                0x5e04);
  }
};

// MC_BIOS_DATA_0_0_0_MCHBAR_PCU (Memory Controller BIOS Data)
//
// The Kaby Lake and Skylake datasheets only document the
// PCU_CR_MC_BIOS_REQ_0_0_0_MCHBAR_PCU register, which has the same semantics as
// the MC_BIOS_REQ_0_0_0_MCHBAR_PCU register in the Ice Lake and Tiger Lake
// datasheets. The MC_BIOS_REQ_0_0_0_MCHBAR_PCU (Memory Controller BIOS Request)
// register carries the latest request from the software to the memory
// controller, whereas this register (Memory Controller BIOS Data) represents
// the last request issued by the MRC (Memory Reference Code).
//
// Comet Lake: 615212-003 Section 9.40 pages 238-239
// Coffee Lake: 337345-003 Section 7.91 pages 237-238
// Whiskey Lake: 338024-001 Section 7.60 pages 192-193
// Amber Lake: 334662-005 Sections 6.83 pages 197-198
// Kaby Lake S: 335196-002 Section 7.83 pages 229-230
// Kaby Lake H: 335191-003 Section 7.91 pages 239-240
// Skylake U: 332991-003 Section 7.83 pages 230-231
// Skylake S: 332688-003 Section 7.83 pages 206-207
// Skylake H: 332987-003 Section 7.83 pages 230-231
class MemoryControllerBiosDataSkylake
    : public hwreg::RegisterBase<MemoryControllerBiosDataSkylake, uint32_t> {
 public:
  // The multipliers for the memory controller clock frequencies.
  //
  // After the MRC (Memory Reference initialization Code) runs, the multiplier
  // should be greater than or equal to 3. A multiplier of 0 indicates that the
  // memory controller PLL will be shut down. Multipliers 1 and 2 are reserved.
  //
  // The bases are 400/3 MHz for the memory controller's Dclk (double-clock) and
  // 800/3 MHz for the memory controller's Qclk (quad-clock).
  DEF_FIELD(3, 0, controller_frequency_multiplier);

  // The memory controller's Qclk (quad-clock) frequency, in Hz.
  //
  // Returns 0 if the memory controller PLL is disabled.
  int64_t controller_quad_clock_frequency_hz() const {
    static constexpr int64_t kQuadClockBaseMultiplierHz = 800'000'000;
    static constexpr int kQuadClockBaseDivider = 3;

    return (static_cast<int>(controller_frequency_multiplier()) * kQuadClockBaseMultiplierHz +
            (kQuadClockBaseDivider / 2)) /
           kQuadClockBaseDivider;
  }

  static auto Get() {
    return hwreg::RegisterAddr<MemoryControllerBiosDataSkylake>(kMemoryControllerBarMirrorOffset +
                                                                0x5e04);
  }
};

}  // namespace registers

#endif  // SRC_GRAPHICS_DISPLAY_DRIVERS_INTEL_DISPLAY_REGISTERS_MEMORY_CONTROLLER_H_
