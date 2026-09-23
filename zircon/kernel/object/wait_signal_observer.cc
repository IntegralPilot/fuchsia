// Copyright 2020 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "object/wait_signal_observer.h"

#include <stddef.h>

#include <kernel/ffi.h>

#include <ktl/enforce.h>

static_assert(sizeof(WaitSignalObserver) == kWaitSignalObserverSize);
static_assert(alignof(WaitSignalObserver) == kWaitSignalObserverAlign);

#define OBSERVER_VERIFY_OFFSET(Class, offset_const)                           \
  _Pragma("GCC diagnostic push")                                              \
      _Pragma("GCC diagnostic ignored \"-Winvalid-offsetof\"") static_assert( \
          offsetof(Class, opaque_storage_) == (offset_const),                 \
          #Class " opaque_storage_ offset mismatch");                         \
  _Pragma("GCC diagnostic pop")

extern "C" {
void rust_wait_signal_observer_init(void* storage);
void rust_wait_signal_observer_destroy(void* storage);
void rust_wait_signal_observer_on_match(void* storage, zx_signals_t signals,
                                        OwnedWaitQueue* queue_to_own);
void rust_wait_signal_observer_on_cancel(void* storage, zx_signals_t signals);
}

WaitSignalObserver::WaitSignalObserver() {
  OBSERVER_VERIFY_OFFSET(WaitSignalObserver, kWaitSignalObserverStorageOffset);
  rust_wait_signal_observer_init(&opaque_storage_);
}

WaitSignalObserver::~WaitSignalObserver() { rust_wait_signal_observer_destroy(&opaque_storage_); }

void WaitSignalObserver::OnMatch(zx_signals_t signals, OwnedWaitQueue* queue_to_own) {
  rust_wait_signal_observer_on_match(&opaque_storage_, signals, queue_to_own);
}

void WaitSignalObserver::OnCancel(zx_signals_t signals) {
  rust_wait_signal_observer_on_cancel(&opaque_storage_, signals);
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
extern "C" FFI_ALWAYS_INLINE void cpp_wait_signal_observer_init(
    ffi::Uninitialized<WaitSignalObserver>* observer) {
  observer->Initialize();
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
extern "C" FFI_ALWAYS_INLINE void cpp_wait_signal_observer_destroy(WaitSignalObserver* observer) {
  observer->~WaitSignalObserver();
}
