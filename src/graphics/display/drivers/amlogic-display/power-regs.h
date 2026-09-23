// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_DRIVERS_AMLOGIC_DISPLAY_POWER_REGS_H_
#define SRC_GRAPHICS_DISPLAY_DRIVERS_AMLOGIC_DISPLAY_POWER_REGS_H_

#include <cstdint>

#include <hwreg/bitfields.h>

namespace amlogic_display {

// The AO (Always On) power domain is described in the A311D datasheet Section
// 8.2.1 "Top Level Power Domains", which gives an overview of all power
// domains.  The EE (Everything Else) abbreviation is decoded in the
// AO_RTI_PWR_CNTL_REG0 register documentation, in the A311D datasheet Section
// 8.2.5 "Power/Isolation/Memory Power Down Register Summary".
//
// The S912 datasheet Section 17 "Power Domain" presents similar information in
// more depth. The S912 also has AO / EE / CPU power domains, and the main
// difference from our supported chips is the lack of an A72 CPU complex. In
// particular, it's helpful to compare the following figures:
// * S912: Figure III.17.1 "Power Domain" vs A311D: Figure 8-2 "Power Domain"
// * S912: Table III.17.1 "Power on Sequence of Different Power Domains" vs
//   A311D: Table 8-2 with the same title
//
// The S905Y4 datasheet Section 7.2.1 "Power Domain" > "Overview" lists commonly
// used field suffixes and value encodings in power-related registers. These
// support our interpretations of the field names in the register-level
// documentation.

// The A311D datasheet Section 8.2.5 "Power/Isolation/Memory Power Down Register
// Summary" states that the registers are based at 0xff80'0000. Section 8.1
// "System" > "Memory Map" lists this address under the RTI entry.

// All datasheets include a VPU power sequence, in addition to register-level
// documentation. The VPU power sequences reference the register bit fields that
// make up the sequence, using register names and/or MMIO addresses. In some
// cases, the sequences also document the fields' encodings and the order in
// which fields should be set.
//
// The VPU power sequence is documented in the following places.
// * A311D datasheet Section 8.2.3 "EE Top Level Power Modes", Table 8-6 "Power
//   Sequence of VPU", page 88
// * S905D3 datasheet Section 6.2.3.2 "EE Top Level Power Modes" > "VPU",
//   Table 6-3 "Power & Global Clock Control Summary", page 75
// * S905D2 datasheet Section 6.2.3 "EE Top Level Power Modes", Table 6-4
//   "Power Sequence of EE Domain", page 84

// AO_RTI_GEN_PWR_SLEEP0
//
// This register is shared between multiple hardware modules, driven by
// different drivers. It must be updated via read-modify-write operations
// TODO(https://fxbug.dev/42121690): Concurrent access from different drivers is still
// racy.
//
// A311D datasheet Section 8.2.5 "Power/Isolation/Memory Power Down Register
//     Summary" page 91; VPU power sequence
// S905D3 datasheet Section 6.2.4 "Power Domain" > "Register Description"
//     pages 80-81; VPU power sequence
// S905D2 datasheet Section 6.2.5 "Power/Isolation/Memory Power Down Register
//     Summary" page 86
class AlwaysOnGeneralPowerSleep : public hwreg::RegisterBase<AlwaysOnGeneralPowerSleep, uint32_t> {
 public:
  // Register space: RTI
  static hwreg::RegisterAddr<AlwaysOnGeneralPowerSleep> Get() { return {0x3a * sizeof(uint32_t)}; }

  // S905D3 only. The bits are documented as "unused" on A311D and S905D2.
  DEF_BIT(19, ge2d_powered_off_s905d3);
  DEF_BIT(18, pci_comb_powered_off_s905d3);
  DEF_BIT(17, usb_comb_powered_off_s905d3);
  DEF_BIT(16, nna_comb_powered_off_s905d3);

