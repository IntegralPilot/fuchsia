// Copyright 2016 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT
#include "vm/vm_object_physical.h"

#include <align.h>
#include <assert.h>
#include <inttypes.h>
#include <lib/console.h>
#include <lib/page/size.h>
#include <stdlib.h>
#include <string.h>
#include <trace.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <fbl/alloc_checker.h>
#include <ktl/utility.h>
#include <vm/physmap.h>
#include <vm/vm.h>
#include <vm/vm_address_region.h>
#include <vm/vm_constants.h>

#include "vm_priv.h"

#include <ktl/enforce.h>

#define LOCAL_TRACE VM_GLOBAL_TRACE(0)

// LookupContiguousResult maps to the Rust FFI type LookupContiguousResult.
// This allows returning both status and physical address by value from FFI
// without needing raw out pointers.
struct LookupContiguousResult {
  zx_status_t status;
  paddr_t paddr;
};
// ValidateChildSliceResult maps to the Rust FFI type ValidateChildSliceResult.
// This allows returning both status and physical address base by value from FFI
// without needing raw out pointers.
struct ValidateChildSliceResult {
  zx_status_t status;
  paddr_t base;
};

extern "C" {
void rust_vm_object_physical_state_init(void* state, paddr_t base, uint64_t size, bool is_slice,
                                        uint64_t parent_user_id);
void rust_vm_object_physical_state_destroy(void* state);
Lock<CriticalMutex>* rust_vm_object_physical_state_get_lock(const void* state);
uint64_t rust_vm_object_physical_state_get_size(const void* state);
paddr_t rust_vm_object_physical_state_get_base(const void* state);
bool rust_vm_object_physical_state_get_is_slice(const void* state);
uint64_t rust_vm_object_physical_state_get_parent_user_id(const void* state);
VmObjectPhysical* rust_vm_object_physical_state_get_parent_locked(const void* state);
void rust_vm_object_physical_state_set_parent_locked(void* state, VmObjectPhysical* parent);

LookupContiguousResult rust_vm_object_physical_state_lookup_contiguous_locked(const void* state,
                                                                              uint64_t offset,
                                                                              uint64_t len);
zx_status_t rust_vm_object_physical_state_commit_range_pinned(const void* state, uint64_t offset,
                                                              uint64_t len);
zx_status_t rust_vm_object_physical_state_prefetch_range(const void* state, uint64_t offset,
                                                         uint64_t len);
zx_status_t rust_vm_object_physical_state_lookup(const void* state, uint64_t offset, uint64_t len,
                                                 const VmObject::LookupFunction* lookup_fn);
zx_status_t rust_vm_object_physical_set_mapping_cache_policy(VmObjectPhysical* vmo,
                                                             const void* state,
                                                             arch_mmu_flags_t cache_policy);
ValidateChildSliceResult rust_vm_object_physical_validate_child_slice_args(const void* state,
                                                                           uint64_t offset,
                                                                           uint64_t size);
void rust_vm_object_physical_dump(const void* state, uint32_t depth, uintptr_t cpp_vmo_addr,
                                  int32_t ref_count);
}

VmObjectPhysical::VmObjectPhysical(paddr_t base, uint64_t size, bool is_slice,
                                   uint64_t parent_user_id)
    : VmObject(0) {
  LTRACEF("%p, size %#" PRIx64 "\n", this, size);

#pragma GCC diagnostic push
#pragma GCC diagnostic ignored "-Winvalid-offsetof"
  static_assert(
      offsetof(VmObjectPhysical, opaque_storage_) == kVmObjectPhysicalStateOffset,
      "kVmObjectPhysicalStateOffset must match offsetof(VmObjectPhysical, opaque_storage_)");
#pragma GCC diagnostic pop

  DEBUG_ASSERT(IsPageRounded(size));

  rust_vm_object_physical_state_init(&opaque_storage_, base, size, is_slice, parent_user_id);
  AddToGlobalList();
}

VmObjectPhysical::~VmObjectPhysical() {
  canary_.Assert();
  LTRACEF("%p\n", this);

  {
    Guard<CriticalMutex> guard{ChildListLock::Get()};
    fbl::RefPtr<VmObjectPhysical> parent_ptr = parent_locked();
    if (parent_ptr) {
      parent_ptr->RemoveChild(this, guard.take());
      // Avoid recursing destructors when we delete our parent by using the deferred deletion
      // method.
      VmDeferredDeleter<VmObjectPhysical>::DoDeferredDelete(ktl::move(parent_ptr));
      set_parent_locked(nullptr);
    }
  }

  RemoveFromGlobalList();
  rust_vm_object_physical_state_destroy(&opaque_storage_);
}

