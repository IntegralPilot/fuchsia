// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/lib/timekeeper/test_clock.h"

namespace timekeeper {

TestClock::TestClock() : current_utc_(0), current_monotonic_(0), current_boot_(0) {}

TestClock::~TestClock() = default;

zx_status_t TestClock::GetUtcTime(zx_time_t* time) const {
  *time = current_utc_;
  return ZX_OK;
}

zx_instant_mono_t TestClock::GetMonotonicTime() const { return current_monotonic_; }

zx_instant_boot_t TestClock::GetBootTime() const { return current_boot_; }

}  // namespace timekeeper
