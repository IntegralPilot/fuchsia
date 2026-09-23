// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "dev/timer.h"

#include <debug.h>
#include <lib/arch/intrin.h>
#include <lib/arch/ticks.h>

#include <pdev/timer.h>

namespace {

constexpr pdev_timer_ops default_ops = {
    .current_ticks = []() -> zx_ticks_t { return 0; },
    .set_oneshot_timer = [](zx_ticks_t deadline) -> zx_status_t { PANIC_UNIMPLEMENTED; },
    .stop = []() -> zx_status_t { PANIC_UNIMPLEMENTED; },
    .shutdown = []() -> zx_status_t { PANIC_UNIMPLEMENTED; },
};

const pdev_timer_ops* timer_ops = &default_ops;

}  // anonymous namespace

zx_ticks_t timer_current_ticks() { return timer_ops->current_ticks(); }

zx_status_t timer_set_oneshot_timer(zx_ticks_t deadline) {
  return timer_ops->set_oneshot_timer(deadline);
}

zx_status_t timer_stop() { return timer_ops->stop(); }

zx_status_t timer_shutdown() { return timer_ops->shutdown(); }

void pdev_register_timer(const pdev_timer_ops* ops) {
  timer_ops = ops;
  arch::ThreadMemoryBarrier();
}
