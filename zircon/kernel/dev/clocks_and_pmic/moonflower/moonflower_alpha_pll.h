// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_DEV_CLOCKS_AND_PMIC_MOONFLOWER_MOONFLOWER_ALPHA_PLL_H_
#define ZIRCON_KERNEL_DEV_CLOCKS_AND_PMIC_MOONFLOWER_MOONFLOWER_ALPHA_PLL_H_

#include <stdint.h>

#include <hwreg/bitfields.h>
#include <hwreg/mmio.h>

#include "moonflower_clock_defs.h"

class MoonflowerAlphaPll {
 public:
  enum class ResetOnDisable { No, Yes };

  constexpr MoonflowerAlphaPll(uint32_t id, uint32_t reg_offset)
      : gpll_id_(id), reg_offset_(reg_offset) {}

  // No copy, no move.
  MoonflowerAlphaPll(const MoonflowerAlphaPll&) = delete;
  MoonflowerAlphaPll(MoonflowerAlphaPll&&) = delete;
  MoonflowerAlphaPll& operator=(const MoonflowerAlphaPll&) = delete;
  MoonflowerAlphaPll& operator=(MoonflowerAlphaPll&&) = delete;

  void Enable(hwreg::RegisterMmio& io);
  void Disable(hwreg::RegisterMmio& io, ResetOnDisable reset);

 private:
  template <typename Predicate>
  bool WaitFor(Predicate predicate, uint32_t usec_max) {
    for (uint32_t i = 0; i < usec_max; ++i) {
      if (predicate()) {
        return true;
      }
      spin(1);
    }
    return false;
  }

  bool WaitForLock(hwreg::RegisterMmio& io, uint32_t usec_max);
  bool IsEnabled(hwreg::RegisterMmio& io);

  // Register offsets for non-100% register definitions taken from the
  // CLK_ALPHA_PLL_TYPE_LUCID_EVO flavor of PLLs.
  //
  // Note: There are some bits defined in these registers which either overlap
  // other fields (latch_interface and the lock_count field in the Mode
  // register), or which have alternate names (fsm_ena and vote_fsm_ena, or
  // alpha_mode and evo_enable_vote_run).  It is VERY important to NOT list
  // these alias here.  Doing so causes the `hwreg` library to DEBUG_ASSERT at
  // time that a register is instantiated, but at runtime, not compile time.
  // ASSERTing as we are turning off all of our clocks is Very Bad (and
  // difficult to debug as well).
  //
  // So, we list the aliases here for reference's sake, but we leave them
  // commented out.
  struct ModeReg : public hwreg::RegisterBase<ModeReg, uint32_t> {
    DEF_BIT(0, outctrl);
    DEF_BIT(1, bypassnl);
    DEF_BIT(2, reset_n);
    DEF_BIT(7, offline_req);
    DEF_FIELD(13, 8, lock_count);
    DEF_FIELD(19, 14, bias_count);
    // DEF_BIT(11, latch_interface);
    DEF_BIT(20, vote_fsm_ena);
    // DEF_BIT(20, fsm_ena);
    DEF_BIT(21, vote_fsm_reset);
    DEF_BIT(22, update);
    DEF_BIT(23, update_bypass);
    DEF_BIT(24, fsm_legacy_mode);
    DEF_BIT(28, offline_ack);
    DEF_BIT(29, ack_latch);
    DEF_BIT(30, active_flag);
    DEF_BIT(31, lock_det);

    static auto Get(uint32_t offset) { return hwreg::RegisterAddr<ModeReg>(offset + 0x00); }
  };

  struct OpModeReg : public hwreg::RegisterBase<OpModeReg, uint32_t> {
    DEF_BIT(0, run);

    static auto Get(uint32_t offset) { return hwreg::RegisterAddr<OpModeReg>(offset + 0x04); }
  };

  struct UserCtlReg : public hwreg::RegisterBase<UserCtlReg, uint32_t> {
    DEF_FIELD(19, 8, post_div);
    DEF_FIELD(21, 20, vco);
    DEF_BIT(24, alpha_en);
    // DEF_BIT(25, alpha_mode);
    DEF_BIT(25, evo_enable_vote_run);

    static auto Get(uint32_t offset) { return hwreg::RegisterAddr<UserCtlReg>(offset + 0x18); }
  };

  struct MainPllEnbReg : public hwreg::RegisterBase<MainPllEnbReg, uint32_t> {
    DEF_BIT(0, gpll0_enb);
    DEF_BIT(3, gpll3_enb);
    DEF_BIT(4, gpll4_enb);
    DEF_BIT(6, gpll6_enb);
    DEF_BIT(7, gpll7_enb);
    DEF_BIT(8, gpll8_enb);
    DEF_BIT(9, gpll9_enb);
    DEF_BIT(10, gpll10_enb);

    static auto Get() { return hwreg::RegisterAddr<MainPllEnbReg>(0x79000); }
  };

  const uint32_t gpll_id_;
  const uint32_t reg_offset_;
};

#endif  // ZIRCON_KERNEL_DEV_CLOCKS_AND_PMIC_MOONFLOWER_MOONFLOWER_ALPHA_PLL_H_
