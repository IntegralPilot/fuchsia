// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/driver/mmio/cpp/mmio-buffer.h>
#include <lib/driver/mmio/cpp/mmio-view.h>
#include <string.h>
#include <zircon/assert.h>
#include <zircon/status.h>
#include <zircon/syscalls.h>
#include <zircon/types.h>

#include <utility>

#include <fbl/algorithm.h>
#include <soc/aml-s905d2/s905d2-hiu-regs.h>
#include <soc/aml-s905d2/s905d2-hiu.h>
#include <soc/aml-s905d2/s905d2-hw.h>

static inline uint32_t hiu_clk_get_reg(fdf::MmioBuffer* dev, uint32_t offset) {
  return dev->Read32(offset);
}

static inline uint32_t hiu_clk_set_reg(fdf::MmioBuffer* dev, uint32_t offset, uint32_t value) {
  dev->Write32(value, offset);
  return hiu_clk_get_reg(dev, offset);
}

static inline uint32_t hiu_get_pll_offs(aml_pll_dev_t* pll_dev) {
  switch (pll_dev->pll_num) {
    case GP0_PLL:
      return HHI_GP0_PLL_CNTL0;
    case PCIE_PLL:
      return HHI_PCIE_PLL_CNTL0;
    case HIFI_PLL:
      return HHI_HIFI_PLL_CNTL0;
    case SYS_PLL:
      return HHI_SYS_PLL_CNTL0;
    case SYS1_PLL:
      return HHI_SYS1_PLL_CNTL0;
    default:
      ZX_DEBUG_ASSERT(0);
  }
  return 0;
}

zx_status_t s905d2_hiu_init(zx_handle_t mmio_resource, fdf::MmioBuffer* device) {
  const size_t vmo_size = fbl::round_up<size_t>(S905D2_HIU_LENGTH, zx_system_get_page_size());
  zx::vmo vmo;
  zx_status_t status =
      zx::vmo::create_physical(zx::resource(mmio_resource), S905D2_HIU_BASE, vmo_size, &vmo);
  if (status != ZX_OK) {
    return status;
  }
  zx::result<fdf::MmioBuffer> mmio = fdf::MmioBuffer::Create(0, S905D2_HIU_LENGTH, std::move(vmo),
                                                             ZX_CACHE_POLICY_UNCACHED_DEVICE);
  if (mmio.is_error()) {
    return mmio.status_value();
  }

  *device = std::move(*mmio);
  return ZX_OK;
}

zx_status_t s905d2_hiu_init_etc(fdf::MmioBuffer* device, fdf::MmioView hiubase) {
  *device = std::move(hiubase);
  return ZX_OK;
}

