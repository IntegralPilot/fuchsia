// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_VM_INCLUDE_VM_VM_OBJECT_FFI_H_
#define ZIRCON_KERNEL_VM_INCLUDE_VM_VM_OBJECT_FFI_H_

#include <zircon/compiler.h>
#include <zircon/types.h>

#include <kernel/ffi.h>
#include <vm/attribution.h>
#include <vm/fault.h>

#include "vm/page_source.h"
#include "vm/vm_object.h"

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
__BEGIN_CDECLS

typedef zx_status_t (*cpp_vm_object_lookup_fn)(void* ctx, uint64_t offset, uint64_t paddr);

FFI_ALWAYS_INLINE void* cpp_vm_object_get_ref_counted(const VmObject* vmo);
FFI_ALWAYS_INLINE void cpp_vm_object_free(VmObject* vmo);
FFI_ALWAYS_INLINE zx_status_t cpp_vm_object_decommit_range(VmObject* vmo, uint64_t offset,
                                                           uint64_t len);
FFI_ALWAYS_INLINE zx_status_t cpp_vm_object_commit_range(VmObject* vmo, uint64_t offset,
                                                         uint64_t len);
FFI_ALWAYS_INLINE zx_status_t cpp_vm_object_hint_range(VmObject* vmo, uint64_t offset, uint64_t len,
                                                       VmObject::EvictionHint hint);
FFI_ALWAYS_INLINE uint64_t cpp_vm_object_size(const VmObject* vmo);
FFI_ALWAYS_INLINE bool cpp_vm_object_is_resizable(const VmObject* vmo);
FFI_ALWAYS_INLINE bool cpp_vm_object_is_contiguous(const VmObject* vmo);
FFI_ALWAYS_INLINE bool cpp_vm_object_is_stream_compatible(const VmObject* vmo);
FFI_ALWAYS_INLINE zx_status_t cpp_vm_object_resize(VmObject* vmo, uint64_t size);
FFI_ALWAYS_INLINE zx_status_t cpp_vm_object_write(VmObject* vmo, const void* ptr, uint64_t offset,
                                                  size_t len);
FFI_ALWAYS_INLINE zx_status_t cpp_vm_object_set_name(VmObject* vmo, const char* name, size_t len);
FFI_ALWAYS_INLINE void cpp_vm_object_get_name(const VmObject* vmo, char* out_name, size_t len);
FFI_ALWAYS_INLINE void cpp_vm_object_set_child_observer(VmObject* vmo,
                                                        VmObjectChildObserver* child_observer);
FFI_ALWAYS_INLINE zx_status_t cpp_vm_object_commit_range_pinned(VmObject* vmo, uint64_t offset,
                                                                uint64_t len, bool write);
FFI_ALWAYS_INLINE void cpp_vm_object_unpin(VmObject* vmo, uint64_t offset, uint64_t len);
FFI_ALWAYS_INLINE arch_mmu_flags_t cpp_vm_object_get_mapping_cache_policy(const VmObject* vmo);
FFI_ALWAYS_INLINE zx_status_t cpp_vm_object_set_mapping_cache_policy(VmObject* vmo,
                                                                     arch_mmu_flags_t cache_policy);
FFI_ALWAYS_INLINE VmObject* cpp_vm_object_create_clone(VmObject* vmo, Resizability resizable,
                                                       SnapshotType snapshot_type, uint64_t offset,
                                                       uint64_t size, bool copy_name,
                                                       zx_status_t* out_status);
FFI_ALWAYS_INLINE zx_status_t cpp_vm_object_get_page_blocking(VmObject* vmo, uint64_t offset,
                                                              uint32_t pf_flags,
                                                              vm_page_t** out_page,
                                                              paddr_t* out_pa);
FFI_ALWAYS_INLINE VmObject* cpp_vm_object_create_child_slice(VmObject* vmo, uint64_t offset,
                                                             uint64_t size, bool copy_name,
                                                             zx_status_t* out_status);
FFI_ALWAYS_INLINE VmObject* cpp_vm_object_create_child_reference(
    VmObject* vmo, Resizability resizable, uint64_t offset, uint64_t size, bool copy_name,
    bool* out_first_child, zx_status_t* out_status);
