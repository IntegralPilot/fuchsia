// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_DEV_CLOCKS_AND_PMIC_MOONFLOWER_MOONFLOWER_CLOCK_DEFS_H_
#define ZIRCON_KERNEL_DEV_CLOCKS_AND_PMIC_MOONFLOWER_MOONFLOWER_CLOCK_DEFS_H_

#include <stdint.h>

enum ClockId : uint8_t {
  P_BI_TCXO = 0,
  P_GPLL0_OUT_EVEN,
  P_GPLL0_OUT_MAIN,
  P_GPLL10_OUT_EVEN,
  P_GPLL10_OUT_MAIN,
  P_GPLL3_OUT_EVEN,
  P_GPLL3_OUT_MAIN,
  P_GPLL4_OUT_EVEN,
  P_GPLL6_OUT_EVEN,
  P_GPLL6_OUT_MAIN,
  P_GPLL7_OUT_EVEN,
  P_GPLL8_OUT_EVEN,
  P_GPLL8_OUT_MAIN,
  P_GPLL9_OUT_EVEN,
  P_GPLL9_OUT_MAIN,
  P_SLEEP_CLK,
};

struct ParentMap {
  constexpr ParentMap(ClockId id, uint8_t cfg) : id(id), cfg(cfg) {}
  const ClockId id;
  const uint8_t cfg;
};

struct FreqConfig {
  constexpr FreqConfig(uint64_t freq, ClockId src_id, float float_pre_div, uint16_t m, uint16_t n,
                       uint64_t src_freq = 0)
      : freq(freq),
        src_id(src_id),
        pre_div(static_cast<uint8_t>((float_pre_div * 2) - 1)),
        m(m),
        n(n),
        src_freq(src_freq) {}

  uint64_t freq;
  ClockId src_id;
  uint8_t pre_div;
  uint16_t m;
  uint16_t n;
  uint64_t src_freq;
};

static constexpr FreqConfig CXO_F{19200000, P_BI_TCXO, 1, 0, 0};
static constexpr FreqConfig ftbl_gcc_qupv3_wrap0_s5_clk_src[] = {
    {7372800, P_GPLL0_OUT_EVEN, 1, 384, 15625},
    {14745600, P_GPLL0_OUT_EVEN, 1, 768, 15625},
    {19200000, P_BI_TCXO, 1, 0, 0},
    {29491200, P_GPLL0_OUT_EVEN, 1, 1536, 15625},
    {32000000, P_GPLL0_OUT_EVEN, 1, 8, 75},
    {48000000, P_GPLL0_OUT_EVEN, 1, 4, 25},
    {64000000, P_GPLL0_OUT_EVEN, 1, 16, 75},
    {75000000, P_GPLL0_OUT_EVEN, 4, 0, 0},
    {80000000, P_GPLL0_OUT_EVEN, 1, 4, 15},
    {96000000, P_GPLL0_OUT_EVEN, 1, 8, 25},
    {100000000, P_GPLL0_OUT_EVEN, 3, 0, 0},
    {102400000, P_GPLL0_OUT_EVEN, 1, 128, 375},
    {112000000, P_GPLL0_OUT_EVEN, 1, 28, 75},
    {117964800, P_GPLL0_OUT_EVEN, 1, 6144, 15625},
    {120000000, P_GPLL0_OUT_EVEN, 2.5, 0, 0},
};

// clang-format off
static constexpr ParentMap gcc_parent_map_0[] = {
    {P_BI_TCXO, 0},
    {P_GPLL0_OUT_MAIN, 1},
    {P_GPLL0_OUT_EVEN, 2},
};

static constexpr ParentMap gcc_parent_map_1[] = {
    {P_BI_TCXO, 0},
    {P_GPLL0_OUT_MAIN, 1},
    {P_GPLL0_OUT_EVEN, 2},
    {P_GPLL6_OUT_EVEN, 4},
};

static constexpr ParentMap gcc_parent_map_2[] = {
    {P_BI_TCXO, 0},
    {P_GPLL0_OUT_MAIN, 1},
    {P_GPLL0_OUT_EVEN, 2},
    {P_SLEEP_CLK, 5},
};

