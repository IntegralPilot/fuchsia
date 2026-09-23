// Copyright 2017 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "object/bus_transaction_initiator_dispatcher.h"

#include <align.h>
#include <lib/object-constants.h>
#include <lib/page/size.h>
#include <zircon/errors.h>
#include <zircon/rights.h>
#include <zircon/types.h>

#include <new>

#include <dev/iommu/iommu.h>
#include <kernel/ffi.h>
#include <object/pinned_memory_token_dispatcher.h>
#include <vm/pinned_vm_object.h>
#include <vm/vm_object.h>

BusTransactionInitiatorDispatcher::BusTransactionInitiatorDispatcher(fbl::RefPtr<Bti> bti)
    : Dispatcher(0u) {
  DISPATCHER_VERIFY_OFFSET(BusTransactionInitiatorDispatcher,
                           kBusTransactionInitiatorDispatcherStateOffset);
  rust_bus_transaction_initiator_dispatcher_state_init(&opaque_storage_, this,
                                                       fbl::ExportToRawPtr(&bti));
}

IMPLEMENT_DISPATCHER_RUST_STATE(BusTransactionInitiatorDispatcher,
                                rust_bus_transaction_initiator_dispatcher_state_get_lock,
                                rust_bus_transaction_initiator_dispatcher_state_destroy)

void BusTransactionInitiatorDispatcher::on_zero_handles() {
  rust_bus_transaction_initiator_dispatcher_on_zero_handles(this);
}

iommu::Bti& BusTransactionInitiatorDispatcher::bti() const {
  return *rust_bus_transaction_initiator_dispatcher_get_bti(this);
}
