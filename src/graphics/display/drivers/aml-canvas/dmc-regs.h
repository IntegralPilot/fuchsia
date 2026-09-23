// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_DRIVERS_AML_CANVAS_DMC_REGS_H_
#define SRC_GRAPHICS_DISPLAY_DRIVERS_AML_CANVAS_DMC_REGS_H_

#include <hwreg/bitfields.h>

// The register definitions here are stitched together from the A311D datasheet
// revision 08 section 13.1.2 "Memory Interface" > "DDR" > "Register
// Description", and from experimentally confirmed clues in the S912 datasheet
// revision 0.1 section 30.2 "DDR" > "Register Description".
//
// The A311D defines two sets of registers, and lists the same base
// 0xff63'8000 for both sets. Fortunately, the S912 datasheet offered us clues
// for recovering from this typographical error. We confirmed experimentally
// that our guess below is correct.
//
// The S912 datasheet lists separate base addresses for the two sets - the DMC_
// registers are based at 0xc883'8000, while the AM_DDR_ registers (for the DDR
// PHY) are based at 0xc883'7000. Section 16 "Memory Map" in the S912 datasheet
// tells us that the DMC_ registers are in the MMIO region labeled "DMC",
// whereas the AM_DDR_ registers are in the second half of the "DDR TOP" MMIO
// region.
//
// Section 8.1 "Memory Map" in the A311D datasheet lists 0xff63'8000 as the
// base of the DMC register set.
//
// Section 13.1.2 has some (fortunately obvious) typographical errors in some of
// the register names. The "DMC_" prefix is incorrectly replaced with "DC_". The
// S912 datasheet uses the "DMC_" prefix for the same registers, confirming the
// suspicion that "DC_" is a typographical error.

// The DMC is briefly described in A311D datasheet section 13.1.1 "Memory
// Interface" > "DDR" > "Overview" and S912 datasheet section 30.1 "DDR" >
// "Overview".

namespace aml_canvas {

// AMLogic documentation uses "DMC" to refer to the DDR memory controller. The
// canvas driver controls a special subset of that controller, used to translate
// image data accesses from (x, y) coordinates to memory addresses.

constexpr uint32_t kDmcCavLutDataL = (0x12 << 2);
constexpr uint32_t kDmcCavLutDataH = (0x13 << 2);
constexpr uint32_t kDmcCavLutAddr = (0x14 << 2);
constexpr uint32_t kDmcCavMaxRegAddr = kDmcCavLutAddr;

// DMC_CAV_LUT_DATAL
//
// This register is typo-ed as DC_CAV_LUT_DATAL in the A311D datasheet
class CanvasLutDataLow : public hwreg::RegisterBase<CanvasLutDataLow, uint32_t> {
 public:
  // Bottom 3 bits of width in 8-byte units, 32-byte aligned.
  DEF_FIELD(31, 29, dmc_cav_width);
  // Starting physical address of data in 8-byte units, 32-byte aligned.
  DEF_FIELD(28, 0, dmc_cav_addr);

  static auto Get() { return hwreg::RegisterAddr<CanvasLutDataLow>(kDmcCavLutDataL); }

  void SetDmcCavWidth(uint32_t width) { set_dmc_cav_width(width & kDmcCavWidthLmask_); }

 private:
  // Mask to extract the bits of dmc_cav_width encoded in CanvasLutDataLow.
  // The remaining bits go in CanvasLutDataHigh.
  static constexpr uint32_t kDmcCavWidthLmask_ = 7;
};

// DMC_CAV_LUT_DATAH, typo-ed as DC_CAV_LUT_DATAH
class CanvasLutDataHigh : public hwreg::RegisterBase<CanvasLutDataHigh, uint32_t> {
 public:
  DEF_RSVDZ_FIELD(31, 30);

  // Swap words in image data. 1<<3 = 64-bit, 1<<2 = 32-bit, 1<<1 = 16-bit, 1 =
  // 8-bit, 0 = none.
  DEF_FIELD(29, 26, dmc_cav_endianness);
  // 0 = linear, 1 = 32x32, 2 = 64x32
  DEF_FIELD(25, 24, dmc_cav_blkmode);
  // If 1, y coordinates are computed modulo height
  DEF_BIT(23, dmc_cav_ywrap);
  // If 1, x coordinates are computed modulo width
  DEF_BIT(22, dmc_cav_xwrap);
  // Height in rows.
  DEF_FIELD(21, 9, dmc_cav_height);
  // Width in 8-byte units, 32-byte aligned.
  DEF_FIELD(8, 0, dmc_cav_width);

  static auto Get() { return hwreg::RegisterAddr<CanvasLutDataHigh>(kDmcCavLutDataH); }

  void SetDmcCavWidth(uint32_t width) { set_dmc_cav_width(width >> kDmcCavWidthLwidth_); }

  static constexpr uint32_t kDmcCavYwrap = (1 << 23);
  static constexpr uint32_t kDmcCavXwrap = (1 << 22);

 private:
  // Number of bits of dmc_cav_width encoded in CanvasLutDataLow.
  static constexpr uint32_t kDmcCavWidthLwidth_ = 3;
};

// DMC_CAV_LUT_ADDR, typo-ed as DC_CAV_LUT_ADDR
//
// An index register used to read or write canvas entries in the LUT.
// Writes must be followed by at least one read to ensure that they have been
// committed, e.g. CanvasLutDataHigh::Get().ReadFrom(mmio_space)
class CanvasLutAddr : public hwreg::RegisterBase<CanvasLutAddr, uint32_t> {
 public:
  DEF_RSVDZ_FIELD(31, 10);

  DEF_BIT(9, dmc_cav_addr_wr);
  DEF_BIT(8, dmc_cav_addr_rd);
  DEF_FIELD(7, 0, dmc_cav_addr_index);

  static auto Get() { return hwreg::RegisterAddr<CanvasLutAddr>(kDmcCavLutAddr); }
};

}  // namespace aml_canvas

#endif  // SRC_GRAPHICS_DISPLAY_DRIVERS_AML_CANVAS_DMC_REGS_H_
