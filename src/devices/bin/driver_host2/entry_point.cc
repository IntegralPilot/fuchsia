// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <zircon/syscalls.h>

extern "C" int64_t Start(zx_handle_t bootstrap, void* vdso);

extern "C" [[noreturn]] void _start(zx_handle_t bootstrap, void* vdso) {
  zx_process_exit(Start(bootstrap, vdso));
}