Lock<CriticalMutex>* VmObjectPhysical::get_lock() const {
  return rust_vm_object_physical_state_get_lock(state());
}

bool VmObjectPhysical::is_slice() const {
  return rust_vm_object_physical_state_get_is_slice(state());
}

uint64_t VmObjectPhysical::parent_user_id() const {
  return rust_vm_object_physical_state_get_parent_user_id(state());
}

uint64_t VmObjectPhysical::size_locked() const {
  return rust_vm_object_physical_state_get_size(state());
}

fbl::RefPtr<VmObjectPhysical> VmObjectPhysical::parent_locked() const {
  return fbl::ImportFromRawPtr(rust_vm_object_physical_state_get_parent_locked(state()));
}

void VmObjectPhysical::set_parent_locked(fbl::RefPtr<VmObjectPhysical> parent) {
  rust_vm_object_physical_state_set_parent_locked(state(), fbl::ExportToRawPtr(&parent));
}

zx_status_t VmObjectPhysical::Create(paddr_t base, uint64_t size,
                                     fbl::RefPtr<VmObjectPhysical>* obj) {
  if (!IsPageRounded(base) || !IsPageRounded(size) || size == 0) {
    return ZX_ERR_INVALID_ARGS;
  }

  // check that base + size is a valid range
  paddr_t safe_base;
  if (add_overflow(base, size - 1, &safe_base)) {
    return ZX_ERR_INVALID_ARGS;
  }

  fbl::AllocChecker ac;

  auto vmo = fbl::AdoptRef<VmObjectPhysical>(
      new (&ac) VmObjectPhysical(base, size, /*is_slice=*/false, /*parent_user_id=*/0));
  if (!ac.check()) {
    return ZX_ERR_NO_MEMORY;
  }

  // Physical VMOs should default to uncached access.
  vmo->SetMappingCachePolicy(ARCH_MMU_FLAG_UNCACHED);

  *obj = ktl::move(vmo);

  return ZX_OK;
}

zx_status_t VmObjectPhysical::CreateChildSlice(uint64_t offset, uint64_t size, bool copy_name,
                                               fbl::RefPtr<VmObject>* child_vmo) {
  canary_.Assert();

  // Offset must be page aligned.
  if (!IsPageRounded(offset)) {
    return ZX_ERR_INVALID_ARGS;
  }

  // Make sure size is page aligned.
  if (!IsPageRounded(size)) {
    return ZX_ERR_INVALID_ARGS;
  }

  if (size > MAX_SIZE) {
    return ZX_ERR_OUT_OF_RANGE;
  }

  // Forbid creating children of resizable VMOs. This restriction may be lifted in the future.
  if (is_resizable()) {
    return ZX_ERR_NOT_SUPPORTED;
  }

  ValidateChildSliceResult result =
      rust_vm_object_physical_validate_child_slice_args(state(), offset, size);
  if (result.status != ZX_OK) {
    return result.status;
  }
  paddr_t child_base = result.base;

  // To mimic a slice we can just create a physical vmo with the correct region. This works since
  // nothing is resizable and the slice must be wholly contained.
  // We can read and store the user_id here since for a slice to be being created the dispatcher
  // side of this object must have completed, and hence the user_id has been set.
  fbl::AllocChecker ac;
  auto vmo = fbl::AdoptRef<VmObjectPhysical>(
      new (&ac) VmObjectPhysical(child_base, size, /*is_slice=*/true, user_id()));
  if (!ac.check()) {
    return ZX_ERR_NO_MEMORY;
  }

  {
    Guard<CriticalMutex> guard{lock()};

    // Inherit the current cache policy
    vmo->cache_policy_ = cache_policy_;
    // Initialize parent
    vmo->set_parent_locked(fbl::RefPtr(this));

    // add the new vmo as a child.
    AddChild(vmo.get());

    if (copy_name) {
      vmo->name_ = name_;
    }
  }

  *child_vmo = ktl::move(vmo);

  return ZX_OK;
}

void VmObjectPhysical::Dump(uint depth, bool verbose) {
  canary_.Assert();

  rust_vm_object_physical_dump(state(), depth, reinterpret_cast<uintptr_t>(this),
                               ref_count_debug());
}

zx_status_t VmObjectPhysical::Lookup(uint64_t offset, uint64_t len,
                                     VmObject::LookupFunction lookup_fn) {
  canary_.Assert();
  return rust_vm_object_physical_state_lookup(state(), offset, len, &lookup_fn);
}