  // VPU must be fully powered on before disabling isolation.
  //
  // This bit is documented as "unused" in the register-level documentation in
  // the A311D and S905D3 datasheets. However, it is documented indirectly in
  // the A311D datasheet's VPU power sequence.
  //
  // This bit is documented as "VPU D1 powered off" in the S905D2 datasheet's
  // register-level reference, and is not included in the VPU power sequence.
  // The S905D3 datasheet has the same functionality in the
  // `AlwaysOnGeneralPowerIsolation` register.
  DEF_BIT(9, vpu_hdmi_isolation_enabled_s905d2_a311d);

  // Documented directly in the S905D3 datasheet. Documented indirectly in the
  // A311D datasheet's VPU power sequence.
  //
  // This bit is documented as "VPU powered off" in the S905D2 datasheet's
  // register-level reference, and is not included in the VPU power sequence.
  DEF_BIT(8, vpu_hdmi_powered_off);

  // Bits 0-7 are different between S905D3 and A311D. This driver does not use
  // the bits, so we omit them, instead of creating two register definitions.
};

// AO_RTI_GEN_PWR_ISO0 with S905D3 fields
//
// This register is shared between multiple hardware modules, driven by
// different drivers. It must be updated via read-modify-write operations
// TODO(https://fxbug.dev/42121690): Concurrent access from different drivers is still
// racy.
//
// This register also exists on S905D2 and A311D, but its bits have entirely
// different semantics there. This driver does not use the S905D2 and A311D
// bits, so we only have the S905D3 definition.
//
// A311D datasheet Section 8.2.5 "Power/Isolation/Memory Power Down Register
//     Summary" page 91
// S905D3 datasheet Section 6.2.4 "Power Domain" > "Register Description",
//     Table 6-18 page 81; VPU power sequence
// S905D2 datasheet Section 6.2.5 "Power/Isolation/Memory Power Down Register
//     Summary" page 86
class AlwaysOnGeneralPowerIsolationS905D3
    : public hwreg::RegisterBase<AlwaysOnGeneralPowerIsolationS905D3, uint32_t> {
 public:
  // Register space: RTI
  static hwreg::RegisterAddr<AlwaysOnGeneralPowerIsolationS905D3> Get() {
    return {0x3b * sizeof(uint32_t)};
  }

  // The bit-level encoding is documented in the S905D3 datasheet.

  // S905D3 only. The bits are documented as "unused" on A311D and S905D2.
  DEF_BIT(19, ge2d_isolation_enabled);
  DEF_BIT(18, pci_comb_isolation_enabled);
  DEF_BIT(17, usb_comb_isolation_enabled);
  DEF_BIT(16, nna_comb_isolation_enabled);

  // VPU must be fully powered on before disabling isolation.
  //
  // This bit is not documented in the S905D2 register-level documentation. The
  // A311D register-level documentation assigns different functionality to this
  // bit (video decoder 2 input isolation).
  //
  // The VPU power sequence in the S905D3 datasheet uses the wrong register
  // (AO_RTI_GEN_PWR_SLEEP0) instead of this register. This error is repeated in
  // Sections 6.2.3.2 - 6.2.3.7, which have the same mismatch against the
  // register-level documentation.
  DEF_BIT(8, vpu_hdmi_output_isolation_enabled);
};

// AO_RTI_GEN_PWR_ACK0
//
// This register is read-only.
//
// A311D datasheet Section 8.2.5 "Power/Isolation/Memory Power Down Register
//     Summary" page 92; VPU power sequence
// S905D3 datasheet Section 6.2.4 "Power Domain" > "Register Description"
//     Table 6-19 pages 81-82; VPU power sequence
// S905D2 datasheet Section 6.2.5 "Power/Isolation/Memory Power Down Register
//     Summary" pages 86-87
class AlwaysOnGeneralPowerStatus
    : public hwreg::RegisterBase<AlwaysOnGeneralPowerStatus, uint32_t> {
 public:
  // Register space: RTI
  static hwreg::RegisterAddr<AlwaysOnGeneralPowerSleep> Get() { return {0x3c * sizeof(uint32_t)}; }

  // The bit-level encoding is documented in the S905D3 datasheet, Section
  // 6.2.3.2 "EE Top Level Power Modes" > "VPU".

  // S905D3 only. The bits are documented as "reserved" on A311D and S905D2.
  DEF_BIT(19, ge2d_powered_off_s905d3);
  DEF_BIT(18, pci_comb_powered_off_s905d3);
  DEF_BIT(17, usb_comb_powered_off_s905d3);
  DEF_BIT(16, nna_comb_powered_off_s905d3);

  // A311D and S905D2. This bit is documented as "reserved" on S905D3.
  DEF_BIT(9, vpu_memory_powered_off_s905d2_a311d);

  // Covers both VPU and HDMI on S905D3.
  DEF_BIT(8, hdmi_powered_off);

  // Bits 0-7 are different between S905D3 and A311D / S905D2. This driver does
  // not use the bits, so we omit them, instead of creating two register
  // definitions.
};

// The A311D datasheet Section 8.7.5 "Clock" > "Register Description" states
// that the registers are based at 0xff63'c000, in the HIU region. This matches
// Table 8-1 "A311D Memory Map" in Section 8.1 "System" > "Memory Map".

// A311D datasheet Section 8.7.5 "Clock" > "Register Description", register
//     HHI_VPU_MEM_PD_REG0, field "Deinterlacer - di_post", page 144
// S905D2 datasheet Section 6.6.6 "Clock" > "Register Description", register
//     HHI_VPU_MEM_PD_REG0, field "Deinterlacer - di_post" page 130
// S905Y4 datasheet Section 7.2.1 "Overview", list item "MEMPD", page 52
enum class MemoryPowerDomainMode {
  kPoweredOff = 0b11,
  kPoweredOn = 0b00,
};

// HHI_MEM_PD_REG0
//
// A311D datasheet Section 8.7.5 "Clock" > "Register Description" page 144;
//    VPU power sequence
// S905D3 datasheet Section 6.2.4 "Power Domain" > "Register Description"
//    page 77; VPU power sequence
// S905D2 datasheet Section 6.6.6 "Clock" > "Register Description" page 130;
//    VPU power sequence, "HDMI Memory PD" entry
class MemoryPower0 : public hwreg::RegisterBase<MemoryPower0, uint32_t> {
 public:
  // Register space: HIU
  static hwreg::RegisterAddr<MemoryPower0> Get() { return {0x40 * sizeof(uint32_t)}; }

