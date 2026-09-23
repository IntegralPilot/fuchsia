// Copyright 2020 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_INCLUDE_KERNEL_EVENT_LIMITER_H_
#define ZIRCON_KERNEL_INCLUDE_KERNEL_EVENT_LIMITER_H_

#include <zircon/types.h>

#include <ktl/atomic.h>
#include <platform/timer.h>

// An EventLimiter allows an action to only be taken every K seconds in a thread-safe manner.
//
// Use as follows:
//
//   EventLimiter<ZX_SEC(1)> limiter;
//   while (...) {
//     if (limiter.Ready()) {
//       printf("...");
//     }
//     // ...
//   }
//
template <zx_duration_mono_t Duration>
class EventLimiter {
 public:
  bool Ready() {
    zx_instant_mono_t now = current_mono_time();

    // If we have recently taken action, we don't need to do it again.
    zx_instant_mono_t last_event = last_event_.load(ktl::memory_order_relaxed);
    if (last_event != 0 && now < last_event + Duration) {
      return false;
    }

    // Otherwise, record that we have acted. If we race with another thread, assume it has taken
    // action and we don't need to.
    if (!last_event_.compare_exchange_strong(last_event, now, ktl::memory_order_relaxed)) {
      return false;
    }

    return true;
  }

 private:
  ktl::atomic<zx_instant_mono_t> last_event_ = 0;
};

#endif  // ZIRCON_KERNEL_INCLUDE_KERNEL_EVENT_LIMITER_H_
