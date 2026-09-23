// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "moonflower_rcg_clock.h"

#include <debug.h>
#include <stdint.h>

#include <hwreg/mmio.h>

void MoonflowerRcgClock::Enable(hwreg::RegisterMmio& io, uint64_t freq) {
  const bool force_enable_rcg_flag = (flags_ & FLAG_FORCE_ENABLE_RCG) != 0;
  if (force_enable_rcg_flag) {
    ForceEnable(io, true);
  }

  const FreqConfig* const freq_config = (freq == CXO_F.freq) ? &CXO_F : FindFreqConfig(freq);
  if (!freq_config) {
    return;
  }

  if (!force_enable_rcg_flag) {
    ForceEnable(io, true);
    Configure(io, *freq_config);
    ForceEnable(io, false);
  } else {
    Configure(io, *freq_config);
  }
}

void MoonflowerRcgClock::Disable(hwreg::RegisterMmio& io) {
  ForceEnable(io, true);
  Configure(io, CXO_F);
  ForceEnable(io, false);
}

void MoonflowerRcgClock::Configure(hwreg::RegisterMmio& io, const FreqConfig& config) const {
  ktl::optional<uint8_t> src_cfg = FindParentSource(config.src_id);
  if (!src_cfg) {
    return;
  }

  if (mnd_width_ && config.n) {
    // Update M
    const uint32_t cur_M = MReg::Get(rcgr_offset_).ReadFrom(&io).value();
    const uint32_t new_M = (cur_M & ~mnd_mask()) | (config.m & mnd_mask());
    MReg::Get(rcgr_offset_).FromValue(new_M).WriteTo(&io);

    // Update N
    const uint32_t n_minus_m = config.n - config.m;
    const uint32_t cur_N = NReg::Get(rcgr_offset_).ReadFrom(&io).value();
    const uint32_t new_N = (cur_N & ~mnd_mask()) | (~n_minus_m & mnd_mask());
    NReg::Get(rcgr_offset_).FromValue(new_N).WriteTo(&io);

    // Update D
    const uint32_t n_minus_m_2x = n_minus_m << 1;
    const uint32_t cur_D = NReg::Get(rcgr_offset_).ReadFrom(&io).value();
    const uint32_t dval = ktl::clamp<uint32_t>(config.n, config.m, n_minus_m_2x);
    const uint32_t new_D = (cur_D & ~mnd_mask()) | (~dval & mnd_mask());
    DReg::Get(rcgr_offset_).FromValue(new_D).WriteTo(&io);
  }

  auto cfg_reg = CfgReg::Get(rcgr_offset_).ReadFrom(&io);
  constexpr uint32_t kDualEdgeMode = 0x2;
  const uint32_t mode = (mnd_width_ && config.n && (config.n != config.m)) ? kDualEdgeMode : 0;
  const uint32_t src_div = (cfg_reg.src_div() & ~hid_mask()) | (config.pre_div & hid_mask());

  cfg_reg.set_src_div(src_div)
      .set_src_sel(src_cfg.value())
      .set_mode(mode)
      .set_hw_clk_ctrl((flags_ & FLAG_HW_CLK_CTRL_MODE) ? 1 : 0)
      .WriteTo(&io);

  UpdateConfig(io);
}

// Issue the update command and wait for acknowledgement
void MoonflowerRcgClock::UpdateConfig(hwreg::RegisterMmio& io) const {
  CmdReg::Get(rcgr_offset_).ReadFrom(&io).set_update(1).WriteTo(&io);

  // Existing code says that we need to way ~3 cycles of the "old and new
  // clock rates".  When we are parked, we should be running at 19.2 MHz
  // (passthru I think).  When we are running, we should be running at ~7MHz.
  // So, it should not take much more than 1 uSec to configure the clock.
  //
  // Existing linux code seems pretty wrong, however.  They are computing a
  // timeout of zero uSec for both of these rates, and the way their timeout
  // loop works, this means that they are whacking the "update" command bit,
  // but never (not even once) polling it to see if we have actually started
  // before moving on.
  //
  // For now, we use a hard-coded 10 uSec as our update timeout.
  for (uint32_t i = 0; i < 10; ++i) {
    CmdReg reg = CmdReg::Get(rcgr_offset_).ReadFrom(&io);
    if (reg.update() == 0) {
      break;
    }
    spin(1);
  }
}

ktl::optional<uint8_t> MoonflowerRcgClock::FindParentSource(ClockId id) const {
  for (const ParentMap& pm : parent_map_) {
    if (id == pm.id) {
      return pm.cfg;
    }
  }

  return ktl::nullopt;
}

const FreqConfig* MoonflowerRcgClock::FindFreqConfig(uint64_t freq) const {
  if (freq_table_.size() == 0) {
    return nullptr;
  }

  for (const FreqConfig& fc : freq_table_) {
    if (freq <= fc.freq) {
      return &fc;
    }
  }

  return &freq_table_[freq_table_.size() - 1];
}

void MoonflowerRcgClock::ForceEnable(hwreg::RegisterMmio& io, bool enable) const {
  if (enable) {
    CmdReg::Get(rcgr_offset_).ReadFrom(&io).set_root_en(1).WriteTo(&io);
    for (uint32_t i = 0; (i < 500) && !is_enabled(io); ++i) {
      spin(1);
    }
  } else {
    CmdReg::Get(rcgr_offset_).ReadFrom(&io).set_root_en(0).WriteTo(&io);
  }
}