  // The S905D3 datasheet documents the GE2D power sequence, which includes
  // flipping bits 25-18. The other datasheets mention the GE2D unit, but don't
  // document any power sequence for it.

  // Only on A311D and T931. The bits are reserved on other SoCs.
  DEF_ENUM_FIELD(MemoryPowerDomainMode, 21, 20, axi_sram_memory_power);
  DEF_ENUM_FIELD(MemoryPowerDomainMode, 19, 18, apical_gdc_memory_power);

  // Only on S905D3. The bits are reserved on other SoCs.
  DEF_ENUM_FIELD(MemoryPowerDomainMode, 17, 16, ddr_memory_power);

  // All the datasheets document HDMI memory power as a single 8-bit field.
  // However, AMLogic-supplied bringup code flips each bit separately, and
  // pauses 5us between flips.
  DEF_BIT(15, hdmi_memory7_powered_off);
  DEF_BIT(14, hdmi_memory6_powered_off);
  DEF_BIT(13, hdmi_memory5_powered_off);
  DEF_BIT(12, hdmi_memory4_powered_off);
  DEF_BIT(11, hdmi_memory3_powered_off);
  DEF_BIT(10, hdmi_memory2_powered_off);
  DEF_BIT(9, hdmi_memory1_powered_off);
  DEF_BIT(8, hdmi_memory0_powered_off);

  // Only on S905D2. The bits are reserved on other SOCs.
  DEF_ENUM_FIELD(MemoryPowerDomainMode, 7, 6, audio_memory2_power);

