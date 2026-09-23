// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "moonflower_alpha_pll.h"

#include <debug.h>
#include <lib/arch/intrin.h>
#include <stdint.h>

#include <hwreg/mmio.h>

void MoonflowerAlphaPll::Enable(hwreg::RegisterMmio& io) {
  // If we are in "FSM mode" apparently we are just supposed to vote and wait
  // for the PLL to declare that it has locked.
  if (UserCtlReg::Get(reg_offset_).ReadFrom(&io).evo_enable_vote_run() != 0) {
    // Set the bit in the main PLL enable register.
    uint32_t new_val = MainPllEnbReg::Get().ReadFrom(&io).reg_value() | (0x1 << gpll_id_);
    MainPllEnbReg::Get().FromValue(new_val).WriteTo(&io);

    // Wait for lock and we are done.
    WaitForLock(io, 1500);
    return;
  }

  // If we are already enabled, there is nothing to do.
  if (IsEnabled(io)) {
    return;
  }

  // Set the reset_n bit (eg; release the PLL from reset) and set the run bit in
  // the op-mode register.  Then, wait for lock.
  ModeReg::Get(reg_offset_).ReadFrom(&io).set_reset_n(1).WriteTo(&io);
  OpModeReg::Get(reg_offset_).ReadFrom(&io).set_run(1).WriteTo(&io);
  WaitForLock(io, 1500);

  // Turn on the output, and make sure that every write has been retired before
  // proceeding.
  ModeReg::Get(reg_offset_).ReadFrom(&io).set_outctrl(1).WriteTo(&io);
  arch::DeviceMemoryBarrier();
}

void MoonflowerAlphaPll::Disable(hwreg::RegisterMmio& io, ResetOnDisable reset) {
  // If we are in "FSM mode", just remove our vote and we should be finished.
  if (UserCtlReg::Get(reg_offset_).ReadFrom(&io).evo_enable_vote_run() != 0) {
    // Clear the bit in the main PLL enable register.
    uint32_t new_val = MainPllEnbReg::Get().ReadFrom(&io).reg_value() & ~(0x1 << gpll_id_);
    MainPllEnbReg::Get().FromValue(new_val).WriteTo(&io);
    return;
  }

  // Turn off the output first.
  ModeReg::Get(reg_offset_).ReadFrom(&io).set_outctrl(0).WriteTo(&io);

  // Place the PLL in standby.  Note that QCOM's code does this by writing a 0
  // value to the entire OpMode register (it does not bother to obey any field
  // boundaries), so that is what we do as well.
  OpModeReg::Get(reg_offset_).FromValue(0).WriteTo(&io);

  // Finally, place the device into reset if we were asked to do so.
  if (reset == ResetOnDisable::Yes) {
    ModeReg::Get(reg_offset_).ReadFrom(&io).set_reset_n(0).WriteTo(&io);
  }
}

bool MoonflowerAlphaPll::WaitForLock(hwreg::RegisterMmio& io, uint32_t usec_max) {
  return WaitFor([&]() { return ModeReg::Get(reg_offset_).ReadFrom(&io).lock_det() != 0; },
                 usec_max);
}

bool MoonflowerAlphaPll::IsEnabled(hwreg::RegisterMmio& io) {
  return (ModeReg::Get(reg_offset_).ReadFrom(&io).outctrl() != 0) &&
         (OpModeReg::Get(reg_offset_).ReadFrom(&io).run() != 0);
}
