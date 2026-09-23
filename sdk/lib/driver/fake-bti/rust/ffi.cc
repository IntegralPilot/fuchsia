// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "sdk/lib/driver/fake-bti/rust/ffi.h"

#include <vector>

#include "sdk/lib/driver/fake-bti/cpp/fake-bti.h"

namespace {

__BEGIN_C_DECLS

__EXPORT uintptr_t g_fake_bti_phys_addr = FAKE_BTI_PHYS_ADDR;

__EXPORT
zx_status_t fake_bti_create(zx_handle_t* out) {
  zx::result result = fake_bti::CreateFakeBti();
  if (result.is_error()) {
    return result.status_value();
  }
  *out = result->release();
  return ZX_OK;
}

__EXPORT
zx_status_t fake_bti_set_paddrs(zx_handle_t bti, const zx_paddr_t* paddrs, size_t count) {
  std::vector<zx_paddr_t> owned_paddrs(paddrs, paddrs + count);
  zx::result result = fake_bti::SetPaddrs(zx::unowned_bti(bti), std::move(owned_paddrs));
  if (result.is_error()) {
    return result.status_value();
  }
  return ZX_OK;
}

__EXPORT
zx_status_t fake_bti_get_pinned_vmo(zx_handle_t bti, fake_bti_pinned_vmo_info_t* out_vmo_info,
                                    size_t out_vmo_info_count, size_t* out_actual) {
  zx::result result = fake_bti::GetPinnedVmo(zx::unowned_bti(bti));
  if (result.is_error()) {
    return result.status_value();
  }

  if (out_actual != nullptr) {
    *out_actual = result->size();
  }

  if (out_vmo_info != nullptr) {
    size_t count = std::min(out_vmo_info_count, result->size());
    for (size_t i = 0; i < count; i++) {
      out_vmo_info[i].vmo = result.value()[i].vmo.release();
      out_vmo_info[i].size = result.value()[i].size;
      out_vmo_info[i].offset = result.value()[i].offset;
    }
  }
  return ZX_OK;
}

__EXPORT
zx_status_t fake_bti_get_vmo_phys_address(zx_handle_t bti,
                                          const fake_bti_pinned_vmo_info_t* vmo_info,
                                          zx_paddr_t* out_paddrs, size_t out_paddrs_count,
                                          size_t* out_actual) {
  fake_bti::FakeBtiPinnedVmoInfo info_cpp;
  info_cpp.vmo.reset(
      vmo_info->vmo);  // take ownership temporarily but we don't want to close it here
  info_cpp.size = vmo_info->size;
  info_cpp.offset = vmo_info->offset;

  zx::result result = fake_bti::GetVmoPhysAddress(zx::unowned_bti(bti), info_cpp);

  // release ownership so we don't close the caller's vmo handle
  [[maybe_unused]] auto status = info_cpp.vmo.release();

  if (result.is_error()) {
    return result.status_value();
  }

  if (out_actual != nullptr) {
    *out_actual = result->size();
  }

  if (out_paddrs != nullptr) {
    size_t count = std::min(out_paddrs_count, result->size());
    for (size_t i = 0; i < count; i++) {
      out_paddrs[i] = result.value()[i];
    }
  }
  return ZX_OK;
}

__END_C_DECLS

}  // namespace
