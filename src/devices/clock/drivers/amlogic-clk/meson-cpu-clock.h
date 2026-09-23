// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_CLOCK_DRIVERS_AMLOGIC_CLK_MESON_CPU_CLOCK_H_
#define SRC_DEVICES_CLOCK_DRIVERS_AMLOGIC_CLK_MESON_CPU_CLOCK_H_

#include "aml-a5-blocks.h"
#include "meson-pll-clock.h"
#include "meson-rate-clock.h"

namespace amlogic_clock {

class MesonCpuClock : public MesonRateClock {
 public:
  explicit MesonCpuClock(const fdf::MmioBuffer* hiu, const uint32_t offset, MesonPllClock* sys_pll,
                         const uint32_t initial_rate)
      : hiu_(hiu), offset_(offset), sys_pll_(sys_pll), current_rate_hz_(initial_rate) {}
  explicit MesonCpuClock(const fdf::MmioBuffer* hiu, const uint32_t offset, MesonPllClock* sys_pll,
                         const uint32_t initial_rate, const uint32_t chip_id)
      : hiu_(hiu),
        offset_(offset),
        sys_pll_(sys_pll),
        current_rate_hz_(initial_rate),
        chip_id_(chip_id) {}
  explicit MesonCpuClock(const fdf::MmioBuffer* hiu, const uint32_t offset, MesonPllClock* sys_pll,
                         const uint32_t initial_rate, const uint32_t chip_id,
                         zx::resource smc_resource)
      : hiu_(hiu),
        offset_(offset),
        sys_pll_(sys_pll),
        current_rate_hz_(initial_rate),
        chip_id_(chip_id),
        smc_(std::move(smc_resource)) {}
  MesonCpuClock(MesonCpuClock&& other)
      : hiu_(other.hiu_),
        offset_(other.offset_),
        sys_pll_(other.sys_pll_),
        current_rate_hz_(other.current_rate_hz_),
        chip_id_(other.chip_id_),
        smc_(std::move(other.smc_)) {}
  ~MesonCpuClock() override = default;

  // Implement MesonRateClock
  zx_status_t SetRate(uint32_t hz) final;
  zx::result<uint64_t> QuerySupportedRate(uint64_t max_rate) final;
  zx::result<uint64_t> GetRate() final;

 private:
  zx_status_t ConfigCpuFixedPll(uint32_t new_rate);
  zx_status_t ConfigureSysPLL(uint32_t new_rate);
  zx_status_t WaitForBusyCpu();
  zx_status_t SecSetClk(uint32_t func_id, uint64_t arg1, uint64_t arg2, uint64_t arg3,
                        uint64_t arg4, uint64_t arg5, uint64_t arg6);
  zx_status_t SetRateA5(uint32_t hz);
  zx_status_t SecSetCpuClkMux(uint64_t clock_source);
  zx_status_t SecSetSys0DcoPll(const pll_params_table& pll_params);
  zx_status_t SecSetCpuClkDyn(const cpu_dyn_table& dyn_params);
  zx_status_t SetRateA1(uint32_t hz);

  static constexpr uint32_t kFrequencyThresholdHz = 1'000'000'000;
  // Final Mux for selecting clock source.
  static constexpr uint32_t kFixedPll = 0;
  static constexpr uint32_t kSysPll = 1;

  static constexpr uint32_t kSysCpuWaitBusyRetries = 5;
  static constexpr uint32_t kSysCpuWaitBusyTimeoutUs = 10'000;

  const fdf::MmioBuffer* hiu_;
  const uint32_t offset_;

  MesonPllClock* sys_pll_;

  uint32_t current_rate_hz_;
  uint32_t chip_id_ = 0;
  zx::resource smc_;
};

}  // namespace amlogic_clock

#endif  // SRC_DEVICES_CLOCK_DRIVERS_AMLOGIC_CLK_MESON_CPU_CLOCK_H_
