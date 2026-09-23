// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_INCLUDE_KERNEL_RECURRING_CALLBACK_H_
#define ZIRCON_KERNEL_INCLUDE_KERNEL_RECURRING_CALLBACK_H_

#include <fbl/macros.h>
#include <kernel/spinlock.h>
#include <kernel/timer.h>

class RecurringCallback {
 public:
  using CallbackFunc = void (*)();

  explicit RecurringCallback(CallbackFunc callback) : func_(callback) {}

  void Toggle();

 private:
  DISALLOW_COPY_ASSIGN_AND_MOVE(RecurringCallback);

  static void CallbackWrapper(Timer* t, zx_instant_mono_t now, void* arg);

  DECLARE_SPINLOCK(RecurringCallback) lock_;
  Timer timer_;
  bool started_ = false;
  CallbackFunc func_ = nullptr;
};

#endif  // ZIRCON_KERNEL_INCLUDE_KERNEL_RECURRING_CALLBACK_H_
