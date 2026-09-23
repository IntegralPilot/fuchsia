// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <kernel/recurring_callback.h>
#include <kernel/thread.h>

static constexpr TimerSlack kSlack{ZX_MSEC(10), TIMER_SLACK_CENTER};

void RecurringCallback::CallbackWrapper(Timer* t, zx_instant_mono_t now, void* arg) {
  auto cb = static_cast<RecurringCallback*>(arg);
  cb->func_();

  {
    Guard<SpinLock, IrqSave> guard{&cb->lock_};

    if (cb->started_) {
      const Deadline deadline(zx_time_add_duration(now, ZX_SEC(1)), kSlack);
      t->Set(deadline, CallbackWrapper, arg);
    }
  }

  // Reschedule to give the debuglog a chance to run
  Thread::Current::preemption_state().PreemptSetPending();
}

void RecurringCallback::Toggle() {
  Guard<SpinLock, IrqSave> guard{&lock_};

  if (!started_) {
    const Deadline deadline = Deadline::after_mono(ZX_SEC(1), kSlack);
    // Start the timer
    timer_.Set(deadline, CallbackWrapper, static_cast<void*>(this));
    started_ = true;
  } else {
    timer_.Cancel();
    started_ = false;
  }
}
