// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <zircon/types.h>

#include <dev/iommu/pmt.h>
#include <kernel/ffi.h>

extern "C" {

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE void cpp_pmt_recycle(iommu::Pmt* pmt) { delete pmt; }

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE void* cpp_pmt_get_ref_counted(const iommu::Pmt* pmt) {
  return const_cast<fbl::RefCounted<iommu::Pmt>*>(
      static_cast<const fbl::RefCounted<iommu::Pmt>*>(pmt));
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE void cpp_pmt_release_pinned_memory(iommu::Pmt* pmt) {
  pmt->ReleasePinnedMemory();
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE void cpp_pmt_on_dispatcher_zero_handles(iommu::Pmt* pmt) {
  pmt->OnDispatcherZeroHandles();
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE uint64_t cpp_pmt_size(const iommu::Pmt* pmt) { return pmt->size(); }

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE zx_status_t
cpp_pmt_query_address(iommu::Pmt* pmt, uint64_t query_offset, size_t query_size,
                      ffi::Uninitialized<iommu::QueryAddressResult>* out_result) {
  zx::result<iommu::QueryAddressResult> result = pmt->QueryAddress(query_offset, query_size);
  if (!result.is_ok()) {
    return result.status_value();
  }
  out_result->Initialize(result.value());
  return ZX_OK;
}

}  // extern "C"