  DEF_ENUM_FIELD(MemoryPowerDomainMode, 5, 4, audio_memory1_power);
  DEF_ENUM_FIELD(MemoryPowerDomainMode, 3, 2, ethernet_memory_power);
};

// HHI_VPU_MEM_PD_REG0
//
// A311D datasheet Section 8.7.5 "Clock" > "Register Description" page 144;
//    VPU power sequence
// S905D3 datasheet Section 6.2.4 "Power Domain" > "Register Description"
//    page 78; VPU power sequence
// S905D2 datasheet Section 6.6.6 "Clock" > "Register Description" page 130;
//    VPU power sequence, "VPU Memory PD" entry
class VpuMemoryPower0 : public hwreg::RegisterBase<VpuMemoryPower0, uint32_t> {
 public:
  // Register space: HIU
  static hwreg::RegisterAddr<VpuMemoryPower0> Get() { return {0x41 * sizeof(uint32_t)}; }

  DEF_ENUM_FIELD(MemoryPowerDomainMode, 31, 30, sharpener_power);
  DEF_ENUM_FIELD(MemoryPowerDomainMode, 29, 28, deinterlacer_post_power);

  // On S905D3, paired with `deinterlacer_pre2_power` in `VpuMemoryPower2`.
  DEF_ENUM_FIELD(MemoryPowerDomainMode, 27, 26, deinterlacer_pre_power);

  // The S905D2 datasheet leaves these bits undocumented. However, the VPU power
  // sequence references the entire register without a "Bits[]" subset,
  // which seems to imply that we flip all the register's bits.
  DEF_ENUM_FIELD(MemoryPowerDomainMode, 25, 24, deinterlacer_scaler_memory_power);

  DEF_ENUM_FIELD(MemoryPowerDomainMode, 23, 22, afbc_decoder1_power);
  DEF_ENUM_FIELD(MemoryPowerDomainMode, 21, 20, srscl_super_scaler_power);
  DEF_ENUM_FIELD(MemoryPowerDomainMode, 19, 18, vdin1_memory_power);
  DEF_ENUM_FIELD(MemoryPowerDomainMode, 17, 16, vdin0_memory_power);
  DEF_ENUM_FIELD(MemoryPowerDomainMode, 15, 14, osd_scaler_memory_power);
  DEF_ENUM_FIELD(MemoryPowerDomainMode, 13, 12, scaler_memory_power);
  DEF_ENUM_FIELD(MemoryPowerDomainMode, 11, 10, vpp_output_fifo_power);
  DEF_ENUM_FIELD(MemoryPowerDomainMode, 9, 8, color_management_power);
  DEF_ENUM_FIELD(MemoryPowerDomainMode, 7, 6, vd2_memory_power);
  DEF_ENUM_FIELD(MemoryPowerDomainMode, 5, 4, vd1_memory_power);
  DEF_ENUM_FIELD(MemoryPowerDomainMode, 3, 2, osd2_memory_power);
  DEF_ENUM_FIELD(MemoryPowerDomainMode, 1, 0, osd1_memory_power);
};

// HHI_VPU_MEM_PD_REG1
//
// A311D datasheet Section 8.7.5 "Clock" > "Register Description" page 145; VPU
//     power sequence
// S905D3 datasheet Section 6.2.4 "Power Domain" > "Register Description"
//     page 78; VPU power sequence
// S905D2 datasheet Section 6.6.6 "Clock" > "Register Description"
//     pages 130-131; VPU power sequence, "VPU Memory PD" entry
class VpuMemoryPower1 : public hwreg::RegisterBase<VpuMemoryPower1, uint32_t> {
 public:
  // Register space: HIU
  static hwreg::RegisterAddr<VpuMemoryPower1> Get() { return {0x42 * sizeof(uint32_t)}; }

  // On A311D and S905D2, xvycc LUT and ATV demodulator.
  DEF_ENUM_FIELD(MemoryPowerDomainMode, 31, 30, vd2_osd_scaler_power);

  // On A311D and S905D2, also CVD2 TV decoder.
  //
  // The datasheets use the "ldim_stts" abbreviation for this field name. The
  // expansion is in the S905Y4 datasheet register-level documentation for
  // VDIN0_LDIM_STTS_HIST_REGION_IDX and VDIN1_LDIM_STTS_HIST_REGION_IDX.
  DEF_ENUM_FIELD(MemoryPowerDomainMode, 29, 28, local_dimming_statistics_power);

