// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "test_helper_ffi.h"

#include <zircon/types.h>

#include <kernel/ffi.h>

#include "test_helper.h"

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
extern "C" {

FFI_ALWAYS_INLINE zx_status_t cpp_make_committed_pager_vmo(size_t num_pages, bool trap_dirty,
                                                           bool resizable, vm_page_t** out_pages,
                                                           VmObjectPaged** out_vmo) {
  fbl::RefPtr<VmObjectPaged> vmo;
  zx_status_t status =
      vm_unittest::make_committed_pager_vmo(num_pages, trap_dirty, resizable, out_pages, &vmo);
  *out_vmo = fbl::ExportToRawPtr(&vmo);
  return status;
}

FFI_ALWAYS_INLINE zx_status_t cpp_make_partially_committed_pager_vmo(
    size_t num_pages, size_t committed_pages, bool trap_dirty, bool resizable, bool ignore_requests,
    vm_page_t** out_pages, VmObjectPaged** out_vmo) {
  fbl::RefPtr<VmObjectPaged> vmo;
  zx_status_t status = vm_unittest::make_partially_committed_pager_vmo(
      num_pages, committed_pages, trap_dirty, resizable, ignore_requests, out_pages, &vmo);
  *out_vmo = fbl::ExportToRawPtr(&vmo);
  return status;
}

FFI_ALWAYS_INLINE zx_status_t cpp_supply_pager_vmo_pages(VmObjectPaged* vmo, uint64_t page_offset,
                                                         uint64_t num_pages,
                                                         vm_page_t** out_pages) {
  return vm_unittest::supply_pager_vmo_pages(vmo, page_offset, num_pages, out_pages);
}

FFI_ALWAYS_INLINE bool cpp_verify_continuous_attribution_bytes(VmObject* vmo,
                                                               uint64_t expected_bytes) {
  return vm_unittest::verify_continuous_attribution_bytes(*vmo, expected_bytes);
}

FFI_ALWAYS_INLINE void cpp_make_private_attribution_counts(uint64_t uncompressed,
                                                           uint64_t compressed,
                                                           vm::AttributionCounts* out_counts) {
  *out_counts = vm_unittest::make_private_attribution_counts(uncompressed, compressed);
}

FFI_ALWAYS_INLINE void cpp_change_vmo_high_priority_count(VmObjectPaged* vmo, int64_t delta) {
  vm_unittest::change_vmo_high_priority_count(*vmo, delta);
}

FFI_ALWAYS_INLINE void cpp_fill_region(uintptr_t seed, void* ptr, size_t len) {
  vm_unittest::fill_region(seed, ptr, len);
}

FFI_ALWAYS_INLINE bool cpp_test_region(uintptr_t seed, void* ptr, size_t len) {
  return vm_unittest::test_region(seed, ptr, len);
}

FFI_ALWAYS_INLINE uint32_t cpp_test_rand(uint32_t seed) { return vm_unittest::test_rand(seed); }

}  // extern "C"
