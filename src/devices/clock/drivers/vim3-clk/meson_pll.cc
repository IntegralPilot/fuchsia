// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "meson_pll.h"

#include <hwreg/bitfields.h>

#include "aml-fclk.h"

namespace vim3_clock {

class SysCpuClkControl : public hwreg::RegisterBase<SysCpuClkControl, uint32_t> {
 public:
  DEF_BIT(29, busy_cnt);
  DEF_BIT(28, busy);
  DEF_BIT(26, dyn_enable);
  DEF_FIELD(25, 20, mux1_divn_tcnt);
  DEF_BIT(18, postmux1);
  DEF_FIELD(17, 16, premux1);
  DEF_BIT(15, manual_mux_mode);
  DEF_BIT(14, manual_mode_post);
  DEF_BIT(13, manual_mode_pre);
  DEF_BIT(12, force_update_t);
  DEF_BIT(11, final_mux_sel);
  DEF_BIT(10, final_dyn_mux_sel);
  DEF_FIELD(9, 4, mux0_divn_tcnt);
  DEF_BIT(3, rev);
  DEF_BIT(2, postmux0);
  DEF_FIELD(1, 0, premux0);

  static auto Get(uint32_t offset) { return hwreg::RegisterAddr<SysCpuClkControl>(offset); }
};

void MesonPllClock::Init() {
  const hhi_pll_rate_t* rate_table = nullptr;
  size_t rate_table_size = 0;

  s905d2_pll_init_etc(hiudev_, &pll_, pll_num_);

  rate_table = s905d2_pll_get_rate_table(pll_num_);
  rate_table_size = s905d2_get_rate_table_count(pll_num_);

  // Make sure that the rate table is sorted in strictly ascending order.
  for (size_t i = 0; i < rate_table_size - 1; i++) {
    ZX_ASSERT(rate_table[i].rate < rate_table[i + 1].rate);
  }
}

zx_status_t MesonPllClock::SetRate(const uint32_t hz) { return s905d2_pll_set_rate(&pll_, hz); }

zx_status_t MesonPllClock::QuerySupportedRate(const uint64_t max_rate, uint64_t* result) {
  // Find the largest rate that does not exceed `max_rate`

  // Start by getting the rate tables.
  const hhi_pll_rate_t* rate_table = nullptr;
  size_t rate_table_size = 0;
  const hhi_pll_rate_t* best_rate = nullptr;

  rate_table = s905d2_pll_get_rate_table(pll_num_);
  rate_table_size = s905d2_get_rate_table_count(pll_num_);

  // The rate table is already sorted in ascending order so pick the largest
  // element that does not exceed max_rate.
  for (size_t i = 0; i < rate_table_size; i++) {
    if (rate_table[i].rate <= max_rate) {
      best_rate = &rate_table[i];
    } else {
      break;
    }
  }

  if (best_rate == nullptr) {
    return ZX_ERR_NOT_FOUND;
  }

  *result = best_rate->rate;
  return ZX_OK;
}

zx_status_t MesonPllClock::GetRate(uint64_t* result) { return ZX_ERR_NOT_SUPPORTED; }

zx_status_t MesonPllClock::Toggle(const bool enable) {
  if (enable) {
    return s905d2_pll_ena(&pll_);
  }

  s905d2_pll_disable(&pll_);
  return ZX_OK;
}

zx_status_t MesonCpuClock::SetRate(const uint32_t hz) {
  zx_status_t status;

  if (hz > kFrequencyThresholdHz && current_rate_hz_ > kFrequencyThresholdHz) {
    // Switching between two frequencies both higher than 1GHz.
    // In this case, as per the datasheet it is recommended to change
    // to a frequency lower than 1GHz first and then switch to higher
    // frequency to avoid glitches.

    // Let's first switch to 1GHz
    status = SetRate(kFrequencyThresholdHz);
    if (status != ZX_OK) {
      return status;
    }

    // Now let's set SYS_PLL rate to hz.
    status = ConfigureSysPLL(hz);

  } else if (hz > kFrequencyThresholdHz && current_rate_hz_ <= kFrequencyThresholdHz) {
    // Switching from a frequency lower than 1GHz to one greater than 1GHz.
    // In this case we just need to set the SYS_PLL to required rate and
    // then set the final mux to 1 (to select SYS_PLL as the source.)

    // Now let's set SYS_PLL rate to hz.
    status = ConfigureSysPLL(hz);

  } else {
    // Switching between two frequencies below 1GHz.
    // In this case we change the source and dividers accordingly
    // to get the required rate from MPLL and do not touch the
    // final mux.
    status = ConfigCpuFixedPll(hz);
  }

  if (status == ZX_OK) {
    current_rate_hz_ = hz;
  }

  return status;
}

zx_status_t MesonCpuClock::QuerySupportedRate(const uint64_t max_rate, uint64_t* result) {
  // Cpu Clock supported rates fall into two categories based on whether they're below
  // or above the 1GHz threshold. This method scans both the syspll and the fclk to
  // determine the maximum rate that does not exceed `max_rate`.
  uint64_t syspll_rate = 0;
  uint64_t fclk_rate = 0;
  zx_status_t syspll_status = ZX_ERR_NOT_FOUND;
  zx_status_t fclk_status = ZX_ERR_NOT_FOUND;

  syspll_status = sys_pll_->QuerySupportedRate(max_rate, &syspll_rate);

  const aml_fclk_rate_table_t* fclk_rate_table = s905d2_fclk_get_rate_table();
  size_t rate_count = s905d2_fclk_get_rate_table_count();

  for (size_t i = 0; i < rate_count; i++) {
    if (fclk_rate_table[i].rate > fclk_rate && fclk_rate_table[i].rate <= max_rate) {
      fclk_rate = fclk_rate_table[i].rate;
      fclk_status = ZX_OK;
    }
  }

  // 4 cases: rate supported by syspll only, rate supported by fclk only
  //          rate supported by neither or rate supported by both.
  if (syspll_status == ZX_OK && fclk_status != ZX_OK) {
    // Case 1
    *result = syspll_rate;
    return ZX_OK;
  } else if (syspll_status != ZX_OK && fclk_status == ZX_OK) {
    // Case 2
    *result = fclk_rate;
    return ZX_OK;
  } else if (syspll_status != ZX_OK && fclk_status != ZX_OK) {
    // Case 3
    return ZX_ERR_NOT_FOUND;
  }

  // Case 4
  if (syspll_rate > kFrequencyThresholdHz) {
    *result = syspll_rate;
  } else {
    *result = fclk_rate;
  }
  return ZX_OK;
}

zx_status_t MesonCpuClock::GetRate(uint64_t* result) {
  if (result == nullptr) {
    return ZX_ERR_INVALID_ARGS;
  }

  *result = current_rate_hz_;
  return ZX_OK;
}

// NOTE: This block doesn't modify the MPLL, it just programs the muxes &
// dividers to get the new_rate in the sys_pll_div block. Refer fig. 6.6 Multi
// Phase PLLS for A53 & A73 in the datasheet.
zx_status_t MesonCpuClock::ConfigCpuFixedPll(const uint32_t new_rate) {
  const aml_fclk_rate_table_t* fclk_rate_table;
  size_t rate_count;
  size_t i;

  fclk_rate_table = s905d2_fclk_get_rate_table();
  rate_count = s905d2_fclk_get_rate_table_count();

  // Validate if the new_rate is available
  for (i = 0; i < rate_count; i++) {
    if (new_rate == fclk_rate_table[i].rate) {
      break;
    }
  }
  if (i == rate_count) {
    return ZX_ERR_NOT_SUPPORTED;
  }

  zx_status_t status = WaitForBusyCpu();
  if (status != ZX_OK) {
    return status;
  }

  auto sys_cpu_ctrl0 = SysCpuClkControl::Get(offset_).ReadFrom(&*hiu_);

  if (sys_cpu_ctrl0.final_dyn_mux_sel()) {
    // Dynamic mux 1 is in use, we setup dynamic mux 0
    sys_cpu_ctrl0.set_final_dyn_mux_sel(0)
        .set_mux0_divn_tcnt(fclk_rate_table[i].mux_div)
        .set_postmux0(fclk_rate_table[i].postmux)
        .set_premux0(fclk_rate_table[i].premux);
  } else {
    // Dynamic mux 0 is in use, we setup dynamic mux 1
    sys_cpu_ctrl0.set_final_dyn_mux_sel(1)
        .set_mux1_divn_tcnt(fclk_rate_table[i].mux_div)
        .set_postmux1(fclk_rate_table[i].postmux)
        .set_premux1(fclk_rate_table[i].premux);
  }

  // Select the final mux.
  sys_cpu_ctrl0.set_final_mux_sel(kFixedPll).WriteTo(&*hiu_);

  return ZX_OK;
}

zx_status_t MesonCpuClock::ConfigureSysPLL(uint32_t new_rate) {
  // This API also validates if the new_rate is valid.
  // So no need to validate it here.
  zx_status_t status = sys_pll_->SetRate(new_rate);
  if (status != ZX_OK) {
    return status;
  }

  // Now we need to change the final mux to select input as SYS_PLL.
  status = WaitForBusyCpu();
  if (status != ZX_OK) {
    return status;
  }

  // Select the final mux.
  auto sys_cpu_ctrl0 = SysCpuClkControl::Get(offset_).ReadFrom(&*hiu_);
  sys_cpu_ctrl0.set_final_mux_sel(kSysPll).WriteTo(&*hiu_);

  return status;
}

zx_status_t MesonCpuClock::WaitForBusyCpu() {
  auto sys_cpu_ctrl0 = SysCpuClkControl::Get(offset_).ReadFrom(&*hiu_);

  // Wait till we are not busy.
  for (uint32_t i = 0; i < kSysCpuWaitBusyRetries; i++) {
    sys_cpu_ctrl0 = SysCpuClkControl::Get(offset_).ReadFrom(&*hiu_);

    if (sys_cpu_ctrl0.busy()) {
      // Wait a little bit before trying again.
      zx_nanosleep(zx_deadline_after(ZX_USEC(kSysCpuWaitBusyTimeoutUs)));
      continue;
    } else {
      return ZX_OK;
    }
  }
  return ZX_ERR_TIMED_OUT;
}

}  // namespace vim3_clock