  // The A311D and S905D2 datasheets leaves these bits undocumented. However,
  // the VPU power sequences in both datasheets involve flipping all the
  // register's bits.
  //
  // The datasheets use the "lc_stts" abbreviation for this field name. The
  // expansion is in the S905Y4 datasheet register-level documentation for
  // SRSHARP1_LC_TOP_CTRL and VDIN1_LDIM_STTS_HIST_REGION_IDX.
  DEF_ENUM_FIELD(MemoryPowerDomainMode, 27, 26, local_contrast_enhancement_statistics_power);

  DEF_ENUM_FIELD(MemoryPowerDomainMode, 25, 24, enci_cvbs_power);
  DEF_ENUM_FIELD(MemoryPowerDomainMode, 23, 22, encl_panel_top_power);
  DEF_ENUM_FIELD(MemoryPowerDomainMode, 21, 20, encp_hdmi_power);

  // On A311D and S905D2, OSD AFBC decoder.
  DEF_ENUM_FIELD(MemoryPowerDomainMode, 19, 18, vd2_scaler_power);

  DEF_ENUM_FIELD(MemoryPowerDomainMode, 17, 16, afbc_decoder0_power);
  DEF_ENUM_FIELD(MemoryPowerDomainMode, 15, 14, vpu_arbiter_power);
  DEF_ENUM_FIELD(MemoryPowerDomainMode, 13, 12, dolby1b_power);
  DEF_ENUM_FIELD(MemoryPowerDomainMode, 11, 10, dolby1a_power);
  DEF_ENUM_FIELD(MemoryPowerDomainMode, 9, 8, dolby0_power);
  DEF_ENUM_FIELD(MemoryPowerDomainMode, 7, 6, dolby_core3_power);

  // The datasheets use the "vks" and "vkstone" abbreviations. The "ks"
  // expansion to keystone is documented in all datasheets, in the
  // register-level documentation for VKS_PRELPF_YCOEF0.
  DEF_ENUM_FIELD(MemoryPowerDomainMode, 5, 4, vertical_keystone_correction_power);

  // On S905D3, paired with `viu2_power` in `VpuMemoryPower2`.
  //
  // The S905D2 datasheet leaves these bits undocumented.
  DEF_ENUM_FIELD(MemoryPowerDomainMode, 3, 2, viu2_output_fifo_power);
  DEF_ENUM_FIELD(MemoryPowerDomainMode, 1, 0, viu2_osd1_power);
};

// HHI_VPU_MEM_PD_REG3
//
// Does not exist on A311D. The MMIO address is assigned to a different
// register (HHI_NANOQ_MEM_PD_REG0).
//
// S905D3 datasheet Section 6.2.4 "Power Domain" > "Register Description"
//     page 79; VPU power sequence
class VpuMemoryPower3 : public hwreg::RegisterBase<VpuMemoryPower3, uint32_t> {
 public:
  // Register space: HIU
  static hwreg::RegisterAddr<VpuMemoryPower3> Get() { return {0x43 * sizeof(uint32_t)}; }

  // All bits are marked reserved and default to 1. However, the S905D3 VPU
  // power sequence includes setting all bits to 0 (for power on) or 1 (for
  // power off). AMLogic-supplied bringup code flips each group of 2 bits at a
  // time, and pauses 5us between flips.
};

// HHI_VPU_MEM_PD_REG4
//
// Does not exist on A311D. The MMIO address is assigned to a different
// register (HHI_NANOQ_MEM_PD_REG1).
//
// S905D3 datasheet Section 6.2.4 "Power Domain" > "Register Description"
//     page 79; VPU power sequence
class VpuMemoryPower4 : public hwreg::RegisterBase<VpuMemoryPower4, uint32_t> {
 public:
  // Register space: HIU
  static hwreg::RegisterAddr<VpuMemoryPower4> Get() { return {0x44 * sizeof(uint32_t)}; }

  // Bits 31-6 are marked reserved. However, the S905D3 VPU power sequence
  // includes setting all the register's bits to 0 (for power on) or 1 (for
  // power off).  AMLogic-supplied bringup code flips each group of 2 bits at a
  // time, and pauses 5us between flips.

