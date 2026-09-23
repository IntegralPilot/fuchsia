// Copyright 2020 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_OBJECT_INCLUDE_OBJECT_WAIT_SIGNAL_OBSERVER_H_
#define ZIRCON_KERNEL_OBJECT_INCLUDE_OBJECT_WAIT_SIGNAL_OBSERVER_H_

#include <lib/object-constants.h>
#include <stdint.h>
#include <zircon/types.h>

#include <kernel/ffi.h>
#include <object/opaque_storage.h>
#include <object/signal_observer.h>

class OwnedWaitQueue;

// Helper class for Waiting on the wait_one and wait_many syscalls.
// Delegates business logic to its Rust state stored in opaque_storage_.
class WaitSignalObserver final : public SignalObserver {
 public:
  WaitSignalObserver();
  ~WaitSignalObserver() final;

  // |SignalObserver| implementation.
  void OnMatch(zx_signals_t signals, OwnedWaitQueue* queue_to_own) final;
  void OnCancel(zx_signals_t signals) final;

 private:
  WaitSignalObserver(const WaitSignalObserver&) = delete;
  WaitSignalObserver& operator=(const WaitSignalObserver&) = delete;

  OpaqueStorage<kWaitSignalObserverStorageSize, kWaitSignalObserverStorageAlign> opaque_storage_;
};

extern "C" {
void cpp_wait_signal_observer_init(ffi::Uninitialized<WaitSignalObserver>* observer);
void cpp_wait_signal_observer_destroy(WaitSignalObserver* observer);
}

#endif  // ZIRCON_KERNEL_OBJECT_INCLUDE_OBJECT_WAIT_SIGNAL_OBSERVER_H_
