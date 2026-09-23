//===-- Fuchsia header <zircon/process.h> --===//
//
// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
//
//===---------------------------------------------------------------------===//

#ifndef _LLVM_LIBC_ZIRCON_PROCESS_H
#define _LLVM_LIBC_ZIRCON_PROCESS_H

#include "../__llvm-libc-common.h"
#include "../llvm-libc-macros/ZX_HANDLE_ACQUIRE.h"
#include "../llvm-libc-macros/ZX_HANDLE_ACQUIRE_UNOWNED.h"
#include "../llvm-libc-types/zx_handle_t.h"
#include <stdint.h>

__BEGIN_C_DECLS

ZX_HANDLE_ACQUIRE_UNOWNED zx_handle_t zx_job_default(void) __NOEXCEPT;
ZX_HANDLE_ACQUIRE_UNOWNED zx_handle_t _zx_job_default(void) __NOEXCEPT;

ZX_HANDLE_ACQUIRE_UNOWNED zx_handle_t zx_process_self(void) __NOEXCEPT;
ZX_HANDLE_ACQUIRE_UNOWNED zx_handle_t _zx_process_self(void) __NOEXCEPT;

ZX_HANDLE_ACQUIRE zx_handle_t zx_take_startup_handle(uint32_t) __NOEXCEPT;

ZX_HANDLE_ACQUIRE_UNOWNED zx_handle_t zx_thread_self(void) __NOEXCEPT;
ZX_HANDLE_ACQUIRE_UNOWNED zx_handle_t _zx_thread_self(void) __NOEXCEPT;

ZX_HANDLE_ACQUIRE_UNOWNED zx_handle_t zx_vmar_root_self(void) __NOEXCEPT;
ZX_HANDLE_ACQUIRE_UNOWNED zx_handle_t _zx_vmar_root_self(void) __NOEXCEPT;

__END_C_DECLS

#endif // _LLVM_LIBC_ZIRCON_PROCESS_H
