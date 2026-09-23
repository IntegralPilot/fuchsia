// Copyright 2016 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "object/interrupt_event_dispatcher.h"

#include <lib/counters.h>
#include <platform.h>
#include <stdio.h>
#include <zircon/rights.h>
#include <zircon/syscalls-next.h>
#include <zircon/types.h>

#include <dev/interrupt.h>
#include <fbl/alloc_checker.h>
#include <kernel/mutex.h>

KCOUNTER(dispatcher_interrupt_event_create_count, "dispatcher.interrupt_event.create")
KCOUNTER(dispatcher_interrupt_event_destroy_count, "dispatcher.interrupt_event.destroy")

zx_status_t InterruptEventDispatcher::Create(KernelHandle<InterruptDispatcher>* handle,
                                             zx_rights_t* rights, uint32_t vector, uint32_t options,
                                             bool allow_ack_without_port_for_test) {
  if (!is_valid_interrupt(vector, 0)) {
    return ZX_ERR_INVALID_ARGS;
  }

  if (options & ~(ZX_INTERRUPT_REMAP_IRQ | ZX_INTERRUPT_MODE_MASK | ZX_INTERRUPT_WAKE_VECTOR |
                  ZX_INTERRUPT_TIMESTAMP_MONO)) {
    return ZX_ERR_INVALID_ARGS;
  }

  bool default_mode = false;
  enum interrupt_trigger_mode tm = interrupt_trigger_mode::EDGE;
  enum interrupt_polarity pol = interrupt_polarity::LOW;
  switch (options & ZX_INTERRUPT_MODE_MASK) {
    case ZX_INTERRUPT_MODE_DEFAULT:
      default_mode = true;
      if (zx_status_t status = get_interrupt_config(vector, &tm, &pol); status != ZX_OK) {
        dprintf(CRITICAL, "Internal error fetching IRQ %u's config (%d)\n", vector, status);
        return ZX_ERR_INTERNAL;
      }
      break;
    case ZX_INTERRUPT_MODE_EDGE_LOW:
      tm = interrupt_trigger_mode::EDGE;
      pol = interrupt_polarity::LOW;
      break;
    case ZX_INTERRUPT_MODE_EDGE_HIGH:
      tm = interrupt_trigger_mode::EDGE;
      pol = interrupt_polarity::HIGH;
      break;
    case ZX_INTERRUPT_MODE_LEVEL_LOW:
      tm = interrupt_trigger_mode::LEVEL;
      pol = interrupt_polarity::LOW;
      break;
    case ZX_INTERRUPT_MODE_LEVEL_HIGH:
      tm = interrupt_trigger_mode::LEVEL;
      pol = interrupt_polarity::HIGH;
      break;
    default:
      return ZX_ERR_INVALID_ARGS;
  }

  Flags interrupt_flags{};

  if (options & ZX_INTERRUPT_TIMESTAMP_MONO) {
    interrupt_flags = Flags(interrupt_flags | INTERRUPT_TIMESTAMP_MONO);
  }

  if (tm == interrupt_trigger_mode::LEVEL) {
    interrupt_flags = Flags(interrupt_flags | INTERRUPT_UNMASK_PREWAIT | INTERRUPT_MASK_POSTWAIT);
  }

  if (options & ZX_INTERRUPT_WAKE_VECTOR) {
    interrupt_flags = Flags(interrupt_flags | INTERRUPT_WAKE_VECTOR);
  }

  if (allow_ack_without_port_for_test) {
    interrupt_flags = Flags(interrupt_flags | INTERRUPT_ALLOW_ACK_WITHOUT_PORT_FOR_TEST);
  }

  // Attempt to construct the dispatcher.
  // Do not create a KernelHandle until all initialization has succeeded;
  // if an interrupt already exists on |vector| our on_zero_handles() would
  // tear down the existing interrupt when creation fails.
  fbl::AllocChecker ac;
  auto disp = fbl::AdoptRef(new (&ac) InterruptEventDispatcher(vector, interrupt_flags, options));
  if (!ac.check()) {
    return ZX_ERR_NO_MEMORY;
  }

  // Remap the vector if we have been asked to do so.
  if (options & ZX_INTERRUPT_REMAP_IRQ) {
    vector = remap_interrupt(vector);
  }

  if (!default_mode) {
    zx_status_t status = configure_interrupt(vector, tm, pol);
    if (status != ZX_OK) {
      return status;
    }
  }

  // Register the interrupt
  zx_status_t status = disp->RegisterInterruptHandler();
  if (status != ZX_OK) {
    return status;
  }

  // TODO(https://fxbug.dev/348668110): Revisit this logging.  Still needed?
  if (options & ZX_INTERRUPT_WAKE_VECTOR) {
    dprintf(INFO, "creating interrupt wake vector with vector %u, koid %" PRIu64 "\n", vector,
            disp->get_koid());
  }

  unmask_interrupt(vector);

  // Transfer control of the new dispatcher to the creator and we are done.
  *rights = default_rights();
  handle->reset(ktl::move(disp));

  return ZX_OK;
}

void InterruptEventDispatcher::GetDiagnostics(WakeVector::Diagnostics& diagnostics_out) const {
  diagnostics_out.enabled = is_wake_vector();
  diagnostics_out.koid = get_koid();
  diagnostics_out.PrintExtra("IRQ %" PRIu32, vector_);
}

InterruptEventDispatcher::InterruptEventDispatcher(uint32_t vector, Flags flags, uint32_t options)
    : InterruptDispatcher(flags, options), vector_(vector) {
  kcounter_add(dispatcher_interrupt_event_create_count, 1);
  InitializeWakeEvent();
}

InterruptEventDispatcher::~InterruptEventDispatcher() {
  kcounter_add(dispatcher_interrupt_event_destroy_count, 1);
  DestroyWakeEvent();
}

void InterruptEventDispatcher::MaskInterrupt() { mask_interrupt(vector_); }

void InterruptEventDispatcher::UnmaskInterrupt() { unmask_interrupt(vector_); }

void InterruptEventDispatcher::DeactivateInterrupt() {
#if __aarch64__
  // deactivate_interrupt only exist in arm64
  deactivate_interrupt(vector_);
#endif
}

zx_status_t InterruptEventDispatcher::RegisterInterruptHandler() {
  return register_int_handler(
      vector_,
      interrupt_handler_t{this, [](void* cookie) {
                            static_cast<InterruptEventDispatcher*>(cookie)->InterruptHandler();
                          }});
}

void InterruptEventDispatcher::UnregisterInterruptHandler() {
  register_int_handler(vector_, nullptr);
}
