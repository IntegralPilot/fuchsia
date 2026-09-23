// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_DEV_CLOCKS_AND_PMIC_MOONFLOWER_MOONFLOWER_RCG_CLOCK_H_
#define ZIRCON_KERNEL_DEV_CLOCKS_AND_PMIC_MOONFLOWER_MOONFLOWER_RCG_CLOCK_H_

#include <debug.h>
#include <stdint.h>

#include <hwreg/bitfields.h>
#include <hwreg/mmio.h>
#include <ktl/algorithm.h>
#include <ktl/optional.h>
#include <ktl/span.h>

#include "moonflower_clock_defs.h"

class MoonflowerRcgClock {
 public:
  static inline constexpr uint32_t FLAG_FORCE_ENABLE_RCG = (1u << 0);
  static inline constexpr uint32_t FLAG_HW_CLK_CTRL_MODE = (1u << 1);
  static inline constexpr uint32_t FLAG_DFS_SUPPORT = (1u << 2);

  constexpr MoonflowerRcgClock(ktl::span<const ParentMap> parent_map,
                               ktl::span<const FreqConfig> freq_table, uint32_t rcgr_offset,
                               uint32_t mnd_width, uint32_t hid_width, uint32_t flags)
      : parent_map_(parent_map),
        freq_table_(freq_table),
        rcgr_offset_(rcgr_offset),
        mnd_width_(mnd_width),
        hid_width_(hid_width),
        flags_(flags) {}

  void Enable(hwreg::RegisterMmio& io, uint64_t freq);
  void Disable(hwreg::RegisterMmio& io);

 private:
  struct CmdReg : public hwreg::RegisterBase<CmdReg, uint32_t> {
    DEF_BIT(0, update);
    DEF_BIT(1, root_en);
    DEF_BIT(4, dirty_cfg);
    DEF_BIT(5, dirty_n);
    DEF_BIT(6, dirty_m);
    DEF_BIT(7, dirty_d);
    DEF_BIT(31, root_off);

    static auto Get(uint32_t offset) { return hwreg::RegisterAddr<CmdReg>(offset + 0x0); }
  };

  struct CfgReg : public hwreg::RegisterBase<CfgReg, uint32_t> {
    DEF_FIELD(7, 0, src_div);
    DEF_FIELD(10, 8, src_sel);
    DEF_FIELD(13, 12, mode);
    DEF_BIT(20, hw_clk_ctrl);
    static auto Get(uint32_t offset) { return hwreg::RegisterAddr<CfgReg>(offset + 0x4); }
  };

  struct MReg : public hwreg::RegisterBase<MReg, uint32_t> {
    DEF_FIELD(31, 0, value);
    static auto Get(uint32_t offset) { return hwreg::RegisterAddr<MReg>(offset + 0x8); }
  };

  struct NReg : public hwreg::RegisterBase<NReg, uint32_t> {
    DEF_FIELD(31, 0, value);
    static auto Get(uint32_t offset) { return hwreg::RegisterAddr<NReg>(offset + 0xc); }
  };

  struct DReg : public hwreg::RegisterBase<DReg, uint32_t> {
    DEF_FIELD(31, 0, value);
    static auto Get(uint32_t offset) { return hwreg::RegisterAddr<DReg>(offset + 0x10); }
  };

  void Configure(hwreg::RegisterMmio& io, const FreqConfig& config) const;
  void UpdateConfig(hwreg::RegisterMmio& io) const;
  ktl::optional<uint8_t> FindParentSource(ClockId id) const;
  const FreqConfig* FindFreqConfig(uint64_t freq) const;
  void ForceEnable(hwreg::RegisterMmio& io, bool enable) const;

  bool is_enabled(hwreg::RegisterMmio& io) const {
    return CmdReg::Get(rcgr_offset_).ReadFrom(&io).root_off() == 0;
  }

  uint32_t mnd_mask() const { return (1u << mnd_width_) - 1; }
  uint32_t hid_mask() const { return (1u << hid_width_) - 1; }

  const ktl::span<const ParentMap> parent_map_;
  const ktl::span<const FreqConfig> freq_table_;
  const uint32_t rcgr_offset_;
  const uint32_t mnd_width_;
  const uint32_t hid_width_;
  const uint32_t flags_;
};

#endif  // ZIRCON_KERNEL_DEV_CLOCKS_AND_PMIC_MOONFLOWER_MOONFLOWER_RCG_CLOCK_H_
