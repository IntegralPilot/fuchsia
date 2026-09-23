// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "vm/vm_object_ffi.h"

#include <lib/user_copy/user_ptr.h>
#include <zircon/types.h>

#include <kernel/ffi.h>

#include "vm/page_source.h"
#include "vm/vm_object.h"
#include "vm/vm_object_paged.h"

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
extern "C" {

FFI_ALWAYS_INLINE void* cpp_vm_object_get_ref_counted(const VmObject* vmo) {
  return const_cast<fbl::RefCountedUpgradeable<VmObject>*>(
      static_cast<const fbl::RefCountedUpgradeable<VmObject>*>(vmo));
}

FFI_ALWAYS_INLINE void cpp_vm_object_free(VmObject* vmo) { delete vmo; }

FFI_ALWAYS_INLINE uint64_t cpp_vm_object_size(const VmObject* vmo) { return vmo->size(); }

FFI_ALWAYS_INLINE bool cpp_vm_object_is_resizable(const VmObject* vmo) {
  return vmo->is_resizable();
}

FFI_ALWAYS_INLINE bool cpp_vm_object_is_contiguous(const VmObject* vmo) {
  return vmo->is_contiguous();
}

FFI_ALWAYS_INLINE bool cpp_vm_object_is_stream_compatible(const VmObject* vmo) {
  return vmo->is_stream_compatible();
}

FFI_ALWAYS_INLINE zx_status_t cpp_vm_object_resize(VmObject* vmo, uint64_t size) {
  return vmo->Resize(size);
}

FFI_ALWAYS_INLINE zx_status_t cpp_vm_object_write(VmObject* vmo, const void* ptr, uint64_t offset,
                                                  size_t len) {
  return vmo->Write(ptr, offset, len);
}

FFI_ALWAYS_INLINE zx_status_t cpp_vm_object_set_name(VmObject* vmo, const char* name, size_t len) {
  return vmo->set_name(name, len);
}

FFI_ALWAYS_INLINE void cpp_vm_object_get_name(const VmObject* vmo, char* out_name, size_t len) {
  vmo->get_name(out_name, len);
}

FFI_ALWAYS_INLINE void cpp_vm_object_set_child_observer(VmObject* vmo,
                                                        VmObjectChildObserver* child_observer) {
  vmo->SetChildObserver(child_observer);
}

FFI_ALWAYS_INLINE bool cpp_vm_object_is_paged(const VmObject* vmo) { return vmo->is_paged(); }

FFI_ALWAYS_INLINE zx_status_t cpp_vm_object_decommit_range(VmObject* vmo, uint64_t offset,
                                                           uint64_t len) {
  return vmo->DecommitRange(offset, len);
}

FFI_ALWAYS_INLINE zx_status_t cpp_vm_object_commit_range(VmObject* vmo, uint64_t offset,
                                                         uint64_t len) {
  return vmo->CommitRange(offset, len);
}

FFI_ALWAYS_INLINE zx_status_t cpp_vm_object_commit_range_pinned(VmObject* vmo, uint64_t offset,
                                                                uint64_t len, bool write) {
  return vmo->CommitRangePinned(offset, len, write);
}

FFI_ALWAYS_INLINE void cpp_vm_object_unpin(VmObject* vmo, uint64_t offset, uint64_t len) {
  vmo->Unpin(offset, len);
}

FFI_ALWAYS_INLINE zx_status_t cpp_vm_object_hint_range(VmObject* vmo, uint64_t offset, uint64_t len,
                                                       VmObject::EvictionHint hint) {
  return vmo->HintRange(offset, len, hint);
}

FFI_ALWAYS_INLINE arch_mmu_flags_t cpp_vm_object_get_mapping_cache_policy(const VmObject* vmo) {
  return vmo->GetMappingCachePolicy();
}

FFI_ALWAYS_INLINE zx_status_t
cpp_vm_object_set_mapping_cache_policy(VmObject* vmo, arch_mmu_flags_t cache_policy) {
  return vmo->SetMappingCachePolicy(cache_policy);
}

FFI_ALWAYS_INLINE VmObject* cpp_vm_object_create_clone(VmObject* vmo, Resizability resizable,
                                                       SnapshotType snapshot_type, uint64_t offset,
                                                       uint64_t size, bool copy_name,
                                                       zx_status_t* out_status) {
  fbl::RefPtr<VmObject> child;
  zx_status_t status = vmo->CreateClone(resizable, snapshot_type, offset, size, copy_name, &child);
  *out_status = status;
  return fbl::ExportToRawPtr(&child);
}

FFI_ALWAYS_INLINE VmObject* cpp_vm_object_create_child_slice(VmObject* vmo, uint64_t offset,
                                                             uint64_t size, bool copy_name,
                                                             zx_status_t* out_status) {
  fbl::RefPtr<VmObject> child;
  *out_status = vmo->CreateChildSlice(offset, size, copy_name, &child);
  return fbl::ExportToRawPtr(&child);
}

FFI_ALWAYS_INLINE VmObject* cpp_vm_object_create_child_reference(
    VmObject* vmo, Resizability resizable, uint64_t offset, uint64_t size, bool copy_name,
    bool* out_first_child, zx_status_t* out_status) {
  fbl::RefPtr<VmObject> child;
  *out_status =
      vmo->CreateChildReference(resizable, offset, size, copy_name, out_first_child, &child);
  return fbl::ExportToRawPtr(&child);
}

FFI_ALWAYS_INLINE zx_status_t cpp_vm_object_get_page_blocking(VmObject* vmo, uint64_t offset,
                                                              uint32_t pf_flags,
                                                              vm_page_t** out_page,
                                                              paddr_t* out_pa) {
  return vmo->GetPageBlocking(offset, pf_flags, out_page, out_pa);
}

FFI_ALWAYS_INLINE void cpp_vm_object_set_user_id(VmObject* vmo, uint64_t user_id) {
  vmo->set_user_id(user_id);
}

FFI_ALWAYS_INLINE uint64_t cpp_vm_object_user_id(const VmObject* vmo) { return vmo->user_id(); }

FFI_ALWAYS_INLINE uint64_t cpp_vm_object_parent_user_id(const VmObject* vmo) {
  return vmo->parent_user_id();
}

FFI_ALWAYS_INLINE uint32_t cpp_vm_object_num_children(const VmObject* vmo) {
  return vmo->num_children();
}

FFI_ALWAYS_INLINE zx_status_t cpp_vm_object_lookup(VmObject* vmo, uint64_t offset, uint64_t len,
                                                   void* ctx, cpp_vm_object_lookup_fn callback) {
  return vmo->Lookup(offset, len, [ctx, callback](uint64_t offset, paddr_t pa) {
    return callback(ctx, offset, pa);
  });
}

FFI_ALWAYS_INLINE zx_status_t cpp_vm_object_lookup_contiguous(VmObject* vmo, uint64_t offset,
                                                              uint64_t len, paddr_t* out_paddr) {
  return vmo->LookupContiguous(offset, len, out_paddr);
}

FFI_ALWAYS_INLINE zx_status_t cpp_vm_object_get_page(VmObject* vmo, uint64_t offset,
                                                     uint32_t pf_flags,
                                                     MultiPageRequest* page_request,
                                                     vm_page_t** out_page, paddr_t* out_pa) {
  return vmo->GetPage(offset, pf_flags, page_request, out_page, out_pa);
}

FFI_ALWAYS_INLINE void cpp_vm_object_get_attributed_memory(const VmObject* vmo,
                                                           vm::AttributionCounts* out_counts) {
  *out_counts = vmo->GetAttributedMemory();
}

FFI_ALWAYS_INLINE void cpp_vm_object_get_attributed_memory_in_reference_owner(
    const VmObject* vmo, vm::AttributionCounts* out_counts) {
  *out_counts = vmo->GetAttributedMemoryInReferenceOwner();
}

FFI_ALWAYS_INLINE zx_status_t cpp_vm_object_read(VmObject* vmo, void* ptr, uint64_t offset,
                                                 size_t len) {
  return vmo->Read(ptr, offset, len);
}

FFI_ALWAYS_INLINE zx_status_t cpp_vm_object_zero_range(VmObject* vmo, uint64_t offset,
                                                       uint64_t len) {
  return vmo->ZeroRange(offset, len);
}

FFI_ALWAYS_INLINE zx_status_t cpp_vm_object_try_lock_range(VmObject* vmo, uint64_t offset,
                                                           uint64_t len) {
  return vmo->TryLockRange(offset, len);
}

FFI_ALWAYS_INLINE zx_status_t cpp_vm_object_lock_range(VmObject* vmo, uint64_t offset, uint64_t len,
                                                       zx_vmo_lock_state_t* lock_state_out) {
  return vmo->LockRange(offset, len, lock_state_out);
}

FFI_ALWAYS_INLINE zx_status_t cpp_vm_object_unlock_range(VmObject* vmo, uint64_t offset,
                                                         uint64_t len) {
  return vmo->UnlockRange(offset, len);
}

FFI_ALWAYS_INLINE zx_status_t cpp_vm_object_dirty_pages(VmObject* vmo, uint64_t offset,
                                                        uint64_t len) {
  return vmo->DirtyPages(offset, len);
}

FFI_ALWAYS_INLINE zx_status_t cpp_vm_object_writeback_begin(VmObject* vmo, uint64_t offset,
                                                            uint64_t len, bool is_zero_range) {
  return vmo->WritebackBegin(offset, len, is_zero_range);
}

FFI_ALWAYS_INLINE zx_status_t cpp_vm_object_writeback_end(VmObject* vmo, uint64_t offset,
                                                          uint64_t len) {
  return vmo->WritebackEnd(offset, len);
}

FFI_ALWAYS_INLINE uint64_t cpp_vm_object_reclamation_event_count(const VmObject* vmo) {
  return vmo->ReclamationEventCount();
}

FFI_ALWAYS_INLINE void cpp_vm_object_detach_source(VmObject* vmo) { vmo->DetachSource(); }

FFI_ALWAYS_INLINE void cpp_vm_object_get_attributed_memory_in_range(
    const VmObject* vmo, uint64_t offset, uint64_t len, vm::AttributionCounts* out_counts) {
  *out_counts = vmo->GetAttributedMemoryInRange(offset, len);
}

zx_status_t cpp_vm_object_read_user(VmObject* vmo, void* buffer, uint64_t offset, size_t size,
                                    size_t* out_actual) {
  auto [st, out_actual_bytes] = vmo->ReadUser(make_user_out_ptr(buffer).reinterpret<char>(), offset,
                                              size, VmObjectReadWriteOptions::TrimLength);
  if (st != ZX_OK) {
    return st;
  }
  *out_actual = out_actual_bytes;
  return ZX_OK;
}

zx_status_t cpp_vm_object_write_user(VmObject* vmo, const void* buffer, uint64_t offset,
                                     size_t size, size_t* out_actual) {
  auto [st, out_actual_bytes] =
      vmo->WriteUser(make_user_in_ptr(buffer).reinterpret<const char>(), offset, size,
                     VmObjectReadWriteOptions::TrimLength, /*on_bytes_transferred=*/nullptr);
  if (st != ZX_OK) {
    return st;
  }
  *out_actual = out_actual_bytes;
  return ZX_OK;
}

FFI_ALWAYS_INLINE zx_status_t cpp_vm_object_take_pages(VmObject* vmo, uint64_t offset, uint64_t len,
                                                       VmPageSpliceList* pages) {
  return vmo->TakePages(offset, len, pages);
}

FFI_ALWAYS_INLINE zx_status_t cpp_vm_object_supply_pages(VmObject* vmo, uint64_t offset,
                                                         uint64_t len, VmPageSpliceList* pages,
                                                         SupplyOptions options) {
  return vmo->SupplyPages(offset, len, pages, options);
}

}  // extern "C"
