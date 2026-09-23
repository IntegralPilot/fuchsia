// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_CLOCK_DRIVERS_VIM3_CLK_MESON_PLL_H_
#define SRC_DEVICES_CLOCK_DRIVERS_VIM3_CLK_MESON_PLL_H_

#include <lib/driver/mmio/cpp/mmio.h>

#include <soc/aml-meson/aml-pll.h>
#include <soc/aml-s905d2/s905d2-hiu.h>

#include "meson_gate.h"

namespace vim3_clock {

class MesonRateClock {
 public:
  virtual zx_status_t SetRate(uint32_t hz) = 0;
  virtual zx_status_t QuerySupportedRate(uint64_t max_rate, uint64_t* result) = 0;
  virtual zx_status_t GetRate(uint64_t* result) = 0;
  virtual ~MesonRateClock() {}
};

using meson_cpu_clk_t = struct meson_cpu_clk {
  uint32_t reg;
  hhi_plls_t pll;
  uint32_t initial_hz;
};

static constexpr meson_cpu_clk_t kG12bCpuClks[] = {
    {.reg = kG12bHhiSysCpubClkCntl, .pll = SYS_PLL, .initial_hz = 1'000'000'000},  // Big Cluster
    {.reg = kHhiSysCpuClkCntl0, .pll = SYS1_PLL, .initial_hz = 1'200'000'000},     // Little Cluster
};

class MesonPllClock : public MesonRateClock {
 public:
  explicit MesonPllClock(const hhi_plls_t pll_num, fdf::MmioBuffer* hiudev)
      : pll_num_(pll_num), hiudev_(hiudev) {}

  ~MesonPllClock() = default;

  void Init();

  zx_status_t SetRate(uint32_t hz) final;
  zx_status_t QuerySupportedRate(uint64_t max_rate, uint64_t* result) final;
  zx_status_t GetRate(uint64_t* result) final;

  zx_status_t Toggle(bool enable);

 private:
  const hhi_plls_t pll_num_;
  aml_pll_dev_t pll_;
  fdf::MmioBuffer* hiudev_;
};

class MesonCpuClock : public MesonRateClock {
 public:
  explicit MesonCpuClock(const fdf::MmioBuffer* hiu, const uint32_t offset, MesonPllClock* sys_pll,
                         const uint32_t initial_rate)
      : hiu_(hiu), offset_(offset), sys_pll_(sys_pll), current_rate_hz_(initial_rate) {}
  ~MesonCpuClock() = default;

  // Implement MesonRateClock
  zx_status_t SetRate(const uint32_t hz) final override;
  zx_status_t QuerySupportedRate(const uint64_t max_rate, uint64_t* result) final override;
  zx_status_t GetRate(uint64_t* result) final override;

 private:
  zx_status_t ConfigCpuFixedPll(const uint32_t new_rate);
  zx_status_t ConfigureSysPLL(uint32_t new_rate);
  zx_status_t WaitForBusyCpu();

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
};

}  // namespace vim3_clock

#endif  // SRC_DEVICES_CLOCK_DRIVERS_VIM3_CLK_MESON_PLL_H_
