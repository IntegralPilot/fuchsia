// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_OBJECT_INCLUDE_OBJECT_IO_BUFFER_SHARED_REGION_DISPATCHER_H_
#define ZIRCON_KERNEL_OBJECT_INCLUDE_OBJECT_IO_BUFFER_SHARED_REGION_DISPATCHER_H_

#include <lib/object-constants.h>
#include <zircon/rights.h>
#include <zircon/types.h>

#include <fbl/ref_ptr.h>
#include <kernel/ffi.h>
#include <object/dispatcher.h>
#include <object/handle.h>
#include <object/opaque_storage.h>

class VmMapping;
class VmObjectPaged;
class IoBufferSharedRegionDispatcher;

extern "C" {
zx_status_t cpp_io_buffer_shared_region_dispatcher_create(
    const fbl::RefPtr<VmObjectPaged>& vmo, const fbl::RefPtr<VmMapping>& mapping, vaddr_t base,
    ffi::Uninitialized<KernelHandle<IoBufferSharedRegionDispatcher>>* handle_out);
}

class IoBufferSharedRegionDispatcher final : public Dispatcher {
 public:
  explicit IoBufferSharedRegionDispatcher(const fbl::RefPtr<VmObjectPaged>& vmo,
                                          const fbl::RefPtr<VmMapping>& mapping, vaddr_t base);
  ~IoBufferSharedRegionDispatcher() override;

  zx_obj_type_t get_type() const final { return ZX_OBJ_TYPE_IOB_SHARED_REGION; }
  zx_koid_t get_related_koid() const final { return ZX_KOID_INVALID; }
  bool is_waitable() const final { return true; }

  zx_status_t user_signal_self(uint32_t clear_mask, uint32_t set_mask) final {
    return UserSignalSelfSolo(this, clear_mask, set_mask, 0);
  }
  zx_status_t user_signal_peer(uint32_t clear_mask, uint32_t set_mask) final {
    return ZX_ERR_NOT_SUPPORTED;
  }

 protected:
  Lock<CriticalMutex>* get_lock() const final;

 private:
  OpaqueStorage<kIoBufferSharedRegionDispatcherStateSize, kIoBufferSharedRegionDispatcherStateAlign>
      opaque_storage_;
};

#endif  // ZIRCON_KERNEL_OBJECT_INCLUDE_OBJECT_IO_BUFFER_SHARED_REGION_DISPATCHER_H_