FFI_ALWAYS_INLINE void cpp_vm_object_set_user_id(VmObject* vmo, uint64_t user_id);
FFI_ALWAYS_INLINE uint64_t cpp_vm_object_user_id(const VmObject* vmo);
FFI_ALWAYS_INLINE uint64_t cpp_vm_object_parent_user_id(const VmObject* vmo);
FFI_ALWAYS_INLINE uint32_t cpp_vm_object_num_children(const VmObject* vmo);
FFI_ALWAYS_INLINE zx_status_t cpp_vm_object_lookup(VmObject* vmo, uint64_t offset, uint64_t len,
                                                   void* ctx, cpp_vm_object_lookup_fn callback);
FFI_ALWAYS_INLINE zx_status_t cpp_vm_object_lookup_contiguous(VmObject* vmo, uint64_t offset,
                                                              uint64_t len, paddr_t* out_paddr);
FFI_ALWAYS_INLINE zx_status_t cpp_vm_object_get_page(VmObject* vmo, uint64_t offset,
                                                     uint32_t pf_flags,
                                                     MultiPageRequest* page_request,
                                                     vm_page_t** out_page, paddr_t* out_pa);
FFI_ALWAYS_INLINE void cpp_vm_object_get_attributed_memory(const VmObject* vmo,
                                                           vm::AttributionCounts* out_counts);
FFI_ALWAYS_INLINE void cpp_vm_object_get_attributed_memory_in_reference_owner(
    const VmObject* vmo, vm::AttributionCounts* out_counts);
FFI_ALWAYS_INLINE zx_status_t cpp_vm_object_read(VmObject* vmo, void* ptr, uint64_t offset,
                                                 size_t len);
FFI_ALWAYS_INLINE zx_status_t cpp_vm_object_zero_range(VmObject* vmo, uint64_t offset,
                                                       uint64_t len);
FFI_ALWAYS_INLINE zx_status_t cpp_vm_object_dirty_pages(VmObject* vmo, uint64_t offset,
                                                        uint64_t len);
FFI_ALWAYS_INLINE zx_status_t cpp_vm_object_writeback_begin(VmObject* vmo, uint64_t offset,
                                                            uint64_t len, bool is_zero_range);
FFI_ALWAYS_INLINE zx_status_t cpp_vm_object_writeback_end(VmObject* vmo, uint64_t offset,
                                                          uint64_t len);
FFI_ALWAYS_INLINE uint64_t cpp_vm_object_reclamation_event_count(const VmObject* vmo);
FFI_ALWAYS_INLINE void cpp_vm_object_detach_source(VmObject* vmo);
FFI_ALWAYS_INLINE void cpp_vm_object_get_attributed_memory_in_range(
    const VmObject* vmo, uint64_t offset, uint64_t len, vm::AttributionCounts* out_counts);
FFI_ALWAYS_INLINE zx_status_t cpp_vm_object_try_lock_range(VmObject* vmo, uint64_t offset,
                                                           uint64_t len);
FFI_ALWAYS_INLINE zx_status_t cpp_vm_object_lock_range(VmObject* vmo, uint64_t offset, uint64_t len,
                                                       zx_vmo_lock_state_t* lock_state_out);
FFI_ALWAYS_INLINE zx_status_t cpp_vm_object_unlock_range(VmObject* vmo, uint64_t offset,
                                                         uint64_t len);

zx_status_t cpp_vm_object_read_user(VmObject* vmo, void* buffer, uint64_t offset, size_t size,
                                    size_t* out_actual);
zx_status_t cpp_vm_object_write_user(VmObject* vmo, const void* buffer, uint64_t offset,
                                     size_t size, size_t* out_actual);
zx_status_t cpp_vm_object_take_pages(VmObject* vmo, uint64_t offset, uint64_t len,
                                     VmPageSpliceList* pages);
zx_status_t cpp_vm_object_supply_pages(VmObject* vmo, uint64_t offset, uint64_t len,
                                       VmPageSpliceList* pages, SupplyOptions options);

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE bool cpp_vm_object_is_paged(const VmObject* vmo);

__END_CDECLS

#endif  // ZIRCON_KERNEL_VM_INCLUDE_VM_VM_OBJECT_FFI_H_
