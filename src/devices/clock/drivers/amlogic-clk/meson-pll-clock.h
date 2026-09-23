// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_CLOCK_DRIVERS_AMLOGIC_CLK_MESON_PLL_CLOCK_H_
#define SRC_DEVICES_CLOCK_DRIVERS_AMLOGIC_CLK_MESON_PLL_CLOCK_H_

#include <soc/aml-meson/aml-clk-common.h>
#include <soc/aml-meson/aml-meson-pll.h>
#include <soc/aml-s905d2/s905d2-hiu.h>

#include "meson-rate-clock.h"

namespace amlogic_clock {

class MesonPllClock : public MesonRateClock {
 public:
  explicit MesonPllClock(const hhi_plls_t pll_num, fdf::MmioBuffer* hiudev)
      : pll_num_(pll_num), hiudev_(hiudev) {}
  explicit MesonPllClock(std::unique_ptr<AmlMesonPllDevice> meson_hiudev)
      : pll_num_(HIU_PLL_COUNT),  // A5 doesn't use it.
        meson_hiudev_(std::move(meson_hiudev)) {}
  MesonPllClock(MesonPllClock&& other)
      : pll_num_(other.pll_num_),  // A5 doesn't use it.
        pll_(other.pll_),
        hiudev_(other.hiudev_),
        meson_hiudev_(std::move(other.meson_hiudev_)) {}
  ~MesonPllClock() override = default;

  void Init();

  // Implement MesonRateClock
  zx_status_t SetRate(uint32_t hz) final;
  zx::result<uint64_t> QuerySupportedRate(uint64_t max_rate) final;
  zx::result<uint64_t> GetRate() final;

  zx_status_t Toggle(bool enable);

 private:
  const hhi_plls_t pll_num_;
  aml_pll_dev_t pll_;
  fdf::MmioBuffer* hiudev_;
  std::unique_ptr<AmlMesonPllDevice> meson_hiudev_;
};

}  // namespace amlogic_clock

#endif  // SRC_DEVICES_CLOCK_DRIVERS_AMLOGIC_CLK_MESON_PLL_CLOCK_H_
