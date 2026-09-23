// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "moonflower_qupv3_clock.h"

#include <debug.h>
#include <stdint.h>

#include <hwreg/mmio.h>

uint32_t MoonflowerQupV3Clock::GetClockRegState(hwreg::RegisterMmio& io) {
  return ClockReg::Get().ReadFrom(&io).reg_value();
}

void MoonflowerQupV3Clock::SetClockRegState(hwreg::RegisterMmio& io, uint32_t val) {
  ClockReg::Get().FromValue(val).WriteTo(&io);
}

void MoonflowerQupV3Clock::WaitForDisabled(hwreg::RegisterMmio& io) const {
  if (InHwClockGateMode(io)) {
    return;
  }

  spin(10);
}

void MoonflowerQupV3Clock::WaitForEnabled(hwreg::RegisterMmio& io) const {
  if (InHwClockGateMode(io)) {
    return;
  }

  auto halt_reg = ClkHaltReg::Get(halt_reg_offset_);
  while (true) {
    constexpr uint32_t STATUS_ON = (1u << 1);
    auto rv = halt_reg.ReadFrom(&io);

    if (!rv.clk_off() || (rv.noc_fsm_status() == STATUS_ON)) {
      break;
    }

    spin(1);
  }
}

bool MoonflowerQupV3Clock::InHwClockGateMode(hwreg::RegisterMmio& io) const {
  if (hwcg_reg_offset_ == 0) {
    return false;
  }

  return HWClkGateReg::Get(hwcg_reg_offset_).ReadFrom(&io).hw_clk_gate_mode() != 0;
}
