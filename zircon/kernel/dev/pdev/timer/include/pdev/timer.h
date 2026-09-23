// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_DEV_PDEV_TIMER_INCLUDE_PDEV_TIMER_H_
#define ZIRCON_KERNEL_DEV_PDEV_TIMER_INCLUDE_PDEV_TIMER_H_

#include <zircon/time.h>
#include <zircon/types.h>

// Hooks that a pdev timer driver must implement.
// These correspond directly to the timer interface in dev/timer.h.
struct pdev_timer_ops {
  zx_ticks_t (*current_ticks)();
  zx_status_t (*set_oneshot_timer)(zx_ticks_t deadline);
  zx_status_t (*stop)();
  zx_status_t (*shutdown)();
};

void pdev_register_timer(const pdev_timer_ops* ops);

#endif  // ZIRCON_KERNEL_DEV_PDEV_TIMER_INCLUDE_PDEV_TIMER_H_
