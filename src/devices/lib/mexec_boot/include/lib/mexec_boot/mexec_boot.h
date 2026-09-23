// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_LIB_MEXEC_BOOT_INCLUDE_LIB_MEXEC_BOOT_MEXEC_BOOT_H_
#define SRC_DEVICES_LIB_MEXEC_BOOT_INCLUDE_LIB_MEXEC_BOOT_MEXEC_BOOT_H_

#include <zircon/types.h>

extern "C" {

// Performs an mexec boot.
//
// This function will obtain the kernel and data ZBIs, prepare them, and then
// call the mexec syscall.
//
// `mexec_resource` must be a handle to the mexec resource. The caller retains
// ownership of the handle.
//
// This function does not return on success. On failure, it returns a
// zx_status_t.
zx_status_t mexec_boot(zx_handle_t mexec_resource);

}  // extern "C"

#endif  // SRC_DEVICES_LIB_MEXEC_BOOT_INCLUDE_LIB_MEXEC_BOOT_MEXEC_BOOT_H_
