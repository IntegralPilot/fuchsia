// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ZIRCON_KERNEL_INCLUDE_ARCH_MP_UNPLUG_EVENT_H_
#define ZIRCON_KERNEL_INCLUDE_ARCH_MP_UNPLUG_EVENT_H_

#include <kernel/auto_preempt_disabler.h>
#include <kernel/event.h>
#include <kernel/spinlock.h>

class MpUnplugEvent : protected Event {
 public:
  explicit constexpr MpUnplugEvent(bool initial = false) : MpUnplugEvent(initial, Flags::NONE) {}
  ~MpUnplugEvent() = default;

  MpUnplugEvent(const MpUnplugEvent&) = delete;
  MpUnplugEvent& operator=(const MpUnplugEvent&) = delete;
  MpUnplugEvent(MpUnplugEvent&&) = delete;
  MpUnplugEvent& operator=(MpUnplugEvent&&) = delete;

  void Signal() {
    // Disable preemption, and hold the lock while we signal the event.  This
    // will ensure that we have made it all of the way out of the event signal
    // operation before a woken waiter wakes up and destroys our MpUnplugEvent
    // out from under us.
    //
    // We need to make sure that preemption is disabled as well.  If we end up
    // signaling a thread, and it becomes scheduled on our CPU, we could end up
    // attempting to context switch during `event_.Signal()` while holding a
    // spinlock, which would produce a panic.
    AutoPreemptDisabler apd;
    Guard<SpinLock, IrqSave> guard{&lock_};
    Event::Signal();
  }

  zx_status_t Unsignal() {
    Guard<SpinLock, IrqSave> guard{&lock_};
    return Event::Unsignal();
  }

  zx_status_t WaitDeadline(zx_instant_mono_t deadline, Interruptible interruptible) {
    const zx_status_t ret = Event::WaitDeadline(deadline, interruptible);

    // Bouncing through the lock on our way out ensures that our signaler has
    // completely exited the event's Signal operation before we get of our
    // WaitDeadline.
    Guard<SpinLock, IrqSave> guard{&lock_};
    return ret;
  }

 protected:
  constexpr MpUnplugEvent(bool initial, Flags flags) : Event(initial, flags) {}

 private:
  DECLARE_SPINLOCK(MpUnplugEvent) lock_ {};
};

class AutounsignalMpUnplugEvent : public MpUnplugEvent {
 public:
  explicit constexpr AutounsignalMpUnplugEvent(bool initial = false)
      : MpUnplugEvent(initial, Flags::AUTOUNSIGNAL) {}
};

#endif  // ZIRCON_KERNEL_INCLUDE_ARCH_MP_UNPLUG_EVENT_H_