zx_status_t VmObjectPhysical::CommitRangePinned(uint64_t offset, uint64_t len, bool write) {
  canary_.Assert();
  return rust_vm_object_physical_state_commit_range_pinned(state(), offset, len);
}

zx_status_t VmObjectPhysical::PrefetchRange(uint64_t offset, uint64_t len) {
  canary_.Assert();
  return rust_vm_object_physical_state_prefetch_range(state(), offset, len);
}

zx_status_t VmObjectPhysical::LookupContiguous(uint64_t offset, uint64_t len, paddr_t* out_paddr) {
  Guard<CriticalMutex> guard{lock()};
  return LookupContiguousLocked(offset, len, out_paddr);
}

zx_status_t VmObjectPhysical::LookupContiguousLocked(uint64_t offset, uint64_t len,
                                                     paddr_t* out_paddr) {
  canary_.Assert();
  LookupContiguousResult result =
      rust_vm_object_physical_state_lookup_contiguous_locked(state(), offset, len);
  if (result.status == ZX_OK && out_paddr != nullptr) {
    *out_paddr = result.paddr;
  }
  return result.status;
}

zx_status_t VmObjectPhysical::SetMappingCachePolicy(arch_mmu_flags_t cache_policy) {
  return rust_vm_object_physical_set_mapping_cache_policy(this, state(), cache_policy);
}

extern "C" {
VmObjectPhysical* cpp_vm_object_physical_create(paddr_t base, size_t size, zx_status_t* out_status);
VmObject* cpp_vm_object_physical_as_vm_object(VmObjectPhysical* vmo);
zx_status_t cpp_vm_object_lookup_fn_invoke(const VmObject::LookupFunction* lookup_fn,
                                           uint64_t offset, paddr_t pa);
arch_mmu_flags_t cpp_vm_object_get_mapping_cache_policy_locked(const VmObjectPhysical* vmo);
size_t cpp_vm_object_num_mappings_locked(const VmObjectPhysical* vmo);
bool cpp_child_list_lock_acquire();
void cpp_child_list_lock_release(bool should_clear);
bool cpp_vm_object_has_children_locked(const VmObjectPhysical* vmo);
void cpp_vm_object_set_cache_policy_locked(VmObjectPhysical* vmo, arch_mmu_flags_t cache_policy);
VmObjectPhysical* cpp_vm_object_physical_create(paddr_t base, size_t size,
                                                zx_status_t* out_status) {
  fbl::RefPtr<VmObjectPhysical> vmo;
  *out_status = VmObjectPhysical::Create(base, size, &vmo);
  return fbl::ExportToRawPtr(&vmo);
}

VmObject* cpp_vm_object_physical_as_vm_object(VmObjectPhysical* vmo) {
  return static_cast<VmObject*>(vmo);
}

zx_status_t cpp_vm_object_lookup_fn_invoke(const VmObject::LookupFunction* lookup_fn,
                                           uint64_t offset, paddr_t pa) {
  return (*lookup_fn)(offset, pa);
}

arch_mmu_flags_t cpp_vm_object_get_mapping_cache_policy_locked(const VmObjectPhysical* vmo)
    TA_NO_THREAD_SAFETY_ANALYSIS {
  return vmo->GetMappingCachePolicyLocked();
}

size_t cpp_vm_object_num_mappings_locked(const VmObjectPhysical* vmo) TA_NO_THREAD_SAFETY_ANALYSIS {
  return vmo->num_mappings_locked();
}

// Note: We return the `ShouldClear` timeslice extension state as a boolean back to Rust,
// and receive it back on release. This avoids using a `thread_local` variable to store the
// token, which would trigger page faults during early boot before TLS is initialized.
bool cpp_child_list_lock_acquire() TA_NO_THREAD_SAFETY_ANALYSIS {
  return VmObjectPhysical::ChildListLockAcquire() == CriticalMutex::ShouldClear::Yes;
}

void cpp_child_list_lock_release(bool should_clear) TA_NO_THREAD_SAFETY_ANALYSIS {
  VmObjectPhysical::ChildListLockRelease(should_clear ? CriticalMutex::ShouldClear::Yes
                                                      : CriticalMutex::ShouldClear::No);
}

bool cpp_vm_object_has_children_locked(const VmObjectPhysical* vmo) TA_NO_THREAD_SAFETY_ANALYSIS {
  return vmo->has_children_locked();
}

void cpp_vm_object_set_cache_policy_locked(VmObjectPhysical* vmo, arch_mmu_flags_t cache_policy)
    TA_NO_THREAD_SAFETY_ANALYSIS {
  vmo->set_cache_policy_locked(cache_policy);
}
}