static zx_status_t s905d2_pll_init_regs(aml_pll_dev_t* pll_dev) {
  fdf::MmioBuffer* device = pll_dev->hiu;

  if (pll_dev->pll_num == HIFI_PLL) {
    // write config values
    hiu_clk_set_reg(device, HHI_HIFI_PLL_CNTL1, G12A_HIFI_PLL_CNTL1);
    hiu_clk_set_reg(device, HHI_HIFI_PLL_CNTL2, G12A_HIFI_PLL_CNTL2);
    hiu_clk_set_reg(device, HHI_HIFI_PLL_CNTL3, G12A_HIFI_PLL_CNTL3);
    hiu_clk_set_reg(device, HHI_HIFI_PLL_CNTL4, G12A_HIFI_PLL_CNTL4);
    hiu_clk_set_reg(device, HHI_HIFI_PLL_CNTL5, G12A_HIFI_PLL_CNTL5);
    hiu_clk_set_reg(device, HHI_HIFI_PLL_CNTL6, G12A_HIFI_PLL_CNTL6);
    zx_nanosleep(zx_deadline_after(ZX_USEC(10)));
    return ZX_OK;
  } else if (pll_dev->pll_num == SYS_PLL) {
    // write config values
    hiu_clk_set_reg(device, HHI_SYS_PLL_CNTL1, G12A_SYS_PLL_CNTL1);
    hiu_clk_set_reg(device, HHI_SYS_PLL_CNTL2, G12A_SYS_PLL_CNTL2);
    hiu_clk_set_reg(device, HHI_SYS_PLL_CNTL3, G12A_SYS_PLL_CNTL3);
    hiu_clk_set_reg(device, HHI_SYS_PLL_CNTL4, G12A_SYS_PLL_CNTL4);
    hiu_clk_set_reg(device, HHI_SYS_PLL_CNTL5, G12A_SYS_PLL_CNTL5);
    hiu_clk_set_reg(device, HHI_SYS_PLL_CNTL6, G12A_SYS_PLL_CNTL6);
    zx_nanosleep(zx_deadline_after(ZX_USEC(10)));
    return ZX_OK;
  } else if (pll_dev->pll_num == SYS1_PLL) {
    // write config values
    hiu_clk_set_reg(device, HHI_SYS1_PLL_CNTL1, G12A_SYS1_PLL_CNTL1);
    hiu_clk_set_reg(device, HHI_SYS1_PLL_CNTL2, G12A_SYS1_PLL_CNTL2);
    hiu_clk_set_reg(device, HHI_SYS1_PLL_CNTL3, G12A_SYS1_PLL_CNTL3);
    hiu_clk_set_reg(device, HHI_SYS1_PLL_CNTL4, G12A_SYS1_PLL_CNTL4);
    hiu_clk_set_reg(device, HHI_SYS1_PLL_CNTL5, G12A_SYS1_PLL_CNTL5);
    hiu_clk_set_reg(device, HHI_SYS1_PLL_CNTL6, G12A_SYS1_PLL_CNTL6);
    zx_nanosleep(zx_deadline_after(ZX_USEC(10)));
    return ZX_OK;
  } else if (pll_dev->pll_num == GP0_PLL) {
    // write config values
    hiu_clk_set_reg(device, HHI_GP0_PLL_CNTL1, G12A_GP0_PLL_CNTL1);
    hiu_clk_set_reg(device, HHI_GP0_PLL_CNTL2, G12A_GP0_PLL_CNTL2);
    hiu_clk_set_reg(device, HHI_GP0_PLL_CNTL3, G12A_GP0_PLL_CNTL3);
    hiu_clk_set_reg(device, HHI_GP0_PLL_CNTL4, G12A_GP0_PLL_CNTL4);
    hiu_clk_set_reg(device, HHI_GP0_PLL_CNTL5, G12A_GP0_PLL_CNTL5);
    hiu_clk_set_reg(device, HHI_GP0_PLL_CNTL6, G12A_GP0_PLL_CNTL6);
    zx_nanosleep(zx_deadline_after(ZX_USEC(10)));
    return ZX_OK;
  } else if (pll_dev->pll_num == PCIE_PLL) {
    // write config values
    hiu_clk_set_reg(device, HHI_PCIE_PLL_CNTL0, G12A_PCIE_PLL_CNTL0_0);
    hiu_clk_set_reg(device, HHI_PCIE_PLL_CNTL0, G12A_PCIE_PLL_CNTL0_1);
    hiu_clk_set_reg(device, HHI_PCIE_PLL_CNTL1, G12A_PCIE_PLL_CNTL1);
    hiu_clk_set_reg(device, HHI_PCIE_PLL_CNTL2, G12A_PCIE_PLL_CNTL2_0);
    hiu_clk_set_reg(device, HHI_PCIE_PLL_CNTL3, G12A_PCIE_PLL_CNTL3);
    hiu_clk_set_reg(device, HHI_PCIE_PLL_CNTL4, G12A_PCIE_PLL_CNTL4_0);
    hiu_clk_set_reg(device, HHI_PCIE_PLL_CNTL5, G12A_PCIE_PLL_CNTL5_0);
    hiu_clk_set_reg(device, HHI_PCIE_PLL_CNTL5, G12A_PCIE_PLL_CNTL5_1);
    zx_nanosleep(zx_deadline_after(ZX_USEC(20)));
    hiu_clk_set_reg(device, HHI_PCIE_PLL_CNTL4, G12A_PCIE_PLL_CNTL4_1);
    zx_nanosleep(zx_deadline_after(ZX_USEC(10)));
    hiu_clk_set_reg(device, HHI_PCIE_PLL_CNTL0, G12A_PCIE_PLL_CNTL0_2);
    hiu_clk_set_reg(device, HHI_PCIE_PLL_CNTL0, G12A_PCIE_PLL_CNTL0_3);
    zx_nanosleep(zx_deadline_after(ZX_USEC(10)));
    hiu_clk_set_reg(device, HHI_PCIE_PLL_CNTL2, G12A_PCIE_PLL_CNTL2_1);
  }
  return ZX_ERR_NOT_SUPPORTED;
}

void s905d2_pll_init_etc(fdf::MmioBuffer* device, aml_pll_dev_t* pll_dev, hhi_plls_t pll_num) {
  ZX_DEBUG_ASSERT(device);
  ZX_DEBUG_ASSERT(pll_dev);

  pll_dev->hiu = device;

  pll_dev->rate_table = s905d2_pll_get_rate_table(pll_num);
  pll_dev->rate_idx = 0;
  pll_dev->frequency = 0;
  pll_dev->pll_num = pll_num;
  pll_dev->rate_count = s905d2_get_rate_table_count(pll_num);

  ZX_DEBUG_ASSERT(pll_dev->rate_table);
  ZX_DEBUG_ASSERT(pll_dev->rate_count);
}

