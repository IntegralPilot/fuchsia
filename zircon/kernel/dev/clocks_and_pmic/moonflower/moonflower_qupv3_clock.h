// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_DEV_CLOCKS_AND_PMIC_MOONFLOWER_MOONFLOWER_QUPV3_CLOCK_H_
#define ZIRCON_KERNEL_DEV_CLOCKS_AND_PMIC_MOONFLOWER_MOONFLOWER_QUPV3_CLOCK_H_

#include <debug.h>
#include <stdint.h>

#include <hwreg/bitfields.h>
#include <hwreg/mmio.h>

#include "moonflower_clock_defs.h"

class MoonflowerQupV3Clock {
 public:
  constexpr MoonflowerQupV3Clock(uint32_t enable_bit, uint32_t halt_reg_offset,
                                 uint32_t hwcg_reg_offset = 0)
      : enable_bit_(enable_bit),
        halt_reg_offset_(halt_reg_offset),
        hwcg_reg_offset_(hwcg_reg_offset) {}

  static uint32_t GetClockRegState(hwreg::RegisterMmio& io);
  static void SetClockRegState(hwreg::RegisterMmio& io, uint32_t val);

  uint32_t enable_bit() const { return enable_bit_; }
  uint32_t enable_mask() const { return (1u << enable_bit_); }

  void WaitForDisabled(hwreg::RegisterMmio& io) const;
  void WaitForEnabled(hwreg::RegisterMmio& io) const;

 private:
  struct ClockReg : public hwreg::RegisterBase<ClockReg, uint32_t> {
    DEF_BIT(0, qmip_camera_nrt_ahb_clk);
    DEF_BIT(1, qmip_disp_ahb_clk);
    DEF_BIT(2, qmip_camera_rt_ahb_clk);
    DEF_BIT(4, qmip_gpu_cfg_ahb_clk);
    DEF_BIT(5, disp_throttle_core_clk);
    DEF_BIT(6, qupv3_wrap_0_m_ahb_clk);
    DEF_BIT(7, qupv3_wrap_0_s_ahb_clk);
    DEF_BIT(8, qupv3_wrap0_core_clk);
    DEF_BIT(9, qupv3_wrap0_core_2x_clk);
    DEF_BIT(10, qupv3_wrap0_s0_clk);
    DEF_BIT(11, qupv3_wrap0_s1_clk);
    DEF_BIT(12, qupv3_wrap0_s2_clk);
    DEF_BIT(13, qupv3_wrap0_s3_clk);
    DEF_BIT(14, qupv3_wrap0_s4_clk);
    DEF_BIT(15, qupv3_wrap0_s5_clk);
    DEF_BIT(16, qupv3_wrap0_s6_clk);
    DEF_BIT(17, qupv3_wrap0_s7_clk);
    static auto Get() { return hwreg::RegisterAddr<ClockReg>(0x7900C); }
  };

  struct ClkHaltReg : public hwreg::RegisterBase<ClkHaltReg, uint32_t> {
    DEF_FIELD(7, 4, sleep);
    DEF_FIELD(11, 8, wakeup);
    DEF_BIT(12, force_mem_periph_off);
    DEF_BIT(13, force_mem_periph_on);
    DEF_BIT(14, force_mem_core_on);
    DEF_FIELD(30, 28, noc_fsm_status);
    DEF_BIT(31, clk_off);

    static auto Get(uint32_t offset) { return hwreg::RegisterAddr<ClkHaltReg>(offset); }
  };

  struct HWClkGateReg : public hwreg::RegisterBase<HWClkGateReg, uint32_t> {
    DEF_BIT(1, hw_clk_gate_mode);

    static auto Get(uint32_t offset) { return hwreg::RegisterAddr<HWClkGateReg>(offset); }
  };

  bool InHwClockGateMode(hwreg::RegisterMmio& io) const;

  const uint32_t enable_bit_;
  const uint32_t halt_reg_offset_;
  const uint32_t hwcg_reg_offset_;
};

#endif  // ZIRCON_KERNEL_DEV_CLOCKS_AND_PMIC_MOONFLOWER_MOONFLOWER_QUPV3_CLOCK_H_
