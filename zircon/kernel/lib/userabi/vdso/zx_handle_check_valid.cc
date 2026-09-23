// Copyright 2025 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <zircon/status.h>

#include "private.h"

__EXPORT zx_status_t _zx_handle_check_valid(zx_handle_t handle) {
  if (handle == ZX_HANDLE_INVALID) {
    return ZX_ERR_INVALID_ARGS;
  }

  if ((handle & ZX_HANDLE_FIXED_BITS_MASK) != ZX_HANDLE_FIXED_BITS_MASK) {
    return ZX_ERR_OUT_OF_RANGE;
  }

  zx_status_t status =
      SYSCALL_zx_object_get_info(handle, ZX_INFO_HANDLE_VALID, nullptr, 0, nullptr, nullptr);
  if (status != ZX_OK) {
    // The only error value expected for ZX_INFO_HANDLE_VALID is
    // ZX_ERR_BAD_HANDLE. Any other error indicates a bug in the vDSO or kernel.
    if (status != ZX_ERR_BAD_HANDLE) {
      __builtin_trap();
    }
    return ZX_ERR_NOT_FOUND;
  }

  return ZX_OK;
}

VDSO_INTERFACE_FUNCTION(zx_handle_check_valid);
