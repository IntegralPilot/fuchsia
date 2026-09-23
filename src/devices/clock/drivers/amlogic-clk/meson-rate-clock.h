// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_CLOCK_DRIVERS_AMLOGIC_CLK_MESON_RATE_CLOCK_H_
#define SRC_DEVICES_CLOCK_DRIVERS_AMLOGIC_CLK_MESON_RATE_CLOCK_H_

#include <lib/zx/result.h>
#include <zircon/types.h>

namespace amlogic_clock {

class MesonRateClock {
 public:
  virtual zx_status_t SetRate(uint32_t hz) = 0;
  virtual zx::result<uint64_t> QuerySupportedRate(uint64_t max_rate) = 0;
  virtual zx::result<uint64_t> GetRate() = 0;
  virtual ~MesonRateClock() = default;
};

}  // namespace amlogic_clock

#endif  // SRC_DEVICES_CLOCK_DRIVERS_AMLOGIC_CLK_MESON_RATE_CLOCK_H_
