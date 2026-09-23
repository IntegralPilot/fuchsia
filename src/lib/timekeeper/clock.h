// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_TIMEKEEPER_CLOCK_H_
#define SRC_LIB_TIMEKEEPER_CLOCK_H_

#include <lib/zx/time.h>

namespace timekeeper {

// The bogus clock ID for the clock on the UTC timeline. Chosen to be a fairly
// unlikely to be used elsewhere (it's a date), but makes time_utc distinct
// from other clock types lurking in Fuchsia.
static constexpr zx_clock_t kUtcClockId = 20241204;

// The type used to measure UTC time.
using time_utc = zx::basic_time<kUtcClockId>;

// Abstraction over the clock.
//
// This class allows to retrieve the current time for any supported clock id.
// This being a class, it allows to inject custom behavior for tests.
class Clock {
 public:
  Clock() = default;
  virtual ~Clock() = default;
  Clock(const Clock&) = delete;
  Clock& operator=(const Clock&) = delete;

  // Returns the current UTC time.
  zx_status_t UtcNow(time_utc* result) const {
    zx_time_t time;
    zx_status_t status = GetUtcTime(&time);
    *result = time_utc(time);
    return status;
  }

  // Returns the current monotonic time. See |zx_clock_get_monotonic|.
  zx::basic_time<ZX_CLOCK_MONOTONIC> MonotonicNow() const { return zx::time(GetMonotonicTime()); }

  // Returns the current boot time. See |zx_clock_get_boot|.
  zx::basic_time<ZX_CLOCK_BOOT> BootNow() const {
    return zx::basic_time<ZX_CLOCK_BOOT>(GetBootTime());
  }

 protected:
  // Returns the current UTC time.
  virtual zx_status_t GetUtcTime(zx_time_t* time) const = 0;
  // Returns the current monotonic time. See |zx_clock_get_monotonic|.
  virtual zx_instant_mono_t GetMonotonicTime() const = 0;
  // Returns the current boot time. See |zx_clock_get_boot|.
  virtual zx_instant_boot_t GetBootTime() const = 0;
};

}  // namespace timekeeper

#endif  // SRC_LIB_TIMEKEEPER_CLOCK_H_
