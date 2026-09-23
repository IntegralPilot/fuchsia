// Copyright 2017 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "object/timer_dispatcher.h"

#include <lib/counters.h>
#include <lib/object-constants.h>
#include <zircon/compiler.h>
#include <zircon/errors.h>
#include <zircon/rights.h>
#include <zircon/types.h>

#include <new>

#include <fbl/alloc_checker.h>
#include <kernel/ffi.h>
#include <ktl/utility.h>

#include <ktl/enforce.h>

static_assert(sizeof(Dpc) == kDpcStorageSize, "Dpc size mismatch");
static_assert(alignof(Dpc) == kDpcStorageAlign, "Dpc alignment mismatch");

void timer_irq_callback(Timer* timer, zx_time_t now, void* arg) {
  // We are in IRQ context and cannot touch the timer state_tracker, so we
  // schedule a DPC to do so. TODO(cpu): figure out ways to reduce the lag.
  auto dpc = reinterpret_cast<Dpc*>(arg);
  DpcRunner::Enqueue(*dpc, DpcRunner::QueueType::LowLatency);
}

static void dpc_callback(Dpc* d) {
  rust_timer_dispatcher_on_timer_fired(d->arg<TimerDispatcher>());
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE void cpp_timer_dispatcher_init_dpc(void* dpc_storage,
                                                     const TimerDispatcher* disp) {
  new (dpc_storage) Dpc(&dpc_callback, const_cast<TimerDispatcher*>(disp));
}

zx_status_t cpp_timer_dispatcher_create(
    uint32_t options, zx_clock_t clock_id,
    ffi::Uninitialized<KernelHandle<TimerDispatcher>>* handle_out) {
  fbl::AllocChecker ac;
  auto disp = fbl::AdoptRef(new (&ac) TimerDispatcher(options, clock_id));
  if (!ac.check()) {
    return ZX_ERR_NO_MEMORY;
  }

  handle_out->Initialize(ktl::move(disp));
  return ZX_OK;
}

TimerDispatcher::TimerDispatcher(uint32_t options, zx_clock_t clock_id) : Dispatcher(0u) {
  DISPATCHER_VERIFY_OFFSET(TimerDispatcher, kTimerDispatcherStateOffset);
  rust_timer_dispatcher_state_init(&opaque_storage_, this, options, clock_id);
}

IMPLEMENT_DISPATCHER_RUST_STATE(TimerDispatcher, rust_timer_dispatcher_state_get_lock,
                                rust_timer_dispatcher_state_destroy)

void TimerDispatcher::OnTimerFired() { rust_timer_dispatcher_on_timer_fired(this); }

zx_info_timer_t TimerDispatcher::GetInfo() const { return rust_timer_dispatcher_get_info(this); }
