// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <new>

#include <kernel/timer.h>

static_assert(sizeof(Timer) == 72, "Timer size mismatch");
static_assert(alignof(Timer) == 8, "Timer alignment mismatch");

extern "C" {

void cpp_timer_init(Timer* timer, zx_clock_t clock_id);
void cpp_timer_destroy(Timer* timer);
void cpp_timer_set(Timer* timer, const Deadline* deadline, Timer::Callback callback, void* arg);
bool cpp_timer_cancel(Timer* timer);

void cpp_timer_init(Timer* timer, zx_clock_t clock_id) {
  DEBUG_ASSERT(timer != nullptr);
  new (timer) Timer(clock_id);
}

void cpp_timer_destroy(Timer* timer) {
  DEBUG_ASSERT(timer != nullptr);
  timer->~Timer();
}

void cpp_timer_set(Timer* timer, const Deadline* deadline, Timer::Callback callback, void* arg) {
  DEBUG_ASSERT(timer != nullptr);
  DEBUG_ASSERT(deadline != nullptr);
  timer->Set(*deadline, callback, arg);
}

bool cpp_timer_cancel(Timer* timer) {
  DEBUG_ASSERT(timer != nullptr);
  return timer->Cancel();
}

}  // extern "C"