  DEF_ENUM_FIELD(MemoryPowerDomainMode, 5, 4, mali_afbc_encoder_power);

  // The datasheet documents bits 3-0 as one field. However, AMLogic-supplied
  // bringup code flips each 2-bit group separately, and pauses 5us between
  // flips.
  DEF_ENUM_FIELD(MemoryPowerDomainMode, 3, 2, axi_arbiter2_power);
  DEF_ENUM_FIELD(MemoryPowerDomainMode, 1, 0, axi_arbiter1_power);
};

// HHI_VPU_MEM_PD_REG2
//
// A311D datasheet Section 8.7.5 "Clock" > "Register Description" pages 146-147;
//     VPU power sequence
// S905D3 datasheet Section 6.2.4 "Power Domain" > "Register Description"
//     page 79; VPU power sequence
// S905D2 datasheet Section 6.6.6 "Clock" > "Register Description" page 132
class VpuMemoryPower2 : public hwreg::RegisterBase<VpuMemoryPower2, uint32_t> {
 public:
  // Register space: HIU
  static hwreg::RegisterAddr<VpuMemoryPower2> Get() { return {0x4d * sizeof(uint32_t)}; }

  // Not documented in any of the datasheets. These bits are documented and
  // flipped in AMLogic-supplied bringup code.
  DEF_ENUM_FIELD(MemoryPowerDomainMode, 31, 30, rdma_power);

  // Bits 29-26 are documented as "unused" on A311D and S905D2, and "reserved"
  // on S905D3. The VPU power sequences in the A311D and S905D3 datasheets flip
  // all the bits in this register. The VPU power sequence in the S905D2
  // datasheet does not mention this register.

  // Paired with `deinterlacer_pre_power` in `VpuMemoryPower`
  //
  // S905D3 only. The bits are documentd as "unused" on A311D and S905D2.
  DEF_ENUM_FIELD(MemoryPowerDomainMode, 25, 24, deinterlacer_pre2_power_s905d3);

  // Paired with `deinterlacer_pre_power` in `VpuMemoryPower`
  //
  // S905D3 only. The bits are documentd as "unused" on A311D and S905D2.
  DEF_ENUM_FIELD(MemoryPowerDomainMode, 23, 22, viu2_power_s905d3);

  // S905D3 only. The bits are documentd as "unused" on A311D and S905D2.
  DEF_ENUM_FIELD(MemoryPowerDomainMode, 21, 20, lut3d_power_s905d3);
  DEF_ENUM_FIELD(MemoryPowerDomainMode, 19, 18, ds_power_s905d3);

  // A311D and S905D3 only. The bits are documented as "unused" on S905D2.
  DEF_ENUM_FIELD(MemoryPowerDomainMode, 17, 16, vd2_output_fifo_power);
  DEF_ENUM_FIELD(MemoryPowerDomainMode, 15, 14, prime_dolby_ram_power);
  DEF_ENUM_FIELD(MemoryPowerDomainMode, 13, 12, osd_bld34_power);
  DEF_ENUM_FIELD(MemoryPowerDomainMode, 11, 10, vd1_scaler_power);
  DEF_ENUM_FIELD(MemoryPowerDomainMode, 9, 8, mali_afbc_decoder_power);
  DEF_ENUM_FIELD(MemoryPowerDomainMode, 7, 6, video_input_osd4_power);
  DEF_ENUM_FIELD(MemoryPowerDomainMode, 5, 4, video_input_osd3_power);

  // A311D only. The bits are documented as "unused" on S905D2 and "reserved" on
  // S905D3. The AMLogic-supplied bringup code avoids modifying these bits.
  DEF_ENUM_FIELD(MemoryPowerDomainMode, 3, 2, tcon_power_a311d);

  DEF_ENUM_FIELD(MemoryPowerDomainMode, 1, 0, vpp_watermark_power);
};

}  // namespace amlogic_display

#endif  // SRC_GRAPHICS_DISPLAY_DRIVERS_AMLOGIC_DISPLAY_POWER_REGS_H_
