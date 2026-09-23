// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_DRIVERS_AMLOGIC_DISPLAY_RDMA_REGS_H_
#define SRC_GRAPHICS_DISPLAY_DRIVERS_AMLOGIC_DISPLAY_RDMA_REGS_H_

#include <lib/driver/mmio/cpp/mmio-buffer.h>

#include <cstdint>

#include <hwreg/bitfields.h>

// The register definitions here are from AMLogic S912 Datasheet revision 0.1.
// The information was cross-checked against AMLogic's VLSI GXL RDMA
// Specification, Version 1.0, dated September 26, 2018.

// The RDMA (Register Direct Memory Access) is briefly described in the S912
// Datasheet section 27.3 "Video Output" > "RDMA".
//
// The A311D datasheet Section 10.2.1 "Video Output" > "Overview" mentions that
// the VPU includes an RDMA submodule, but does not list the registers. Our
// experiments and passing tests indicate that the RDMA functionality we use is
// unchanged.

// Register addresses from the S912 datasheet section 27.3 "RDMA", Table
// IV.29.3 "Register List of RDMA". The table specifies absolute addresses, but
// the addresses are relative to the VPU's MMIO block. S912 datasheet section
// 16 "Memory Map" indicates that the S912's VPU's MMIO block starts at
// 0xd0100000.
#define VPU_RDMA_AHB_START_ADDR_MAN 0x4400
#define VPU_RDMA_AHB_END_ADDR_MAN 0x4404
#define VPU_RDMA_AHB_START_ADDR_1 0x4408
#define VPU_RDMA_AHB_END_ADDR_1 0x440c
#define VPU_RDMA_AHB_START_ADDR_2 0x4410
#define VPU_RDMA_AHB_END_ADDR_2 0x4414
#define VPU_RDMA_AHB_START_ADDR_3 0x4418
#define VPU_RDMA_AHB_END_ADDR_3 0x441c
#define VPU_RDMA_AHB_START_ADDR_4 0x4420
#define VPU_RDMA_AHB_END_ADDR_4 0x4424
#define VPU_RDMA_AHB_START_ADDR_5 0x4428
#define VPU_RDMA_AHB_END_ADDR_5 0x442c
#define VPU_RDMA_AHB_START_ADDR_6 0x4430
#define VPU_RDMA_AHB_END_ADDR_6 0x4434
#define VPU_RDMA_AHB_START_ADDR_7 0x4438
#define VPU_RDMA_AHB_END_ADDR_7 0x443c
#define VPU_RDMA_ACCESS_AUTO 0x4440
#define VPU_RDMA_ACCESS_AUTO2 0x4444
#define VPU_RDMA_ACCESS_AUTO3 0x4448
#define VPU_RDMA_ACCESS_MAN 0x444c
#define VPU_RDMA_CTRL 0x4450
#define VPU_RDMA_STATUS 0x4454
#define VPU_RDMA_STATUS2 0x4458
#define VPU_RDMA_STATUS3 0x445c

#define VPU_RDMA_AHB_START_ADDR(x) (VPU_RDMA_AHB_START_ADDR_MAN + ((x) << 3))
#define VPU_RDMA_AHB_END_ADDR(x) (VPU_RDMA_AHB_END_ADDR_MAN + ((x) << 3))

// VPU_RDMA_ACCESS_AUTO Bit Definition
#define RDMA_ACCESS_AUTO_INT_EN(channel) (1 << ((channel) << 3))
#define RDMA_ACCESS_AUTO_WRITE(channel) (1 << ((channel) + 4))

// VPU_RDMA_CTRL Bit Definition
#define RDMA_CTRL_INT_DONE(channel) (1 << (channel))

// VPU_RDMA_STATUS Bit Definition
#define RDMA_STATUS_DONE(channel) (1 << (channel))

class RdmaStatusReg : public hwreg::RegisterBase<RdmaStatusReg, uint32_t> {
 public:
  // Both of these fields use bit 0 for manual RDMA. Auto RDMA is 1-indexed.
  DEF_FIELD(31, 24, done);
  DEF_FIELD(7, 0, req_latch);
  static auto Get() { return hwreg::RegisterAddr<RdmaStatusReg>(VPU_RDMA_STATUS); }

  bool RequestLatched(uint8_t chan) const { return req_latch() & (1 << chan); }

  bool ChannelDone(uint8_t chan) const { return done() & (1 << chan); }

  static bool ChannelDone(uint8_t chan, fdf::MmioBuffer* mmio) {
    return Get().ReadFrom(mmio).ChannelDone(chan);
  }

  // compute a value for .done() to match given the current state of
  // VPU_RDMA_ACCESS_AUTO, AUTO2, and AUTO3.
  static uint32_t DoneFromAccessAuto(const uint32_t aa, const uint32_t aa2, const uint32_t aa3) {
    const uint32_t chn7 = ((aa3 >> 24) & 0x1) << 7;
    const uint32_t chn2 = ((aa >> 24) & 0x1) << 2;
    const uint32_t chn1 = ((aa >> 16) & 0x1) << 1;
    const uint32_t chn0 = ((aa >> 8) & 0x1) << 0;
    return chn7 | chn2 | chn1 | chn0;
  }
};

class RdmaCtrlReg : public hwreg::RegisterBase<RdmaCtrlReg, uint32_t> {
 public:
  // As with RDMA_STATUS, this field uses bit 0 for manual RDMA.
  DEF_FIELD(31, 24, clear_done);
  DEF_BIT(7, write_urgent);
  DEF_BIT(6, read_urgent);
  static auto Get() { return hwreg::RegisterAddr<RdmaCtrlReg>(VPU_RDMA_CTRL); }

  static void ClearInterrupt(uint8_t chan, fdf::MmioBuffer* mmio) {
    Get().ReadFrom(mmio).set_clear_done(1 << chan).WriteTo(mmio);
  }
};

class RdmaAccessAutoReg : public hwreg::RegisterBase<RdmaAccessAutoReg, uint32_t> {
 public:
  DEF_FIELD(31, 24, chn3_intr);
  DEF_FIELD(23, 16, chn2_intr);
  DEF_FIELD(15, 8, chn1_intr);
  DEF_BIT(7, chn3_auto_write);
  DEF_BIT(6, chn2_auto_write);
  DEF_BIT(5, chn1_auto_write);
  static auto Get() { return hwreg::RegisterAddr<RdmaAccessAutoReg>(VPU_RDMA_ACCESS_AUTO); }
};

class RdmaAccessAuto2Reg : public hwreg::RegisterBase<RdmaAccessAuto2Reg, uint32_t> {
 public:
  DEF_BIT(7, chn7_auto_write);
  static auto Get() { return hwreg::RegisterAddr<RdmaAccessAuto2Reg>(VPU_RDMA_ACCESS_AUTO2); }
};

class RdmaAccessAuto3Reg : public hwreg::RegisterBase<RdmaAccessAuto3Reg, uint32_t> {
 public:
  DEF_FIELD(31, 24, chn7_intr);
  static auto Get() { return hwreg::RegisterAddr<RdmaAccessAuto3Reg>(VPU_RDMA_ACCESS_AUTO3); }
};

#endif  // SRC_GRAPHICS_DISPLAY_DRIVERS_AMLOGIC_DISPLAY_RDMA_REGS_H_
