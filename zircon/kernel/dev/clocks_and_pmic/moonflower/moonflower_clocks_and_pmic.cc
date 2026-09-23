// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/debuglog.h>
#include <zircon/types.h>

#include <arch/arm64/periphmap.h>
#include <dev/clocks_and_pmic/moonflower/init.h>
#include <hwreg/bitfields.h>
#include <hwreg/mmio.h>
#include <kernel/spinlock.h>
#include <pdev/clocks_and_pmic.h>

#include "moonflower_alpha_pll.h"
#include "moonflower_clock_defs.h"
#include "moonflower_qupv3_clock.h"
#include "moonflower_rcg_clock.h"

namespace {

class MoonflowerClocksAndPmic {
 public:
  MoonflowerClocksAndPmic() = default;
  ~MoonflowerClocksAndPmic() = default;

  zx_status_t Init() TA_EXCL(lock_);
  zx_status_t PrepareForSuspend() TA_EXCL(lock_);
  zx_status_t WakeupFromSuspend() TA_EXCL(lock_);

 private:
  void SetClocksEnabled(bool enabled) TA_REQ(lock_);

  static inline constexpr const ktl::array kQupV3HaltSequence_{
      // clang-format off
      // MoonflowerQupV3Clock{10, 0x1f14c},       // gcc_qupv3_wrap0_s0_clk
      // MoonflowerQupV3Clock{11, 0x1f280},       // gcc_qupv3_wrap0_s1_clk
      // MoonflowerQupV3Clock{12, 0x1f3b4},       // gcc_qupv3_wrap0_s2_clk
      // MoonflowerQupV3Clock{13, 0x1f4e8},       // gcc_qupv3_wrap0_s3_clk
      // MoonflowerQupV3Clock{14, 0x1f61c},       // gcc_qupv3_wrap0_s4_clk
      // MoonflowerQupV3Clock{15, 0x1f750},       // gcc_qupv3_wrap0_s5_clk
      MoonflowerQupV3Clock{16, 0x1f884},          // gcc_qupv3_wrap0_s6_clk
      // MoonflowerQupV3Clock{17, 0x1f9b8},       // gcc_qupv3_wrap0_s7_clk
      MoonflowerQupV3Clock{6, 0x1f004, 0x1f004},  // gcc_qupv3_wrap_0_m_ahb_clk
      MoonflowerQupV3Clock{7, 0x1f008, 0x1f008},  // gcc_qupv3_wrap_0_s_ahb_clk
      MoonflowerQupV3Clock{8, 0x1f00c},           // gcc_qupv3_wrap0_core_clk
      MoonflowerQupV3Clock{9, 0x1f018},           // gcc_qupv3_wrap0_core_2x_clk

      // Unrelated clocks.  Don't touch these.
      // MoonflowerQupV3Clock{0u, 0x17014},
      // MoonflowerQupV3Clock{1u, 0x17018},
      // MoonflowerQupV3Clock{2u, 0x17068},
      // MoonflowerQupV3Clock{4u, 0x36040},
      // MoonflowerQupV3Clock{5u, 0x1706c},
      // clang-format on
  };

  DECLARE_SPINLOCK(MoonflowerClocksAndPmic) lock_;

  TA_GUARDED(lock_) hwreg::RegisterMmio io_ { 0 };
  TA_GUARDED(lock_) bool prepared_for_suspend_ { false };
  TA_GUARDED(lock_) uint32_t qup_v3_clockstate_at_halt_time_ { 0 };

  // HW_CLK_CTRL_MODE is set by default, but is cleared in Linux during boot.
  // Here we do the same by setting flags to 0. See https://fxbug.dev/483279438
  TA_GUARDED(lock_)
  MoonflowerRcgClock gcc_qupv3_wrap0_s6_clk_src_{gcc_parent_map_1,
                                                 ftbl_gcc_qupv3_wrap0_s5_clk_src,
                                                 0x1f88c,  // rcgr_offset
                                                 16,       // mnd_width
                                                 5,        // hid_width
                                                 0};       // flags