static constexpr ParentMap gcc_parent_map_3[] = {
    {P_BI_TCXO, 0},
    {P_GPLL0_OUT_MAIN, 1},
    {P_GPLL9_OUT_MAIN, 2},
    {P_GPLL10_OUT_EVEN, 3},
    {P_GPLL9_OUT_EVEN, 5},
    {P_GPLL3_OUT_EVEN, 6},
};

static constexpr ParentMap gcc_parent_map_4[] = {
    {P_BI_TCXO, 0},
    {P_GPLL0_OUT_MAIN, 1},
    {P_GPLL0_OUT_EVEN, 2},
    {P_GPLL10_OUT_EVEN, 3},
    {P_GPLL4_OUT_EVEN, 5},
    {P_GPLL3_OUT_MAIN, 6},
};

static constexpr ParentMap gcc_parent_map_5[] = {
    {P_BI_TCXO, 0},
    {P_GPLL0_OUT_MAIN, 1},
    {P_GPLL0_OUT_EVEN, 2},
    {P_GPLL4_OUT_EVEN, 5},
    {P_GPLL3_OUT_EVEN, 6},
};

static constexpr ParentMap gcc_parent_map_6[] = {
    {P_BI_TCXO, 0},
    {P_GPLL0_OUT_MAIN, 1},
    {P_GPLL8_OUT_MAIN, 2},
    {P_GPLL10_OUT_EVEN, 3},
    {P_GPLL8_OUT_EVEN, 4},
    {P_GPLL9_OUT_EVEN, 5},
    {P_GPLL3_OUT_MAIN, 6},
};

static constexpr ParentMap gcc_parent_map_7[] = {
    {P_BI_TCXO, 0},
    {P_GPLL0_OUT_MAIN, 1},
    {P_GPLL8_OUT_MAIN, 2},
    {P_GPLL10_OUT_MAIN, 3},
    {P_GPLL8_OUT_EVEN, 4},
    {P_GPLL9_OUT_EVEN, 5},
    {P_GPLL3_OUT_EVEN, 6},
};

static constexpr ParentMap gcc_parent_map_8[] = {
    {P_BI_TCXO, 0},
    {P_GPLL0_OUT_MAIN, 1},
    {P_GPLL8_OUT_MAIN, 2},
    {P_GPLL10_OUT_EVEN, 3},
    {P_GPLL6_OUT_EVEN, 4},
    {P_GPLL9_OUT_EVEN, 5},
    {P_GPLL3_OUT_MAIN, 6},
};

static constexpr ParentMap gcc_parent_map_9[] = {
    {P_BI_TCXO, 0},
    {P_GPLL0_OUT_MAIN, 1},
    {P_GPLL0_OUT_EVEN, 2},
    {P_GPLL10_OUT_EVEN, 3},
    {P_GPLL8_OUT_EVEN, 4},
    {P_GPLL9_OUT_EVEN, 5},
    {P_GPLL3_OUT_MAIN, 6},
};

static constexpr ParentMap gcc_parent_map_10[] = {
    {P_BI_TCXO, 0},
    {P_GPLL0_OUT_MAIN, 1},
    {P_GPLL8_OUT_MAIN, 2},
    {P_GPLL10_OUT_EVEN, 3},
    {P_GPLL6_OUT_MAIN, 5},
    {P_GPLL3_OUT_EVEN, 6},
};

static constexpr ParentMap gcc_parent_map_11[] = {
    {P_BI_TCXO, 0},
    {P_GPLL0_OUT_MAIN, 1},
    {P_GPLL0_OUT_EVEN, 2},
    {P_GPLL7_OUT_EVEN, 3},
    {P_GPLL4_OUT_EVEN, 5},
};

static constexpr ParentMap gcc_parent_map_12[] = {
    {P_BI_TCXO, 0},
    {P_GPLL0_OUT_MAIN, 1},
    {P_GPLL6_OUT_MAIN, 5},
};
// clang-format on

#endif  // ZIRCON_KERNEL_DEV_CLOCKS_AND_PMIC_MOONFLOWER_MOONFLOWER_CLOCK_DEFS_H_
