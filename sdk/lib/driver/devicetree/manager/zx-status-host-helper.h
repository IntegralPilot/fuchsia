// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_DEVICETREE_MANAGER_ZX_STATUS_HOST_HELPER_H_
#define LIB_DRIVER_DEVICETREE_MANAGER_ZX_STATUS_HOST_HELPER_H_

#include <zircon/types.h>

#if !defined(__Fuchsia__)
// Provide zx_status_get_string for host builds to avoid linker errors.
inline const char* zx_status_get_string(zx_status_t status) {
  switch (status) {
    case 0:
      return "ZX_OK";
    case -1:
      return "ZX_ERR_INTERNAL";
    case -2:
      return "ZX_ERR_NOT_SUPPORTED";
    case -3:
      return "ZX_ERR_NO_RESOURCES";
    case -4:
      return "ZX_ERR_NO_MEMORY";
    case -13:
      return "ZX_ERR_INVALID_ARGS";
    case -25:
      return "ZX_ERR_NOT_FOUND";
    case -27:
      return "ZX_ERR_ALREADY_EXISTS";
    case -28:
      return "ZX_ERR_TIMED_OUT";
    default:
      return "ZX_ERR_UNKNOWN";
  }
}
#endif

#endif  // LIB_DRIVER_DEVICETREE_MANAGER_ZX_STATUS_HOST_HELPER_H_
