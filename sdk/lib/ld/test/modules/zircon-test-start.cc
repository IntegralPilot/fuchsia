// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <stdint.h>
#include <zircon/assert.h>
#include <zircon/syscalls.h>
#include <zircon/types.h>

#include "backtrace.h"

extern "C" int64_t TestStart(zx_handle_t bootstrap, void* vdso, zx_handle_t svc_server_end);

extern "C" [[noreturn]] void _start(zx_handle_t bootstrap, void* vdso, zx_handle_t svc_server_end) {
  ZX_ASSERT(__builtin_return_address(0) == nullptr);
  const FramePointer* fp = FramePointer::Get();
  ZX_ASSERT(fp->ra == 0);
  ZX_ASSERT(fp->fp == nullptr);
  int64_t exit_code = TestStart(bootstrap, vdso, svc_server_end);
  __asm__(
      // This doesn't really need to be at the end of _start, just past the
      // call to TestStart so its return address will be here or earlier,
      // for the benefit of the backtrace.cc test code.
      R"""(
      .globl _start_end
      _start_end:
      )"""
      : "=r"(exit_code)
      : "0"(exit_code));
  zx_process_exit(exit_code);
}