  // GPLL0 is final PLL we need to turn off after we've shut off everything else.
  TA_GUARDED(lock_) MoonflowerAlphaPll gcc_gpll0_ { 0, 0 };
};

zx_status_t MoonflowerClocksAndPmic::Init() {
  constexpr paddr_t kPhysRegBase = 0x1400000;
  vaddr_t reg_base = periph_paddr_to_vaddr(kPhysRegBase);
  if (reg_base == 0) {
    return ZX_ERR_NO_RESOURCES;
  }

  Guard<SpinLock, IrqSave> guard{&lock_};
  io_ = hwreg::RegisterMmio{reinterpret_cast<volatile void*>(reg_base)};

  return ZX_OK;
}

zx_status_t MoonflowerClocksAndPmic::PrepareForSuspend() {
  Guard<SpinLock, IrqSave> guard{&lock_};

  if (!prepared_for_suspend_) {
    SetClocksEnabled(false);
    prepared_for_suspend_ = true;
  }

  return ZX_OK;
}

zx_status_t MoonflowerClocksAndPmic::WakeupFromSuspend() {
  Guard<SpinLock, IrqSave> guard{&lock_};

  if (prepared_for_suspend_) {
    SetClocksEnabled(true);
    prepared_for_suspend_ = false;
  }

  return ZX_OK;
}

void MoonflowerClocksAndPmic::SetClocksEnabled(bool enabled) {
  uint32_t clk_reg_state = MoonflowerQupV3Clock::GetClockRegState(io_);

  if (!enabled) {
    // Stash the state of the clocks as we halt.
    qup_v3_clockstate_at_halt_time_ = clk_reg_state;
  } else {
    // Turn on GPLL0 first.
    gcc_gpll0_.Enable(io_);

    // Enable the S6 src PLL next.
    constexpr uint64_t kEnabledS6SrcFreq = 7372800;
    gcc_qupv3_wrap0_s6_clk_src_.Enable(io_, kEnabledS6SrcFreq);
  }

  for (size_t i = 0; i < kQupV3HaltSequence_.size(); ++i) {
    // When enabling, run the halt sequence in reverse.  Otherwise, iterate
    // forward through it.
    const size_t ndx = enabled ? (kQupV3HaltSequence_.size() - i - 1) : i;
    const MoonflowerQupV3Clock& clk = kQupV3HaltSequence_[ndx];
    const uint32_t enb_mask = clk.enable_mask();

    // Only enabled/disable the clock if it was enabled when we started the
    // suspend operation.
    if (qup_v3_clockstate_at_halt_time_ & enb_mask) {
      if (enabled) {
        clk_reg_state |= enb_mask;
      } else {
        clk_reg_state &= ~enb_mask;
      }

      // Tell the clock to change states.
      MoonflowerQupV3Clock::SetClockRegState(io_, clk_reg_state);

      // Now wait for it to finish.
      if (enabled) {
        clk.WaitForEnabled(io_);
      } else {
        clk.WaitForDisabled(io_);
      }
    }
  }

  if (!enabled) {
    // Disable the S6 source PLL.
    gcc_qupv3_wrap0_s6_clk_src_.Disable(io_);

    // Turn off GPLL0 last.
    gcc_gpll0_.Disable(io_, MoonflowerAlphaPll::ResetOnDisable::No);
  }
}

MoonflowerClocksAndPmic gMoonflowerClocksAndPmic;
struct pdev_clocks_and_pmic_ops moonflower_clocks_and_pmic_ops{
    .prepare_for_suspend = []() { return gMoonflowerClocksAndPmic.PrepareForSuspend(); },
    .wakeup_from_suspend = []() { return gMoonflowerClocksAndPmic.WakeupFromSuspend(); }};

}  // namespace

void moonflower_clocks_and_pmic_init_early() {
  if (const zx_status_t status = gMoonflowerClocksAndPmic.Init(); status == ZX_OK) {
    dprintf(INFO, "CLOCKS: registering moonflower Clocks and PMIC hooks\n");
    pdev_register_clocks_and_pmic(&moonflower_clocks_and_pmic_ops);
  } else {
    dprintf(CRITICAL, "ERROR: failed to initialize moonflower Clocks and PMIC driver (%d)\n",
            status);
  }
}
