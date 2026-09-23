// Copyright 2016 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_VM_INCLUDE_VM_VM_OBJECT_PHYSICAL_H_
#define ZIRCON_KERNEL_VM_INCLUDE_VM_VM_OBJECT_PHYSICAL_H_

#include <lib/user_copy/user_ptr.h>
#include <lib/zircon-internal/thread_annotations.h>
#include <stdint.h>
#include <zircon/types.h>

#include <fbl/array.h>
#include <fbl/canary.h>
#include <fbl/intrusive_double_list.h>
#include <fbl/macros.h>
#include <fbl/ref_counted.h>
#include <fbl/ref_ptr.h>
#include <kernel/mutex.h>
#include <object/opaque_storage.h>
#include <vm/vm.h>
#include <vm/vm_constants.h>
#include <vm/vm_object.h>

extern "C" void cpp_vm_object_physical_free(VmObjectPhysical* vmo);

// VMO representing a physical range of memory
class VmObjectPhysical final : public VmObject, public VmDeferredDeleter<VmObjectPhysical> {
 public:
  static zx_status_t Create(paddr_t base, uint64_t size, fbl::RefPtr<VmObjectPhysical>* vmo);

  Lock<CriticalMutex>* get_lock() const;
  Lock<CriticalMutex>* lock() const override TA_RET_CAP(get_lock()) { return get_lock(); }
  Lock<CriticalMutex>& lock_ref() const override TA_RET_CAP(get_lock()) { return *get_lock(); }

  VmObject* self_locked() TA_REQ(lock()) TA_ASSERT(self_locked()->lock()) { return this; }

  zx_status_t CreateChildSlice(uint64_t offset, uint64_t size, bool copy_name,
                               fbl::RefPtr<VmObject>* child_vmo) override
      // This function reaches into the created child, which confuses analysis.
      TA_NO_THREAD_SAFETY_ANALYSIS;

  ChildType child_type() const override {
    return is_slice() ? ChildType::kSlice : ChildType::kNotChild;
  }
  bool is_contiguous() const override { return true; }
  bool is_slice() const;
  uint64_t parent_user_id() const override;

  uint64_t size_locked() const override;

  zx_status_t Lookup(uint64_t offset, uint64_t len, LookupFunction lookup_fn) override;
  zx_status_t LookupContiguous(uint64_t offset, uint64_t len, paddr_t* out_paddr) override;
  zx_status_t LookupContiguousLocked(uint64_t offset, uint64_t len, paddr_t* out_paddr)
      TA_REQ(lock());

  zx_status_t CommitRangePinned(uint64_t offset, uint64_t len, bool write) override;
  zx_status_t PrefetchRange(uint64_t offset, uint64_t len) override;

  void Unpin(uint64_t offset, uint64_t len) override {
    // Unpin is a no-op for physical VMOs as they are always pinned.
  }

  void SetUserStreamSize(fbl::RefPtr<StreamSizeManager> ssm) override {
    // Physical VMOs have no operations that can be told to use the user stream size, so can safely
    // just ignore this request.
  }

  // Physical VMOs are implicitly pinned.
  bool DebugIsRangePinned(uint64_t offset, uint64_t len) override { return true; }

  void Dump(uint depth, bool verbose) override;

  zx_status_t GetPage(uint64_t offset, uint pf_flags, MultiPageRequest* page_request,
                      vm_page_t** page, paddr_t* pa) override {
    return ZX_ERR_NOT_SUPPORTED;
  }

  zx_status_t SetMappingCachePolicy(arch_mmu_flags_t cache_policy) override;

  void MaybeDeadTransition() {}

  // Helper functions for FFI access
  const void* state() const { return &opaque_storage_; }
  void* state() { return &opaque_storage_; }

  static CriticalMutex::ShouldClear ChildListLockAcquire() TA_NO_THREAD_SAFETY_ANALYSIS {
    return ChildListLock::Get()->lock().Acquire();
  }
  static void ChildListLockRelease(CriticalMutex::ShouldClear should_clear)
      TA_NO_THREAD_SAFETY_ANALYSIS {
    ChildListLock::Get()->lock().Release(should_clear);
  }

  bool has_children_locked() const TA_REQ(ChildListLock::Get()) { return children_list_len_ != 0; }

  // There's no way good way to convince the static analysis that the lock() that we hold is
  // also the VmObject::lock() and so we disable analysis to set the cache_policy_.
  void set_cache_policy_locked(uint8_t cache_policy) TA_NO_THREAD_SAFETY_ANALYSIS {
    cache_policy_ = cache_policy;
  }

 private:
  // private constructor (use Create())
  VmObjectPhysical(paddr_t base, uint64_t size, bool is_slice_, uint64_t parent_user_id);

  // private destructor, only called from refptr
  ~VmObjectPhysical() override;
  friend fbl::RefPtr<VmObjectPhysical>;
  friend void ::cpp_vm_object_physical_free(VmObjectPhysical* vmo);

  DISALLOW_COPY_ASSIGN_AND_MOVE(VmObjectPhysical);

  // parent pointer FFI helpers
  fbl::RefPtr<VmObjectPhysical> parent_locked() const TA_REQ(ChildListLock::Get());
  void set_parent_locked(fbl::RefPtr<VmObjectPhysical> parent) TA_REQ(ChildListLock::Get());

  // members
  OpaqueStorage<kVmObjectPhysicalStateSize, kVmObjectPhysicalStateAlign> opaque_storage_;
};

#endif  // ZIRCON_KERNEL_VM_INCLUDE_VM_VM_OBJECT_PHYSICAL_H_
