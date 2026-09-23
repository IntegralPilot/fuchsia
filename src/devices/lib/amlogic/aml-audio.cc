// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <zircon/assert.h>

#include <soc/aml-common/aml-audio-regs.h>

// It follows up `S905D2 Datasheet`
// - Page908(EE_AUDIO_MCLK_A_CTRL)
// - Page920(EE_AUDIO_CLK_PDMIN_CTRL0)
// - Page920(EE_AUDIO_CLK_PDMIN_CTRL1)
// and also`A311D Datasheet`
// - Page1174(EE_AUDIO_MCLK_A_CTRL)
// - Page1185(EE_AUDIO_CLK_PDMIN_CTRL0)
// - Page1186(EE_AUDIO_CLK_PDMIN_CTRL1)
uint8_t ToS905D2AndA311DAudioClkSrcSel(ee_audio_mclk_src_t clk_src) {
  switch (clk_src) {
    case MP0_PLL:
      return 0;
    case MP1_PLL:
      return 1;
    case MP2_PLL:
      return 2;
    case MP3_PLL:
      return 3;
    case HIFI_PLL:
      return 4;
    case FCLK_DIV3:
      return 5;
    case FCLK_DIV4:
      return 6;
    case GP0_PLL:
      return 7;
    default:
      break;
  }
  ZX_PANIC("Unsupported clk_src: %d", clk_src);
}

// It follows up `S905D3G Datasheet`
// - Page814(EE_AUDIO_MCLK_A_CTRL)
// - Page823(EE_AUDIO_CLK_PDMIN_CTRL0)
// - Page823(EE_AUDIO_CLK_PDMIN_CTRL1)
uint8_t ToS905D3GAudioClkSrcSel(ee_audio_mclk_src_t clk_src) {
  switch (clk_src) {
    case MP0_PLL:
      return 0;
    case MP1_PLL:
      return 1;
    case MP2_PLL:
      return 2;
    case MP3_PLL:
      return 3;
    case HIFI_PLL:
      return 4;
    case FCLK_DIV3:
      return 5;
    case FCLK_DIV4:
      return 6;
    default:
      break;
  }
  ZX_PANIC("Unsupport clk_src: %d", clk_src);
}

// It follows up `A113X2 Datasheet`
// - Page293(EE_AUDIO_MCLK_A_CTRL)
// - Page375(EE_AUDIO2_CLK_PDMIN_CRTL0)
// - Page375(EE_AUDIO2_CLK_PDMIN_CTRL1)
uint8_t ToA5AudioClkSrcSel(ee_audio_mclk_src_t clk_src) {
  switch (clk_src) {
    case MP0_PLL:
      return 0;
    case MP1_PLL:
      return 1;
    case MP2_PLL:
      return 2;
    case HIFI_PLL:
      return 4;
    case FCLK_DIV4:
      return 6;
    default:
      break;
  }
  ZX_PANIC("Unsupport clk_src: %d", clk_src);
}

// It follows up `A113L Datasheet`
// - Page153(EE_AUDIO_MCLK_A_CTRL)
// - Page224(EE_AUDIO2_CLK_PDMIN_CRTL0)
// - Page224(EE_AUDIO2_CLK_PDMIN_CTRL1)
uint8_t ToA1AudioClkSrcSel(ee_audio_mclk_src_t clk_src) {
  switch (clk_src) {
    case HIFI_PLL:
      return 3;
    default:
      break;
  }
  ZX_PANIC("Unsupport clk_src: %d", clk_src);
}
