// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "vm/vm_object_paged_ffi.h"

#include <lib/user_copy/user_iovec.h>
#include <zircon/types.h>

#include <kernel/ffi.h>

#include "vm/vm_object_paged.h"

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
extern "C" {

FFI_ALWAYS_INLINE VmObjectPaged* cpp_vm_object_paged_create(uint32_t pmm_alloc_flags,
                                                            uint32_t options, uint64_t size,
                                                            zx_status_t* out_status) {
  fbl::RefPtr<VmObjectPaged> vmo;
  *out_status = VmObjectPaged::Create(pmm_alloc_flags, options, size, &vmo);
  return fbl::ExportToRawPtr(&vmo);
}

FFI_ALWAYS_INLINE VmObjectPaged* cpp_vm_object_paged_create_contiguous(uint32_t pmm_alloc_flags,
                                                                       uint64_t size,
                                                                       uint8_t alignment_log2,
                                                                       zx_status_t* out_status) {
  fbl::RefPtr<VmObjectPaged> vmo;
  *out_status = VmObjectPaged::CreateContiguous(pmm_alloc_flags, size, alignment_log2, &vmo);
  return fbl::ExportToRawPtr(&vmo);
}

FFI_ALWAYS_INLINE VmObject* cpp_vm_object_paged_as_vm_object(VmObjectPaged* vmo) {
  return static_cast<VmObject*>(vmo);
}

FFI_ALWAYS_INLINE VmCowPages* cpp_vm_object_paged_debug_get_cow_pages(VmObjectPaged* vmo) {
  fbl::RefPtr<VmCowPages> cow = vmo->DebugGetCowPages();
  return fbl::ExportToRawPtr(&cow);
}

FFI_ALWAYS_INLINE VmObjectPaged* cpp_vm_object_as_vm_object_paged(VmObject* vmo) {
  return DownCastVmObject<VmObjectPaged>(vmo);
}

FFI_ALWAYS_INLINE vm_page_t* cpp_vm_object_paged_debug_get_page(VmObjectPaged* vmo,
                                                                uint64_t offset) {
  return vmo->DebugGetPage(offset);
}

FFI_ALWAYS_INLINE zx_status_t cpp_vm_object_paged_read_user_vector(const VmObjectPaged* vmo,
                                                                   user_out_ptr<zx_iovec_t> vector,
                                                                   size_t count, uint64_t offset,
                                                                   size_t length,
                                                                   size_t* out_actual) {
  auto [status, actual] = const_cast<VmObjectPaged*>(vmo)->ReadUserVector(
      make_user_out_iovec(vector, count), offset, length);
  *out_actual = actual;
  return status;
}

FFI_ALWAYS_INLINE zx_status_t cpp_vm_object_paged_write_user_vector(
    const VmObjectPaged* vmo, user_in_ptr<const zx_iovec_t> vector, size_t count, uint64_t offset,
    size_t length, size_t* out_actual) {
  auto [status, actual] = const_cast<VmObjectPaged*>(vmo)->WriteUserVector(
      make_user_in_iovec(vector, count), offset, length,
      VmObject::OnWriteBytesTransferredCallback());
  *out_actual = actual;
  return status;
}

struct ProgressCookie {
  void (*cb)(void*, uint64_t, size_t);
  void* cookie;
  uint64_t prev_stream_size;
};

FFI_ALWAYS_INLINE zx_status_t cpp_vm_object_paged_write_user_vector_progress(
    const VmObjectPaged* vmo, user_in_ptr<const zx_iovec_t> vector, size_t count, uint64_t offset,
    size_t length, uint64_t prev_stream_size, size_t* out_actual,
    void (*cb)(void*, uint64_t, size_t), void* cookie) {
  ProgressCookie pc{cb, cookie, prev_stream_size};
  auto [status, actual] = const_cast<VmObjectPaged*>(vmo)->WriteUserVector(
      make_user_in_iovec(vector, count), offset, length,
      [&pc](const uint64_t write_offset, const size_t len) {
        if (write_offset + len > pc.prev_stream_size) {
          pc.cb(pc.cookie, write_offset, len);
        }
      });
  *out_actual = actual;
  return status;
}

FFI_ALWAYS_INLINE zx_status_t cpp_vm_object_paged_zero_range(const VmObjectPaged* vmo,
                                                             uint64_t offset, uint64_t length) {
  return const_cast<VmObjectPaged*>(vmo)->ZeroRange(offset, length);
}

FFI_ALWAYS_INLINE zx_status_t cpp_vm_object_paged_zero_range_untracked(const VmObjectPaged* vmo,
                                                                       uint64_t offset,
                                                                       uint64_t length) {
  return const_cast<VmObjectPaged*>(vmo)->ZeroRangeUntracked(offset, length);
}

FFI_ALWAYS_INLINE zx_status_t cpp_vm_object_paged_resize(VmObjectPaged* vmo, uint64_t size) {
  return vmo->Resize(size);
}

FFI_ALWAYS_INLINE void cpp_vm_object_paged_unmap_and_call(VmObjectPaged* vmo, uint64_t offset,
                                                          uint64_t len, void (*call_fn)(void* ctx),
                                                          void* call_ctx) {
  Guard<CriticalMutex> vmo_guard{vmo->lock()};
  vmo->ForwardRangeChangeUpdateLocked(offset, len, VmCowPages::RangeChangeOp::Unmap);
  if (call_fn) {
    call_fn(call_ctx);
  }
}

FFI_ALWAYS_INLINE void cpp_vm_object_paged_set_user_stream_size(VmObjectPaged* vmo,
                                                                StreamSizeManager* ssm) {
  vmo->SetUserStreamSize(fbl::ImportFromRawPtr(ssm));
}

FFI_ALWAYS_INLINE bool cpp_vm_object_paged_user_stream_size(const VmObjectPaged* vmo,
                                                            uint64_t* out_stream_size) {
  Guard<CriticalMutex> vmo_guard{vmo->lock()};
  auto result = const_cast<VmObjectPaged*>(vmo)->user_stream_size_locked();
  if (result.has_value()) {
    *out_stream_size = result.value();
    return true;
  }
  return false;
}

}  // extern "C"
