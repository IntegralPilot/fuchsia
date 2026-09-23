// Copyright 2016 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "object/event_dispatcher.h"

#include <zircon/errors.h>
#include <zircon/rights.h>
#include <zircon/types.h>

#include <fbl/alloc_checker.h>

extern "C" {
void rust_event_dispatcher_state_init(void* state, void* disp);
void rust_event_dispatcher_state_destroy(void* state);
Lock<CriticalMutex>* rust_event_dispatcher_state_get_lock(const void* state);
}  // extern "C"

EventDispatcher::EventDispatcher(uint32_t options) : Dispatcher(0u) {
  DISPATCHER_VERIFY_OFFSET(EventDispatcher, kEventDispatcherStateOffset);
  rust_event_dispatcher_state_init(&opaque_storage_, this);
}

IMPLEMENT_DISPATCHER_RUST_STATE(EventDispatcher, rust_event_dispatcher_state_get_lock,
                                rust_event_dispatcher_state_destroy)

zx_status_t EventDispatcher::user_signal_self(uint32_t clear_mask, uint32_t set_mask) {
  return UserSignalSelfSolo(this, clear_mask, set_mask, ZX_EVENT_SIGNALED);
}

zx_status_t MemoryStallEventDispatcher::Create(zx_system_memory_stall_type_t kind,
                                               zx_duration_mono_t threshold,
                                               zx_duration_mono_t window,
                                               KernelHandle<EventDispatcher>* handle,
                                               zx_rights_t* rights) {
  fbl::AllocChecker ac;
  KernelHandle dispatcher(fbl::AdoptRef(new (&ac) MemoryStallEventDispatcher(kind)));
  if (!ac.check()) {
    return ZX_ERR_NO_MEMORY;
  }

  zx::result<ktl::unique_ptr<StallObserver>> observer =
      StallObserver::Create(threshold, window, dispatcher.dispatcher().get());
  if (observer.is_error()) {
    return observer.error_value();
  }

  StallAggregator* aggregator = StallAggregator::GetStallAggregator();
  switch (kind) {
    case ZX_SYSTEM_MEMORY_STALL_SOME:
      aggregator->AddObserverSome((*observer).get());
      break;
    case ZX_SYSTEM_MEMORY_STALL_FULL:
      aggregator->AddObserverFull((*observer).get());
      break;
    default:
      return ZX_ERR_INVALID_ARGS;
  }

  dispatcher.dispatcher()->observer_ = ktl::move(*observer);

  *rights = ZX_DEFAULT_SYSTEM_MEMORY_STALL_EVENT_RIGHTS;
  *handle = ktl::move(dispatcher);
  return ZX_OK;
}

MemoryStallEventDispatcher::~MemoryStallEventDispatcher() {
  if (!observer_) {
    return;
  }

  StallAggregator* aggregator = StallAggregator::GetStallAggregator();
  switch (kind_) {
    case ZX_SYSTEM_MEMORY_STALL_SOME:
      aggregator->RemoveObserverSome(observer_.get());
      break;
    case ZX_SYSTEM_MEMORY_STALL_FULL:
      aggregator->RemoveObserverFull(observer_.get());
      break;
    default:
      ZX_PANIC("Impossible stall kind value");
  }
}

MemoryStallEventDispatcher::MemoryStallEventDispatcher(zx_system_memory_stall_type_t kind)
    : EventDispatcher(0), kind_(kind) {}

void MemoryStallEventDispatcher::OnAboveThreshold() { UpdateState(0, ZX_EVENT_SIGNALED); }

void MemoryStallEventDispatcher::OnBelowThreshold() { UpdateState(ZX_EVENT_SIGNALED, 0); }
