// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <zircon/types.h>

#include <array>
#include <iterator>

#include "aml-fclk.h"

#define FCLK_PLL_RATE(_r, _premux, _postmux, _mux_div) \
  {                                                    \
      .rate = (_r),                                    \
      .premux = (_premux),                             \
      .postmux = (_postmux),                           \
      .mux_div = (_mux_div),                           \
  }

/*Fix pll rate table*/
static const aml_fclk_rate_table_t fclk_pll_rate_table[] = {
    FCLK_PLL_RATE(100000000, 1, 1, 9),  FCLK_PLL_RATE(250000000, 1, 1, 3),
    FCLK_PLL_RATE(500000000, 1, 1, 1),  FCLK_PLL_RATE(667000000, 2, 0, 0),
    FCLK_PLL_RATE(1000000000, 1, 0, 0),
};

const aml_fclk_rate_table_t* s905d2_fclk_get_rate_table() { return fclk_pll_rate_table; }

size_t s905d2_fclk_get_rate_table_count() { return std::size(fclk_pll_rate_table); }

// Fixed PLL: DCO = 1536M, FCLK_DIV2 = 768M, FCLK_DIV3 = 512M
static const aml_fclk_rate_table_t a1_fclk_pll_rate_table[] = {
    FCLK_PLL_RATE(128'000'000, 2, 1, 3),  // FCLK_DIV3 source
    FCLK_PLL_RATE(256'000'000, 2, 1, 1),  // FCLK_DIV3 source
    FCLK_PLL_RATE(512'000'000, 2, 0, 0),  // FCLK_DIV3 source
    FCLK_PLL_RATE(768'000'000, 1, 0, 0),  // FCLK_DIV2 source
};

const aml_fclk_rate_table_t* a1_fclk_get_rate_table() { return a1_fclk_pll_rate_table; }

size_t a1_fclk_get_rate_table_count() { return std::size(a1_fclk_pll_rate_table); }
