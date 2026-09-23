// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <stdint.h>
#include <stdlib.h>
#include <threads.h>
#include <unistd.h>
#include <zircon/assert.h>
#include <zircon/types.h>

#include <bits/limits.h>
#include <soc/aml-a113/a113-clocks.h>

#define DIV_ROUND_UP(n, d) ((n + d - 1) / d)

namespace {
void a113_clk_update_reg(fdf::MmioBuffer *dev, uint32_t offset, uint32_t pos, uint32_t bits,
                         uint32_t value) {
  uint32_t reg = a113_clk_get_reg(dev, offset);
  reg &= ~(((1 << bits) - 1) << pos);
  reg |= (value & ((1 << bits) - 1)) << pos;
  a113_clk_set_reg(dev, offset, reg);
}
}  // namespace

zx_status_t a113_clk_set_mpll2(fdf::MmioBuffer *device, uint64_t rate, uint64_t *actual) {
  /* Overall ratio is reference/(n + sdm/16384)
     In this case the 2.0GHz fixed rate pll is the reference
  */
  uint64_t n = A113_FIXED_PLL_RATE / rate;  // calculate the integer ratio;
  ZX_DEBUG_ASSERT(n < (1 << 9));

  uint64_t sdm = DIV_ROUND_UP((A113_FIXED_PLL_RATE - n * rate) * SDM_FRACTIONALITY, rate);
  ZX_DEBUG_ASSERT(sdm < (1 << 14));
  a113_clk_update_reg(device, A113_HHI_MPLL_CNTL8, 0, 14, static_cast<uint32_t>(sdm));
  a113_clk_update_reg(device, A113_HHI_MPLL_CNTL8, 16, 9, static_cast<uint32_t>(n));

  // Enable sdm divider
  a113_clk_update_reg(device, A113_HHI_MPLL_CNTL8, 15, 1, 1);
  // Enable mpll2
  a113_clk_update_reg(device, A113_HHI_MPLL_CNTL8, 14, 1, 1);
  // Gate mpll2 through to rest of system
  a113_clk_update_reg(device, A113_HHI_PLL_TOP_MISC, 2, 1, 1);

  if (actual) {
    *actual = (static_cast<uint64_t>(SDM_FRACTIONALITY) * A113_FIXED_PLL_RATE) /
              ((SDM_FRACTIONALITY * n) + sdm);
  }

  return ZX_OK;
}
