// Copyright 2016 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "object/virtual_interrupt_dispatcher.h"

#include <lib/counters.h>
#include <platform.h>
#include <zircon/rights.h>

#include <dev/interrupt.h>
#include <fbl/alloc_checker.h>
#include <kernel/mutex.h>

KCOUNTER(dispatcher_virtual_interrupt_create_count, "dispatcher.virtual_interrupt.create")
KCOUNTER(dispatcher_virtual_interrupt_destroy_count, "dispatcher.virtual_interrupt.destroy")

zx_status_t VirtualInterruptDispatcher::Create(KernelHandle<InterruptDispatcher>* handle,
                                               zx_rights_t* rights, uint32_t options) {
  if (!(options & ZX_INTERRUPT_VIRTUAL)) {
    return ZX_ERR_INVALID_ARGS;
  }

  constexpr uint32_t permitted_options =
      ZX_INTERRUPT_VIRTUAL | ZX_INTERRUPT_TIMESTAMP_MONO | ZX_INTERRUPT_WAKE_VECTOR;
  if (options & ~permitted_options) {
    return ZX_ERR_INVALID_ARGS;
  }

  Flags flags = INTERRUPT_VIRTUAL;
  if (options & ZX_INTERRUPT_TIMESTAMP_MONO) {
    flags = Flags(flags | INTERRUPT_TIMESTAMP_MONO);
  }

  if (options & ZX_INTERRUPT_WAKE_VECTOR) {
    flags = Flags(flags | INTERRUPT_WAKE_VECTOR);
  }

  // Attempt to construct the dispatcher.
  fbl::AllocChecker ac;
  KernelHandle new_handle(fbl::AdoptRef(new (&ac) VirtualInterruptDispatcher(flags, options)));
  if (!ac.check())
    return ZX_ERR_NO_MEMORY;

  // Transfer control of the new dispatcher to the creator and we are done.
  *rights = default_rights();
  *handle = ktl::move(new_handle);

  return ZX_OK;
}

VirtualInterruptDispatcher::VirtualInterruptDispatcher(Flags flags, uint32_t options)
    : InterruptDispatcher(flags, options) {
  // Immediately after construction, virtual interrupt objects are always in the
  // untriggered state.  Make sure that our initial signal state reflects this.
  UpdateState(0, ZX_VIRTUAL_INTERRUPT_UNTRIGGERED);
  kcounter_add(dispatcher_virtual_interrupt_create_count, 1);
  if (is_wake_vector()) {
    InitializeWakeEvent();
  }
}

VirtualInterruptDispatcher::~VirtualInterruptDispatcher() {
  if (is_wake_vector()) {
    DestroyWakeEvent();
  }
  kcounter_add(dispatcher_virtual_interrupt_destroy_count, 1);
}

void VirtualInterruptDispatcher::GetDiagnostics(WakeVector::Diagnostics& diagnostics_out) const {
  diagnostics_out.enabled = is_wake_vector();
  diagnostics_out.koid = get_koid();
  diagnostics_out.PrintExtra("VirtIRQ %" PRIu64, get_koid());
}

void VirtualInterruptDispatcher::MaskInterrupt() {}
void VirtualInterruptDispatcher::UnmaskInterrupt() {}
void VirtualInterruptDispatcher::DeactivateInterrupt() {}
void VirtualInterruptDispatcher::UnregisterInterruptHandler() {}

zx_status_t VirtualInterruptDispatcher::WaitForInterrupt(zx_time_t* out_timestamp) {
  while (true) {
    // Hold the dispatcher lock during the first phase, and if we decide that we
    // need to block, be sure that our UNTRIGGERED signal has been asserted.
    {
      Guard<CriticalMutex> guard{get_lock()};
      const ktl::optional<zx_status_t> opt_status = BeginWaitForInterrupt(out_timestamp);
      if (opt_status.has_value()) {
        return opt_status.value();
      }

      UpdateStateLocked(0, ZX_VIRTUAL_INTERRUPT_UNTRIGGERED);
    }

    const zx_status_t block_status = DoWaitForInterruptBlock();
    if (block_status != ZX_OK) {
      return block_status;
    }
  }
}

zx_status_t VirtualInterruptDispatcher::Trigger(zx_time_t timestamp) {
  Guard<CriticalMutex> dispatcher_guard{get_lock()};
  const zx_status_t result = InterruptDispatcher::Trigger(timestamp);

  // If the trigger operation succeeded, then we know that we must be in either
  // the triggered or need-ack state, and we should clear the UNTRIGGERED
  // signal.
  if (result == ZX_OK) {
    UpdateStateLocked(ZX_VIRTUAL_INTERRUPT_UNTRIGGERED, 0);
  }

  return result;
}

zx_status_t VirtualInterruptDispatcher::Ack() {
  // Hold the dispatcher lock while we perform our lower level ack operation.
  // If this succeeds, make sure that the UNTRIGGERED signal is asserted.
  Guard<CriticalMutex> guard{get_lock()};
  const zx::result<PostAckState> result = AckInternal();

  // If something goes wrong with the ack, then our UNTRIGGERED signal state
  // should not have changed.
  if (result.is_error()) {
    return result.error_value();
  }

  // Things went well, we successfully acked the interrupt and should assert the
  // UNTRIGGERED signal.  If, however, the interrupt immediately retriggered
  // (because it already had another pending signal), be sure to clear the
  // UNTRIGGERED signal after asserting it.
  UpdateStateLocked(0, ZX_VIRTUAL_INTERRUPT_UNTRIGGERED);
  if (result.value() == PostAckState::Retriggered) {
    UpdateStateLocked(ZX_VIRTUAL_INTERRUPT_UNTRIGGERED, 0);
  } else {
    DEBUG_ASSERT(result.value() == PostAckState::FullyAcked);
  }

  return ZX_OK;
}

zx_status_t VirtualInterruptDispatcher::Destroy() {
  // An interrupt which has been explicitly destroyed (using a call to
  // zx_interrupt_destroy) can no longer be in the triggered state.  Make sure
  // that we assert the virtual interrupt's UNTRIGGERED signal to be sure that
  // we don't accidentally end up with waiters stuck waiting on a destroyed
  // interrupt.
  Guard<CriticalMutex> guard{get_lock()};
  const zx_status_t result = InterruptDispatcher::Destroy();
  UpdateStateLocked(0, ZX_VIRTUAL_INTERRUPT_UNTRIGGERED);
  return result;
}
