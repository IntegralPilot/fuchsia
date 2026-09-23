// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_INCLUDE_DEV_TIMER_H_
#define ZIRCON_KERNEL_INCLUDE_DEV_TIMER_H_

#include <zircon/time.h>
#include <zircon/types.h>

// Systemwide timer interface, for platforms or architectures
// that utilize the abstraction.
//
// These correspond (currently) to calls defined in platform/timer.h

// Read the current ticks of the hardware timer, at the rate the timer is ticking.
// Generally converted to real time in current_mono_time().
zx_ticks_t timer_current_ticks();

// Set a timer to fire at the deadline specified that calls timer_tick().
zx_status_t timer_set_oneshot_timer(zx_ticks_t deadline);

// Cancel a pending oneshot timer. Okay to call if no pending oneshot.
zx_status_t timer_stop();

// Stop the timer hardware. Stop should be called before shutdown.
zx_status_t timer_shutdown();

#endif  // ZIRCON_KERNEL_INCLUDE_DEV_TIMER_H_
