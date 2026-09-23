// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "private.h"

// TODO(https://fxbug.dev/478347581): The four-register API will eventually be
// renamed back to zx_thread_start and this wrapper will no longer be used.
__EXPORT zx_status_t _zx_thread_start(zx_handle_t handle, zx_vaddr_t thread_entry, zx_vaddr_t stack,
                                      uintptr_t arg1, uintptr_t arg2) {
  return VDSO_zx_thread_start_regs(handle, thread_entry, stack, arg1, arg2, 0, 0);
}

VDSO_INTERFACE_FUNCTION(zx_thread_start);