zx_status_t s905d2_pll_init(fdf::MmioBuffer* device, aml_pll_dev_t* pll_dev, hhi_plls_t pll_num) {
  ZX_DEBUG_ASSERT(device);
  ZX_DEBUG_ASSERT(pll_dev);

  s905d2_pll_init_etc(device, pll_dev, pll_num);

  // Disable and reset the pll
  s905d2_pll_disable(pll_dev);
  // Write configuration registers
  return s905d2_pll_init_regs(pll_dev);
}

bool s905d2_pll_disable(aml_pll_dev_t* pll_dev) {
  uint32_t offs = hiu_get_pll_offs(pll_dev);
  uint32_t ctl0 = hiu_clk_get_reg(pll_dev->hiu, offs);

  bool retval = ctl0 & HHI_PLL_CNTL0_EN;

  ctl0 = (ctl0 & ~HHI_PLL_CNTL0_EN) | HHI_PLL_CNTL0_RESET;
  hiu_clk_set_reg(pll_dev->hiu, offs, ctl0);

  return retval;
}

zx_status_t s905d2_pll_ena(aml_pll_dev_t* pll_dev) {
  ZX_DEBUG_ASSERT(pll_dev);

  uint32_t offs = hiu_get_pll_offs(pll_dev);
  uint32_t reg_val = hiu_clk_get_reg(pll_dev->hiu, offs);

  if (reg_val & HHI_PLL_CNTL0_EN) {
    // Already enabled. Return early.
    return ZX_OK;
  }

  // Set Enable bit
  reg_val |= HHI_PLL_CNTL0_EN;
  hiu_clk_set_reg(pll_dev->hiu, offs, reg_val);
  zx_nanosleep(zx_deadline_after(ZX_USEC(50)));

  // Clear Reset bit
  reg_val &= ~HHI_PLL_CNTL0_RESET;
  hiu_clk_set_reg(pll_dev->hiu, offs, reg_val);

  uint32_t wait_count = 100;
  while (wait_count) {
    if (hiu_clk_get_reg(pll_dev->hiu, offs) & HHI_PLL_LOCK) {
      return ZX_OK;
    }
    zx_nanosleep(zx_deadline_after(ZX_USEC(10)));
    wait_count--;
  }

  return ZX_ERR_TIMED_OUT;
}

/* Notes:
    -VCO needs to be between 3-6GHz per the datasheet. It appears that if you
    provide values which would result in a VCO outside of this range, it will
    still oscillate, but at unknown (but likely close to target) frequency.
*/
zx_status_t s905d2_pll_set_rate(aml_pll_dev_t* pll_dev, uint64_t freq) {
  ZX_DEBUG_ASSERT(pll_dev);

  const hhi_pll_rate_t* pll_rate;

  zx_status_t status = s905d2_pll_fetch_rate(pll_dev, freq, &pll_rate);
  if (status != ZX_OK) {
    return status;
  }
  // Disable/reset the pll, save previous state
  bool ena = s905d2_pll_disable(pll_dev);

  // Initialize the registers to defaults (may not be retained after reset)
  s905d2_pll_init_regs(pll_dev);

  // The rate for PCIE PLL is fixed. No need to change any control regs, so return early.
  if (pll_dev->pll_num == PCIE_PLL) {
    return ZX_OK;
  }

  uint32_t offs = hiu_get_pll_offs(pll_dev);
  uint32_t ctl0 = hiu_clk_get_reg(pll_dev->hiu, offs);

  ctl0 &= ~HHI_PLL_CNTL0_M;
  ctl0 |= pll_rate->m << HHI_PLL_CNTL0_M_SHIFT;

  ctl0 &= ~HHI_PLL_CNTL0_N;
  ctl0 |= pll_rate->n << HHI_PLL_CNTL0_N_SHIFT;

  ctl0 &= ~HHI_PLL_CNTL0_OD;
  ctl0 |= pll_rate->od << HHI_PLL_CNTL0_OD_SHIFT;

  hiu_clk_set_reg(pll_dev->hiu, offs, ctl0);

  hiu_clk_set_reg(pll_dev->hiu, offs + 4, pll_rate->frac);

  if (ena) {
    return s905d2_pll_ena(pll_dev);
  }

  return ZX_OK;
}
