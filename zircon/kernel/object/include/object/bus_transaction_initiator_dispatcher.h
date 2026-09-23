// Copyright 2017 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_OBJECT_INCLUDE_OBJECT_BUS_TRANSACTION_INITIATOR_DISPATCHER_H_
#define ZIRCON_KERNEL_OBJECT_INCLUDE_OBJECT_BUS_TRANSACTION_INITIATOR_DISPATCHER_H_

#include <lib/object-constants.h>
#include <sys/types.h>
#include <zircon/rights.h>
#include <zircon/syscalls/object.h>
#include <zircon/types.h>

#include <dev/iommu/bti.h>
#include <kernel/ffi.h>
#include <kernel/lockdep.h>
#include <object/dispatcher.h>
#include <object/handle.h>
#include <object/iommu_dispatcher.h>
#include <object/opaque_storage.h>
#include <object/pinned_memory_token_dispatcher.h>

class BusTransactionInitiatorDispatcher;

extern "C" {
zx_status_t cpp_bus_transaction_initiator_dispatcher_create(
    const iommu::Iommu* iommu, uint64_t bti_id,
    ffi::Uninitialized<KernelHandle<BusTransactionInitiatorDispatcher>>* handle_out);

void rust_bus_transaction_initiator_dispatcher_state_init(void* state, void* disp, iommu::Bti* bti);
void rust_bus_transaction_initiator_dispatcher_state_destroy(void* state);
Lock<CriticalMutex>* rust_bus_transaction_initiator_dispatcher_state_get_lock(const void* state);
iommu::Bti* rust_bus_transaction_initiator_dispatcher_get_bti(
    const BusTransactionInitiatorDispatcher* disp);
void rust_bus_transaction_initiator_dispatcher_on_zero_handles(
    const BusTransactionInitiatorDispatcher* disp);
}

class BusTransactionInitiatorDispatcher final : public Dispatcher {
 public:
  using Iommu = ::iommu::Iommu;
  using Bti = ::iommu::Bti;

  ~BusTransactionInitiatorDispatcher() final;
  zx_obj_type_t get_type() const final { return ZX_OBJ_TYPE_BTI; }
  zx_koid_t get_related_koid() const final { return ZX_KOID_INVALID; }
  bool is_waitable() const final { return false; }

  zx_status_t user_signal_self(uint32_t clear_mask, uint32_t set_mask) final {
    return UserSignalSelfSolo(this, clear_mask, set_mask, 0);
  }
  zx_status_t user_signal_peer(uint32_t clear_mask, uint32_t set_mask) final {
    return ZX_ERR_NOT_SUPPORTED;
  }

  using Dispatcher::UpdateState;
  using Dispatcher::UpdateStateLocked;

  // Releases all quarantined PMTs. The memory pins are released and the VMO
  // references are dropped, so the underlying VMOs may be immediately destroyed, and the
  // underlying physical memory may be reallocated.
  void ReleaseQuarantine() { bti().ReleaseQuarantine(); }

  void on_zero_handles() final;

  [[nodiscard]] zx_status_t set_name(const char* name, size_t len) final __NONNULL((2)) {
    return bti().set_name(name, len);
  }

  [[nodiscard]] zx_status_t get_name(char (&out_name)[ZX_MAX_NAME_LEN]) const final {
    return bti().get_name(out_name);
  }

  Bti& bti() const;

  // Pin will always be able to return addresses that are contiguous for at
  // least this many bytes. E.g. if this returns 1MB, then a call to Pin()
  // with a size of 2MB will return at most two physically-contiguous runs. If the size
  // were 2.5MB, it will return at most three physically-contiguous runs.
  uint64_t minimum_contiguity() const { return bti().minimum_contiguity(); }

  // The number of bytes in the address space (UINT64_MAX if 2^64).
  uint64_t aspace_size() const { return bti().aspace_size(); }

  // The count of the pinned memory object tokens.
  uint64_t pmo_count() const { return bti().pmo_count(); }

  // The count of the quarantined pinned memory object tokens.
  uint64_t quarantine_count() const { return bti().quarantine_count(); }

  uint64_t bti_id() const { return bti().bti_id(); }

 protected:
  friend PinnedMemoryTokenDispatcher;
  Lock<CriticalMutex>* get_lock() const final;

 private:
  friend zx_status_t cpp_bus_transaction_initiator_dispatcher_create(
      const iommu::Iommu* iommu, uint64_t bti_id,
      ffi::Uninitialized<KernelHandle<BusTransactionInitiatorDispatcher>>* handle_out);

  explicit BusTransactionInitiatorDispatcher(fbl::RefPtr<Bti> bti);

  OpaqueStorage<kBusTransactionInitiatorDispatcherStateSize,
                kBusTransactionInitiatorDispatcherStateAlign>
      opaque_storage_;
};

#endif  // ZIRCON_KERNEL_OBJECT_INCLUDE_OBJECT_BUS_TRANSACTION_INITIATOR_DISPATCHER_H_
