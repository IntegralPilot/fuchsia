// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "meson-cpu-clock.h"

#include <lib/ddk/platform-defs.h>
#include <lib/driver/logging/cpp/logger.h>
#include <zircon/syscalls/smc.h>

#include <hwreg/bitfields.h>

#include "src/devices/clock/drivers/amlogic-clk/aml-fclk.h"

namespace amlogic_clock {

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

zx_status_t MesonCpuClock::SecSetClk(uint32_t func_id, uint64_t arg1, uint64_t arg2, uint64_t arg3,
                                     uint64_t arg4, uint64_t arg5, uint64_t arg6) {
  zx_status_t status;

  zx_smc_parameters_t smc_params = {
      .func_id = func_id,
      .arg1 = arg1,
      .arg2 = arg2,
      .arg3 = arg3,
      .arg4 = arg4,
      .arg5 = arg5,
      .arg6 = arg6,
  };

  zx_smc_result_t smc_result;
  status = zx_smc_call(smc_.get(), &smc_params, &smc_result);
  if (status != ZX_OK) {
    fdf::error("zx_smc_call failed: {}", zx_status_get_string(status));
  }

  return status;
}

zx_status_t MesonCpuClock::SecSetCpuClkMux(uint64_t clock_source) {
  zx_status_t status = ZX_OK;

  status = SecSetClk(kSecureCpuClk, static_cast<uint64_t>(SecPll::kSecidCpuClkSel),
                     kFinalMuxSelMask, clock_source, 0, 0, 0);
  if (status != ZX_OK) {
    fdf::error("kSecidCpuClkSel failed: {}", zx_status_get_string(status));
  }
  return status;
}

zx_status_t MesonCpuClock::SecSetSys0DcoPll(const pll_params_table& pll_params) {
  zx_status_t status = ZX_OK;

  status = SecSetClk(kSecurePllClk, static_cast<uint64_t>(SecPll::kSecidSys0DcoPll), pll_params.m,
                     pll_params.n, pll_params.od, 0, 0);
  if (status != ZX_OK) {
    fdf::error("kSecidSys0DcoPll failed: {}", zx_status_get_string(status));
  }
  return status;
}

zx_status_t MesonCpuClock::SecSetCpuClkDyn(const cpu_dyn_table& dyn_params) {
  zx_status_t status = ZX_OK;

  status = SecSetClk(kSecureCpuClk, static_cast<uint64_t>(SecPll::kSecidCpuClkDyn),
                     dyn_params.dyn_pre_mux, dyn_params.dyn_post_mux, dyn_params.dyn_div, 0, 0);
  if (status != ZX_OK) {
    fdf::error("kSecidCpuClkDyn failed: {}", zx_status_get_string(status));
  }
  return status;
}

zx_status_t MesonCpuClock::SetRateA5(const uint32_t hz) {
  zx_status_t status;

  // CPU clock tree: sys_pll(high clock source), final_dyn_mux(low clock source)
  //
  // cts_osc_clk ->|->premux0->|->mux0_divn->|->postmux0->|
  // fclk_div2   ->|           |  -------->  |            |->final_dyn_mux->|
  // fclk_div3   ->|->premux1->|->mux1_divn->|->postmux1->|                 |
  // fclk_div2p5 ->|           |  -------->  |            |                 |->final_mux->cpu_clk
  //                                                                        |
  // sys_pll     ->|            -------------------->                       |
  //
  if (hz > kFrequencyThresholdHz) {
    auto rate = std::ranges::find_if(a5_sys_pll_params_table,
                                     [hz](const pll_params_table& a) { return a.rate == hz; });
    if (rate == std::end(a5_sys_pll_params_table)) {
      fdf::error("Invalid cpu freq");
      return ZX_ERR_NOT_SUPPORTED;
    }

    // Switch to low freq source(cpu_dyn)
    status = SecSetCpuClkMux(kFinalMuxSelCpuDyn);
    if (status != ZX_OK) {
      fdf::error("SecSetCpuClkMux failed: {}", zx_status_get_string(status));
      return status;
    }

    // Set clock by sys_pll
    status = SecSetSys0DcoPll(*rate);
    if (status != ZX_OK) {
      fdf::error("SecSetSys0DcoPll failed: {}", zx_status_get_string(status));
      return status;
    }

    // Switch to high freq source(sys_pll)
    status = SecSetCpuClkMux(kFinalMuxSelSysPll);
    if (status != ZX_OK) {
      fdf::error("SecSetCpuClkMux failed: {}", zx_status_get_string(status));
    }
  } else {
    auto rate = std::ranges::find_if(a5_cpu_dyn_table,
                                     [hz](const cpu_dyn_table& a) { return a.rate == hz; });
    if (rate == std::end(a5_cpu_dyn_table)) {
      fdf::error("Invalid cpu freq");
      return ZX_ERR_NOT_SUPPORTED;
    }

    // Set clock by cpu_dyn
    status = SecSetCpuClkDyn(*rate);
    if (status != ZX_OK) {
      fdf::error("SecSetCpuClkDyn failed: {}", zx_status_get_string(status));
      return status;
    }

    // Switch to low freq source(cpu_dyn)
    status = SecSetCpuClkMux(kFinalMuxSelCpuDyn);
    if (status != ZX_OK) {
      fdf::error("SecSetCpuClkMux failed: {}", zx_status_get_string(status));
    }
  }

  return status;
}

zx_status_t MesonCpuClock::SetRateA1(const uint32_t hz) {
  zx_status_t status;

  if (hz > kFrequencyThresholdHz) {
    // switch to low freq source
    auto sys_cpu_ctrl0 = SysCpuClkControl::Get(offset_).ReadFrom(&*hiu_);
    sys_cpu_ctrl0.set_final_mux_sel(kFixedPll).WriteTo(&*hiu_);

    status = ConfigureSysPLL(hz);
  } else {
    status = ConfigCpuFixedPll(hz);
  }

  return status;
}

zx_status_t MesonCpuClock::SetRate(uint32_t hz) {
  if (chip_id_ == PDEV_PID_AMLOGIC_A5) {
    if (zx_status_t status = SetRateA5(hz); status != ZX_OK) {
      return status;
    }
  } else if (chip_id_ == PDEV_PID_AMLOGIC_A1) {
    if (zx_status_t status = SetRateA1(hz); status != ZX_OK) {
      return status;
    }
  } else {
    if (hz > kFrequencyThresholdHz && current_rate_hz_ > kFrequencyThresholdHz) {
      // Switching between two frequencies both higher than 1GHz.
      // In this case, as per the datasheet it is recommended to change
      // to a frequency lower than 1GHz first and then switch to higher
      // frequency to avoid glitches.

      // Let's first switch to 1GHz
      if (zx_status_t status = SetRate(kFrequencyThresholdHz); status != ZX_OK) {
        fdf::error("{}: failed to set CPU freq to intermediate freq, status = {}", __func__,
                   status);
        return status;
      }

      // Now let's set SYS_PLL rate to hz.
      if (zx_status_t status = ConfigureSysPLL(hz); status != ZX_OK) {
        fdf::error("Failed to configure sys PLL: {}", zx_status_get_string(status));
        return status;
      }

    } else if (hz > kFrequencyThresholdHz && current_rate_hz_ <= kFrequencyThresholdHz) {
      // Switching from a frequency lower than 1GHz to one greater than 1GHz.
      // In this case we just need to set the SYS_PLL to required rate and
      // then set the final mux to 1 (to select SYS_PLL as the source.)

      // Now let's set SYS_PLL rate to hz.
      if (zx_status_t status = ConfigureSysPLL(hz); status != ZX_OK) {
        fdf::error("Failed to configure sys PLL: {}", zx_status_get_string(status));
        return status;
      }

    } else {
      // Switching between two frequencies below 1GHz.
      // In this case we change the source and dividers accordingly
      // to get the required rate from MPLL and do not touch the
      // final mux.
      if (zx_status_t status = ConfigCpuFixedPll(hz); status != ZX_OK) {
        fdf::error("Failed to configure CPU fixed PLL: {}", zx_status_get_string(status));
        return status;
      }
    }
  }

  current_rate_hz_ = hz;
  return ZX_OK;
}

zx_status_t MesonCpuClock::ConfigureSysPLL(uint32_t new_rate) {
  // This API also validates if the new_rate is valid.
  // So no need to validate it here.
  zx_status_t status = sys_pll_->SetRate(new_rate);
  if (status != ZX_OK) {
    fdf::error("Failed to set SYS_PLL rate: {}", zx_status_get_string(status));
    return status;
  }

  // Now we need to change the final mux to select input as SYS_PLL.
  status = WaitForBusyCpu();
  if (status != ZX_OK) {
    fdf::error("{}: failed to wait for busy, status = {}", __func__, status);
    return status;
  }

  // Select the final mux.
  auto sys_cpu_ctrl0 = SysCpuClkControl::Get(offset_).ReadFrom(&*hiu_);
  sys_cpu_ctrl0.set_final_mux_sel(kSysPll).WriteTo(&*hiu_);

  return status;
}

zx::result<uint64_t> MesonCpuClock::QuerySupportedRate(const uint64_t max_rate) {
  // Cpu Clock supported rates fall into two categories based on whether they're below
  // or above the 1GHz threshold. This method scans both the syspll and the fclk to
  // determine the maximum rate that does not exceed `max_rate`.
  zx::result<uint64_t> syspll_rate = zx::error(ZX_ERR_NOT_FOUND);
  zx::result<uint64_t> fclk_rate = zx::error(ZX_ERR_NOT_FOUND);

  if (chip_id_ == PDEV_PID_AMLOGIC_A5) {
    for (const auto& entry : a5_cpu_dyn_table) {
      if (entry.rate > fclk_rate && entry.rate <= max_rate) {
        fclk_rate = zx::ok(entry.rate);
      }
    }
    for (const auto& entry : a5_sys_pll_params_table) {
      if (entry.rate > fclk_rate && entry.rate <= max_rate) {
        syspll_rate = zx::ok(entry.rate);
      }
    }
  } else {
    syspll_rate = sys_pll_->QuerySupportedRate(max_rate);

    const aml_fclk_rate_table_t* fclk_rate_table = s905d2_fclk_get_rate_table();
    size_t rate_count = s905d2_fclk_get_rate_table_count();

    for (size_t i = 0; i < rate_count; i++) {
      if (fclk_rate_table[i].rate > fclk_rate && fclk_rate_table[i].rate <= max_rate) {
        fclk_rate = zx::ok(fclk_rate_table[i].rate);
      }
    }
  }

  // 4 cases: rate supported by syspll only, rate supported by fclk only
  //          rate supported by neither or rate supported by both.
  if (syspll_rate.is_ok() && fclk_rate.is_error()) {
    // Case 1
    return syspll_rate;
  }
  if (syspll_rate.is_error() && fclk_rate.is_ok()) {
    // Case 2
    return fclk_rate;
  }
  if (syspll_rate.is_error() && fclk_rate.is_error()) {
    // Case 3
    return zx::error(ZX_ERR_NOT_FOUND);
  }

  // Case 4
  if (syspll_rate.value() > kFrequencyThresholdHz) {
    return syspll_rate;
  }
  return fclk_rate;
}

zx::result<uint64_t> MesonCpuClock::GetRate() { return zx::ok(current_rate_hz_); }

// NOTE: This block doesn't modify the MPLL, it just programs the muxes &
// dividers to get the new_rate in the sys_pll_div block. Refer fig. 6.6 Multi
// Phase PLLS for A53 & A73 in the datasheet.
zx_status_t MesonCpuClock::ConfigCpuFixedPll(const uint32_t new_rate) {
  const aml_fclk_rate_table_t* fclk_rate_table;
  size_t rate_count;
  size_t i;

  if (chip_id_ == PDEV_PID_AMLOGIC_A1) {
    fclk_rate_table = a1_fclk_get_rate_table();
    rate_count = a1_fclk_get_rate_table_count();
  } else {
    fclk_rate_table = s905d2_fclk_get_rate_table();
    rate_count = s905d2_fclk_get_rate_table_count();
  }
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
    fdf::error("{}: failed to wait for busy, status = {}", __func__, status);
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

zx_status_t MesonCpuClock::WaitForBusyCpu() {
  auto sys_cpu_ctrl0 = SysCpuClkControl::Get(offset_).ReadFrom(&*hiu_);

  // Wait till we are not busy.
  for (uint32_t i = 0; i < kSysCpuWaitBusyRetries; i++) {
    sys_cpu_ctrl0 = SysCpuClkControl::Get(offset_).ReadFrom(&*hiu_);

    if (sys_cpu_ctrl0.busy()) {
      // Wait a little bit before trying again.
      zx_nanosleep(zx_deadline_after(ZX_USEC(kSysCpuWaitBusyTimeoutUs)));
      continue;
    }
    return ZX_OK;
  }
  return ZX_ERR_TIMED_OUT;
}

}  // namespace amlogic_clock
