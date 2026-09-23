// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_POWER_STATE_RECORDER_CPP_COMMON_INTERNAL_H_
#define LIB_POWER_STATE_RECORDER_CPP_COMMON_INTERNAL_H_

#include <lib/zx/clock.h>
#include <lib/zx/time.h>
#include <zircon/compiler.h>
#include <zircon/syscalls.h>

#include <algorithm>
#include <mutex>
#include <string>
#include <vector>

namespace power_observability::internal {

inline zx_ticks_t boot_time_to_ticks(zx::time_boot timestamp) {
  // Boot ticks and boot nanos are related by a simple ratio; ticks==0 ==> nanos==0.
  __int128_t ticks =
      static_cast<__uint128_t>(timestamp.get()) * zx_ticks_per_second() / zx::sec(1).to_nsecs();

  return static_cast<zx_ticks_t>(ticks);
}

}  // namespace power_observability::internal

#endif  // LIB_POWER_STATE_RECORDER_CPP_COMMON_INTERNAL_H_
