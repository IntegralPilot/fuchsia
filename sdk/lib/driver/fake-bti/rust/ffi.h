// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_FAKE_BTI_RUST_FFI_H_
#define LIB_DRIVER_FAKE_BTI_RUST_FFI_H_

#include <zircon/compiler.h>
#include <zircon/types.h>

// LINT.IfChange

__BEGIN_CDECLS

extern uintptr_t g_fake_bti_phys_addr;

zx_status_t fake_bti_create(zx_handle_t* out);

zx_status_t fake_bti_set_paddrs(zx_handle_t bti, const zx_paddr_t* paddrs, size_t count);

struct fake_bti_pinned_vmo_info_t {
  zx_handle_t vmo;
  uint64_t size;
  uint64_t offset;
};

zx_status_t fake_bti_get_pinned_vmo(zx_handle_t bti, fake_bti_pinned_vmo_info_t* out_vmo_info,
                                    size_t out_vmo_info_count, size_t* out_actual);
zx_status_t fake_bti_get_vmo_phys_address(zx_handle_t bti,
                                          const fake_bti_pinned_vmo_info_t* vmo_info,
                                          zx_paddr_t* out_paddrs, size_t out_paddrs_count,
                                          size_t* out_actual);

__END_CDECLS

// LINT.ThenChange(//sdk/lib/driver/fake-bti/rust/src/ffi.rs)

#endif  // LIB_DRIVER_FAKE_BTI_RUST_FFI_H_
