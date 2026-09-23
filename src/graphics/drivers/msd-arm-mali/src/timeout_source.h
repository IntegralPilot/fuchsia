// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DRIVERS_MSD_ARM_MALI_SRC_TIMEOUT_SOURCE_H_
#define SRC_GRAPHICS_DRIVERS_MSD_ARM_MALI_SRC_TIMEOUT_SOURCE_H_
#include <chrono>

class TimeoutSource {
 public:
  using Clock = std::chrono::steady_clock;

  virtual Clock::time_point GetCurrentTimeoutPoint() = 0;
  virtual void TimeoutTriggered() = 0;

  // Returns true if the timeout shouldn't be triggered if the device thread has pending work (and
  // potentially was delayed a long time).
  virtual bool CheckForDeviceThreadDelay() { return false; }
};

#endif  // SRC_GRAPHICS_DRIVERS_MSD_ARM_MALI_SRC_TIMEOUT_SOURCE_H_
